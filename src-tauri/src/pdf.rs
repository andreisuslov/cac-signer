//! Signature fields: list them, check existing signatures, add new ones and sign.
//!
//! Every change is appended as an incremental update, so the original bytes (and any earlier
//! signatures over them) stay untouched. Objects that change are copied from the file byte for
//! byte, with only the affected entries rewritten (see raw.rs).

use std::collections::{BTreeMap, HashMap};

use lopdf::encryption::{encrypt_object, EncryptionState};
use lopdf::{Dictionary, Document, Object, ObjectId, Permissions, Stream, StringFormat};
use serde::{Deserialize, Serialize};

use crate::stamp::num;
use crate::{card, raw, stamp};

/// Space reserved for the CMS signature. It is stored hex-encoded, at twice this size.
const SIG_RESERVE: usize = 16 * 1024;
/// Space reserved inside /ByteRange [...] for the four offsets.
const BYTE_RANGE_RESERVE: usize = 60;

#[derive(Serialize)]
pub struct Info {
    /// Page sizes in points, as displayed (rotation applied).
    pub pages: Vec<[f32; 2]>,
    pub fields: Vec<FieldInfo>,
    /// Top-level field names already in the form. New fields need names of their own.
    pub taken: Vec<String>,
    pub can_add: bool,
    pub can_sign: bool,
    /// Why adding or signing is limited, for a certified document.
    pub notice: Option<String>,
}

#[derive(Serialize)]
pub struct FieldInfo {
    pub name: String,
    pub page: Option<u32>,
    /// [left, top, width, height] in points from the displayed page's top-left corner.
    pub rect: Option<[f32; 4]>,
    pub signature: Option<SignatureInfo>,
}

#[derive(Serialize)]
pub struct SignatureInfo {
    pub signer: String,
    pub time: String,
    pub status: &'static str,
    pub detail: String,
}

/// A signature field to create.
#[derive(Deserialize)]
pub struct NewField {
    pub name: String,
    pub page: u32,
    /// [left, top, width, height] in points from the displayed page's top-left corner.
    pub rect: [f32; 4],
}

struct SigField {
    name: String,
    field: ObjectId,
    widget: ObjectId,
    page: Option<u32>,
    rect: [f32; 4],
    signed: Option<ObjectId>,
}

const NO_CHANGES: &str = "This PDF is certified with no changes allowed, so it cannot be signed.";
const FILL_ONLY: &str =
    "This PDF is certified to allow signing its existing fields only, so new fields cannot be added.";
const SECURED: &str = "This PDF's security settings do not allow signing.";
const SECURED_FILL_ONLY: &str =
    "This PDF's security settings allow signing its existing fields only, so new fields cannot be added.";

/// What the document allows: (sign an existing field, add fields), and why not, if it doesn't.
fn limits(doc: &Document) -> (bool, bool, Option<&'static str>) {
    let permissions = doc.encryption_state.as_ref().map(|state| state.permissions());
    let may = |p: Permissions| permissions.is_none_or(|granted| granted.contains(p));
    let docmdp = docmdp(doc);
    if docmdp == Some(1) {
        (false, false, Some(NO_CHANGES))
    } else if !may(Permissions::FILLABLE) && !may(Permissions::ANNOTABLE) {
        (false, false, Some(SECURED))
    } else if docmdp == Some(2) {
        (true, false, Some(FILL_ONLY))
    } else if !may(Permissions::ANNOTABLE | Permissions::MODIFIABLE) {
        (true, false, Some(SECURED_FILL_ONLY))
    } else {
        (true, true, None)
    }
}

/// Parses the file. Files encrypted with an empty user password (an owner password only)
/// are decrypted, as any PDF reader would open them.
fn load(bytes: &[u8]) -> Result<Document, String> {
    let doc = Document::load_mem(bytes).map_err(|e| match e {
        lopdf::Error::Decryption(_) | lopdf::Error::InvalidPassword => {
            "This PDF needs a password to open, which CAC Signer does not support.".to_string()
        }
        e => format!("Could not read the PDF: {e}"),
    })?;
    if doc.trailer.has(b"Encrypt") && !doc.was_encrypted() {
        return Err("This PDF needs a password to open, which CAC Signer does not support.".into());
    }
    let catalog = doc.catalog().map_err(|e| format!("Could not read the PDF: {e}"))?;
    if catalog.get(b"NeedsRendering").and_then(Object::as_bool).unwrap_or(false) {
        return Err("This is an Adobe XFA form, which only Acrobat and Adobe Reader can display.".into());
    }
    Ok(doc)
}

pub fn inspect(bytes: &[u8]) -> Result<Info, String> {
    let doc = load(bytes)?;
    let geometry: BTreeMap<u32, Geometry> =
        doc.get_pages().into_iter().map(|(n, id)| (n, Geometry::of(&doc, id))).collect();
    let fields = sig_fields(&doc);
    let (can_sign, can_add, notice) = limits(&doc);
    Ok(Info {
        pages: geometry.values().map(|g| g.size()).collect(),
        taken: taken_names(&doc, &fields),
        fields: fields
            .into_iter()
            .map(|f| FieldInfo {
                rect: f.page.and_then(|p| geometry.get(&p)).map(|g| g.rect_to_display(f.rect)),
                signature: f
                    .signed
                    .and_then(|id| doc.get_dictionary(id).ok())
                    .map(|sig| check(bytes, sig)),
                name: f.name,
                page: f.page,
            })
            .collect(),
        can_add,
        can_sign,
        notice: notice.map(String::from),
    })
}

/// The visible part of a page (its crop box, in user space) and how it is turned for display.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Geometry {
    crop: [f32; 4],
    /// Clockwise: 0, 90, 180 or 270.
    rotate: i64,
}

impl Geometry {
    fn of(doc: &Document, page: ObjectId) -> Geometry {
        let rect = |key: &[u8]| -> Option<[f32; 4]> {
            let n: Vec<f32> = inherited(doc, page, key)?
                .as_array()
                .ok()?
                .iter()
                .filter_map(|o| doc.dereference(o).ok()?.1.as_float().ok())
                .collect();
            let &[a, b, c, d] = n.as_slice() else { return None };
            Some([a.min(c), b.min(d), a.max(c), b.max(d)]).filter(|r| r[0] < r[2] && r[1] < r[3])
        };
        let media = rect(b"MediaBox").unwrap_or([0.0, 0.0, 612.0, 792.0]);
        let crop = rect(b"CropBox")
            .map(|c| [c[0].max(media[0]), c[1].max(media[1]), c[2].min(media[2]), c[3].min(media[3])])
            .filter(|c| c[0] < c[2] && c[1] < c[3])
            .unwrap_or(media);
        let rotate = inherited(doc, page, b"Rotate").and_then(|r| r.as_float().ok()).unwrap_or(0.0) as i64;
        Geometry { crop, rotate: rotate.rem_euclid(360) / 90 * 90 }
    }

