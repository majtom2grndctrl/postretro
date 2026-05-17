// Reusable animation capability interfaces. Composed by entity handles
// (LightEntityHandle, FogVolumeHandle, future animatable types) to
// declare scalar / vec3 animation channels with a shared vocabulary.
//
// The `Channel` type parameter is type-level documentation only — it
// names the channel at the definition site but does not affect runtime
// dispatch. The handle's implementation closure knows which descriptor
// channel to drive.
//
// See: context/lib/scripting.md §7 (Animation capabilities).

import type { Vec3 } from "postretro";
import type { SequenceStep } from "./data_script";

/** Capability for entities with a scalar animation channel. */
export interface AnimatableScalar<Channel extends string> {
  /** Sine pulse oscillating between `min` and `max` over `periodMs`. Loops forever. */
  pulse(opts: { min: number; max: number; periodMs: number }): SequenceStep[];
  /** One-shot linear ramp from `from` to `to` over `periodMs`. Plays exactly once. */
  fade(opts: { from: number; to: number; periodMs: number }): SequenceStep[];
  /** Irregular flicker between `min` and `max` at `rate` Hz. Loops forever. */
  flicker(opts: { min: number; max: number; rate: number }): SequenceStep[];
  /** Phantom field carrying the channel name for tooling. */
  readonly __channel?: Channel;
}

/** Capability for entities with a vec3 animation channel. */
export interface AnimatableVec3<Channel extends string> {
  /** Uniform cycle through the given vectors over `periodMs`. No handle composes this directly — vec3 channels are declared with channel-named methods (`colorShift`, `sweep`) on the handle to avoid TypeScript method-name collision. This interface documents the algorithm shape for future handles. */
  cycle(opts: { values: Vec3[]; periodMs: number }): SequenceStep[];
  readonly __channel?: Channel;
}
