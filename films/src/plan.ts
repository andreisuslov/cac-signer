/* One timeline for the whole film: every frame number comes from here, from each line's voice,
 * so the voice, the caption and the screen cannot drift apart. */
import { Easing, interpolate } from "remotion";

export const FPS = 30;
export const WIDTH = 1920;
export const HEIGHT = 1080;
/** The desktop on screen. The caption bar is under it, so a caption never lies over the app. */
export const STAGE = 924;
export const TITLE = 2 * FPS;
export const FADE = 8;
/** How long the pointer takes to get to a target. */
export const ARRIVES = 15;
/** How long the ring takes to come and go. */
export const RING = 6;

/** The app's window as captured, in its own pixels, and the title bar drawn above it. */
export const APP = { width: 1280, height: 720 };
export const BAR = 28;
const MARGIN = 40;
/** Stage pixels to one of the app's. */
export const SCALE = Math.min((WIDTH - 2 * MARGIN) / APP.width, (STAGE - 2 * MARGIN) / (APP.height + BAR));
export const WINDOW = {
  x: (WIDTH - APP.width * SCALE) / 2,
  y: (STAGE - (APP.height + BAR) * SCALE) / 2,
  width: APP.width * SCALE,
  height: (APP.height + BAR) * SCALE,
};

/** Seconds after a line: the voice's own pause between sentences. */
const GAP = 0.6;
const TAIL = FPS;
const MOST = 1.25;
/** Stage pixels kept in frame around a target, so its neighbors stay. */
const ROOM = 150;
const PUSH = FPS;
/** How long a viewer sees what is about to be pressed. */
const SEEN = Math.round(0.8 * FPS);
/** How long a drag takes. */
const DRAG = Math.round(1.4 * FPS);
/** How long the pointer stays after its last target before it goes. */
const WAITS = FPS;

export type Box = { x: number; y: number; width: number; height: number };
export type Point = { x: number; y: number };
export type Still = { shot: string; x?: number; y?: number; down?: boolean };
/** `whole` marks a step that opens a file: its line shows the whole window. */
export type Step = { kind: "open" | "view" | "point" | "click" | "drag" | "finish"; stills: Still[]; box: Box | null; whole?: boolean };
export type Timeline = {
  title: string;
  number: number;
  of: number;
  lines: { say: string; steps: Step[]; voice: string; seconds: number }[];
  pictures?: Record<string, string>;
};
export type Shot = { src: string; from: number; to: number; fade: number };
/** The pointer leaves its last place at `start` and is at `at` by `frame`. */
export type Move = { start: number; frame: number; at: Point; down: boolean };
export type Ring = { box: Box; from: number; to: number };
export type Camera = { scale: number; x: number; y: number };

export const onStage = (x: number, y: number): Point => ({ x: WINDOW.x + x * SCALE, y: WINDOW.y + (BAR + y) * SCALE });
const boxOnStage = (box: Box): Box => ({ ...onStage(box.x, box.y), width: box.width * SCALE, height: box.height * SCALE });
const pointBox = ({ x, y }: Point): Box => ({ x: x - 20, y: y - 20, width: 40, height: 40 });

const WHOLE: Camera = { scale: 1, x: 0, y: 0 };
export const REST: Point = { x: WIDTH * 0.62, y: STAGE * 0.7 };

/** The closest view, within the limit, that keeps these boxes and what is around them in frame. */
function framed(boxes: Box[]): Camera {
  const left = Math.min(...boxes.map((box) => box.x)) - ROOM;
  const top = Math.min(...boxes.map((box) => box.y)) - ROOM;
  const right = Math.max(...boxes.map((box) => box.x + box.width)) + ROOM;
  const bottom = Math.max(...boxes.map((box) => box.y + box.height)) + ROOM;
  const scale = Math.max(1, Math.min(MOST, WIDTH / (right - left), STAGE / (bottom - top)));
  const inside = (middle: number, whole: number) => Math.min(whole - whole / scale, Math.max(0, middle - whole / scale / 2));
  return { scale, x: inside((left + right) / 2, WIDTH), y: inside((top + bottom) / 2, STAGE) };
}

