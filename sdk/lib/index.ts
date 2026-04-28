// SDK library entry — re-exports every public symbol the prelude installs as
// a global. The `--prelude` mode of `scripts-build` consumes this file.
// See: context/lib/scripting.md §7
//
// When adding exports here, also update TS_SDK_LIB_BLOCK in crates/postretro/src/scripting/typedef.rs and regenerate sdk/lib/prelude.js.

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