    /// Width and height as displayed.
    fn size(self) -> [f32; 2] {
        let [x1, y1, x2, y2] = self.crop;
        if self.rotate % 180 == 0 {
            [x2 - x1, y2 - y1]
        } else {
            [y2 - y1, x2 - x1]
        }
    }

    /// User-space point for a displayed one, measured from the top-left corner.
    fn to_user(self, [u, v]: [f32; 2]) -> [f32; 2] {
        let [x1, y1, x2, y2] = self.crop;
        match self.rotate {
            90 => [x1 + v, y1 + u],
            180 => [x2 - u, y1 + v],
            270 => [x2 - v, y2 - u],
            _ => [x1 + u, y2 - v],
        }
    }

    fn to_display(self, [x, y]: [f32; 2]) -> [f32; 2] {
        let [x1, y1, x2, y2] = self.crop;
        match self.rotate {
            90 => [y - y1, x - x1],
            180 => [x2 - x, y - y1],
            270 => [y2 - y, x2 - x],
            _ => [x - x1, y2 - y],
        }
    }

    /// [x1 y1 x2 y2] in user space for a displayed [left top width height].
    fn rect_to_user(self, [left, top, w, h]: [f32; 4]) -> [f32; 4] {
        let [ax, ay] = self.to_user([left, top]);
        let [bx, by] = self.to_user([left + w, top + h]);
        [ax.min(bx), ay.min(by), ax.max(bx), ay.max(by)]
    }

    fn rect_to_display(self, [x1, y1, x2, y2]: [f32; 4]) -> [f32; 4] {
        let [au, av] = self.to_display([x1, y1]);
        let [bu, bv] = self.to_display([x2, y2]);
        [au.min(bu), av.min(bv), (au - bu).abs(), (av - bv).abs()]
    }
}

/// A page attribute, taken from the nearest page tree node that sets it.
fn inherited<'a>(doc: &'a Document, page: ObjectId, key: &[u8]) -> Option<&'a Object> {
    let mut id = page;
    for _ in 0..64 {
        let dict = doc.get_dictionary(id).ok()?;
        if let Ok(value) = dict.get(key) {
            return doc.dereference(value).ok().map(|(_, v)| v);
        }
        id = dict.get(b"Parent").and_then(Object::as_reference).ok()?;
    }
    None
}

/// DocMDP level of a certified document: 1 allows no changes, 2 filling in and signing
/// existing fields, 3 also annotations and new fields. None when the document is not certified.
fn docmdp(doc: &Document) -> Option<i64> {
    let dict = |o| doc.dereference(o).ok().and_then(|(_, o)| o.as_dict().ok());
    let perms = dict(doc.catalog().ok()?.get(b"Perms").ok()?)?;
    let sig = dict(perms.get(b"DocMDP").ok()?)?;
    let level = array(doc, sig.get(b"Reference"))
        .iter()
        .filter_map(dict)
        .find(|r| r.get(b"TransformMethod").and_then(Object::as_name).is_ok_and(|m| m == b"DocMDP"))
        .and_then(|r| dict(r.get(b"TransformParams").ok()?))
        .and_then(|params| params.get(b"P").and_then(Object::as_i64).ok());
    Some(level.unwrap_or(2))
}

fn acroform(doc: &Document) -> Option<&Dictionary> {
    let acro = doc.catalog().ok()?.get(b"AcroForm").ok()?;
    doc.dereference(acro).ok()?.1.as_dict().ok()
}

fn taken_names(doc: &Document, fields: &[SigField]) -> Vec<String> {
    let top = acroform(doc).map(|acro| array(doc, acro.get(b"Fields"))).unwrap_or_default();
    let mut names: Vec<String> = top
        .iter()
        .filter_map(|f| doc.get_dictionary(f.as_reference().ok()?).ok()?.get(b"T").ok()?.as_str().ok())
        .map(text)
        .chain(fields.iter().map(|f| f.name.clone()))
        .collect();
    names.sort();
    names.dedup();
    names
}

fn check(file: &[u8], sig: &Dictionary) -> SignatureInfo {
    let time = sig
        .get(b"M")
        .and_then(Object::as_str)
        .map(display_date)
        .unwrap_or_default();
    let ranges: Vec<usize> = sig
        .get(b"ByteRange")
        .and_then(Object::as_array)
        .map(|a| a.iter().filter_map(|n| usize::try_from(n.as_i64().ok()?).ok()).collect())
        .unwrap_or_default();
    let in_file = |r: &[usize]| r[0].checked_add(r[1]).is_some_and(|end| end <= file.len());
    if ranges.is_empty() || !ranges.len().is_multiple_of(2) || !ranges.chunks(2).all(in_file) {
        return SignatureInfo {
            signer: "unknown signer".into(),
            time,
            status: "invalid",
            detail: "The signature's byte range is damaged.".into(),
        };
    }
    let content: Vec<u8> = ranges.chunks(2).flat_map(|r| &file[r[0]..r[0] + r[1]]).copied().collect();
    let cms = sig.get(b"Contents").and_then(Object::as_str).unwrap_or_default();
    let verified = card::verify_detached(trim_der(cms), &content);
    SignatureInfo { signer: verified.signer, time, status: verified.status, detail: verified.detail }
}

/// /Contents is zero-padded after the DER signature; cut it to the DER length.
fn trim_der(bytes: &[u8]) -> &[u8] {
    let len = match bytes {
        [0x30, l, ..] if *l < 0x80 => Some(2 + *l as usize),
        [0x30, l, rest @ ..] => {
            let n = (*l & 0x7f) as usize;
            ((1..=4).contains(&n) && rest.len() >= n)
                .then(|| 2 + n + rest[..n].iter().fold(0usize, |acc, b| acc << 8 | *b as usize))
        }
        _ => None,
    };
    &bytes[..len.unwrap_or(bytes.len()).min(bytes.len())]
}

/// "D:20261104182811-05'00'" -> "2026.11.04 18:28:11 -05'00'"
fn display_date(raw: &[u8]) -> String {
    let raw = String::from_utf8_lossy(raw);
    let s = raw.strip_prefix("D:").unwrap_or(&raw);
    if s.len() < 14 || !s.as_bytes()[..14].iter().all(u8::is_ascii_digit) {
        return s.to_string();
    }
    let zone = s[14..].trim();
    format!("{}.{}.{} {}:{}:{} {zone}", &s[..4], &s[4..6], &s[6..8], &s[8..10], &s[10..12], &s[12..14])
        .trim_end()
        .to_string()
}