export function plan(timeline: Timeline) {
  const shots: Shot[] = [];
  const moves: Move[] = [];
  const rings: Ring[] = [];
  const presses: { at: Point; frame: number }[] = [];
  /** Frames the pointer is on screen. */
  const shown: { from: number; to: number }[] = [];
  const cameras: { from: number; start: Camera; end: Camera }[] = [];
  const show = (src: string, from: number, fade = FADE) => {
    if (shots.at(-1)?.src !== src) shots.push({ src, from, to: from, fade });
  };
  const at = (still: Still) => onStage(still.x ?? 0, still.y ?? 0);
  const point = (still: Still, start: number, frame: number) => moves.push({ start, frame, at: at(still), down: Boolean(still.down) });

  let now = TITLE;
  let camera = WHOLE;
  const lines = timeline.lines.map((line, number) => {
    const from = now;
    const length = Math.round((line.seconds + GAP) * FPS);
    now += length;
    const boxes: Box[] = [];
    line.steps.forEach((step, index) => {
      const begins = from + Math.round((index * length) / line.steps.length);
      const ends = from + Math.round(((index + 1) * length) / line.steps.length);
      const [first, ...rest] = step.stills;
      show(first.shot, begins);
      if (step.box) boxes.push(boxOnStage(step.box));
      if (first.x !== undefined) {
        point(first, begins, begins + ARRIVES);
        boxes.push(pointBox(at(first)));
        shown.push({ from: begins, to: ends });
      }
      if (step.kind === "click" && rest.length) {
        const press = Math.min(begins + ARRIVES + SEEN, ends - 2 * FADE);
        presses.push({ at: at(rest[0]), frame: press });
        show(rest[0].shot, press + 3, 4);
        if (step.box) rings.push({ box: boxOnStage(step.box), from: begins, to: press });
      } else if (step.kind === "drag") {
        const start = begins + ARRIVES + Math.round(SEEN / 2);
        const each = DRAG / rest.length;
        rest.forEach((still, i) => {
          const frame = Math.round(start + (i + 1) * each);
          point(still, Math.round(start + i * each), frame);
          show(still.shot, Math.round(start + i * each), 3);
          boxes.push(pointBox(at(still)));
        });
      } else if (step.kind === "finish" && rest.length) {
        show(rest[0].shot, begins + Math.round(0.6 * (ends - begins)));
      } else if (step.box) {
        rings.push({ box: boxOnStage(step.box), from: begins, to: ends - RING });
      }
    });
    // The first line and a new file show the whole window. A line with nothing to point at keeps the last view.
    const whole = number === 0 || line.steps.some((step) => step.kind === "open" || step.whole);
    const start = whole ? WHOLE : camera;
    camera = boxes.length && !whole ? framed(boxes) : start;
    cameras.push({ from, start, end: camera });
    return { from, to: now, say: line.say, voice: line.voice };
  });
  const frames = now + TAIL;
  shots.forEach((shot, index) => (shot.to = shots[index + 1]?.from ?? frames));
  // Spells on screen that follow each other closely are one, so the pointer does not blink.
  const spells: { from: number; to: number }[] = [];
  for (const spell of shown) {
    const last = spells.at(-1);
    if (last && spell.from <= last.to + WAITS) last.to = Math.max(last.to, spell.to);
    else spells.push({ ...spell });
  }
  return { lines, shots, moves, rings, presses, spells, cameras, frames };
}

export type Plan = ReturnType<typeof plan>;

const clamped = { extrapolateLeft: "clamp", extrapolateRight: "clamp" } as const;

export function cameraAt(cameras: Plan["cameras"], frame: number): Camera {
  const move = cameras.findLast((one) => one.from <= frame);
  if (!move) return WHOLE;
  const done = interpolate(frame, [move.from, move.from + PUSH], [0, 1], { easing: Easing.inOut(Easing.cubic), ...clamped });
  const between = (start: number, end: number) => start + (end - start) * done;
  return { scale: between(move.start.scale, move.end.scale), x: between(move.start.x, move.end.x), y: between(move.start.y, move.end.y) };
}

/** Where the pointer is, and whether the button is held. */
export function pointerAt(moves: Move[], frame: number): Point & { down: boolean } {
  const index = moves.findLastIndex((move) => move.start <= frame);
  if (index < 0) return { ...REST, down: false };
  const move = moves[index];
  const was = moves[index - 1]?.at ?? REST;
  const glide = move.down || moves[index - 1]?.down;
  const done = interpolate(frame, [move.start, Math.max(move.frame, move.start + 1)], [0, 1], {
    easing: glide ? Easing.linear : Easing.inOut(Easing.cubic),
    ...clamped,
  });
  return { x: was.x + (move.at.x - was.x) * done, y: was.y + (move.at.y - was.y) * done, down: move.down };
}
