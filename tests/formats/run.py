"""Format tests: builds PDFs the way different programs write them, then adds and signs
fields with the app's PDF code (examples/pdftool.rs, signed with a throwaway test key) and
checks every result with qpdf, pyHanko, Poppler and PDFKit.

    .venv/bin/python tests/formats/run.py [work dir]

Needs Python packages reportlab, pypdf, pyhanko and pillow, plus qpdf and Poppler
(Homebrew). Ghostscript, ImageMagick and Google Chrome are used when present.
"""

import datetime
import json
import shutil
import subprocess
import sys
import traceback
from pathlib import Path

from cryptography import x509
from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.asymmetric import rsa
from cryptography.x509.oid import NameOID
from PIL import Image, ImageChops

HERE = Path(__file__).resolve().parent
APP = HERE.parent.parent / "src-tauri"
WORK = Path(sys.argv[1] if len(sys.argv) > 1 else "/tmp/cac-signer-formats").resolve()
TOOL = APP / "target/release/examples/pdftool"
CHROME = Path("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome")

# Where the tests put fields, in points from the displayed page's top-left corner.
FIRST = [72, 72, 216, 40]
SECOND = [72, 150, 216, 40]


def run(*cmd, **kw):
    return subprocess.run([str(c) for c in cmd], check=True, capture_output=True, **kw)


# Test certificates

def make_keys():
    keys = WORK / "keys"
    keys.mkdir(parents=True, exist_ok=True)
    now = datetime.datetime.now(datetime.timezone.utc)

    def cert(subject, issuer, key, issuer_key, ca):
        builder = (
            x509.CertificateBuilder()
            .subject_name(x509.Name([x509.NameAttribute(NameOID.COMMON_NAME, subject)]))
            .issuer_name(x509.Name([x509.NameAttribute(NameOID.COMMON_NAME, issuer)]))
            .public_key(key.public_key())
            .serial_number(x509.random_serial_number())
            .not_valid_before(now - datetime.timedelta(days=1))
            .not_valid_after(now + datetime.timedelta(days=30))
            .add_extension(x509.BasicConstraints(ca=ca, path_length=None), critical=True)
            .add_extension(
                x509.KeyUsage(
                    digital_signature=not ca, content_commitment=not ca, key_encipherment=False,
                    data_encipherment=False, key_agreement=False, key_cert_sign=ca, crl_sign=ca,
                    encipher_only=False, decipher_only=False,
                ),
                critical=True,
            )
        )
        return builder.sign(issuer_key, hashes.SHA256())

    ca_key = rsa.generate_private_key(public_exponent=65537, key_size=2048)
    ca = cert("Format Test CA", "Format Test CA", ca_key, ca_key, True)
    key = rsa.generate_private_key(public_exponent=65537, key_size=2048)
    signer = cert("TEST.SIGNER.FORMATS.1234567890", "Format Test CA", key, ca_key, False)
    pem = serialization.Encoding.PEM
    (keys / "ca.pem").write_bytes(ca.public_bytes(pem))
    (keys / "signer.pem").write_bytes(signer.public_bytes(pem))
    (keys / "key.pem").write_bytes(
        key.private_bytes(pem, serialization.PrivateFormat.PKCS8, serialization.NoEncryption())
    )
    (keys / "cms.py").write_text(
        "import sys\n"
        "from cryptography import x509\n"
        "from cryptography.hazmat.primitives import hashes, serialization\n"
        "from cryptography.hazmat.primitives.serialization import pkcs7\n"
        "key = serialization.load_pem_private_key(open(sys.argv[1], 'rb').read(), None)\n"
        "certs = [x509.load_pem_x509_certificate(open(p, 'rb').read()) for p in sys.argv[2:]]\n"
        "builder = pkcs7.PKCS7SignatureBuilder().set_data(sys.stdin.buffer.read())\n"
        "builder = builder.add_signer(certs[0], key, hashes.SHA256())\n"
        "for c in certs[1:]:\n"
        "    builder = builder.add_certificate(c)\n"
        "opts = [pkcs7.PKCS7Options.DetachedSignature, pkcs7.PKCS7Options.Binary]\n"
        "sys.stdout.buffer.write(builder.sign(serialization.Encoding.DER, opts))\n"
    )
    return keys


