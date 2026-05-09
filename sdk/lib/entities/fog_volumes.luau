--!strict
-- Fog volume entity vocabulary: the `FogVolumeHandle` returned by
-- `world:query({ component = "fog_volume" })`, plus the pure animation
-- constructors (`fogPulse`, `fogFade`) that produce sequence-step
-- arrays for `registerReaction("levelLoad", { sequence = ... })`.
--
-- The handle is read-only — fog volumes carry dual-channel animation
-- (density and/or saturation) through the `setFogAnimation` reaction
-- primitive, and the Live VM script tick API
-- has been removed. Static one-shot tweaks still go through
-- `setFogDensity`, `setFogScatter`, `setFogEdgeSoftness`,
-- `setFogFalloff`, or `setFogParams`. `fogPulse` and `fogFade` are pure
-- builders that emit `setFogAnimation` step arrays.
--
-- `wrapFogVolumeEntity` is included in the returned table; the Luau
-- prelude evaluator (`luau.rs`) extracts it and installs it as a
-- temporary global before `world.luau` evaluates. It is nil'd out by the
-- prelude after `world.luau` has been evaluated. The public builders
-- (`fogPulse`, `fogFade`) are lifted to bare globals via the
-- `FOG_VOLUMES_LUAU_FIELDS` slice in `luau.rs` — same mechanism that
-- promotes `flicker` / `pulse` / `colorShift` / `sweep` from
-- `entities/lights.luau`.
--
-- Fog ambient color is derived from the SH irradiance volume and is not
-- settable via script.
-- See docs/scripting-reference.md.
-- ---------------------------------------------------------------------

-- `EntityId`, `FogAnimation`, `FogVolumeComponent`, `FogVolumeEntity`,
-- `SetFogAnimationStep`, and `Vec3` are provided ambiently by the
-- generated `sdk/types/postretro.d.luau` definitions file.

--- Typed handle returned by `world:query` for a fog-volume entity.
--- Read-only; authors animate fog through sequenced reactions, not
--- methods on the handle.
export type FogVolumeHandle = {
  id: EntityId,
  position: Vec3,
  tags: {string},
  component: FogVolumeComponent,
}

local FogVolumesSdk = {}

-- ---------------------------------------------------------------------
-- Handle construction
-- ---------------------------------------------------------------------

--- Wrap a `FogVolumeEntity` snapshot returned by `worldQuery`. Used by
--- `world.luau` for `world:query({ component = "fog_volume" })`.
--- Read-only -- no mutation methods. Authors register reactions to
--- animate fog.
function FogVolumesSdk.wrapFogVolumeEntity(snapshot: FogVolumeEntity): FogVolumeHandle
  return {
    id = snapshot.id,
    position = snapshot.position,
    tags = snapshot.tags,
    component = snapshot.component,
  }
end

-- ---------------------------------------------------------------------
-- Density curves
-- ---------------------------------------------------------------------

--- Returns a single-step sequence array installing a looping sine-curve
--- `FogAnimation` on the target fog volume, oscillating between `min`
--- and `max` over `periodMs`.
---
--- One `setFogAnimation` step, not N `setFogDensity` steps: the sequence
--- dispatcher fires every step on the same frame, so a multi-step density
--- array collapses to its last value. Time-varying playback is the fog
--- bridge's job via the `FogAnimation` channel. Greps for the old
--- "16 setFogDensity steps" pattern land here.
---
--- The curve is 16 samples of `mid + amp * sin(2*pi*i/16)` for `i` in
--- `[0, 16)`; the 1-indexed Luau loop converts via `(i - 1)`.
--- `min` / `max` are normalized so the caller may pass them in either
--- order. `playCount` is `nil` — a pulse loops forever.
---
--- Note: the curve definition matches `pulse` on lights, but the
--- runtime sampling differs — fog is sampled with linear interpolation
--- on CPU each frame, while lights are sampled with Catmull-Rom on
--- GPU. The two produce visually similar motion but are not
--- mathematically identical at keyframe boundaries.
function FogVolumesSdk.fogPulse(
  id: EntityId,
  min: number,
  max: number,
  periodMs: number
): {SetFogAnimationStep}
  local SAMPLES = 16
  local lo = math.min(min, max)
  local hi = math.max(min, max)
  local mid = (lo + hi) * 0.5
  local amp = (hi - lo) * 0.5
  local density: {number} = {}
  for i = 1, SAMPLES do
    -- `(i - 1)` converts the 1-indexed Luau counter to 0-indexed theta, matching the TypeScript fogPulse sampling.
    local theta = ((i - 1) / SAMPLES) * math.pi * 2
    density[i] = mid + amp * math.sin(theta)
  end
  local animation: FogAnimation = {
    periodMs = periodMs,
    phase = nil,
    playCount = nil,
    density = density,
  }
  return { { id = id, primitive = "setFogAnimation", args = animation } }
end

--- Returns a single-step sequence array installing a one-shot linear
--- `FogAnimation` that ramps density from `from` to `to` over
--- `periodMs`.
---
--- One `setFogAnimation` step — see the note on `fogPulse` for why
--- multiple `setFogDensity` steps don't produce interpolation.
--- `playCount = 1` so the curve plays exactly once; the bridge writes
--- the final keyframe back as static density.
---
--- The curve is 16 evenly-spaced samples; sample `i` (1-indexed) is
--- `from + (to - from) * ((i - 1) / 15)`, so the first sample carries
--- `from` exactly and the last carries `to` exactly.
---
--- Note: fog animation curves (density and saturation) are sampled with
--- linear interpolation on CPU each frame. Light curves use Catmull-Rom
--- on GPU, so a fog fade and a light fade with the same shape are visually
--- similar but not mathematically identical at keyframe boundaries.
function FogVolumesSdk.fogFade(
  id: EntityId,
  from: number,
  to: number,
  periodMs: number
): {SetFogAnimationStep}
  local SAMPLES = 16
  local density: {number} = {}
  for i = 1, SAMPLES do
    local t = (i - 1) / (SAMPLES - 1)
    density[i] = from + (to - from) * t
  end
  local animation: FogAnimation = {
    periodMs = periodMs,
    phase = nil,
    playCount = 1,
    density = density,
  }
  return { { id = id, primitive = "setFogAnimation", args = animation } }
end

return FogVolumesSdk