fn text(raw: &[u8]) -> String {
    match raw {
        [0xFE, 0xFF, rest @ ..] => {
            let units: Vec<u16> = rest.chunks_exact(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
            String::from_utf16_lossy(&units)
        }
        _ => raw.iter().map(|&b| b as char).collect(),
    }
}

fn array<'a>(doc: &'a Document, obj: Result<&'a Object, lopdf::Error>) -> &'a [Object] {
    obj.and_then(|o| doc.dereference(o))
        .and_then(|(_, o)| o.as_array())
        .map(Vec::as_slice)
        .unwrap_or_default()
}

fn sig_fields(doc: &Document) -> Vec<SigField> {
    let mut out = Vec::new();
    let Some(acro) = acroform(doc) else { return out };
    let pages: HashMap<ObjectId, u32> = doc.get_pages().into_iter().map(|(n, id)| (id, n)).collect();
    for field in array(doc, acro.get(b"Fields")) {
        if let Ok(id) = field.as_reference() {
            walk(doc, id, "", None, &pages, &mut out, 0);
        }
    }
    out
}

fn walk<'a>(
    doc: &'a Document,
    id: ObjectId,
    parent: &str,
    inherited_type: Option<&'a [u8]>,
    pages: &HashMap<ObjectId, u32>,
    out: &mut Vec<SigField>,
    depth: u8,
) {
    let Ok(field) = doc.get_dictionary(id) else { return };
    if depth > 32 {
        return;
    }
    let name = match field.get(b"T").and_then(Object::as_str).map(text) {
        Ok(part) if parent.is_empty() => part,
        Ok(part) => format!("{parent}.{part}"),
        Err(_) => parent.to_string(),
    };
    let field_type = field.get(b"FT").and_then(Object::as_name).ok().or(inherited_type);

    let (children, widgets): (Vec<ObjectId>, Vec<ObjectId>) = array(doc, field.get(b"Kids"))
        .iter()
        .filter_map(|k| k.as_reference().ok())
        .partition(|k| doc.get_dictionary(*k).is_ok_and(|d| d.has(b"T")));
    for child in children {
        walk(doc, child, &name, field_type, pages, out, depth + 1);
    }
    if field_type != Some(b"Sig".as_slice()) {
        return;
    }
    let widget = if field.has(b"Rect") { id } else if let Some(w) = widgets.first() { *w } else { return };
    let Ok(widget_dict) = doc.get_dictionary(widget) else { return };
    let r: Vec<f32> = array(doc, widget_dict.get(b"Rect")).iter().filter_map(|n| n.as_float().ok()).collect();
    let &[x1, y1, x2, y2] = r.as_slice() else { return };
    let page = widget_dict
        .get(b"P")
        .and_then(Object::as_reference)
        .ok()
        .and_then(|p| pages.get(&p).copied())
        .or_else(|| page_with_annot(doc, widget, pages));
    out.push(SigField {
        name,
        field: id,
        widget,
        page,
        rect: [x1.min(x2), y1.min(y2), x1.max(x2), y1.max(y2)],
        signed: field.get(b"V").and_then(Object::as_reference).ok(),
    });
}

fn page_with_annot(doc: &Document, widget: ObjectId, pages: &HashMap<ObjectId, u32>) -> Option<u32> {
    pages.iter().find_map(|(page, number)| {
        let annots = array(doc, doc.get_dictionary(*page).and_then(|p| p.get(b"Annots")));
        annots.iter().any(|a| a.as_reference().ok() == Some(widget)).then_some(*number)
    })
}

/// Offset of the last cross-reference section, from the final `startxref`.
fn last_startxref(file: &[u8]) -> Option<usize> {
    let at = file.windows(9).rposition(|w| w == b"startxref")?;
    let digits: String = file[at + 9..]
        .iter()
        .skip_while(|b| b.is_ascii_whitespace())
        .take_while(|b| b.is_ascii_digit())
        .map(|&b| b as char)
        .collect();
    digits.parse().ok()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02X}")).collect()
}

/// PDF text string bytes: PDFDocEncoding for ASCII, UTF-16BE otherwise.
fn text_bytes(s: &str) -> Vec<u8> {
    if s.is_ascii() {
        return s.as_bytes().to_vec();
    }
    [0xFE, 0xFF].into_iter().chain(s.encode_utf16().flat_map(u16::to_be_bytes)).collect()
}

fn write_object(out: &mut Vec<u8>, obj: &Object) {
    match obj {
        Object::Null => out.extend(b"null"),
        Object::Boolean(b) => out.extend(if *b { b"true".as_slice() } else { b"false" }),
        Object::Integer(i) => out.extend(i.to_string().as_bytes()),
        Object::Real(r) => out.extend(r.to_string().as_bytes()),
        Object::Name(name) => {
            out.push(b'/');
            for &b in name {
                if b.is_ascii_graphic() && !b"()<>[]{}/%#".contains(&b) {
                    out.push(b);
                } else {
                    out.extend(format!("#{b:02X}").as_bytes());
                }
            }
        }
        Object::String(s, _) => out.extend(format!("<{}>", hex(s)).as_bytes()),
        Object::Array(items) => {
            out.push(b'[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(b' ');
                }
                write_object(out, item);
            }
            out.push(b']');
        }
        Object::Dictionary(dict) => {
            out.extend(b"<<");
            for (key, value) in dict.iter() {
                write_object(out, &Object::Name(key.clone()));
                out.push(b' ');
                write_object(out, value);
            }
            out.extend(b">>");
        }
        // Only dictionaries are rewritten here; streams are always referenced, never inline.
        Object::Stream(_) => out.extend(b"null"),
        Object::Reference((num, generation)) => out.extend(format!("{num} {generation} R").as_bytes()),
    }
}

/// /MK /R of a widget: how far its appearance is turned counterclockwise on the page.
fn widget_rotation(doc: &Document, widget: &Dictionary) -> i64 {
    let r = widget
        .get(b"MK")
        .ok()
        .and_then(|mk| doc.dereference(mk).ok()?.1.as_dict().ok())
        .and_then(|mk| mk.get(b"R").ok()?.as_float().ok())
        .unwrap_or(0.0) as i64;
    r.rem_euclid(360) / 90 * 90
}

/// Dictionary entries of a form XObject for a `w` x `h` box (as read), turned `rotate` degrees
/// counterclockwise so it reads upright on a page displayed with the same /Rotate.
fn appearance(w: f32, h: f32, rotate: i64, with_font: bool) -> String {
    let matrix = match rotate {
        90 => " /Matrix [0 1 -1 0 0 0]",
        180 => " /Matrix [-1 0 0 -1 0 0]",
        270 => " /Matrix [0 -1 1 0 0 0]",
        _ => "",
    };
    let resources = if with_font {
        " /Resources << /Font << /Helv << /Type /Font /Subtype /Type1 /BaseFont /Helvetica \
         /Encoding /WinAnsiEncoding >> >> >>"
    } else {
        ""
    };
    format!("/Type /XObject /Subtype /Form /BBox [0 0 {} {}]{matrix}{resources}", num(w), num(h))
}

