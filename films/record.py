"""Makes the video lessons, their GIFs and the README's pictures.

    python films/record.py                     # every lesson in docs/lessons, then the pictures
    python films/record.py 02-add-a-field      # one lesson
    python films/record.py --render [lesson]   # voice and film again from the last capture
    python films/record.py --capture [lesson]  # the stills only, to check a script

Three steps make a film:
1. Capture: the app's own pages run in WebKit with a stand-in for the native side, which calls
   the real PDF code (src-tauri/examples/pdftool.rs). A demo card holder signs with a throwaway
   key, so no real card, PIN, person or path shows anywhere.
2. Voice: Kokoro reads each line on this Mac.
3. Edit: Remotion, in films/, turns the stills and the voice into the film.

films/README.md has the setup and what a script can say.
"""

import argparse
import datetime
import hashlib
import json
import os
import re
import shlex
import shutil
import subprocess
import sys
import time
from pathlib import Path
from urllib.parse import urlsplit

from playwright.sync_api import sync_playwright

ROOT = Path(__file__).resolve().parent.parent
FILMS = ROOT / "films"
BUILD = FILMS / "build"
LESSONS = ROOT / "docs/lessons"
VIDEOS = ROOT / "docs/videos"
PICTURES = ROOT / "docs/screenshots"
UI = ROOT / "ui"
TOOL = ROOT / "src-tauri/target/release/examples/pdftool"
ORIGIN = "http://cac-signer.local"

SIZE = {"width": 1280, "height": 720}
VOICE = "af_heart"
VOICE_HOME = Path(os.environ.get("CAC_SIGNER_VOICE", "~/.cache/cac-signer-voice")).expanduser()
# Words the voice says better spelled another way. The caption keeps the real spelling.
SPOKEN = {"CAC": "kack", "PIN": "pin"}
GIF = {"width": 1280, "fps": 15, "quality": 90}

# The demo card holder, another signer, and the folder the demo files seem to be in.
HOLDER = "DOE.JANE.Q.1234567890"
REVIEWER = "ROE.RICHARD.M.1234567891"
FOLDER = "/Users/jane/Documents"
IDENTITY = {"id": "demo", "kind": "Digital Signature", "name": HOLDER, "issuer": "DOD EMAIL CA-72",
            "expires": "2029-01-31", "expired": False}
# What must never be on a still: the name and home of whoever runs this, and any folder but the demo one.
NOT_DEMO = re.compile("|".join([rf"\b{re.escape(Path.home().name)}\b", re.escape(str(Path.home())),
                                r"/(?:Users/(?!jane/)|private/|tmp/|var/)"]), re.I)
BROKEN = re.compile(r"\bNaN\b|\bundefined\b")

# The bridge to the native side, answered by this script instead of the app.
BRIDGE = """
window.__demo = { open: null, save: null, release: null };
window.__TAURI__ = {
  core: {
    invoke: async (cmd, args) => {
      // Signing waits here, so a still can show the app while macOS would ask for the PIN.
      if (cmd === 'sign_pdf') await new Promise((go) => (window.__demo.release = go));
      const answer = await fetch('/invoke/' + cmd, { method: 'POST', body: JSON.stringify(args ?? {}) });
      if (!answer.ok) throw await answer.text();
      return cmd === 'render_page' ? answer.arrayBuffer() : answer.json();
    },
  },
  dialog: { open: async () => window.__demo.open, save: async () => window.__demo.save, ask: async () => true },
  webview: { getCurrentWebview: () => ({ onDragDropEvent: () => {} }) },
  event: { listen: async () => () => {} },
};
"""
# Every page in view has its picture.
DRAWN = """() => [...document.querySelectorAll('.page')].every((page) => {
  const { top, bottom } = page.getBoundingClientRect();
  const img = page.querySelector('img');
  return bottom < 0 || top > innerHeight || (img.getAttribute('src') && img.complete && img.naturalWidth > 0);
})"""