# The corpus

def reportlab_plain(out, pages=1):
    from reportlab.lib.pagesizes import letter
    from reportlab.pdfgen import canvas

    c = canvas.Canvas(str(out), pagesize=letter)
    for n in range(pages):
        c.setFont("Helvetica-Bold", 20)
        c.drawString(72, 720, f"Format test, page {n + 1}")
        c.setFont("Helvetica", 11)
        for i, y in enumerate(range(690, 100, -18)):
            c.drawString(72, y, f"Line {i + 1}: the quick brown fox jumps over the lazy dog.")
        c.rect(72, 60, 468, 30)
        c.drawString(78, 70, "Signature:")
        c.showPage()
    c.save()


def reportlab_form(out):
    from reportlab.lib.pagesizes import letter
    from reportlab.pdfgen import canvas

    c = canvas.Canvas(str(out), pagesize=letter)
    c.setFont("Helvetica", 12)
    c.drawString(72, 720, "Name:")
    c.acroForm.textfield(name="Name", x=130, y=712, width=300, height=20, value="Jane Q. Tester")
    c.drawString(72, 680, "Unit:")
    c.acroForm.textfield(name="Unit", x=130, y=672, width=300, height=20, value="Example Unit")
    c.showPage()
    c.save()


def _written(writer):
    import io

    buf = io.BytesIO()
    writer.write(buf)
    return buf.getvalue()


def with_pypdf(src, out, change):
    from pypdf import PdfWriter

    w = PdfWriter(clone_from=str(src))
    change(w)
    w.write(str(out))


def swift(code, *args):
    script = WORK / "tmp.swift"
    script.write_text(code)
    run("swift", script, *args)


QUARTZ = r"""
import CoreGraphics
import CoreText
import Foundation
let url = URL(fileURLWithPath: CommandLine.arguments[1]) as CFURL
var box = CGRect(x: 0, y: 0, width: 595.28, height: 841.89)
let ctx = CGContext(url, mediaBox: &box, nil)!
for page in 1...2 {
    ctx.beginPDFPage(nil)
    let font = CTFontCreateWithName("Helvetica" as CFString, 14, nil)
    for i in 0..<30 {
        let text = NSAttributedString(string: "Quartz page \(page), line \(i + 1)", attributes: [kCTFontAttributeName as NSAttributedString.Key: font])
        ctx.textPosition = CGPoint(x: 60, y: 780 - Double(i) * 22)
        CTLineDraw(CTLineCreateWithAttributedString(text), ctx)
    }
    ctx.endPDFPage()
}
ctx.closePDF()
"""

PDFKIT_RESAVE = r"""
import PDFKit
let doc = PDFDocument(url: URL(fileURLWithPath: CommandLine.arguments[1]))!
doc.write(to: URL(fileURLWithPath: CommandLine.arguments[2]))
"""


