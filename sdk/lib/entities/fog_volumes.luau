--!strict
-- Fog volume entity vocabulary: the `FogVolumeHandle` returned by
-- `world:query({ component = "fog_volume" })`, plus the pure animation
-- constructors (`fogPulse`, `fogFade`) that produce sequence-step
-- arrays for `registerReaction("levelLoad", { sequence = ... })`.
--
-- The handle is read-only — fog volumes have no per-component animation
-- channel (unlike lights, which carry `LightAnimation`), and the Live VM
-- script tick API has been removed. Authors animate fog by registering
-- sequenced reactions whose steps invoke `setFogDensity`,
-- `setFogScatter`, `setFogEdgeSoftness`, `setFogFalloff`, or
-- `setFogParams`. `fogPulse` and `fogFade` are pure builders that emit
-- those step arrays.
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

-- `EntityId`, `FogVolumeComponent`, `FogVolumeEntity`, and `Vec3` are
-- provided ambiently by the generated `sdk/types/postretro.d.luau`
-- definitions file.

--- Typed handle returned by `world:query` for a fog-volume entity.
--- Read-only; authors animate fog through sequenced reactions, not
--- methods on the handle.
export type FogVolumeHandle = {
  id: EntityId,
  position: Vec3,
  tags: {string},
  component: FogVolumeComponent,
}

-- The fog density step shape is exported by the generated
-- `postretro.d.luau` definitions file as `SetFogDensityStep`. The SDK
-- builders below return that shape directly so steps slot into
-- `SequenceStep` without an SDK-only step alias.

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

--- Returns a sequence-step array whose steps emit `setFogDensity` calls
--- sampled along a full sine cycle between `min` and `max`. Mirrors the
--- 16-sample `pulse` constructor in `entities/lights.luau`: sample `i`
--- is evaluated at `(i - 1) / 16` of the period (1-indexed Luau,
--- 0-indexed math), and the curve is the same `mid + amp * sin(theta)`
--- shape.
---
--- The caller supplies the target `id`; the same `id` is stamped onto
--- every step.
function FogVolumesSdk.fogPulse(
  id: EntityId,
  min: number,
  max: number
): {SetFogDensityStep}
  local SAMPLES = 16
  local lo = math.min(min, max)
  local hi = math.max(min, max)
  local mid = (lo + hi) * 0.5
  local amp = (hi - lo) * 0.5
  local steps: {SetFogDensityStep} = {}
  for i = 1, SAMPLES do
    -- `(i - 1)` converts the 1-indexed Luau counter to 0-indexed theta, matching the TypeScript fogPulse sampling.
    local theta = ((i - 1) / SAMPLES) * math.pi * 2
    local density = mid + amp * math.sin(theta)
    steps[i] = {
      id = id,
      primitive = "setFogDensity",
      args = { density = density },
    }
  end
  return steps
end

--- Returns a sequence-step array that linearly interpolates `density`
--- from `from` to `to` in evenly-spaced steps.
---
--- Step count is 16 (matching `fogPulse` / `pulse` for symmetry).
--- Sample `i` is evaluated at `(i - 1) / (SAMPLES - 1)` of the way from
--- `from` to `to`, so the first step carries `from` and the last step
--- carries `to` exactly.
---
--- The caller supplies the target `id`; the same `id` is stamped onto
--- every step.
function FogVolumesSdk.fogFade(
  id: EntityId,
  from: number,
  to: number
): {SetFogDensityStep}
  local SAMPLES = 16
  local steps: {SetFogDensityStep} = {}
  for i = 1, SAMPLES do
    local t = (i - 1) / (SAMPLES - 1)
    local density = from + (to - from) * t
    steps[i] = {
      id = id,
      primitive = "setFogDensity",
      args = { density = density },
    }
  end
  return steps
end

return FogVolumesSdk
