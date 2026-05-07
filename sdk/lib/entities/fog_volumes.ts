// Fog volume entity handle and pure animation constructors (fogPulse, fogFade).
// Mirrors the LightEntity vocabulary in `./lights`, adapted to the
// fog_volume ComponentValue surface.
// See: context/lib/scripting.md §10 (external API shape)
// and docs/scripting-reference.md (public API surface).

import type {
  EntityId,
  FogVolumeComponent,
  FogVolumeEntity as GeneratedFogVolumeEntity,
  SetFogDensityStep,
  Vec3,
} from "postretro";

/**
 * Typed handle returned by `world.query({ component: "fog_volume" })`.
 *
 * Fog volume parameters are not mutated through methods on this handle —
 * the Live VM tick API has been removed and fog has no per-component
 * animation channel (unlike lights, which carry `LightAnimation`).
 * Authors animate fog by registering sequenced reactions via
 * `registerReaction("levelLoad", { sequence: [...] })`, where the steps
 * are built with the `fogPulse` / `fogFade` constructors below or with
 * raw `setFogDensity` / `setFogScatter` / `setFogEdgeSoftness` /
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
  /** Full fog-volume component snapshot at query time. */
  readonly component: FogVolumeComponent;
}

/**
 * Wrap a `FogVolumeEntity` snapshot returned by `worldQuery`. Used by
 * `world.ts` for `world.query({ component: "fog_volume" })`. Read-only —
 * no mutation methods. Authors register reactions to animate fog.
 */
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
 * Returns a sequence-step array whose steps emit `setFogDensity` calls
 * sampled along a full sine cycle between `min` and `max`. Mirrors the
 * 16-sample `pulse` constructor in `sdk/lib/entities/lights.ts`: sample
 * `i` is evaluated at `i / 16` of the period using the same
 * `mid + amp * sin(theta)` formula with `theta` in `[0, 2π)`.
 *
 * The caller supplies the target `id`; the same `id` is stamped onto
 * every step. Authors typically use this against a single fog entity:
 *
 * ```ts
 * const handle = world.query({ component: "fog_volume", tag: "haze" })[0];
 * registerReaction("levelLoad", {
 *   sequence: fogPulse(handle.id, 0.2, 1.0),
 * });
 * ```
 *
 * Step count is fixed at 16. The dispatcher fires every step on one
 * frame, so the curve plays back in shader-time, not wall-clock time —
 * pacing isn't a parameter the constructor controls.
 *
 * Returns the generated `SetFogDensityStep` shape from `postretro.d.ts`,
 * so the steps slot directly into a `SequenceStep[]` without a separate
 * SDK-only step interface.
 */
export function fogPulse(
  id: EntityId,
  min: number,
  max: number,
): SetFogDensityStep[] {
  const SAMPLES = 16;
  const lo = Math.min(min, max);
  const hi = Math.max(min, max);
  const mid = (lo + hi) * 0.5;
  const amp = (hi - lo) * 0.5;
  const steps: SetFogDensityStep[] = new Array(SAMPLES);
  for (let i = 0; i < SAMPLES; i++) {
    const theta = (i / SAMPLES) * Math.PI * 2;
    const density = mid + amp * Math.sin(theta);
    steps[i] = {
      id,
      primitive: "setFogDensity",
      args: { density },
    };
  }
  return steps;
}

/**
 * Returns a sequence-step array that linearly interpolates `density`
 * from `from` to `to` in evenly-spaced steps.
 *
 * Step count is 16 (matching `fogPulse` / `pulse` for symmetry). Sample
 * `i` is evaluated at `i / (SAMPLES - 1)` of the way from `from` to
 * `to`, so the first step carries `from` and the last step carries
 * `to` exactly.
 *
 * The caller supplies the target `id`; the same `id` is stamped onto
 * every step. Returns the generated `SetFogDensityStep` shape — the
 * step type is shared with `fogPulse` and any author-built density step.
 */
export function fogFade(
  id: EntityId,
  from: number,
  to: number,
): SetFogDensityStep[] {
  const SAMPLES = 16;
  const steps: SetFogDensityStep[] = new Array(SAMPLES);
  for (let i = 0; i < SAMPLES; i++) {
    const t = i / (SAMPLES - 1);
    const density = from + (to - from) * t;
    steps[i] = {
      id,
      primitive: "setFogDensity",
      args: { density },
    };
  }
  return steps;
}