def build_corpus():
    from pypdf.generic import (
        ArrayObject, BooleanObject, DecodedStreamObject, DictionaryObject, FloatObject,
        NameObject, NumberObject, TextStringObject,
    )

    src = WORK / "corpus"
    src.mkdir(parents=True, exist_ok=True)
    cases = {}

    def case(name, producer, build):
        path = src / f"{name}.pdf"
        try:
            build(path)
            cases[name] = (producer, path)
        except Exception as e:  # a missing tool skips its cases
            cases[name] = (producer, None)
            print(f"  skipped {name}: {e}")

    plain = src / "reportlab_plain.pdf"
    form = src / "reportlab_form.pdf"
    case("reportlab_plain", "ReportLab", plain_build := (lambda p: reportlab_plain(p, pages=2)))
    case("reportlab_form", "ReportLab, text fields", reportlab_form)
    case("quartz", "macOS Quartz", lambda p: swift(QUARTZ, p))
    case("pdfkit_resave", "macOS PDFKit re-save", lambda p: swift(PDFKIT_RESAVE, form, p))

    def chrome(p):
        html = WORK / "page.html"
        rows = "".join(f"<p>Paragraph {i}: Lorem ipsum dolor sit amet, consectetur.</p>" for i in range(80))
        html.write_text(f"<html><body style='font-family: Helvetica'><h1>Chrome print</h1>{rows}</body></html>")
        run(CHROME, "--headless", "--disable-gpu", "--no-pdf-header-footer", f"--user-data-dir={WORK / 'chrome'}",
            f"--print-to-pdf={p}", html.as_uri())
    if CHROME.exists():
        case("chrome", "Google Chrome (Skia)", chrome)
    case("cairo", "Poppler pdftocairo (cairo)", lambda p: run("pdftocairo", "-pdf", plain, p))
    if shutil.which("gs"):
        case("ghostscript", "Ghostscript pdfwrite",
             lambda p: run("gs", "-q", "-dNOPAUSE", "-dBATCH", "-sDEVICE=pdfwrite", f"-sOutputFile={p}", plain))
        case("ghostscript_pdfa", "Ghostscript PDF/A-2b",
             lambda p: run("gs", "-q", "-dNOPAUSE", "-dBATCH", "-dPDFA=2", "-dPDFACompatibilityPolicy=1",
                           "-sColorConversionStrategy=RGB", "-sDEVICE=pdfwrite", f"-sOutputFile={p}", plain))
    if shutil.which("magick"):
        def scan(p):
            png = WORK / "scan.png"
            run("pdftoppm", "-png", "-r", "100", "-singlefile", plain, png.with_suffix(""))
            run("magick", png, "-colorspace", "Gray", p)
        case("imagemagick_scan", "ImageMagick (scanned image)", scan)
    case("qpdf_objstm", "qpdf object streams", lambda p: run("qpdf", "--object-streams=generate", form, p))
    case("qpdf_linearized", "qpdf linearized", lambda p: run("qpdf", "--linearize", plain, p))
    case("qpdf_qdf", "qpdf QDF (uncompressed)", lambda p: run("qpdf", "--qdf", "--object-streams=disable", form, p))
    for angle in (90, 180, 270):
        case(f"rotate_{angle}", f"/Rotate {angle}", lambda p, a=angle: run("qpdf", f"--rotate=+{a}", plain, p))

    def inherited(w):
        pages = w._root_object["/Pages"].get_object()
        pages[NameObject("/Rotate")] = NumberObject(90)
        for page in w.pages:
            page.pop(NameObject("/Rotate"), None)  # ReportLab writes /Rotate 0 on every page
            page[NameObject("/CropBox")] = ArrayObject([FloatObject(v) for v in (36, 48.5, 560, 740.25)])
    case("inherited_rotate_cropbox", "Rotate on the page tree, CropBox", lambda p: with_pypdf(plain, p, inherited))

    def shifted(w):
        for page in w.pages:
            page[NameObject("/MediaBox")] = ArrayObject([FloatObject(v) for v in (-100, -50, 512, 742)])
    case("shifted_mediabox", "MediaBox not at 0 0", lambda p: with_pypdf(plain, p, shifted))

    def indirect_annots(w):
        link = DictionaryObject({
            NameObject("/Type"): NameObject("/Annot"), NameObject("/Subtype"): NameObject("/Link"),
            NameObject("/Rect"): ArrayObject([FloatObject(v) for v in (72, 700, 300, 740)]),
            NameObject("/Border"): ArrayObject([NumberObject(0), NumberObject(0), NumberObject(1)]),
            NameObject("/A"): DictionaryObject({NameObject("/S"): NameObject("/URI"),
                                                NameObject("/URI"): TextStringObject("https://example.com")}),
        })
        w.pages[0][NameObject("/Annots")] = w._add_object(ArrayObject([w._add_object(link)]))
    case("indirect_annots", "Annots array as its own object", lambda p: with_pypdf(plain, p, indirect_annots))

    def inline_form(w):
        root = w._root_object
        root[NameObject("/AcroForm")] = DictionaryObject(root["/AcroForm"].get_object())
    case("inline_acroform", "AcroForm inside the catalog", lambda p: with_pypdf(form, p, inline_form))

    def xfa(w, needs_rendering):
        def change(w):
            stream = DecodedStreamObject()
            stream.set_data(b'<xdp:xdp xmlns:xdp="http://ns.adobe.com/xdp/"><template/></xdp:xdp>')
            w._root_object["/AcroForm"].get_object()[NameObject("/XFA")] = w._add_object(stream)
            if needs_rendering:
                w._root_object[NameObject("/NeedsRendering")] = BooleanObject(True)
        return change
    case("xfa_static", "XFA form with regular pages", lambda p: with_pypdf(form, p, xfa(None, False)))
    case("xfa_only", "XFA-only form (NeedsRendering)", lambda p: with_pypdf(form, p, xfa(None, True)))

    def incremental(p):
        from pypdf import PdfWriter
        w = PdfWriter(str(cases["qpdf_objstm"][1]), incremental=True)
        w.add_metadata({"/Title": "Edited in an incremental update"})
        w.write(str(p))
    case("pypdf_incremental", "pypdf incremental update", incremental)

    def trailing(p):
        p.write_bytes(plain.read_bytes() + b"\n\x00\x00 trailing bytes after %%EOF\n")
    case("trailing_garbage", "bytes after %%EOF", trailing)

    def no_newline(p):
        p.write_bytes(form.read_bytes().rstrip(b"\r\n"))
    case("no_final_newline", "no newline after %%EOF", no_newline)

    def pyhanko(p, certify=None, empty_field=False):
        from pyhanko.pdf_utils.incremental_writer import IncrementalPdfFileWriter
        from pyhanko.sign import fields, signers

        keys = WORK / "keys"
        signer = signers.SimpleSigner.load(str(keys / "key.pem"), str(keys / "signer.pem"),
                                           ca_chain_files=(str(keys / "ca.pem"),))
        with open(form, "rb") as f:
            w = IncrementalPdfFileWriter(f)
            if empty_field:
                fields.append_signature_field(w, fields.SigFieldSpec("Open", box=(320, 600, 540, 640)))
            spec = fields.SigFieldSpec("First", box=(320, 700, 540, 740))
            meta = signers.PdfSignatureMetadata(
                field_name="First", certify=certify is not None,
                docmdp_permissions=certify or fields.MDPPerm.FILL_FORMS)
            out = signers.PdfSigner(meta, signer=signer, new_field_spec=spec).sign_pdf(w)
            p.write_bytes(out.getvalue())

    from pyhanko.sign.fields import MDPPerm
    case("pyhanko_signed", "signed by pyHanko", pyhanko)
    case("certified_no_changes", "certified, no changes", lambda p: pyhanko(p, MDPPerm.NO_CHANGES))
    case("certified_fill_forms", "certified, form filling", lambda p: pyhanko(p, MDPPerm.FILL_FORMS, True))

    def open_field(p):
        from pyhanko.pdf_utils.incremental_writer import IncrementalPdfFileWriter
        from pyhanko.sign import fields

        with open(form, "rb") as f:
            w = IncrementalPdfFileWriter(f)
            fields.append_signature_field(w, fields.SigFieldSpec("Open", box=(320, 600, 540, 640)))
            p.write_bytes(_written(w))
    case("open_field", "ReportLab form with an empty signature field", open_field)
    case("certified_annotate", "certified, annotations", lambda p: pyhanko(p, MDPPerm.ANNOTATE))
    case("encrypted_aes256", "AES-256, owner password only",
         lambda p: run("qpdf", "--encrypt", "", "owner-secret", "256", "--", form, p))
    case("encrypted_aes128", "AES-128, owner password only",
         lambda p: run("qpdf", "--encrypt", "", "owner-secret", "128", "--use-aes=y", "--", form, p))
    case("encrypted_rc4", "RC4 128-bit, owner password only",
         lambda p: run("qpdf", "--allow-weak-crypto", "--encrypt", "", "owner-secret", "128", "--use-aes=n", "--",
                       form, p))
    case("encrypted_objstm", "AES-256 with object streams",
         lambda p: run("qpdf", "--object-streams=generate", "--encrypt", "", "owner-secret", "256", "--", form, p))
    case("encrypted_sign_only", "AES-256, only form filling allowed",
         lambda p: run("qpdf", "--encrypt", "", "owner-secret", "256", "--modify=form", "--",
                       cases["open_field"][1], p))
    case("encrypted_no_signing", "AES-256, no changes allowed",
         lambda p: run("qpdf", "--encrypt", "", "owner-secret", "256", "--modify=none", "--", form, p))
    case("encrypted_user_password", "needs a password to open",
         lambda p: run("qpdf", "--encrypt", "user-secret", "owner-secret", "256", "--", form, p))
    return cases