CMS = """
import sys
from cryptography import x509
from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.serialization import pkcs7
key = serialization.load_pem_private_key(open(sys.argv[1], 'rb').read(), None)
certs = [x509.load_pem_x509_certificate(open(p, 'rb').read()) for p in sys.argv[2:]]
builder = pkcs7.PKCS7SignatureBuilder().set_data(sys.stdin.buffer.read()).add_signer(certs[0], key, hashes.SHA256())
for cert in certs[1:]:
    builder = builder.add_certificate(cert)
sys.stdout.buffer.write(builder.sign(serialization.Encoding.DER, [pkcs7.PKCS7Options.DetachedSignature, pkcs7.PKCS7Options.Binary]))
"""

SPEAK = """
import json, os, soundfile, sys
from kokoro_onnx import Kokoro
home, voice = sys.argv[1:]
kokoro = Kokoro(f"{home}/kokoro-v1.0.onnx", f"{home}/voices-v1.0.bin")
for text, path in json.load(sys.stdin):
    samples, rate = kokoro.create(text, voice=voice)
    soundfile.write(f"{path}.part", samples, rate, format="WAV")
    os.replace(f"{path}.part", path)
"""


class Stopped(Exception):
    pass


def run(*command, **more):
    return subprocess.run([str(part) for part in command], check=True, capture_output=True, **more)


def tool(*args):
    return run(TOOL, *args).stdout.decode()


# The demo: two signers with throwaway keys, and the sample PDFs.

def make_keys(folder):
    from cryptography import x509
    from cryptography.hazmat.primitives import hashes, serialization
    from cryptography.hazmat.primitives.asymmetric import rsa
    from cryptography.x509.oid import NameOID

    folder.mkdir(parents=True, exist_ok=True)
    now = datetime.datetime.now(datetime.timezone.utc)
    name = lambda cn: x509.Name([x509.NameAttribute(NameOID.COMMON_NAME, cn)])

    def cert(subject, key, issuer, issuer_key, ca):
        usage = x509.KeyUsage(digital_signature=not ca, content_commitment=not ca, key_encipherment=False,
                              data_encipherment=False, key_agreement=False, key_cert_sign=ca, crl_sign=ca,
                              encipher_only=False, decipher_only=False)
        return (x509.CertificateBuilder().subject_name(name(subject)).issuer_name(name(issuer))
                .public_key(key.public_key()).serial_number(x509.random_serial_number())
                .not_valid_before(now - datetime.timedelta(days=1)).not_valid_after(now + datetime.timedelta(days=30))
                .add_extension(x509.BasicConstraints(ca=ca, path_length=None), critical=True)
                .add_extension(usage, critical=True).sign(issuer_key, hashes.SHA256()))

    pem = serialization.Encoding.PEM
    ca_key = rsa.generate_private_key(public_exponent=65537, key_size=2048)
    ca = cert("Demo CA", ca_key, "Demo CA", ca_key, True)
    (folder / "ca.pem").write_bytes(ca.public_bytes(pem))
    for who in (HOLDER, REVIEWER):
        key = rsa.generate_private_key(public_exponent=65537, key_size=2048)
        (folder / f"{who}.pem").write_bytes(cert(who, key, "Demo CA", ca_key, False).public_bytes(pem))
        (folder / f"{who}.key").write_bytes(
            key.private_bytes(pem, serialization.PrivateFormat.PKCS8, serialization.NoEncryption()))
    (folder / "cms.py").write_text(CMS)


def sign(source, target, field, who, new_fields=()):
    """Signs `field` in `source` as `who`, with the demo keys. Returns the time on the stamp."""
    fields = BUILD / "fields.json"
    fields.write_text(json.dumps(list(new_fields)))
    keys = BUILD / "keys"
    return tool("sign", source, target, fields, field, who, sys.executable, keys / "cms.py",
                keys / f"{who}.key", keys / f"{who}.pem", keys / "ca.pem").strip()