fn references(ids: &[ObjectId]) -> String {
    ids.iter().map(|(n, g)| format!("{n} {g} R")).collect::<Vec<_>>().join(" ")
}

/// Objects for an incremental update: new ones, and changed copies of existing ones.
struct Update<'a> {
    doc: &'a Document,
    file: &'a [u8],
    next: u32,
    objects: BTreeMap<ObjectId, Vec<u8>>,
    /// Set for an encrypted document: strings and streams in the update are encrypted too.
    crypt: Option<&'a EncryptionState>,
}

impl Update<'_> {
    /// `object` encrypted for `id` when the document is encrypted.
    fn encrypt(&self, id: ObjectId, mut object: Object) -> Result<Object, String> {
        if let Some(state) = self.crypt {
            encrypt_object(state, id, &mut object).map_err(|e| format!("Could not encrypt the update: {e}"))?;
        }
        Ok(object)
    }

    /// `bytes` as a string in object `id`.
    fn string(&self, id: ObjectId, bytes: &[u8]) -> Result<String, String> {
        let string = self.encrypt(id, Object::String(bytes.to_vec(), StringFormat::Hexadecimal))?;
        Ok(format!("<{}>", hex(string.as_str().unwrap_or_default())))
    }

    /// Stores stream object `id` with dictionary entries `dict` and contents `data`.
    fn add_stream(&mut self, id: ObjectId, dict: &str, data: &[u8]) -> Result<(), String> {
        let stream = self.encrypt(id, Object::Stream(Stream::new(Dictionary::new(), data.to_vec())))?;
        let data = stream.as_stream().map(|s| s.content.as_slice()).unwrap_or_default();
        let mut body = format!("<< {dict} /Length {} >>\nstream\n", data.len()).into_bytes();
        body.extend(data);
        body.extend(b"\nendstream");
        self.objects.insert(id, body);
        Ok(())
    }

    fn reserve(&mut self) -> ObjectId {
        self.next += 1;
        (self.next - 1, 0)
    }

    fn add(&mut self, body: Vec<u8>) -> ObjectId {
        let id = self.reserve();
        self.objects.insert(id, body);
        id
    }

    /// An existing object as it stands in this update: already changed, or as in the file.
    fn source(&self, id: ObjectId) -> Result<Vec<u8>, String> {
        if let Some(body) = self.objects.get(&id) {
            return Ok(body.clone());
        }
        let unreadable = || format!("Could not read object {} of the PDF.", id.0);
        match raw::source(self.doc, self.file, id) {
            // Strings inside an object stream are not encrypted on their own, but they must be
            // once the object is written out by itself.
            Some((src, true)) if self.crypt.is_some() => raw::map_strings(&src, |s| {
                let string = self.encrypt(id, Object::String(s.to_vec(), StringFormat::Hexadecimal)).ok()?;
                string.as_str().ok().map(<[u8]>::to_vec)
            })
            .ok_or_else(unreadable),
            Some((src, _)) => Ok(src),
            None => {
                // Not where the cross-reference data says, so write out the parsed object.
                let obj = self.doc.get_object(id).ok().filter(|o| o.as_stream().is_err()).ok_or_else(unreadable)?;
                let mut out = Vec::new();
                write_object(&mut out, &self.encrypt(id, obj.clone())?);
                Ok(out)
            }
        }
    }

    fn patch(&mut self, id: ObjectId, edits: &[(&[u8], Option<&[u8]>)]) -> Result<(), String> {
        let patched = raw::patch(&self.source(id)?, edits)
            .ok_or_else(|| format!("Could not update object {} of the PDF.", id.0))?;
        self.objects.insert(id, patched);
        Ok(())
    }

    /// Adds `items` to the array under `key` in `dict`, whose bytes are `src`. Returns the key's
    /// new value, or None when the array is an object of its own and was updated instead.
    fn extend_array(
        &mut self,
        dict: &Dictionary,
        src: &[u8],
        key: &[u8],
        items: &str,
    ) -> Result<Option<Vec<u8>>, String> {
        if let Ok(Object::Reference(id)) = dict.get(key) {
            if self.doc.get_object(*id).is_ok_and(|o| o.as_array().is_ok()) {
                let array = raw::append(&self.source(*id)?, items.as_bytes())
                    .ok_or_else(|| format!("Could not update object {} of the PDF.", id.0))?;
                self.objects.insert(*id, array);
                return Ok(None);
            }
        }
        Ok(Some(match raw::get(src, key).filter(|v| v.starts_with(b"[")) {
            Some(array) => raw::append(array, items.as_bytes()).ok_or("Could not update the PDF.")?,
            None => format!("[{items}]").into_bytes(),
        }))
    }
}

type SignCms<'a> = Box<dyn FnOnce(&[u8]) -> Result<Vec<u8>, String> + 'a>;

struct Signer<'a> {
    field: &'a str,
    name: &'a str,
    unlock: Box<dyn FnOnce() -> Result<(), String> + 'a>,
    sign_cms: SignCms<'a>,
}

/// Adds `new_fields` without signing anything.
pub fn add_fields(input: &[u8], new_fields: &[NewField]) -> Result<Vec<u8>, String> {
    if new_fields.is_empty() {
        return Err("There are no new fields to save.".into());
    }
    update(input, new_fields, None).map(|(out, _)| out)
}

/// Adds `new_fields`, signs `field_name` (an existing field or one of the new ones), and
/// returns the new file and the signing time as shown on the page.
/// `unlock` runs once the document checks pass and before the signing time is taken, so a
/// PIN prompt left open for a while does not make the recorded time early.
/// `sign_cms` gets the bytes covered by /ByteRange and returns a detached CMS signature.
pub fn sign(
    input: &[u8],
    new_fields: &[NewField],
    field_name: &str,
    signer_name: &str,
    unlock: impl FnOnce() -> Result<(), String>,
    sign_cms: impl FnOnce(&[u8]) -> Result<Vec<u8>, String>,
) -> Result<(Vec<u8>, String), String> {
    let signer = Signer { field: field_name, name: signer_name, unlock: Box::new(unlock), sign_cms: Box::new(sign_cms) };
    update(input, new_fields, Some(signer))
}

