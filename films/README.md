# The video lessons

There is one video for each script in `docs/lessons/`. Three steps make a video:

1. Capture: `films/record.py` runs the app's pages in WebKit, walks the script, and takes a still of each step. A stand-in for the app's native side answers with the real PDF code (`src-tauri/examples/pdftool.rs`), so the pages, fields and stamps on screen are the real ones. The card holder is a demo, JANE Q. DOE, signing with a throwaway key, so no real card, PIN, person or folder shows.
2. Voice: Kokoro reads each line on this Mac, so no script text leaves it.
3. Edit: Remotion, in this folder, turns the stills and the voice into the film. gifski makes the README's GIF from it, and Remotion also draws the poster and the README pictures.

Run every command from the top of the repo.

## Setup, once

You need a Mac with Python 3.12, Node, [Bun](https://bun.sh), Rust, `ffmpeg` and `gifski` (`brew install ffmpeg gifski`).

```sh
# Remotion, and the headless browser it draws with
cd films && bun install --frozen-lockfile && bunx remotion browser ensure && cd ..

# The browser that takes the stills, with what makes the sample PDFs
python3.12 -m venv ~/.cache/cac-signer-capture
~/.cache/cac-signer-capture/bin/pip install playwright reportlab pypdf cryptography
~/.cache/cac-signer-capture/bin/playwright install webkit

# The voice, in an environment of its own
python3.12 -m venv ~/.cache/cac-signer-voice && ~/.cache/cac-signer-voice/bin/pip install kokoro-onnx==0.6.1 soundfile
curl -L --output-dir ~/.cache/cac-signer-voice -O "https://github.com/thewh1teagle/kokoro-onnx/releases/download/model-files-v1.0/{kokoro-v1.0.onnx,voices-v1.0.bin}"
```

`CAC_SIGNER_VOICE` points the recorder at another voice environment. Below, `python3` means `~/.cache/cac-signer-capture/bin/python`.

## Commands

```sh
python3 films/record.py                      # capture, voice and render every lesson, then the pictures
python3 films/record.py 02-add-a-field       # one lesson
python3 films/record.py --render             # voice and render again from the last capture
python3 films/record.py --capture            # the stills only, to check the scripts
```

A capture of all three takes under a minute, and a render about a minute per lesson.

| What changed | What to run |
| --- | --- |
| A `Say` line | `--render <lesson>` |
| A `Screen` line, or the app's look | `<lesson>`, which captures again |
| Anything in `films/src` | `--render` |
| `SPOKEN` in the recorder | `--render`; only new words are spoken |

## What a script says

Each numbered line has a `Say:`, which is the voice and the caption, and a `Screen:`, which is what the video does on that line. Steps run in order, split by `;`. A label is the words on the screen; one that starts with `#`, `.` or `[` is a CSS selector. Pages and points are in PDF points from the page's top-left corner.

| Step | What it does |
| --- | --- |
| `open "file.pdf"` | Opens a sample file unseen: the video shows it open |
| `choose "file.pdf"` | Presses Choose PDF (or Open another) and picks the file |
| `look` | A still with nothing pointed at |
| `point "label"` | Puts the pointer and a ring on it |
| `click "label"` | Points at it, then presses it, with a still before and after |
| `scroll 1 520` | Scrolls so that point of page 1 is in the middle of the view |
| `place 1 432 398` | Clicks page 1 there, which adds a field of the usual size |
| `draw 1 72 598 288 638` | Drags across page 1, which draws a field |
| `drag "Name" -24 -6` | Moves a new field by that many points |
| `resize "Name" 24 4` | Drags a new field's corner by that many points |
| `rename "Old" "New"` | Types a new field's name |
| `saveas "file.pdf"` | What the save panel answers next |
| `finish` | Lets a held signature through. The still before it shows while macOS would ask for the PIN |
| `othersigns "in.pdf" "Field" "out.pdf"` | The demo reviewer signs a field, off screen |
| `picture "name"` | Keeps the last still as `docs/screenshots/name.png` |

The recorder stops if a still shows anything that is not demo data, such as this Mac's user name or a real folder, or a NaN or undefined.

## Where things go

| Path | What it holds |
| --- | --- |
| `docs/videos/<lesson>.mp4` | The narrated video |
| `docs/videos/<lesson>.gif` | The same video, silent, for the README |
| `docs/videos/<lesson>.jpg` | Its poster |
| `docs/screenshots/` | The README's pictures |
| `films/build/<lesson>/` | Stills, voice and `timeline.json` (not in git) |
| `films/build/voices/` | Every spoken line, kept by a hash of the words and the voice |

`timeline.json` ties it together. Capture writes each line's words, steps, stills, pointer positions and target boxes. The voice step adds each line's wav and its length, and every frame number in `src/plan.ts` comes from those lengths.

## Inside `src`

| File | What it does |
| --- | --- |
| `plan.ts` | Works out when each line, still, pointer move, ring and camera move falls. It draws nothing |
| `Film.tsx` | Draws the title card, the desktop and window, the pointer and ring, the caption, the poster and the README picture |
| `Root.tsx` | The compositions: `Lesson`, `Poster` and `Picture` |

The colors come from `ui/style.css`, and the font is a local file, so nothing is fetched at render time. Every `remotion` and `@remotion/*` package stays pinned to one exact version. To look at a change frame by frame: `bunx remotion studio src/index.ts --public-dir=build/01-sign-a-form` in this folder.

## Licenses

Remotion is free for an individual or a company of up to 3 employees (`LICENSE.md` in remotion-dev/remotion). Kokoro's model and voices are under the Apache license, and so is Playwright.
