//! Byte-exact edits of objects already in the file.
//!
//! lopdf reads reals as f32 and re-encodes strings, so writing its parsed objects back would
//! alter values that earlier signatures cover. Objects that an update changes are copied from
//! their original bytes instead, with only the changed entries rewritten.

use std::ops::Range;

use lopdf::xref::XrefEntry;
use lopdf::{Document, ObjectId};

fn is_space(b: u8) -> bool {
    matches!(b, b'\0' | b'\t' | b'\n' | b'\x0c' | b'\r' | b' ')
}

fn is_delimiter(b: u8) -> bool {
    b"()<>[]{}/%".contains(&b)
}

/// Position of the next token at or after `at`, past whitespace and comments.
fn skip_space(src: &[u8], mut at: usize) -> usize {
    while let Some(&b) = src.get(at) {
        if is_space(b) {
            at += 1;
        } else if b == b'%' {
            while src.get(at).is_some_and(|b| !matches!(b, b'\r' | b'\n')) {
                at += 1;
            }
        } else {
            break;
        }
    }
    at
}

/// End of the run of regular characters (a number, keyword or name body) starting at `at`.
fn word_end(src: &[u8], mut at: usize) -> usize {
    while src.get(at).is_some_and(|&b| !is_space(b) && !is_delimiter(b)) {
        at += 1;
    }
    at
}

fn number<T: std::str::FromStr>(src: &[u8], span: Range<usize>) -> Option<T> {
    let digits = src.get(span)?;
    if digits.is_empty() || !digits.iter().all(u8::is_ascii_digit) {
        return None;
    }
    std::str::from_utf8(digits).ok()?.parse().ok()
}

/// If the integer at `span` starts an `N G R` reference, the end of the `R`.
fn reference_end(src: &[u8], span: Range<usize>) -> Option<usize> {
    number::<u32>(src, span.clone())?;
    let generation = skip_space(src, span.end);
    let generation_end = word_end(src, generation);
    number::<u16>(src, generation..generation_end)?;
    let r = skip_space(src, generation_end);
    (src.get(r) == Some(&b'R') && word_end(src, r) == r + 1).then_some(r + 1)
}

/// Span of the object that starts at the first token at or after `at`. `12 0 R` counts as one.
pub fn value(src: &[u8], at: usize) -> Option<Range<usize>> {
    let start = skip_space(src, at);
    let end = match *src.get(start)? {
        b'<' if src.get(start + 1) == Some(&b'<') => {
            let mut at = start + 2;
            loop {
                at = skip_space(src, at);
                if src.get(at..at + 2)? == b">>" {
                    break at + 2;
                }
                at = value(src, at)?.end;
            }
        }
        b'<' => start + src[start..].iter().position(|&b| b == b'>')? + 1,
        b'[' => {
            let mut at = start + 1;
            loop {
                at = skip_space(src, at);
                if *src.get(at)? == b']' {
                    break at + 1;
                }
                at = value(src, at)?.end;
            }
        }
        b'(' => {
            let (mut at, mut depth) = (start, 0);
            loop {
                match *src.get(at)? {
                    b'\\' => at += 1,
                    b'(' => depth += 1,
                    b')' if depth == 1 => break at + 1,
                    b')' => depth -= 1,
                    _ => {}
                }
                at += 1;
            }
        }
        b'/' => word_end(src, start + 1),
        b')' | b'>' | b']' | b'{' | b'}' => return None,
        _ => {
            let end = word_end(src, start);
            if end == start {
                return None;
            }
            reference_end(src, start..end).unwrap_or(end)
        }
    };
    Some(start..end)
}

/// Name with its `#xx` escapes decoded, without the leading slash.
fn decode_name(raw: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(raw.len());
    let mut i = 0;
    while i < raw.len() {
        let escaped = (raw[i] == b'#')
            .then(|| raw.get(i + 1..i + 3))
            .flatten()
            .and_then(|hex| u8::from_str_radix(std::str::from_utf8(hex).ok()?, 16).ok());
        match escaped {
            Some(b) => {
                out.push(b);
                i += 3;
            }
            None => {
                out.push(raw[i]);
                i += 1;
            }
        }
    }
    out
}

pub struct Entry {
    pub key: Vec<u8>,
    /// From the key's slash to the end of its value.
    span: Range<usize>,
    pub value: Range<usize>,
}

/// Top-level entries of the dictionary `src`, and the position of its closing `>>`.
pub fn entries(src: &[u8]) -> Option<(Vec<Entry>, usize)> {
    let mut at = skip_space(src, 0);
    if src.get(at..at + 2)? != b"<<" {
        return None;
    }
    at += 2;
    let mut out = Vec::new();
    loop {
        at = skip_space(src, at);
        if src.get(at..at + 2)? == b">>" {
            return Some((out, at));
        }
        let key = value(src, at)?;
        if src[key.start] != b'/' {
            return None;
        }
        let value = value(src, key.end)?;
        at = value.end;
        out.push(Entry { key: decode_name(&src[key.start + 1..key.end]), span: key.start..value.end, value });
    }
}