fn update(input: &[u8], new_fields: &[NewField], signer: Option<Signer>) -> Result<(Vec<u8>, String), String> {
    let doc = load(input)?;
    let fields = sig_fields(&doc);
    let pages = doc.get_pages();
    let (can_sign, can_add, notice) = limits(&doc);
    if (signer.is_some() && !can_sign) || (!new_fields.is_empty() && !can_add) {
        return Err(notice.unwrap_or_default().into());
    }
    let encrypt_ref = doc.encryption_state.as_ref().map(EncryptionState::encrypt_object_id);
    if encrypt_ref == Some(None) {
        return Err("This PDF's encryption settings are not supported.".into());
    }
    let taken = taken_names(&doc, &fields);
    for (i, field) in new_fields.iter().enumerate() {
        let name = field.name.as_str();
        if name.trim().is_empty() {
            return Err("Every new field needs a name.".into());
        }
        if name.contains('.') {
            return Err(format!("\"{name}\": field names cannot contain periods."));
        }
        if taken.iter().any(|t| t == name) || new_fields[..i].iter().any(|f| f.name == name) {
            return Err(format!("There is already a field named \"{name}\"."));
        }
        if !pages.contains_key(&field.page) {
            return Err(format!("The PDF has no page {}.", field.page));
        }
        if !(field.rect[2] >= 4.0 && field.rect[3] >= 4.0) {
            return Err(format!("\"{name}\" is too small."));
        }
    }
    // The field to sign: Ok(index) of a new one, or Err(existing field).
    let target = match &signer {
        None => None,
        Some(signer) => Some(match new_fields.iter().position(|f| f.name == signer.field) {
            Some(i) => Ok(i),
            None => {
                let field = fields
                    .iter()
                    .find(|f| f.name == signer.field)
                    .ok_or_else(|| format!("There is no signature field named \"{}\".", signer.field))?;
                if field.signed.is_some() {
                    return Err(format!("\"{}\" is already signed.", signer.field));
                }
                Err(field)
            }
        }),
    };

    let damaged = || "The PDF's cross-reference data is damaged. Re-save it in Preview and try again.".to_string();
    let prev = last_startxref(input).ok_or_else(damaged)?;
    let rest = input.get(prev..).ok_or_else(damaged)?;
    // The update must use the same kind of cross-reference section as the one before it.
    let xref_stream = !rest.starts_with(b"xref");
    if xref_stream && !rest.first().is_some_and(u8::is_ascii_digit) {
        return Err(damaged());
    }
    let root = doc.trailer.get(b"Root").and_then(Object::as_reference).map_err(|_| damaged())?;
    let read_error = |e: lopdf::Error| format!("Could not read the PDF: {e}");

    let next = doc.trailer.get(b"Size").and_then(Object::as_i64).unwrap_or(0).max(doc.max_id as i64 + 1) as u32;
    let mut up = Update { doc: &doc, file: input, next, objects: BTreeMap::new(), crypt: doc.encryption_state.as_ref() };
    // The signature dictionary and the stamp's appearance stream.
    let sig_ids = signer.as_ref().map(|_| (up.reserve(), up.reserve()));
    // Size (as read) and rotation of the stamp.
    let mut stamp_box = None;

    let mut new_ids = Vec::new();
    let mut by_page: BTreeMap<u32, Vec<ObjectId>> = BTreeMap::new();
    for (i, field) in new_fields.iter().enumerate() {
        let page = pages[&field.page];
        let geometry = Geometry::of(&doc, page);
        let [x1, y1, x2, y2] = geometry.rect_to_user(field.rect);
        let [_, _, w, h] = field.rect;
        let (ap, value) = match (target, sig_ids) {
            (Some(Ok(t)), Some((sig, ap))) if t == i => {
                stamp_box = Some((w, h, geometry.rotate));
                (ap, format!(" /V {} 0 R", sig.0))
            }
            _ => {
                let ap = up.reserve();
                up.add_stream(ap, &appearance(w, h, geometry.rotate, false), b"")?;
                (ap, String::new())
            }
        };
        let turned = match geometry.rotate {
            0 => String::new(),
            r => format!(" /MK << /R {r} >>"),
        };
        let id = up.reserve();
        let widget = format!(
            "<< /Type /Annot /Subtype /Widget /FT /Sig /T {} /F 4 /P {} {} R /Rect [{} {} {} {}]{turned} \
             /AP << /N {} 0 R >>{value} >>",
            up.string(id, &text_bytes(&field.name))?,
            page.0,
            page.1,
            num(x1),
            num(y1),
            num(x2),
            num(y2),
            ap.0
        );
        up.objects.insert(id, widget.into_bytes());
        new_ids.push(id);
        by_page.entry(field.page).or_default().push(id);
    }

    for (number, ids) in &by_page {
        let page = pages[number];
        let dict = doc.get_dictionary(page).map_err(read_error)?;
        let src = up.source(page)?;
        if let Some(annots) = up.extend_array(dict, &src, b"Annots", &references(ids))? {
            up.patch(page, &[(b"Annots", Some(&annots))])?;
        }
    }

    // The form: list the new fields, mark it signed, and drop any XFA version of it, which
    // Acrobat would otherwise keep showing instead of these changes.
    let catalog = doc.get_dictionary(root).map_err(read_error)?;
    let acro_id = catalog.get(b"AcroForm").and_then(Object::as_reference).ok().filter(|id| doc.get_dictionary(*id).is_ok());
    let acro_inline = acro_id.is_none() && catalog.get(b"AcroForm").and_then(Object::as_dict).is_ok();
    let empty = Dictionary::new();
    let acro = acroform(&doc).filter(|_| acro_id.is_some() || acro_inline).unwrap_or(&empty);
    let acro_src = match acro_id {
        Some(id) => up.source(id)?,
        None if acro_inline => raw::get(&up.source(root)?, b"AcroForm").ok_or("Could not read the PDF's form.")?.to_vec(),
        None => b"<< >>".to_vec(),
    };
    let mut edits: Vec<(&[u8], Option<Vec<u8>>)> = Vec::new();
    if !new_ids.is_empty() {
        if let Some(list) = up.extend_array(acro, &acro_src, b"Fields", &references(&new_ids))? {
            edits.push((b"Fields", Some(list)));
        }
    }
    if signer.is_some() {
        let flags = acro.get(b"SigFlags").and_then(Object::as_i64).unwrap_or(0);
        if flags & 3 != 3 {
            // SignaturesExist | AppendOnly
            edits.push((b"SigFlags", Some((flags | 3).to_string().into_bytes())));
        }
    }
    if acro.has(b"XFA") {
        edits.push((b"XFA", None));
    }
    if !edits.is_empty() {
        let edits: Vec<(&[u8], Option<&[u8]>)> = edits.iter().map(|(k, v)| (*k, v.as_deref())).collect();
        let form = raw::patch(&acro_src, &edits).ok_or("Could not update the PDF's form.")?;
        match acro_id {
            Some(id) => {
                up.objects.insert(id, form);
            }
            None if acro_inline => up.patch(root, &[(b"AcroForm", Some(&form))])?,
            None => {
                let id = up.add(form);
                up.patch(root, &[(b"AcroForm", Some(format!("{} 0 R", id.0).as_bytes()))])?;
            }
        }
    }

    if let (Some(Err(field)), Some((sig, ap))) = (target, sig_ids) {
        let widget = doc.get_dictionary(field.widget).map_err(read_error)?;
        let rotate = widget_rotation(&doc, widget);
        let [x1, y1, x2, y2] = field.rect;
        stamp_box = Some(if rotate % 180 == 0 { (x2 - x1, y2 - y1, rotate) } else { (y2 - y1, x2 - x1, rotate) });
        let value = format!("{} 0 R", sig.0);
        let ap = format!("<< /N {} 0 R >>", ap.0);
        let mut widget_edits: Vec<(&[u8], Option<&[u8]>)> = vec![(b"AP", Some(ap.as_bytes()))];
        if !widget.has(b"F") {
            widget_edits.push((b"F", Some(b"4"))); // Print
        }
        if field.widget == field.field {
            widget_edits.push((b"V", Some(value.as_bytes())));
            up.patch(field.field, &widget_edits)?;
        } else {
            up.patch(field.field, &[(b"V", Some(value.as_bytes()))])?;
            up.patch(field.widget, &widget_edits)?;
        }
    }

    let mut shown_date = String::new();
    let mut signing = None;
    if let (Some(signer), Some((sig_id, ap_id)), Some((w, h, rotate))) = (signer, sig_ids, stamp_box) {
        (signer.unlock)()?;
        let now = chrono::Local::now();
        let zone = now.format("%z").to_string();
        let zone = format!("{}'{}'", &zone[..3], &zone[3..]);
        let pdf_date = format!("D:{}{zone}", now.format("%Y%m%d%H%M%S"));
        shown_date = format!("{} {zone}", now.format("%Y.%m.%d %H:%M:%S"));
        let stamp = stamp::appearance(w, h, signer.name, &shown_date);
        up.add_stream(ap_id, &appearance(w, h, rotate, true), &stamp)?;

        // /Contents is never encrypted, so only /Name and /M go through up.string.
        let mut sig = format!(
            "<< /Type /Sig /Filter /Adobe.PPKLite /SubFilter /adbe.pkcs7.detached /Name {} /M {} /ByteRange [",
            up.string(sig_id, &text_bytes(signer.name))?,
            up.string(sig_id, pdf_date.as_bytes())?,
        )
        .into_bytes();
        let byte_range_at = sig.len();
        sig.extend([b' '; BYTE_RANGE_RESERVE]);
        sig.extend(b"] /Contents ");
        let contents_at = sig.len();
        sig.push(b'<');
        sig.resize(sig.len() + SIG_RESERVE * 2, b'0');
        sig.extend(b"> >>");
        up.objects.insert(sig_id, sig);
        signing = Some((sig_id, byte_range_at, contents_at, signer.sign_cms));
    }

    let mut out = input.to_vec();
    if !matches!(out.last(), Some(b'\n' | b'\r')) {
        out.push(b'\n');
    }
    let mut offsets: Vec<(ObjectId, usize)> = Vec::new();
    let mut sig_body_at = 0;
    for (id, body) in &up.objects {
        offsets.push((*id, out.len()));
        out.extend(format!("{} {} obj\n", id.0, id.1).as_bytes());
        if signing.as_ref().is_some_and(|s| s.0 == *id) {
            sig_body_at = out.len();
        }
        out.extend(body);
        out.extend(b"\nendobj\n");
    }

    let xref_id = (up.next, 0);
    let size = if xref_stream { up.next + 1 } else { up.next };
    let mut trailer = format!("/Size {size} /Root {} {} R", root.0, root.1);
    if let Ok(Object::Reference(info)) = doc.trailer.get(b"Info") {
        trailer += &format!(" /Info {} {} R", info.0, info.1);
    }
    if let Some(Some((n, g))) = encrypt_ref {
        trailer += &format!(" /Encrypt {n} {g} R");
    }
    if let Ok(Object::Array(ids)) = doc.trailer.get(b"ID") {
        let ids: Vec<String> = ids.iter().filter_map(|id| id.as_str().ok()).map(|id| format!("<{}>", hex(id))).collect();
        trailer += &format!(" /ID [{}]", ids.join(" "));
    }
    trailer += &format!(" /Prev {prev}");

    let xref_at = out.len();
    if xref_stream {
        offsets.push((xref_id, xref_at));
    }
    let mut runs: Vec<&[(ObjectId, usize)]> = Vec::new();
    let mut start = 0;
    for i in 1..=offsets.len() {
        if i == offsets.len() || offsets[i].0 .0 != offsets[i - 1].0 .0 + 1 {
            runs.push(&offsets[start..i]);
            start = i;
        }
    }
    if xref_stream {
        let index: Vec<String> = runs.iter().map(|run| format!("{} {}", run[0].0 .0, run.len())).collect();
        let mut data = Vec::new();
        for ((_, generation), offset) in &offsets {
            data.push(1u8);
            data.extend((*offset as u32).to_be_bytes());
            data.extend(generation.to_be_bytes());
        }
        out.extend(
            format!(
                "{} 0 obj\n<< /Type /XRef /W [1 4 2] /Index [{}] {trailer} /Length {} >>\nstream\n",
                xref_id.0,
                index.join(" "),
                data.len()
            )
            .as_bytes(),
        );
        out.extend(data);
        out.extend(b"\nendstream\nendobj\n");
    } else {
        out.extend(b"xref\n");
        for run in &runs {
            out.extend(format!("{} {}\n", run[0].0 .0, run.len()).as_bytes());
            for ((_, generation), offset) in run.iter() {
                out.extend(format!("{offset:010} {generation:05} n\r\n").as_bytes());
            }
        }
        out.extend(format!("trailer\n<< {trailer} >>\n").as_bytes());
    }
    out.extend(format!("startxref\n{xref_at}\n%%EOF\n").as_bytes());

    if let Some((_, byte_range_at, contents_at, sign_cms)) = signing {
        // Fill in /ByteRange, then sign everything except the /Contents placeholder.
        let contents_start = sig_body_at + contents_at;
        let contents_end = contents_start + SIG_RESERVE * 2 + 2;
        let byte_range = format!("0 {contents_start} {contents_end} {}", out.len() - contents_end);
        let at = sig_body_at + byte_range_at;
        out[at..at + byte_range.len()].copy_from_slice(byte_range.as_bytes());

        let signed: Vec<u8> = [&out[..contents_start], &out[contents_end..]].concat();
        let cms = sign_cms(&signed)?;
        if cms.len() > SIG_RESERVE {
            return Err("The signature is larger than the space reserved for it.".into());
        }
        let cms_hex = hex(&cms);
        out[contents_start + 1..contents_start + 1 + cms_hex.len()].copy_from_slice(cms_hex.as_bytes());
    }
    Ok((out, shown_date))
}

