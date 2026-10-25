
declare module "postretro/audio" {
  import type { PrimitiveReactionDescriptor } from "postretro";

  /** Ghost bus fields `setGhostAudio` may set. Every field is optional, but a call must name at least one of `volume`, `highPassHz`, `lowPassHz` or `drive`. */
  export type GhostAudioFields = {
    /** Bus volume, linear 0..=1 on the perceptual volume taper; 0 silences ghost voices. */
    volume?: number;
    /** High-pass cutoff in Hz, 20..=20000. Must stay below the low-pass cutoff. */
    highPassHz?: number;
    /** Low-pass cutoff in Hz, 20..=20000. Must stay above the high-pass cutoff. */
    lowPassHz?: number;
    /** Distortion drive 0..=1; 0 applies no distortion. */
    drive?: number;
    /** Ramp length in milliseconds (finite, >= 0). Defaults to 0: the change lands at once. */
    periodMs?: number;
  };

  /** Change the mod-wide Ghost bus mix during play. Returns a primitive reaction body to pass whole to `defineReaction` (it names no target). Applies on the host at its next tick, in call order, and replicates; does nothing on a connected client. Each named field ramps on its own over `periodMs`; an omitted field keeps its value and any ramp in flight, and fields no script set follow the manifest `audio.ghost`. A non-finite or out-of-range field, a call naming no field, or targets that would leave `highPassHz` at or above `lowPassHz` warn and drop the reaction. */
  export function setGhostAudio(fields: GhostAudioFields): PrimitiveReactionDescriptor;
}