/// The raw value stored under `key` in the dictionary `src`.
pub fn get<'a>(src: &'a [u8], key: &[u8]) -> Option<&'a [u8]> {
    let (entries, _) = entries(src)?;
    entries.into_iter().find(|e| e.key == key).map(|e| &src[e.value])
}

/// The dictionary `src` with each `(key, Some(value))` set and each `(key, None)` removed.
/// Every other entry keeps its original bytes.
pub fn patch(src: &[u8], edits: &[(&[u8], Option<&[u8]>)]) -> Option<Vec<u8>> {
    let (entries, close) = entries(src)?;
    let mut out = Vec::with_capacity(src.len() + 64);
    let mut written = vec![false; edits.len()];
    let mut cursor = 0;
    for entry in &entries {
        let Some(i) = edits.iter().position(|(key, _)| *key == entry.key) else { continue };
        out.extend(&src[cursor..entry.span.start]);
        if let (false, Some(value)) = (written[i], edits[i].1) {
            out.extend([b"/".as_slice(), edits[i].0, b" ", value].concat());
        }
        written[i] = true;
        cursor = entry.span.end;
    }
    out.extend(&src[cursor..close]);
    for (&(key, value), written) in edits.iter().zip(written) {
        if let (false, Some(value)) = (written, value) {
            if !out.last().is_some_and(|&b| is_space(b)) {
                out.push(b' ');
            }
            out.extend([b"/".as_slice(), key, b" ", value, b" "].concat());
        }
    }
    out.extend(&src[close..]);
    Some(out)
}

/// The array `src` with `items` added at the end.
pub fn append(src: &[u8], items: &[u8]) -> Option<Vec<u8>> {
    let span = value(src, 0)?;
    if src[span.start] != b'[' {
        return None;
    }
    let close = span.end - 1;
    Some([&src[..close], b" ", items, &src[close..span.end]].concat())
}

/// The original bytes of object `id`, and whether they come from an object stream rather than
/// the file itself. None for streams and for objects that are not where the cross-reference
/// data says.
pub fn source(doc: &Document, file: &[u8], id: ObjectId) -> Option<(Vec<u8>, bool)> {
    let (src, at): (std::borrow::Cow<[u8]>, usize) = match *doc.reference_table.get(id.0)? {
        XrefEntry::Normal { offset, generation } if generation == id.1 => {
            let at = skip_space(file, offset as usize);
            let num_end = word_end(file, at);
            let generation = skip_space(file, num_end);
            let generation_end = word_end(file, generation);
            let keyword = skip_space(file, generation_end);
            if number::<u32>(file, at..num_end)? != id.0
                || number::<u16>(file, generation..generation_end)? != id.1
                || file.get(keyword..keyword + 3)? != b"obj"
            {
                return None;
            }
            (file.into(), keyword + 3)
        }
        XrefEntry::Compressed { container, .. } if id.1 == 0 => {
            let stream = doc.get_object((container, 0)).ok()?.as_stream().ok()?;
            let first = usize::try_from(stream.dict.get(b"First").ok()?.as_i64().ok()?).ok()?;
            let count = stream.dict.get(b"N").ok()?.as_i64().ok()?;
            let content = stream.decompressed_content().ok()?;
            let mut at = 0;
            let mut offset = None;
            for _ in 0..count {
                let num = value(&content, at)?;
                let off = value(&content, num.end)?;
                at = off.end;
                if number::<u32>(&content, num) == Some(id.0) {
                    offset = number::<usize>(&content, off);
                    break;
                }
            }
            (content.into(), first.checked_add(offset?)?)
        }
        _ => return None,
    };
    let span = value(&src, at)?;
    // A dictionary followed by `stream` is a stream object, which is never patched here.
    let after = skip_space(&src, span.end);
    if src.get(after..after + 6) == Some(b"stream") {
        return None;
    }
    Some((src[span].to_vec(), matches!(src, std::borrow::Cow::Owned(_))))
}

