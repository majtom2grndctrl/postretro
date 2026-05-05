// Fog volume entity handle.
// Mirrors the LightEntity vocabulary in `./lights`, adapted to the
// fog_volume ComponentValue surface.
// See: context/lib/scripting.md §10 (external API shape)
// and docs/scripting-reference.md (public API surface).

import type {
  FogVolumeEntity as GeneratedFogVolumeEntity,
} from "postretro";

/**
 * Typed handle returned by `world.query({ component: "fog_volume" })`.
 *
 * Fog volumes have no engine-side animation primitive (unlike lights),
 * and the Live VM script tick API has been removed. Until a declarative
 * fog-animation primitive lands, the handle exposes only the query-time
 * snapshot. Fog ambient color is derived from the SH irradiance volume
 * and is not settable via script. When no SH irradiance volume is baked,
 * ambient scatter contribution is zero — fog is effectively invisible
 * without dynamic lights nearby.
 */
export type FogVolumeHandle = GeneratedFogVolumeEntity;

/**
 * Wrap a `FogVolumeEntity` snapshot returned by `worldQuery`. Used by
 * `world.ts` for `world.query({ component: "fog_volume" })`. With the
 * tick-callback helpers removed, this is a pass-through; it is retained
 * so the world.query code path stays symmetric with `wrapLightEntity`
 * and so a future fog-animation primitive can re-introduce mutating
 * methods without touching the call site.
 */
export function wrapFogVolumeEntity(
  snapshot: GeneratedFogVolumeEntity,
): FogVolumeHandle {
  return snapshot;
}