def add_fields(source, target, new_fields):
    fields = BUILD / "fields.json"
    fields.write_text(json.dumps(list(new_fields)))
    tool("add", source, target, fields)


def page(path, draw):
    from reportlab.lib.pagesizes import letter
    from reportlab.pdfgen import canvas

    c = canvas.Canvas(str(path), pagesize=letter)
    c.setTitle(path.stem)
    c.setFillColorRGB(0.93, 0.94, 0.96)
    c.rect(0, 702, 612, 90, stroke=0, fill=1)
    c.setFillColorRGB(0.1, 0.1, 0.12)
    draw(c)
    c.showPage()
    c.save()


def training_request(path):
    """A filled-in form with two empty signature fields."""
    rows = [("Name", "DOE, JANE Q."), ("Grade", "SSgt"), ("Unit", "Example Squadron"),
            ("Course", "Advanced First Aid"), ("Dates", "1 to 5 December 2026"),
            ("Location", "Training Center, Building 12"), ("Cost", "$450, unit training funds")]

    def draw(c):
        c.setFont("Helvetica-Bold", 22)
        c.drawString(54, 742, "TRAINING REQUEST")
        c.setFont("Helvetica", 10)
        c.drawString(54, 722, "Example Squadron  ·  sample form for the CAC Signer demo")
        y = 650
        for label, value in rows:
            c.setFont("Helvetica", 11)
            c.drawString(54, y + 7, label)
            c.acroForm.textfield(name=label, x=160, y=y, width=398, height=22, value=value, fontSize=11,
                                 borderColor=None, borderWidth=0, fillColor=None, textColor=None)
            c.setStrokeColorRGB(0.75, 0.77, 0.8)
            c.line(160, y, 558, y)
            y -= 34
        c.setFont("Helvetica-Bold", 11)
        c.drawString(54, 392, "Justification")
        c.setFont("Helvetica", 11)
        for i, text in enumerate([
            "The course certifies the member as the section's first aid instructor, a post open",
            "since August. Seats are held until 15 November; the unit has the funds this quarter.",
        ]):
            c.drawString(54, 372 - i * 16, text)
        for x, label in ((54, "Requester signature"), (324, "Supervisor signature")):
            c.setStrokeColorRGB(0.2, 0.2, 0.22)
            c.line(x, 236, x + 234, 236)
            c.setFont("Helvetica", 10)
            c.drawString(x, 222, label)

    blank = BUILD / "blank.pdf"
    page(blank, draw)
    add_fields(blank, path, [{"name": "Requester Signature", "page": 1, "rect": [54, 512, 234, 42]},
                             {"name": "Supervisor Signature", "page": 1, "rect": [324, 512, 234, 42]}])