# Checks

def tool(*args):
    return run(TOOL, *args).stdout.decode()


def inspect(path):
    return json.loads(tool("inspect", path))


def write_fields(name, fields):
    path = WORK / "fields" / f"{name}.json"
    path.parent.mkdir(exist_ok=True)
    path.write_text(json.dumps(fields))
    return path


def sign(src, out, fields, field, keys):
    return tool("sign", src, out, write_fields(out.stem, fields), field, "TEST.SIGNER.FORMATS.1234567890",
                sys.executable, keys / "cms.py", keys / "key.pem", keys / "signer.pem", keys / "ca.pem")


def qpdf_warnings(path):
    """qpdf's warnings, or None when qpdf finds errors."""
    r = subprocess.run(["qpdf", "--check", str(path)], capture_output=True)
    lines = (r.stdout + r.stderr).decode().replace(str(path), "FILE").splitlines()
    return {line for line in lines if line.startswith("WARNING")} if r.returncode in (0, 3) else None


def new_qpdf_problems(before, after):
    old, new = qpdf_warnings(before), qpdf_warnings(after)
    if new is None:
        return ["qpdf found errors"]
    return sorted(new - (old or set()))


def pyhanko_report(path, keys):
    from asn1crypto import pem, x509 as asn1x509
    from pyhanko.pdf_utils.reader import PdfFileReader
    from pyhanko.sign.validation import validate_pdf_signature
    from pyhanko_certvalidator import ValidationContext

    _, _, der = pem.unarmor((keys / "ca.pem").read_bytes())
    vc = ValidationContext(trust_roots=[asn1x509.Certificate.load(der)])
    with open(path, "rb") as f:
        reader = PdfFileReader(f, strict=False)
        if reader.encrypted:
            reader.decrypt("")
        report = []
        for sig in reader.embedded_signatures:
            st = validate_pdf_signature(sig, vc)
            report.append({
                "field": sig.field_name, "intact": st.intact, "valid": st.valid, "trusted": st.trusted,
                "coverage": st.coverage.name, "changes": st.modification_level.name if st.modification_level else None,
                "docmdp_ok": st.docmdp_ok,
            })
        return report


