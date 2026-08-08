// DEMO CONTENT — data script for the generated stress-warren maps.
//
// `tools/gen_stress_map.py` tags a share of each map's baked coverage lights
// with `warren_script_pulse` and wires this file in via the worldspawn
// `data_script` KVP whenever any such light is emitted. At level load the
// engine runs `setupLevel(ctx)` and drains the returned reactions.
//
// The one reaction here drives every tagged light with a script-authored
// brightness pulse via `setLightAnimation` (spelled through the `light.pulse`
// helper). Because the pulse reaches these baked static lights at compile-time
// evaluation, the compiler's script-derived light-membership pass reserves each
// one's animated-bake structures automatically (build_pipeline.md §Custom FGD,
// `_animated`) — no per-light `_animated 1` KVP is needed.
//
// The map's OTHER animated lights are KVP-driven (an authored `brightness_curve`
// baked entirely at compile time) and need no script at all — this file only
// owns the script-driven half of the animated-light mix.

import { defineReaction, world } from "postretro";

export function setupLevel(_ctx: unknown) {
  const lights = world.query({
    component: "light",
    tag: "warren_script_pulse",
  });
  if (lights.length === 0) {
    return { reactions: [] };
  }
  return {
    reactions: [
      defineReaction("levelLoad", {
        sequence: lights.flatMap((light) =>
          light.pulse({ min: 0.2, max: 1.0, periodMs: 1400 }),
        ),
      }),
    ],
  };
}