/// The bytes a literal string's body stands for, with its escapes resolved.
fn unescape(body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(body.len());
    let mut i = 0;
    while i < body.len() {
        match body[i] {
            b'\\' => {
                i += 1;
                match body.get(i) {
                    Some(b'n') => out.push(b'\n'),
                    Some(b'r') => out.push(b'\r'),
                    Some(b't') => out.push(b'\t'),
                    Some(b'b') => out.push(8),
                    Some(b'f') => out.push(12),
                    // A backslash at the end of a line continues the string on the next one.
                    Some(b'\r') => i += usize::from(body.get(i + 1) == Some(&b'\n')),
                    Some(b'\n') | None => {}
                    Some(b'0'..=b'7') => {
                        let digits = body[i..].iter().take(3).take_while(|b| (b'0'..=b'7').contains(b)).count();
                        let value = body[i..i + digits].iter().fold(0u32, |v, d| v * 8 + u32::from(d - b'0'));
                        out.push(value as u8);
                        i += digits - 1;
                    }
                    Some(&c) => out.push(c),
                }
                i += 1;
            }
            // Any end of line inside a string reads as a single line feed.
            b'\r' => {
                out.push(b'\n');
                i += 1 + usize::from(body.get(i + 1) == Some(&b'\n'));
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    out
}

fn unhex(body: &[u8]) -> Vec<u8> {
    let digits: Vec<u8> = body
        .iter()
        .filter_map(|&b| (b as char).to_digit(16).map(|d| d as u8))
        .collect();
    digits.chunks(2).map(|pair| pair[0] << 4 | pair.get(1).copied().unwrap_or(0)).collect()
}

/// `src` with every string replaced by `f` of its bytes, written in hex. Everything else keeps
/// its original bytes.
pub fn map_strings(src: &[u8], mut f: impl FnMut(&[u8]) -> Option<Vec<u8>>) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(src.len());
    let mut at = 0;
    while at < src.len() {
        let start = skip_space(src, at);
        out.extend(&src[at..start]);
        let Some(&b) = src.get(start) else { break };
        let end = match b {
            b'(' | b'<' if src.get(start + 1) != Some(&b'<') => {
                let end = value(src, start)?.end;
                let body = &src[start + 1..end - 1];
                let bytes = if b == b'(' { unescape(body) } else { unhex(body) };
                let hex: String = f(&bytes)?.iter().map(|b| format!("{b:02X}")).collect();
                out.extend(format!("<{hex}>").as_bytes());
                at = end;
                continue;
            }
            b'<' | b'>' => start + 2,
            b'[' | b']' | b'{' | b'}' => start + 1,
            b'/' => word_end(src, start + 1),
            _ => word_end(src, start).max(start + 1),
        };
        out.extend(&src[start..end.min(src.len())]);
        at = end;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spans_cover_whole_objects() {
        let src = b"<< /A 12 0 R /B [1 0 R 2 (x\\)y(z)) <0A>] /C<</D 3>> /E -0.5 /F true >>";
        let (entries, close) = entries(src).unwrap();
        let keys: Vec<&[u8]> = entries.iter().map(|e| e.key.as_slice()).collect();
        assert_eq!(keys, [b"A".as_slice(), b"B", b"C", b"E", b"F"]);
        assert_eq!(&src[entries[0].value.clone()], b"12 0 R");
        assert_eq!(&src[entries[1].value.clone()], b"[1 0 R 2 (x\\)y(z)) <0A>]");
        assert_eq!(&src[entries[2].value.clone()], b"<</D 3>>");
        assert_eq!(&src[close..], b">>");
    }

    #[test]
    fn patch_keeps_other_entries_byte_for_byte() {
        let src = b"<</Type/Page/MediaBox[0 0 595.27559 841.88976]/Ann#6Fts[5 0 R]%c\n/Rotate 90>>";
        let out = patch(src, &[(b"Annots", Some(b"[5 0 R 9 0 R]")), (b"Rotate", None), (b"P", Some(b"1 0 R"))])
            .unwrap();
        assert_eq!(out, b"<</Type/Page/MediaBox[0 0 595.27559 841.88976]/Annots [5 0 R 9 0 R]%c\n/P 1 0 R >>");
        assert_eq!(append(b"[5 0 R]", b"9 0 R").unwrap(), b"[5 0 R 9 0 R]");
        assert_eq!(get(src, b"MediaBox").unwrap(), b"[0 0 595.27559 841.88976]");
    }

    #[test]
    fn map_strings_reads_every_string_form() {
        let src = b"<< /A (a\\(b\\)\\101\\\ncd\\n) /B <48 49 5> /C [(x)] /D<</E 1 0 R>> >>";
        let mut seen = Vec::new();
        let out = map_strings(src, |s| {
            seen.push(s.to_vec());
            Some(b"!".to_vec())
        })
        .unwrap();
        assert_eq!(seen, [b"a(b)Acd\n".to_vec(), b"HIP".to_vec(), b"x".to_vec()]);
        assert_eq!(out, b"<< /A <21> /B <21> /C [<21>] /D<</E 1 0 R>> >>");
    }
}