def changed_box(before, after):
    diff = ImageChops.difference(Image.open(before).convert("L"), Image.open(after).convert("L"))
    return diff.point(lambda v: 255 if v > 40 else 0).getbbox()


def placement(src, out, page, rect, size):
    """Where Poppler and PDFKit draw the change, against the rect asked for (1 px = 1 pt)."""
    tmp = WORK / "renders"
    tmp.mkdir(exist_ok=True)
    results = []
    for engine in ("poppler", "pdfkit"):
        pngs = []
        for label, pdf in (("before", src), ("after", out)):
            png = tmp / f"{out.stem}-{engine}-{label}.png"
            if engine == "poppler":
                run("pdftoppm", "-png", "-r", "72", "-cropbox", "-f", page, "-l", page, "-singlefile", pdf,
                    png.with_suffix(""))
            else:
                tool("render", pdf, page, round(size[0]), png)
            pngs.append(png)
        bbox = changed_box(*pngs)
        left, top, w, h = rect
        inside = bbox is not None and (bbox[0] >= left - 2 and bbox[1] >= top - 2
                                       and bbox[2] <= left + w + 2 and bbox[3] <= top + h + 2)
        results.append((engine, inside, bbox))
    return results


def check_case(name, producer, path, keys):
    row = {"case": name, "producer": producer, "notes": []}
    if path is None:
        row["result"] = "skipped (tool missing)"
        return row
    head = path.read_bytes()[:1024]
    xref = tool_xref_kind(path)
    row["xref"] = xref
    try:
        info = inspect(path)
    except subprocess.CalledProcessError as e:
        row["result"] = "refused: " + e.stderr.decode().strip()
        return row
    sizes = json.loads(tool("pdfkit-sizes", path))
    same = len(sizes) == len(info["pages"]) and all(
        abs(a[0] - b[0]) < 0.01 and abs(a[1] - b[1]) < 0.01 for a, b in zip(sizes, info["pages"]))
    if not same:
        row["notes"].append(f"page sizes differ from PDFKit: {info['pages']} vs {sizes}")
    row["pages"] = len(info["pages"])
    existing = [f for f in info["fields"] if not f["signature"]]
    signed_before = len([f for f in info["fields"] if f["signature"]])
    out_dir = WORK / "out"
    out_dir.mkdir(exist_ok=True)

    steps = []
    first = out_dir / f"{name}-1.pdf"
    second = out_dir / f"{name}-2.pdf"
    last_page = len(info["pages"])
    try:
        if not info["can_add"]:
            raise RuntimeError(info["notice"])
        sign(path, first, [{"name": "CheckSig1", "page": 1, "rect": FIRST}], "CheckSig1", keys)
        steps.append(("add+sign", path, first, 1, FIRST, info["pages"][0]))
        sign(first, second, [{"name": "CheckSig2", "page": last_page, "rect": SECOND}], "CheckSig2", keys)
        steps.append(("add+sign again", first, second, last_page, SECOND, info["pages"][-1]))
    except (subprocess.CalledProcessError, RuntimeError) as e:
        msg = e.stderr.decode().strip() if isinstance(e, subprocess.CalledProcessError) else str(e)
        row["notes"].append(f"new field refused: {msg}")
    if existing:
        out = out_dir / f"{name}-existing.pdf"
        try:
            sign(path, out, [], existing[0]["name"], keys)
            steps.append(("sign existing", path, out, existing[0]["page"], existing[0]["rect"],
                          info["pages"][existing[0]["page"] - 1]))
        except subprocess.CalledProcessError as e:
            row["notes"].append(f"existing field refused: {e.stderr.decode().strip()}")
    if info["can_add"]:
        unsigned = out_dir / f"{name}-fields.pdf"
        tool("add", path, unsigned, write_fields(unsigned.stem, [{"name": "Later", "page": 1, "rect": SECOND}]))
        for problem in new_qpdf_problems(path, unsigned):
            row["notes"].append(f"fields only: {problem}")
        found = [f for f in inspect(unsigned)["fields"] if f["name"] == "Later"]
        if not found or found[0]["signature"]:
            row["notes"].append("fields only: the empty field is missing")

    failures = []
    earlier_flagged = False
    for label, before, after, page, rect, size in steps:
        if not after.read_bytes().startswith(before.read_bytes()):
            failures.append(f"{label}: earlier bytes changed")
        failures += [f"{label}: {problem}" for problem in new_qpdf_problems(before, after)]
        report = pyhanko_report(after, keys)
        newest = report[-1]
        if not (newest["intact"] and newest["valid"] and newest["trusted"] and newest["coverage"] == "ENTIRE_FILE"):
            failures.append(f"{label}: new signature {newest}")
        allowed = ("FORM_FILLING", "LTA_UPDATES", "NONE", "ANNOTATIONS")
        bad = [o for o in report[:-1]
               if not (o["intact"] and o["valid"] and o["docmdp_ok"] is not False and o["changes"] in allowed)]
        baseline = {b["field"]: b for b in pyhanko_baseline(before, keys, page)} if bad else {}
        earlier_flagged = earlier_flagged or bool(bad)
        for older in bad:
            same = baseline.get(older["field"])
            if same and (same["changes"], same["docmdp_ok"]) == (older["changes"], older["docmdp_ok"]):
                row["notes"].append(f"{label}: pyHanko rates the earlier signature {older['field']} {older['changes']}, "
                                    "the same as when pyHanko adds and signs a field itself")
            else:
                failures.append(f"{label}: earlier signature {older}")
        for engine, inside, bbox in placement(before, after, page, rect, size):
            if not inside:
                failures.append(f"{label}: {engine} drew the change at {bbox}, asked for {rect}")
        row["signatures"] = len(report)
    row["steps"] = [s[0] for s in steps]
    row["result"] = "FAILED" if failures else ("ok: " + ", ".join(row["steps"]) if steps else "nothing signed")
    row["notes"] += failures
    if signed_before and not failures and not earlier_flagged:
        row["notes"].append(f"{signed_before} signature(s) already in the file still check out")
    return row