def memo(path):
    """A memo with no signature fields, and blank lines for three signatures."""
    def draw(c):
        c.setFont("Helvetica-Bold", 18)
        c.drawString(72, 742, "EXAMPLE SQUADRON")
        c.setFont("Helvetica", 10)
        c.drawString(72, 724, "Training Office  ·  sample memo for the CAC Signer demo")
        c.setFont("Helvetica", 11)
        c.drawRightString(540, 672, "5 November 2026")
        for i, text in enumerate(["MEMORANDUM FOR ALL MEMBERS", "FROM: Training Office",
                                  "SUBJECT: December Training Schedule"]):
            c.drawString(72, 640 - i * 18, text)
        body = [
            "1. Upgrade training resumes on 1 December. Sections send their schedules to the",
            "training office by 20 November.",
            "2. First aid, fire extinguisher and records management courses run every Tuesday",
            "and Thursday in Building 12, from 0800 to 1100.",
            "3. Members who cannot attend on their scheduled day trade slots within their",
            "section and tell the training office before the course starts.",
        ]
        for i, text in enumerate(body):
            c.drawString(72, 572 - i * 16 - (i // 2) * 10, text)
        c.drawString(324, 352, "JANE Q. DOE, SSgt")
        c.drawString(324, 338, "Training NCO")
        c.setFont("Helvetica-Bold", 11)
        c.drawString(72, 250, "Coordination")
        c.setFont("Helvetica", 10)
        for x, label in ((72, "Reviewer"), (324, "Approver")):
            c.line(x, 152, x + 216, 152)
            c.drawString(x, 138, label)

    page(path, draw)


def demo_files():
    files = BUILD / "files"
    shutil.rmtree(files, ignore_errors=True)
    files.mkdir(parents=True)
    make_keys(BUILD / "keys")
    training_request(files / "Training Request.pdf")
    memo(files / "Memo.pdf")


# The capture

def real(path):
    """The demo file a path the app was given stands for."""
    if not path.startswith(FOLDER + "/"):
        raise Stopped(f"the app asked for {path}, outside the demo folder")
    return BUILD / "files" / path[len(FOLDER) + 1:]


class Take:
    """The app in WebKit, with this script answering its native side."""

    def __init__(self, context, page):
        self.page = page
        self.waiting, self.broken = set(), None
        context.route(f"{ORIGIN}/**", self.answer)
        page.on("request", lambda request: "/invoke/" in request.url and self.waiting.add(request))
        for event in ("requestfinished", "requestfailed"):
            page.on(event, lambda request: self.waiting.discard(request))
        page.on("pageerror", lambda error: setattr(self, "broken", f"the app broke: {error}"))

    def start(self, folder):
        self.folder, self.shots, self.pictures = folder, 0, {}
        self.page.goto(f"{ORIGIN}/index.html")
        self.page.wait_for_function("document.getElementById('identity').value === 'demo'")
        self.settle()

    def answer(self, route):
        request = route.request
        path = urlsplit(request.url).path
        if not path.startswith("/invoke/"):
            file = UI / (path.lstrip("/") or "index.html")
            return route.fulfill(path=str(file)) if file.is_file() else route.fulfill(status=404)
        args = json.loads(request.post_data or "{}")
        try:
            body = self.invoke(path[len("/invoke/"):], args)
        except subprocess.CalledProcessError as e:
            return route.fulfill(status=500, body=e.stderr.decode().strip())
        if isinstance(body, bytes):
            return route.fulfill(body=body, content_type="application/octet-stream")
        route.fulfill(body=json.dumps(body), content_type="application/json")

    def invoke(self, command, args):
        if command == "list_identities":
            return [IDENTITY]
        if command == "inspect_pdf":
            info = json.loads(tool("inspect", real(args["path"])))
            for field in info["fields"]:
                sig = field["signature"]
                # The demo CA is not one this Mac trusts. A CAC signature shows as valid.
                if sig and sig["signer"] in (HOLDER, REVIEWER) and sig["status"] == "untrusted":
                    sig.update(status="valid", detail="")
            return info
        if command == "render_page":
            png = BUILD / "page.png"
            tool("render", real(args["path"]), args["page"], args["width"], png)
            return png.read_bytes()
        if command == "sign_pdf":
            time = sign(real(args["input"]), real(args["output"]), args["field"], HOLDER, args["newFields"])
            return {"output": args["output"], "signed_at": time}
        if command == "save_fields":
            add_fields(real(args["input"]), real(args["output"]), args["newFields"])
            return args["output"]
        return None

    def settle(self):
        """Until nothing has been asked of the native side for a moment and the pages in view are drawn."""
        quiet, limit = time.monotonic(), time.monotonic() + 30
        while time.monotonic() - quiet < .5:
            if self.broken:
                raise Stopped(self.broken)
            if self.waiting or not self.page.evaluate(DRAWN):
                quiet = time.monotonic()
            if time.monotonic() > limit:
                raise Stopped("the app never settled")
            self.page.wait_for_timeout(100)

    def shoot(self):
        if self.broken:
            raise Stopped(self.broken)
        shown = self.page.evaluate("""() => [document.body.innerText,
            ...[...document.querySelectorAll('input')].map((field) => field.value)].join(' ')""")
        found = NOT_DEMO.search(shown) or BROKEN.search(shown)
        if found:
            raise Stopped(f"the window shows {found.group(0)!r}, which is not for a lesson")
        self.shots += 1
        name = f"{self.shots:03}.png"
        self.page.screenshot(path=self.folder / name, animations="disabled", caret="hide")
        return name

    def find(self, label):
        """A CSS selector, or the words on the screen: a button's name first, then any text."""
        if label[:1] in "#.[":
            options = [self.page.locator(label)]
        else:
            options = [self.page.get_by_role("button", name=label, exact=True), self.page.get_by_text(label, exact=True),
                       self.page.get_by_text(label)]
        for found in options:
            shown = found.locator("visible=true")
            if shown.count():
                return shown.first
        raise Stopped(f"nothing says {label!r}")

    def box(self, found):
        """Where something is, in the window's pixels: the words for text, the whole thing for a control."""
        return found.evaluate("""(element) => {
            const { top, bottom } = element.getBoundingClientRect();
            if (top < 0 || bottom > innerHeight) element.scrollIntoView({ block: 'center', behavior: 'instant' });
            let box = element.getBoundingClientRect();
            if (!element.matches('button, input, select, .box')) {
                const words = document.createRange();
                words.selectNodeContents(element);
                const text = words.getBoundingClientRect();
                if (text.width) box = text;
            }
            const { x, y, width, height } = box;
            return { x, y, width, height };
        }""")

    def at(self, number, x, y):
        """A point on page `number`, in points from its top-left corner, in the window's pixels."""
        return self.page.evaluate("""([number, x, y]) => {
            const page = document.querySelectorAll('.page')[number - 1];
            const viewer = document.getElementById('viewer');
            const [w, h] = state.info.pages[number - 1];
            let r = page.getBoundingClientRect();
            const top = r.top + (y / h) * r.height;
            if (top < 60 || top > innerHeight - 60) {
                viewer.scrollTop += top - innerHeight / 2;
                r = page.getBoundingClientRect();
            }
            return [r.left + (x / w) * r.width, r.top + (y / h) * r.height];
        }""", [number, x, y])

    def points(self):
        """Window pixels to one point on the first page."""
        return self.page.evaluate("document.querySelector('.page').getBoundingClientRect().width / state.info.pages[0][0]")

    # The steps a script's Screen line can take.

    def open(self, name):
        """Opens a demo file unseen: the film shows it open."""
        self.page.evaluate("path => { window.__demo.open = path; choosePdf(); }", f"{FOLDER}/{name}")
        self.page.wait_for_function("path => state.path === path", arg=f"{FOLDER}/{name}")
        self.settle()
        return {"kind": "open", "stills": [{"shot": self.shoot()}], "box": None}

    def scroll(self, number, y):
        """Scrolls page `number` so the point `y` points from its top is in the middle of the view."""
        self.page.evaluate("""([number, y]) => {
            const page = document.querySelectorAll('.page')[number - 1];
            const [, h] = state.info.pages[number - 1];
            const r = page.getBoundingClientRect();
            document.getElementById('viewer').scrollTop += r.top + (y / h) * r.height - innerHeight / 2;
        }""", [int(number), float(y)])
        self.settle()
        return {"kind": "view", "stills": [{"shot": self.shoot()}], "box": None}

    def choose(self, name):
        """Presses Choose PDF and picks a demo file."""
        self.page.evaluate("path => { window.__demo.open = path; }", f"{FOLDER}/{name}")
        return {**self.click("Choose PDF…" if self.page.is_visible("#choose") else "Open another…"), "whole": True}

    def look(self):
        """A still of the window as it is, with nothing pointed at."""
        return {"kind": "view", "stills": [{"shot": self.shoot()}], "box": None}

    def point(self, label):
        found = self.find(label)
        box = self.box(found)
        x, y = box["x"] + box["width"] / 2, box["y"] + box["height"] / 2
        self.page.mouse.move(x, y)
        self.page.wait_for_timeout(150)
        return {"kind": "point", "stills": [{"shot": self.shoot(), "x": x, "y": y}], "box": box}

    def click(self, label):
        step = self.point(label)
        still = step["stills"][0]
        self.page.mouse.down()
        self.page.mouse.up()
        self.settle()
        step["stills"].append({"shot": self.shoot(), "x": still["x"], "y": still["y"], "down": True})
        return {**step, "kind": "click"}

    def place(self, number, x, y):
        """Clicks a page, which adds a field there."""
        x, y = self.at(int(number), float(x), float(y))
        self.settle()
        self.page.mouse.move(x, y)
        before = self.shoot()
        self.page.mouse.down()
        self.page.mouse.up()
        self.settle()
        return {"kind": "click", "stills": [{"shot": before, "x": x, "y": y}, {"shot": self.shoot(), "x": x, "y": y, "down": True}],
                "box": None}

    def stroke(self, start, end, steps=8):
        """Presses at `start`, moves to `end` with a still on the way, and lets go."""
        self.page.mouse.move(*start)
        stills = [{"shot": self.shoot(), "x": start[0], "y": start[1]}]
        self.page.mouse.down()
        for i in range(1, steps + 1):
            x, y = (start[0] + (end[0] - start[0]) * i / steps, start[1] + (end[1] - start[1]) * i / steps)
            self.page.mouse.move(x, y)
            stills.append({"shot": self.shoot(), "x": x, "y": y, "down": i < steps})
        self.page.mouse.up()
        self.settle()
        stills[-1]["shot"] = self.shoot()
        return {"kind": "drag", "stills": stills, "box": None}

    def draw(self, number, x1, y1, x2, y2):
        """Drags across a page, which draws a field."""
        start, end = self.at(int(number), float(x1), float(y1)), self.at(int(number), float(x2), float(y2))
        self.settle()
        return self.stroke(start, end)

    def drag(self, name, dx, dy):
        """Moves a new field by `dx`, `dy` points."""
        box = self.box(self.find(f'.box.new[title="{name}"]'))
        start = (box["x"] + box["width"] / 2, box["y"] + box["height"] / 2)
        scale = self.points()
        return self.stroke(start, (start[0] + float(dx) * scale, start[1] + float(dy) * scale))

    def resize(self, name, dx, dy):
        """Drags a new field's corner by `dx`, `dy` points."""
        box = self.box(self.find(f'.box.new.selected[title="{name}"] .handle'))
        start = (box["x"] + box["width"] / 2, box["y"] + box["height"] / 2)
        scale = self.points()
        return self.stroke(start, (start[0] + float(dx) * scale, start[1] + float(dy) * scale))

    def rename(self, old, new):
        """Types a new name into a new field's name box."""
        fields = self.page.locator("#fields input[type=text]")
        found = next((fields.nth(i) for i in range(fields.count()) if fields.nth(i).input_value() == old), None)
        if found is None:
            raise Stopped(f"no new field is named {old!r}")
        box = self.box(found)
        x, y = box["x"] + box["width"] / 2, box["y"] + box["height"] / 2
        self.page.mouse.move(x, y)
        before = self.shoot()
        found.select_text()
        self.page.keyboard.type(new, delay=30)
        found.evaluate("(field) => field.blur()")
        self.settle()
        return {"kind": "click", "stills": [{"shot": before, "x": x, "y": y}, {"shot": self.shoot(), "x": x, "y": y, "down": True}],
                "box": box}

    def saveas(self, name):
        """What the save panel answers next. Nothing to film."""
        self.page.evaluate("path => { window.__demo.save = path; }", f"{FOLDER}/{name}")

    def finish(self):
        """Lets the held signature through: the still before it is what shows while macOS asks for the PIN."""
        waiting = self.shoot()
        self.page.evaluate("window.__demo.release()")
        self.page.wait_for_function("document.getElementById('status').className !== 'busy'")
        self.settle()
        if self.page.evaluate("document.getElementById('status').className") != "ok":
            raise Stopped("signing failed: " + self.page.inner_text("#status"))
        return {"kind": "finish", "stills": [{"shot": waiting}, {"shot": self.shoot()}], "box": None}

    def othersigns(self, source, field, target):
        """Someone else signs a demo file, off screen."""
        sign(real(f"{FOLDER}/{source}"), real(f"{FOLDER}/{target}"), field, REVIEWER)

    def picture(self, name):
        """Keeps the last still as a README picture."""
        self.pictures[name] = f"{self.shots:03}.png"

    def perform(self, lines):
        shown = []
        for number, line in enumerate(lines, 1):
            steps = []
            for verb, *args in line["screen"]:
                try:
                    step = getattr(self, verb)(*args)
                except Stopped as why:
                    raise Stopped(f"line {number}, {verb} {args}: {why}") from None
                if step:
                    steps.append(step)
            if not steps:
                raise Stopped(f"line {number} shows nothing")
            shown.append(steps)
        return shown


def parse(text):
    """A script's title and its lines, each `1. Say: ...` with `Screen: this; that` under it."""
    title = re.search(r"^# (.+)$", text, re.M).group(1).strip()
    lines = []
    for match in re.finditer(r"^\d+\. Say: (.+?)\n(?:\s+Screen: (.+?)\n)?", text + "\n", re.M):
        say, screen = match.groups()
        lines.append({"say": say.strip(), "screen": [shlex.split(step) for step in (screen or "").split(";") if step.strip()]})
    return title, lines


def spoken(text):
    for word, said in SPOKEN.items():
        text = re.sub(rf"\b{re.escape(word)}\b", said, text)
    return text


def voiced(lines, folder):
    """Each line's wav in the film's folder and how long it runs. No words are spoken twice."""
    store = BUILD / "voices"
    store.mkdir(parents=True, exist_ok=True)
    kept = [store / f"{hashlib.sha256(f'{VOICE} {spoken(line['say'])}'.encode()).hexdigest()[:16]}.wav" for line in lines]
    new = [[spoken(line["say"]), str(path)] for line, path in zip(lines, kept) if not path.exists()]
    if new:
        subprocess.run([VOICE_HOME / "bin/python", "-c", SPEAK, str(VOICE_HOME), VOICE], check=True, text=True,
                       input=json.dumps(new))
    shutil.rmtree(folder / "voice", ignore_errors=True)
    (folder / "voice").mkdir()
    for path in kept:
        shutil.copy(path, folder / "voice")
    return [{"voice": f"voice/{path.name}", "seconds": float(run(
        "ffprobe", "-v", "error", "-show_entries", "format=duration", "-of", "csv=p=0", path, text=True).stdout)}
        for path in kept]


def capture(take, script, number, total):
    title, lines = parse(script.read_text())
    folder = BUILD / script.stem
    shutil.rmtree(folder, ignore_errors=True)
    folder.mkdir(parents=True)
    take.start(folder)
    shown = take.perform(lines)
    timeline = {"title": title, "number": number, "of": total, "captured": time.strftime("%Y-%m-%d"),
                "pictures": take.pictures,
                "lines": [{"say": line["say"], "steps": steps} for line, steps in zip(lines, shown)]}
    (folder / "timeline.json").write_text(json.dumps(timeline, indent=1))
    print(f"{script.stem}: {take.shots} stills", flush=True)


def remotion(*args):
    run("bunx", "remotion", *args, "--log=error", cwd=FILMS)


def render(stem):
    """The film, its GIF and poster, and the README pictures it holds, from the last capture."""
    folder = BUILD / stem
    timeline = json.loads((folder / "timeline.json").read_text())
    for line, voice in zip(timeline["lines"], voiced(timeline["lines"], folder)):
        line.update(voice)
    (folder / "timeline.json").write_text(json.dumps(timeline, indent=1))
    VIDEOS.mkdir(parents=True, exist_ok=True)
    PICTURES.mkdir(parents=True, exist_ok=True)

    raw = folder / "film.mp4"
    remotion("render", "src/index.ts", "Lesson", raw, f"--public-dir={folder}")
    run("ffmpeg", "-y", "-loglevel", "error", "-i", raw, "-c:v", "copy", "-af", "loudnorm=I=-16:TP=-1.5",
        "-c:a", "aac", "-b:a", "160k", "-movflags", "+faststart", VIDEOS / f"{stem}.mp4")
    remotion("still", "src/index.ts", "Poster", VIDEOS / f"{stem}.jpg", f"--public-dir={folder}", "--jpeg-quality=88")

    # The GIF for the README: the same film, silent, read from frames of the finished MP4.
    frames = folder / "gif"
    shutil.rmtree(frames, ignore_errors=True)
    frames.mkdir()
    run("ffmpeg", "-loglevel", "error", "-i", raw, "-vf", f"fps={GIF['fps']},scale={GIF['width']}:-1:flags=lanczos",
        frames / "%04d.png")
    run("gifski", "--quiet", "--fps", GIF["fps"], "--quality", GIF["quality"], "--width", GIF["width"], "-o",
        VIDEOS / f"{stem}.gif", *sorted(frames.glob("*.png")))
    shutil.rmtree(frames)

    for name, still in timeline["pictures"].items():
        remotion("still", "src/index.ts", "Picture", PICTURES / f"{name}.png", f"--public-dir={folder}",
                 "--image-format=png", "--scale=1.5", f"--props={json.dumps({'still': still})}")
    seconds = float(run("ffprobe", "-v", "error", "-show_entries", "format=duration", "-of", "csv=p=0",
                        VIDEOS / f"{stem}.mp4", text=True).stdout)
    sizes = ", ".join(f"{p.name} {p.stat().st_size / 1e6:.1f} MB" for p in sorted(VIDEOS.glob(f"{stem}.*")))
    print(f"{stem}: {seconds:.0f} s; {sizes}", flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("lessons", nargs="*", help="lesson names, such as 02-add-a-field; all when none")
    parser.add_argument("--render", action="store_true", help="voice and film again from the last capture")
    parser.add_argument("--capture", action="store_true", help="capture the stills only, to check the scripts")
    args = parser.parse_args()
    scripts = sorted(LESSONS.glob("*.md"))
    chosen = [s for s in scripts if not args.lessons or s.stem in args.lessons]
    if not chosen:
        sys.exit(f"no lesson named {' '.join(args.lessons)} in {LESSONS}")
    if not args.render:
        run("cargo", "build", "--release", "--example", "pdftool", cwd=ROOT / "src-tauri")
        BUILD.mkdir(parents=True, exist_ok=True)
        demo_files()
        with sync_playwright() as playwright:
            browser = playwright.webkit.launch()
            context = browser.new_context(viewport=SIZE, device_scale_factor=2, color_scheme="light",
                                          reduced_motion="reduce", locale="en-US", timezone_id="America/New_York")
            context.add_init_script(BRIDGE)
            take = Take(context, context.new_page())
            try:
                for script in chosen:
                    capture(take, script, scripts.index(script) + 1, len(scripts))
            except Stopped as why:
                sys.exit(f"Stopped: {why}")
            finally:
                browser.close()
    for script in [] if args.capture else chosen:
        render(script.stem)


if __name__ == "__main__":
    main()
