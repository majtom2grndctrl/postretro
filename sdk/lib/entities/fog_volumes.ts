// Fog volume entity handle and pure animation constructors (fogPulse, fogFade).
// Mirrors the LightEntity vocabulary in `./lights`, adapted to the
// fog_volume ComponentValue surface.
// See: context/lib/scripting.md §9 (external API shape), §10.2 (fog reaction primitives).

import type {
  EntityId,
  FogAnimation,
  FogVolumeComponent,
  FogVolumeEntity as GeneratedFogVolumeEntity,
  SetFogAnimationStep,
  Vec3,
} from "postretro";

/**
 * Typed handle returned by `world.query({ component: "fog_volume" })`.
 *
 * Fog volume parameters are not mutated through methods on this handle —
 * the Live VM tick API has been removed. Authors animate fog by
 * registering sequenced reactions via
 * `registerReaction("levelLoad", { sequence: [...] })`. The
 * dual-channel animation (density and/or saturation) is installed via
 * `setFogAnimation`; the `fogPulse` / `fogFade`
 * constructors below build single-step `setFogAnimation` reactions for
 * the common cases. Static one-shot tweaks still go through the
 * `setFogDensity` / `setFogScatter` / `setFogEdgeSoftness` /
 * `setFogFalloff` / `setFogParams` step descriptors.
 *
 * The handle exposes only the read-only query-time snapshot. Fog ambient
 * color is derived from the SH irradiance volume and is not settable
 * via script. When no SH irradiance volume is baked, ambient scatter
 * contribution is zero — fog is effectively invisible without dynamic
 * lights nearby.
 */
export interface FogVolumeHandle {
  readonly id: EntityId;
  /** Volume center at query time (AABB midpoint, baked at level load). */
  readonly position: Vec3;
  /** The entity's tags at query time. Empty array if untagged. */
  readonly tags: ReadonlyArray<string>;
  readonly component: FogVolumeComponent;
}

export function wrapFogVolumeEntity(
  snapshot: GeneratedFogVolumeEntity,
): FogVolumeHandle {
  return {
    id: snapshot.id,
    position: snapshot.position,
    tags: snapshot.tags,
    component: snapshot.component,
  };
}

/**
 * Returns a single-step sequence array installing a looping sine-curve
 * `FogAnimation` on the target fog volume, oscillating between `min` and
 * `max` over `periodMs`.
 *
 * One `setFogAnimation` step, not N `setFogDensity` steps: the sequence
 * dispatcher fires every step on the same frame, so a multi-step density
 * array collapses to its last value. Time-varying playback is the fog
 * bridge's job via the `FogAnimation` channel. Greps for the old
 * "16 setFogDensity steps" pattern land here.
 *
 * The curve is 17 samples: `mid + amp * sin(2π·i/16)` for `i` in
 * `[0, 16]`. Sample 16 equals sample 0, making the cycle continuous.
 * The sampler treats N samples as N−1 intervals on [0, 1]; the wrap
 * sample encodes cyclic intent so the final interval interpolates
 * cleanly back to the start rather than snapping. `min` / `max` are
 * normalized so the caller may pass them in either order. `playCount`
 * is `null` — a pulse loops forever.
 *
 * Note: fog uses linear interpolation on CPU; lights use Catmull-Rom on
 * GPU. The two produce visually similar motion at a given keyframe
 * density but differ in curvature between keyframes.
 */
export function fogPulse(
  id: EntityId,
  min: number,
  max: number,
  periodMs: number,
): SetFogAnimationStep[] {
  const SAMPLES = 16;
  const lo = Math.min(min, max);
  const hi = Math.max(min, max);
  const mid = (lo + hi) * 0.5;
  const amp = (hi - lo) * 0.5;
  const density: number[] = new Array(SAMPLES + 1);
  for (let i = 0; i <= SAMPLES; i++) {
    const theta = (i / SAMPLES) * Math.PI * 2;
    density[i] = mid + amp * Math.sin(theta);
  }
  const animation: FogAnimation = {
    periodMs,
    phase: null,
    playCount: null,
    density,
  };
  return [{ id, primitive: "setFogAnimation", args: animation }];
}

/**
 * Returns a single-step sequence array installing a one-shot linear
 * `FogAnimation` that ramps density from `from` to `to` over `periodMs`.
 *
 * One `setFogAnimation` step — see the note on `fogPulse` for why
 * multiple `setFogDensity` steps don't produce interpolation.
 * `playCount: 1` so the curve plays exactly once; the bridge writes the
 * final keyframe back as static density.
 *
 * The curve is 16 evenly-spaced samples; sample `i` is
 * `from + (to - from) * (i / 15)`, so the first sample carries `from`
 * exactly and the last carries `to` exactly.
 *
 * Note: fog animation curves (density and saturation) are sampled with
 * linear interpolation on CPU each frame. Light curves use Catmull-Rom
 * on GPU, so a fog fade and a light fade with the same shape are visually
 * similar but not mathematically identical at keyframe boundaries.
 */
export function fogFade(
  id: EntityId,
  from: number,
  to: number,
  periodMs: number,
): SetFogAnimationStep[] {
  const SAMPLES = 16;
  const density: number[] = new Array(SAMPLES);
  for (let i = 0; i < SAMPLES; i++) {
    const t = i / (SAMPLES - 1);
    density[i] = from + (to - from) * t;
  }
  const animation: FogAnimation = {
    periodMs,
    phase: null,
    playCount: 1,
    density,
  };
  return [{ id, primitive: "setFogAnimation", args: animation }];
}