def pyhanko_baseline(src, keys, page):
    """pyHanko's verdicts on the earlier signatures after it adds and signs a field itself."""
    import io

    from pyhanko.pdf_utils.incremental_writer import IncrementalPdfFileWriter
    from pyhanko.sign import fields, signers

    signer = signers.SimpleSigner.load(str(keys / "key.pem"), str(keys / "signer.pem"),
                                       ca_chain_files=(str(keys / "ca.pem"),))
    with open(src, "rb") as f:
        w = IncrementalPdfFileWriter(f, strict=False)
        if w.prev.encrypted:
            w.encrypt("")
        spec = fields.SigFieldSpec("Baseline", box=(72, 72, 288, 112), on_page=page - 1)
        meta = signers.PdfSignatureMetadata(field_name="Baseline")
        out = signers.PdfSigner(meta, signer=signer, new_field_spec=spec).sign_pdf(w)
    path = WORK / "out" / f"{Path(src).stem}-baseline.pdf"
    path.write_bytes(out.getvalue())
    return pyhanko_report(path, keys)[:-1]


def tool_xref_kind(path):
    data = path.read_bytes()
    at = data.rfind(b"startxref")
    offset = int(data[at + 9:].split()[0])
    return "table" if data[offset:offset + 4] == b"xref" else "stream"


def main():
    WORK.mkdir(parents=True, exist_ok=True)
    for sub in ("out", "renders", "fields"):
        shutil.rmtree(WORK / sub, ignore_errors=True)
    print(f"Building pdftool and the corpus in {WORK}")
    run("cargo", "build", "--release", "--example", "pdftool", cwd=APP)
    keys = make_keys()
    cases = build_corpus()
    rows = []
    for name, (producer, path) in cases.items():
        try:
            rows.append(check_case(name, producer, path, keys))
        except Exception:
            rows.append({"case": name, "producer": producer, "result": "ERROR", "notes": [traceback.format_exc()]})
        r = rows[-1]
        print(f"{r['case']:28} {r.get('xref', ''):7} {r['result']}")
        for note in filter(None, r["notes"]):
            print(f"{'':37}{note}")
    (WORK / "report.json").write_text(json.dumps(rows, indent=1, default=str))
    failed = [r for r in rows if r["result"] in ("FAILED", "ERROR")]
    print(f"\n{len(rows) - len(failed)} of {len(rows)} cases passed or were refused on purpose; "
          f"{len(failed)} failed. Details: {WORK / 'report.json'}")
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
