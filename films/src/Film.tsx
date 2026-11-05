import type { CSSProperties, ReactNode } from "react";
import { loadFont } from "@remotion/fonts";
import inter from "@fontsource-variable/inter/files/inter-latin-wght-normal.woff2";
import { AbsoluteFill, Audio, Img, Sequence, interpolate, staticFile, useCurrentFrame } from "remotion";

import icon from "../../app-icon.png";
import {
  APP, BAR, FADE, HEIGHT, RING, SCALE, STAGE, TITLE, WIDTH, WINDOW,
  cameraAt, plan, pointerAt, type Camera, type Plan, type Timeline,
} from "./plan";

// The app's own colors, from ui/style.css.
const INK = "#1d1d1f";
const PAPER = "#ffffff";
const QUIET = "#a1a1aa";
const ACCENT = "#0a5ad4";
const DESKTOP = "linear-gradient(160deg, #2b3445 0%, #1b2130 55%, #141824 100%)";
const TYPE: CSSProperties = { fontFamily: "Inter Variable", fontFeatureSettings: '"cv11", "ss01"' };

void loadFont({ family: "Inter Variable", url: inter, weight: "100 900" });

const clamped = { extrapolateLeft: "clamp", extrapolateRight: "clamp" } as const;
const view = ({ scale, x, y }: Camera): CSSProperties => ({
  position: "absolute", left: 0, top: 0, width: WIDTH, height: STAGE, transformOrigin: "0 0",
  transform: `scale(${scale}) translate(${-x}px, ${-y}px)`,
});

/** A Mac window around the app, `scale` screen pixels to one of the app's. */
export function AppWindow({ scale, children }: { scale: number; children: ReactNode }) {
  const light = (color: string, left: number) => (
    <div style={{ position: "absolute", left: left * scale, top: 8 * scale, width: 12 * scale, height: 12 * scale, borderRadius: "50%", background: color, boxShadow: "inset 0 0 0 0.5px rgb(0 0 0 / 18%)" }} />
  );
  return (
    <div style={{ position: "absolute", inset: 0, borderRadius: 10 * scale, overflow: "hidden", background: "#f5f5f7", boxShadow: `0 ${18 * scale}px ${50 * scale}px rgb(0 0 0 / 45%), 0 0 0 ${scale}px rgb(0 0 0 / 30%)` }}>
      <div style={{ position: "absolute", left: 0, top: 0, right: 0, height: BAR * scale, background: "linear-gradient(#ececec, #e3e3e3)", borderBottom: "1px solid #c9c9c9" }}>
        {light("#ff5f57", 10)}
        {light("#febc2e", 30)}
        {light("#28c840", 50)}
        <div style={{ ...TYPE, position: "absolute", inset: 0, display: "flex", alignItems: "center", justifyContent: "center", fontSize: 13 * scale, fontWeight: 600, color: "#4d4d4d" }}>CAC Signer</div>
      </div>
      <div style={{ position: "absolute", left: 0, top: BAR * scale, width: APP.width * scale, height: APP.height * scale }}>{children}</div>
    </div>
  );
}

function TitleCard({ timeline }: { timeline: Timeline }) {
  const frame = useCurrentFrame();
  return (
    <AbsoluteFill style={{ ...TYPE, background: INK, color: PAPER, alignItems: "center", justifyContent: "center", gap: 32, opacity: interpolate(frame, [TITLE, TITLE + FADE], [1, 0], clamped) }}>
      <Img src={icon} style={{ width: 150, height: 150 }} />
      <div style={{ fontSize: 76, fontWeight: 650, letterSpacing: "-0.02em" }}>{timeline.title}</div>
      <div style={{ fontSize: 36, fontWeight: 500, color: QUIET }}>CAC Signer · Video {timeline.number} of {timeline.of}</div>
    </AbsoluteFill>
  );
}

function Pointer({ moves, rings, presses, spells }: Pick<Plan, "moves" | "rings" | "presses" | "spells">) {
  const frame = useCurrentFrame();
  const spell = spells.find((one) => one.from - FADE <= frame && frame < one.to + FADE);
  const { x, y, down } = pointerAt(moves, frame);
  return (
    <>
      {rings.filter((ring) => ring.from <= frame && frame < ring.to + RING).map((ring) => (
        <div key={ring.from} style={{ position: "absolute", left: ring.box.x - 10, top: ring.box.y - 7, width: ring.box.width + 20, height: ring.box.height + 14, boxSizing: "border-box", border: `4px solid ${ACCENT}`, borderRadius: 12, opacity: Math.min(interpolate(frame, [ring.from + 12, ring.from + 12 + RING], [0, 1], clamped), interpolate(frame, [ring.to, ring.to + RING], [1, 0], clamped)) }} />
      ))}
      {presses.filter((press) => press.frame <= frame && frame < press.frame + 18).map((press) => (
        <div key={press.frame} style={{ position: "absolute", left: press.at.x - 34, top: press.at.y - 34, width: 68, height: 68, borderRadius: "50%", background: ACCENT, opacity: interpolate(frame - press.frame, [0, 18], [0.5, 0]), transform: `scale(${interpolate(frame - press.frame, [0, 18], [0.3, 1.2])})` }} />
      ))}
      {spell && (
        <div style={{ position: "absolute", left: x - 6, top: y - 2, opacity: Math.min(interpolate(frame, [spell.from - FADE, spell.from], [0, 1], clamped), interpolate(frame, [spell.to, spell.to + FADE], [1, 0], clamped)) }}>
          {down && <div style={{ position: "absolute", left: -8, top: -12, width: 28, height: 28, borderRadius: "50%", background: ACCENT, opacity: 0.35 }} />}
          <svg width={38} height={38} viewBox="0 0 24 24" style={{ position: "absolute", left: 0, top: 0 }}>
            <path d="M5 2 L5 19 L10 14.5 L13.5 22 L16.5 20.7 L13 13.5 L20 13.5 Z" fill={INK} stroke={PAPER} strokeWidth={1.6} strokeLinejoin="round" />
          </svg>
        </div>
      )}
    </>
  );
}

