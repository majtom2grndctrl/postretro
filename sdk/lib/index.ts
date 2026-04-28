// SDK library entry — re-exports every public symbol the prelude installs as
// a global. The `--prelude` mode of `scripts-build` consumes this file.
// See: context/lib/scripting.md §7

export type { EasingCurve, LightEntity, EntityForComponent, World } from "./world";
export { world } from "./world";

export type { Keyframe } from "./light_animation";
export {
  flicker,
  pulse,
  colorShift,
  sweep,
  timeline,
  sequence,
} from "./light_animation";
