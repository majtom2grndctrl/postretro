// Flash-limiter strobe fixture for `a11y-strobe-test.map`. Each floor pad plays
// one strobe for three seconds on entry, so the flash limiter can be judged on
// hardware (hub AC 5 and 6, X1). The map sits outside the catalog; no player
// payload ships it.
//
// Pads, west to east:
//   1. full-screen `screen.flash`, white, 10 Hz
//   2. full-screen `screen.flash`, saturated red, 5 Hz
//   3. UI panel below the flash-area threshold (~4% of the screen), 10 Hz
//   4. UI panel above the threshold (~21% of the screen), 10 Hz
//   5. light animation, square wave, 8 Hz
//   6. light animation, 5 Hz sine
// See: context/lib/rendering_pipeline.md §7.8

import {
  type NamedReactionDescriptor,
  type SequenceStep,
  defineReaction,
  fire,
  onTriggerEvent,
  wait,
  world,
} from "postretro";
import {
  Tree,
  VStack,
  defineUiTree,
  flashScreen,
  stateEquals,
  updateState,
  type UiTreeRegistration,
} from "postretro/ui";
import { a11yStrobeStore } from "./a11y-strobe-store";

const BURST_MS = 3000;

/** `on` then `off`, repeated at `hz` for the burst. */
function strobe(hz: number, on: string, off: string): SequenceStep[] {
  const halfMs = 500 / hz;
  const steps: SequenceStep[] = [];
  for (let t = 0; t < BURST_MS; t += 2 * halfMs) {
    steps.push(...fire(on), ...wait(halfMs), ...fire(off), ...wait(halfMs));
  }
  return steps;
}

/** One pad's reaction and trigger event, keyed by the pad's tag. */
function pad(
  tag: string,
  steps: SequenceStep[],
): { reaction: NamedReactionDescriptor; tag: string } {
  return { reaction: defineReaction(`a11y.strobe.${tag}`, { sequence: steps }), tag };
}

/** A white panel of `width` × `height` reference px, shown while `lit` is true. */
function strobePanel(name: string, width: number, height: number, lit: typeof a11yStrobeStore.smallPanel) {
  return defineUiTree({
    name,
    alwaysOn: true,
    tree: Tree(
      { anchor: "center", offset: [0, 0], captureMode: "passthrough" },
      VStack({
        width,
        padding: height / 2,
        fill: [1, 1, 1, 1],
        visibleWhen: stateEquals(lit, true),
      }),
    ),
  });
}

/** One sampled period of a light's brightness. */
function lightStrobe(tag: string, periodMs: number, brightness: number[]): SequenceStep[] {
  return world.query({ component: "light", tag }).map((light) => ({
    id: light.id,
    primitive: "setLightAnimation" as const,
    args: {
      periodMs,
      phase: null,
      playCount: Math.round(BURST_MS / periodMs),
      startActive: true,
      brightness,
      color: null,
      direction: null,
    },
  }));
}

export function setupLevel(_ctx: unknown): {
  reactions: NamedReactionDescriptor[];
  triggerEvents: ReturnType<typeof onTriggerEvent>[];
  uiTrees: UiTreeRegistration[];
} {
  const toggles: NamedReactionDescriptor[] = [
    defineReaction("a11y.flash.white", flashScreen([1, 1, 1, 0.9], 40)),
    defineReaction("a11y.flash.red", flashScreen([1, 0, 0, 0.9], 80)),
    defineReaction("a11y.flash.none", flashScreen([0, 0, 0, 0], 1)),
    defineReaction("a11y.small.on", updateState(a11yStrobeStore.smallPanel, true)),
    defineReaction("a11y.small.off", updateState(a11yStrobeStore.smallPanel, false)),
    defineReaction("a11y.large.on", updateState(a11yStrobeStore.largePanel, true)),
    defineReaction("a11y.large.off", updateState(a11yStrobeStore.largePanel, false)),
  ];

  const sineSamples = 32;
  const sine = Array.from(
    { length: sineSamples },
    (_, i) => 0.5 + 0.5 * Math.sin((2 * Math.PI * i) / sineSamples),
  );
  const pads = [
    pad("strobe_white", strobe(10, "a11y.flash.white", "a11y.flash.none")),
    pad("strobe_red", strobe(5, "a11y.flash.red", "a11y.flash.none")),
    pad("strobe_small_panel", strobe(10, "a11y.small.on", "a11y.small.off")),
    pad("strobe_large_panel", strobe(10, "a11y.large.on", "a11y.large.off")),
    pad("strobe_light_square", lightStrobe("a11y_light_square", 125, [1, 0])),
    pad("strobe_light_sine", lightStrobe("a11y_light_sine", 200, sine)),
  ];

  return {
    reactions: [...toggles, ...pads.map((p) => p.reaction)],
    triggerEvents: pads.map((p) => onTriggerEvent({ tag: p.tag }, "enter", [p.reaction])),
    uiTrees: [
      strobePanel("a11y.strobe.smallPanel", 200, 200, a11yStrobeStore.smallPanel),
      strobePanel("a11y.strobe.largePanel", 480, 400, a11yStrobeStore.largePanel),
    ],
  };
}
