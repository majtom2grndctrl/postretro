// Reflection-probes manual-test map script (maps/reflection-probes-test.map).
// Two lights carry the tag `rp_switch_light`: a cyan one south-east of the
// pool and a cyan one over room B's basin. Both are curveless baked lights;
// this script's setLightAnimation steps earn them an animated-bake
// reservation at compile time, so each takes a slot in the animated-light
// roster and the reflection probes' share tables.
//
// The red switch beside each pair fires `reflectionProbes.lightOff` and the
// green one `reflectionProbes.lightOn`. Both hold their brightness with
// `playCount: null`, so neither step ever settles: a finite count would let
// the light bridge write `intensity *= 0` back as static state after an off,
// and no later on could recover it.

import { defineReaction, getMapEntities } from "postretro";

export function setupLevel() {
  const lights = getMapEntities("light", { tag: "rp_switch_light" });
  if (lights.length === 0) {
    throw new Error(
      "reflection-probes-test: no light tagged `rp_switch_light` -- check `_tags` in reflection-probes-test.map",
    );
  }

  const hold = (brightness: number) =>
    lights.map((light) => ({
      id: light.id,
      primitive: "setLightAnimation" as const,
      args: {
        periodMs: 1000,
        phase: null,
        playCount: null,
        startActive: true,
        brightness: [brightness],
        color: null,
        direction: null,
      },
    }));

  const lightOff = defineReaction("reflectionProbes.lightOff", { sequence: hold(0) });
  const lightOn = defineReaction("reflectionProbes.lightOn", { sequence: hold(1) });

  return { reactions: [lightOff, lightOn] };
}