#[cfg(test)]
mod tests {
    use super::*;
    use lopdf::xref::XrefType;
    use lopdf::{dictionary, SaveOptions};

    /// A DER SEQUENCE that stands in for the CMS signature.
    const FAKE_CMS: [u8; 5] = [0x30, 0x03, 0x02, 0x01, 0x00];

    /// One page with an empty signature field "Approver", saved with xref streams and object
    /// streams when `modern`.
    fn form(modern: bool) -> Vec<u8> {
        let mut doc = Document::with_version("1.7");
        let pages = doc.new_object_id();
        let field = doc.new_object_id();
        let page = doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages,
            "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
            "Annots" => vec![field.into()],
        });
        doc.objects.insert(
            field,
            Object::Dictionary(dictionary! {
                "Type" => "Annot",
                "Subtype" => "Widget",
                "FT" => "Sig",
                "T" => Object::string_literal("Approver"),
                "Rect" => vec![100.into(), 100.into(), 320.into(), 132.into()],
                "P" => page,
            }),
        );
        doc.objects.insert(
            pages,
            Object::Dictionary(dictionary! { "Type" => "Pages", "Kids" => vec![page.into()], "Count" => 1 }),
        );
        let acro = doc.add_object(dictionary! { "Fields" => vec![field.into()] });
        let catalog = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages, "AcroForm" => acro });
        doc.trailer.set("Root", catalog);
        doc.reference_table.cross_reference_type =
            if modern { XrefType::CrossReferenceStream } else { XrefType::CrossReferenceTable };
        let options = SaveOptions::builder().use_xref_streams(modern).use_object_streams(modern).build();
        let mut out = Vec::new();
        doc.save_with_options(&mut out, options).unwrap();
        out
    }

    /// A classic-xref PDF whose objects 1, 2, ... are `objects`, written exactly as given.
    /// Object 1 is the catalog.
    fn pdf(objects: &[&str]) -> Vec<u8> {
        let mut out = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (i, body) in objects.iter().enumerate() {
            offsets.push(out.len());
            out.extend(format!("{} 0 obj\n{body}\nendobj\n", i + 1).as_bytes());
        }
        let xref = out.len();
        out.extend(format!("xref\n0 {}\n0000000000 65535 f\r\n", objects.len() + 1).as_bytes());
        for offset in offsets {
            out.extend(format!("{offset:010} 00000 n\r\n").as_bytes());
        }
        let size = objects.len() + 1;
        out.extend(format!("trailer\n<< /Size {size} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes());
        out
    }

    fn sign_fake(input: &[u8], new_fields: &[NewField], field: &str) -> Vec<u8> {
        sign(input, new_fields, field, "TEST.SIGNER", || Ok(()), |_| Ok(FAKE_CMS.to_vec())).unwrap().0
    }

    fn new_field(name: &str, rect: [f32; 4]) -> NewField {
        NewField { name: name.into(), page: 1, rect }
    }

    /// The last revision's bytes, as text.
    fn update_text(original: &[u8], updated: &[u8]) -> String {
        String::from_utf8_lossy(&updated[original.len()..]).into_owned()
    }

    #[test]
    fn appends_signature_as_incremental_update() {
        for modern in [false, true] {
            let original = form(modern);
            let signed = sign_fake(&original, &[], "Approver");
            assert!(signed.starts_with(&original), "original bytes must stay untouched");

            // Same kind of cross-reference section as the revision before it.
            let xref_at = last_startxref(&signed).unwrap();
            assert_eq!(signed[xref_at..].starts_with(b"xref"), !modern);

            let doc = Document::load_mem(&signed).unwrap();
            let field = &sig_fields(&doc)[0];
            let sig = doc.get_dictionary(field.signed.expect("field is signed")).unwrap();
            let range: Vec<usize> = sig.get(b"ByteRange").unwrap().as_array().unwrap()
                .iter().map(|n| n.as_i64().unwrap() as usize).collect();
            assert_eq!(range[0], 0);
            assert_eq!(range[2] + range[3], signed.len(), "covers the whole file");
            assert_eq!(signed[range[1]], b'<');
            assert_eq!(signed[range[2] - 1], b'>');
            assert_eq!(trim_der(sig.get(b"Contents").unwrap().as_str().unwrap()), FAKE_CMS);
            assert!(doc.get_dictionary(field.widget).unwrap().has(b"AP"));
            assert_eq!(acroform(&doc).unwrap().get(b"SigFlags").unwrap().as_i64().unwrap(), 3);
        }
    }

    #[test]
    fn signs_again_on_top_of_a_signed_revision() {
        let original = form(true);
        let once = sign_fake(&original, &[new_field("Reviewer", [100.0, 500.0, 200.0, 40.0])], "Approver");
        let twice = sign_fake(&once, &[], "Reviewer");
        assert!(twice.starts_with(&once));
        let doc = Document::load_mem(&twice).unwrap();
        let fields = sig_fields(&doc);
        assert_eq!(fields.len(), 2);
        assert!(fields.iter().all(|f| f.signed.is_some()));
    }

    #[test]
    fn adds_fields_to_a_pdf_without_a_form() {
        for modern in [false, true] {
            let mut doc = Document::load_mem(&form(modern)).unwrap();
            let catalog = doc.trailer.get(b"Root").unwrap().as_reference().unwrap();
            doc.get_dictionary_mut(catalog).unwrap().remove(b"AcroForm");
            let options = SaveOptions::builder().use_xref_streams(modern).use_object_streams(modern).build();
            let mut original = Vec::new();
            doc.save_with_options(&mut original, options).unwrap();

            let fields = [new_field("Boss", [72.0, 600.0, 216.0, 36.0]), new_field("Me", [324.0, 600.0, 216.0, 36.0])];
            let signed = sign_fake(&original, &fields, "Me");
            assert!(signed.starts_with(&original));
            let doc = Document::load_mem(&signed).unwrap();
            let found = sig_fields(&doc);
            let names: Vec<&str> = found.iter().map(|f| f.name.as_str()).collect();
            assert_eq!(names, ["Boss", "Me"]);
            assert!(found[0].signed.is_none() && found[1].signed.is_some());
            assert_eq!(found[1].page, Some(1));
            // 600 pt from the top of a 792 pt page.
            assert_eq!(found[1].rect, [324.0, 156.0, 540.0, 192.0]);
            let page = doc.get_pages()[&1];
            let annots = doc.get_dictionary(page).unwrap().get(b"Annots").unwrap().as_array().unwrap();
            assert_eq!(annots.len(), 3, "the old widget and both new ones");
            assert_eq!(acroform(&doc).unwrap().get(b"SigFlags").unwrap().as_i64().unwrap(), 3);

            let unsigned = add_fields(&original, &fields[..1]).unwrap();
            let doc = Document::load_mem(&unsigned).unwrap();
            assert!(acroform(&doc).unwrap().get(b"SigFlags").is_err(), "nothing is signed yet");
            assert_eq!(sig_fields(&doc).len(), 1);
        }
    }

    #[test]
    fn places_fields_on_rotated_and_cropped_pages() {
        let original = pdf(&[
            "<< /Type /Catalog /Pages 2 0 R >>",
            "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 595.27559 841.88976] /Rotate 90 >>",
            "<< /Type /Page /Parent 2 0 R /CropBox [10.5 20.25 585.27559 831.88976] /Annots 4 0 R >>",
            "[]",
        ]);
        let info = inspect(&original).unwrap();
        let [w, h] = info.pages[0];
        assert!((w - 811.64).abs() < 0.01 && (h - 574.78).abs() < 0.01, "{w} x {h}");

        let signed = sign_fake(&original, &[new_field("Boss", [100.0, 50.0, 200.0, 40.0])], "Boss");
        let doc = Document::load_mem(&signed).unwrap();
        let field = &sig_fields(&doc)[0];
        assert_eq!(field.rect, [60.5, 120.25, 100.5, 320.25]);
        let widget = doc.get_dictionary(field.widget).unwrap();
        assert_eq!(widget_rotation(&doc, widget), 90);
        let text = update_text(&original, &signed);
        assert!(text.contains("/BBox [0 0 200 40] /Matrix [0 1 -1 0 0 0]"), "{text}");
        // The Annots array is its own object, so the page itself is left alone.
        assert!(!text.contains("3 0 obj"));
        assert!(text.contains("4 0 obj\n[ "));

        let info = inspect(&signed).unwrap();
        assert_eq!(info.fields[0].rect, Some([100.0, 50.0, 200.0, 40.0]));
        for rotate in [0, 90, 180, 270] {
            let g = Geometry { crop: [10.0, 20.0, 400.0, 700.0], rotate };
            let shown = [30.0, 40.0, 120.0, 50.0];
            assert_eq!(g.rect_to_display(g.rect_to_user(shown)), shown, "rotate {rotate}");
        }
    }

    #[test]
    fn keeps_unchanged_entries_byte_for_byte() {
        let original = pdf(&[
            "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields 5 0 R /DA (/Helv 0 Tf 0 g) /XFA 6 0 R >> >>",
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
            "<</Type/Page/Parent 2 0 R/MediaBox[0 0 595.27559 841.88976]/Annots[4 0 R]/UserUnit 1.0>>",
            "<< /Type /Annot /Subtype /Link /Rect [1.123456 2 3 4] >>",
            "[]",
            "<< /Length 0 >>\nstream\n\nendstream",
        ]);
        let signed = sign_fake(&original, &[new_field("New", [10.0, 10.0, 100.0, 30.0])], "New");
        let text = update_text(&original, &signed);
        // Objects 7 and 8 are the signature and its stamp; 9 is the new field.
        let page = "<</Type/Page/Parent 2 0 R/MediaBox[0 0 595.27559 841.88976]/Annots [4 0 R 9 0 R]/UserUnit 1.0>>";
        assert!(text.contains(&format!("3 0 obj\n{page}")), "{text}");
        assert!(text.contains("5 0 obj\n[ 9 0 R]"), "{text}");
        let doc = Document::load_mem(&signed).unwrap();
        let (catalog, _) = raw::source(&doc, &signed, (1, 0)).unwrap();
        let form = raw::get(&catalog, b"AcroForm").unwrap();
        assert_eq!(raw::get(form, b"DA").unwrap(), b"(/Helv 0 Tf 0 g)");
        assert_eq!(raw::get(form, b"SigFlags").unwrap(), b"3");
        assert!(raw::get(form, b"XFA").is_none(), "the XFA form is dropped");
    }

    #[test]
    fn checks_everything_before_asking_for_the_pin() {
        let signed = sign_fake(&form(false), &[], "Approver");
        let no_pin = || panic!("the PIN must not be requested");
        let attempt = |fields: &[NewField], field: &str| sign(&signed, fields, field, "X", no_pin, |_| unreachable!()).unwrap_err();
        assert!(attempt(&[], "Approver").contains("already signed"));
        assert!(attempt(&[], "Missing").contains("no signature field"));
        assert!(attempt(&[new_field("Approver", [0.0, 0.0, 50.0, 20.0])], "Approver").contains("already a field"));
        assert!(attempt(&[new_field("A.B", [0.0, 0.0, 50.0, 20.0])], "A.B").contains("periods"));
        assert!(attempt(&[new_field("Tiny", [0.0, 0.0, 1.0, 20.0])], "Tiny").contains("too small"));
        let elsewhere = NewField { name: "Far".into(), page: 9, rect: [0.0, 0.0, 50.0, 20.0] };
        assert!(attempt(&[elsewhere], "Far").contains("no page 9"));
    }

    #[test]
    fn respects_certification() {
        let certified = |p: u8| {
            pdf(&[
                "<< /Type /Catalog /Pages 2 0 R /AcroForm 5 0 R /Perms << /DocMDP 6 0 R >> >>",
                "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 612 792] >>",
                "<< /Type /Page /Parent 2 0 R /Annots [4 0 R] >>",
                "<< /Type /Annot /Subtype /Widget /FT /Sig /T (Open) /Rect [0 0 100 30] /P 3 0 R >>",
                "<< /Fields [4 0 R] /SigFlags 3 >>",
                &format!("<< /Type /Sig /Reference [<< /TransformMethod /DocMDP /TransformParams << /P {p} >> >>] >>"),
            ])
        };
        let new = [new_field("Extra", [0.0, 0.0, 100.0, 30.0])];
        let try_sign = |pdf: &[u8], fields: &[NewField], field: &str| {
            sign(pdf, fields, field, "X", || Ok(()), |_| Ok(FAKE_CMS.to_vec())).map(|_| ())
        };
        assert_eq!(try_sign(&certified(1), &[], "Open").unwrap_err(), NO_CHANGES);
        assert_eq!(try_sign(&certified(2), &new, "Extra").unwrap_err(), FILL_ONLY);
        assert!(try_sign(&certified(2), &[], "Open").is_ok());
        assert!(try_sign(&certified(3), &new, "Extra").is_ok());
        let info = inspect(&certified(2)).unwrap();
        assert!(info.can_sign && !info.can_add);
    }
}