function Desktop({ shots, cameras, ...pointer }: Pick<Plan, "shots" | "cameras" | "moves" | "rings" | "presses" | "spells">) {
  const frame = useCurrentFrame();
  return (
    <div style={{ position: "absolute", left: 0, top: 0, width: WIDTH, height: STAGE, overflow: "hidden", background: DESKTOP }}>
      <div style={view(cameraAt(cameras, frame))}>
        <div style={{ position: "absolute", left: WINDOW.x, top: WINDOW.y, width: WINDOW.width, height: WINDOW.height }}>
          <AppWindow scale={SCALE}>
            {shots.filter((shot) => shot.from <= frame && frame < shot.to + shot.fade).map((shot) => (
              <Img key={shot.from} src={staticFile(shot.src)} style={{ position: "absolute", inset: 0, width: "100%", height: "100%", opacity: interpolate(frame, [shot.from, shot.from + shot.fade], [0, 1], clamped) }} />
            ))}
          </AppWindow>
        </div>
        <Pointer {...pointer} />
      </div>
    </div>
  );
}

function Caption({ say }: { say: string }) {
  return (
    <div style={{ ...TYPE, position: "absolute", left: 0, top: STAGE, width: WIDTH, height: HEIGHT - STAGE, boxSizing: "border-box", padding: "0 64px", display: "flex", alignItems: "center", justifyContent: "center", background: INK, color: PAPER, fontSize: 42, lineHeight: "56px", fontWeight: 600, textAlign: "center", textWrap: "balance" }}>
      {say}
    </div>
  );
}

export function Film({ timeline }: { timeline: Timeline }) {
  const frame = useCurrentFrame();
  const { lines, frames, ...rest } = plan(timeline);
  const line = lines.findLast((one) => one.from <= frame);
  return (
    <AbsoluteFill style={{ background: INK }}>
      <Desktop {...rest} />
      {line && <Caption say={line.say} />}
      {frame < TITLE + FADE && <TitleCard timeline={timeline} />}
      {lines.map((one) => (
        <Sequence key={one.from} from={one.from}><Audio src={staticFile(one.voice)} /></Sequence>
      ))}
      <AbsoluteFill style={{ background: "#000", opacity: interpolate(frame, [frames - 15, frames - 1], [0, 1], clamped) }} />
    </AbsoluteFill>
  );
}

/** The README's picture of a film: the app at a moment of it, its title and length, and a play button. */
export function Poster({ timeline, still, length }: { timeline: Timeline; still: string; length: string }) {
  return (
    <AbsoluteFill style={{ background: DESKTOP }}>
      <div style={{ position: "absolute", left: WINDOW.x, top: WINDOW.y + (HEIGHT - STAGE) / 2, width: WINDOW.width, height: WINDOW.height }}>
        <AppWindow scale={SCALE}>
          <Img src={staticFile(still)} style={{ width: "100%", height: "100%" }} />
        </AppWindow>
      </div>
      <AbsoluteFill style={{ background: "linear-gradient(transparent 45%, rgb(10 12 18 / 88%))" }} />
      <div style={{ ...TYPE, position: "absolute", left: 96, bottom: 84, color: PAPER }}>
        <div style={{ fontSize: 36, fontWeight: 500, color: "#cbd5e1" }}>Video {timeline.number} · {length}</div>
        <div style={{ fontSize: 84, fontWeight: 700, letterSpacing: "-0.02em" }}>{timeline.title}</div>
      </div>
      <div style={{ position: "absolute", right: 120, bottom: 96, width: 150, height: 150, borderRadius: "50%", background: "rgb(255 255 255 / 92%)", display: "flex", alignItems: "center", justifyContent: "center", boxShadow: "0 10px 40px rgb(0 0 0 / 40%)" }}>
        <svg width={64} height={64} viewBox="0 0 24 24"><path d="M8 5 L19 12 L8 19 Z" fill={INK} /></svg>
      </div>
    </AbsoluteFill>
  );
}

/** A still of the app in its window, on a clear background, for the README. */
export function Picture({ still }: { still: string }) {
  return (
    <AbsoluteFill>
      <div style={{ position: "absolute", left: 48, top: 40, width: APP.width, height: APP.height + BAR }}>
        <AppWindow scale={1}>
          <Img src={staticFile(still)} style={{ width: "100%", height: "100%" }} />
        </AppWindow>
      </div>
    </AbsoluteFill>
  );
}
