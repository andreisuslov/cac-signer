import { Composition, staticFile } from "remotion";

import { Film, Picture, Poster } from "./Film";
import { APP, BAR, FPS, HEIGHT, WIDTH, plan, type Timeline } from "./plan";

const nothing: Timeline = { title: "", number: 0, of: 0, lines: [] };
const read = async (): Promise<Timeline> => (await fetch(staticFile("timeline.json"))).json();

export const Root = () => (
  <>
    <Composition
      id="Lesson"
      component={Film}
      width={WIDTH}
      height={HEIGHT}
      fps={FPS}
      durationInFrames={1}
      defaultProps={{ timeline: nothing }}
      calculateMetadata={async () => {
        const timeline = await read();
        return { durationInFrames: plan(timeline).frames, props: { timeline } };
      }}
    />
    <Composition
      id="Poster"
      component={Poster}
      width={WIDTH}
      height={HEIGHT}
      fps={FPS}
      durationInFrames={1}
      defaultProps={{ timeline: nothing, still: "", length: "" }}
      calculateMetadata={async () => {
        const timeline = await read();
        const seconds = Math.round(plan(timeline).frames / FPS);
        const still = Object.values(timeline.pictures ?? {})[0] ?? timeline.lines.at(-1)!.steps.at(-1)!.stills.at(-1)!.shot;
        return { props: { timeline, still, length: `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, "0")}` } };
      }}
    />
    <Composition id="Picture" component={Picture} width={APP.width + 96} height={APP.height + BAR + 104} fps={FPS} durationInFrames={1} defaultProps={{ still: "" }} />
  </>
);
