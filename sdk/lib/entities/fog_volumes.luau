--!strict
-- Fog volume entity vocabulary: the `FogVolumeHandle` returned by
-- `world:query({ component = "fog_volume" })`. The handle composes the
-- generated `FogVolumeEntity` snapshot with capability methods
-- (`pulse`, `fade`, `flicker`, `pulseSaturation`, `fadeSaturation`)
-- that return `{SetFogAnimationStep}` arrays suitable for splicing into
-- a `registerReaction("...", { sequence = ... })` body.
--
-- `wrapFogVolumeEntity` is included in the returned table; the Luau
-- prelude evaluator (`luau.rs`) extracts it and installs it as a
-- temporary global before `world.luau` evaluates. It is nil'd out by
-- the prelude after `world.luau` has been evaluated. No other fields
-- from this module are promoted to bare globals — the old free
-- constructors (`fogPulse`, `fogFade`) are now handle methods.
--
-- Fog ambient color is derived from the SH irradiance volume and is not
-- settable via script.
-- See: context/lib/scripting.md §10.2 (fog reaction primitives).
-- ---------------------------------------------------------------------

-- `EntityId`, `FogAnimation`, `FogVolumeComponent`, `FogVolumeEntity`,
-- `SetFogAnimationStep`, and `Vec3` are provided ambiently by the
-- generated `sdk/types/postretro.d.luau` definitions file.

--- Typed handle returned by `world:query` for a fog-volume entity.
--- Carries the query-time snapshot fields plus capability methods called
--- with `:` (e.g. `fog:pulse({ min = 0.2, max = 1.0, periodMs = 1500 })`).
export type FogVolumeHandle = {
  id: EntityId,
  position: Vec3,
  tags: {string},
  component: FogVolumeComponent,

  pulse: (self: FogVolumeHandle, opts: { min: number, max: number, periodMs: number }) -> {SetFogAnimationStep},
  fade: (self: FogVolumeHandle, opts: { from: number, to: number, periodMs: number }) -> {SetFogAnimationStep},
  flicker: (self: FogVolumeHandle, opts: { min: number, max: number, rate: number }) -> {SetFogAnimationStep},
  pulseSaturation: (self: FogVolumeHandle, opts: { min: number, max: number, periodMs: number }) -> {SetFogAnimationStep},
  fadeSaturation: (self: FogVolumeHandle, opts: { from: number, to: number, periodMs: number }) -> {SetFogAnimationStep},
}

local FogVolumesSdk = {}

-- ---------------------------------------------------------------------
-- Curve builders
-- ---------------------------------------------------------------------

-- Same 8-step irregular pattern lights.luau uses for `flicker`.
local FLICKER_PATTERN: {number} = {
  0.95, 0.40, 1.00, 0.72, 0.15, 0.88, 0.30, 0.65,
}

-- 17 samples (16 + 1 wrap): the linear sampler treats N samples as N-1
-- intervals on [0, 1]; the wrap sample encodes cyclic intent so the
-- final interval interpolates cleanly back to the start. (Lights pulse
-- uses 16 samples on GPU Catmull-Rom; fog uses CPU linear, so the wrap
-- sample matters here.)
local function buildPulseSamples(min: number, max: number): {number}
  local SAMPLES = 16
  local lo = math.min(min, max)
  local hi = math.max(min, max)
  local mid = (lo + hi) * 0.5
  local amp = (hi - lo) * 0.5
  local out: {number} = {}
  for i = 1, SAMPLES + 1 do
    local theta = ((i - 1) / SAMPLES) * math.pi * 2
    out[i] = mid + amp * math.sin(theta)
  end
  return out
end

local function buildFadeSamples(from: number, to: number): {number}
  local SAMPLES = 16
  local out: {number} = {}
  for i = 1, SAMPLES do
    local t = (i - 1) / (SAMPLES - 1)
    out[i] = from + (to - from) * t
  end
  return out
end

local function buildFlickerSamples(min: number, max: number): {number}
  local lo = math.min(min, max)
  local hi = math.max(min, max)
  local span = hi - lo
  local out: {number} = {}
  for i, t in ipairs(FLICKER_PATTERN) do
    out[i] = lo + t * span
  end
  return out
end

local function densityAnim(periodMs: number, density: {number}, playCount: number?): FogAnimation
  return {
    periodMs = periodMs,
    phase = nil,
    playCount = playCount,
    density = density,
  }
end

local function saturationAnim(periodMs: number, saturation: {number}, playCount: number?): FogAnimation
  return {
    periodMs = periodMs,
    phase = nil,
    playCount = playCount,
    saturation = saturation,
  }
end

-- ---------------------------------------------------------------------
-- Handle construction
-- ---------------------------------------------------------------------

function FogVolumesSdk.wrapFogVolumeEntity(snapshot: FogVolumeEntity): FogVolumeHandle
  local id: EntityId = snapshot.id

  local handle: FogVolumeHandle = {
    id = id,
    position = snapshot.position,
    tags = snapshot.tags,
    component = snapshot.component,

    pulse = function(self: FogVolumeHandle, opts: { min: number, max: number, periodMs: number }): {SetFogAnimationStep}
      return { { id = id, primitive = "setFogAnimation", args = densityAnim(opts.periodMs, buildPulseSamples(opts.min, opts.max), nil) } }
    end,

    fade = function(self: FogVolumeHandle, opts: { from: number, to: number, periodMs: number }): {SetFogAnimationStep}
      return { { id = id, primitive = "setFogAnimation", args = densityAnim(opts.periodMs, buildFadeSamples(opts.from, opts.to), 1) } }
    end,

    flicker = function(self: FogVolumeHandle, opts: { min: number, max: number, rate: number }): {SetFogAnimationStep}
      return { { id = id, primitive = "setFogAnimation", args = densityAnim(1000 / opts.rate, buildFlickerSamples(opts.min, opts.max), nil) } }
    end,

    pulseSaturation = function(self: FogVolumeHandle, opts: { min: number, max: number, periodMs: number }): {SetFogAnimationStep}
      return { { id = id, primitive = "setFogAnimation", args = saturationAnim(opts.periodMs, buildPulseSamples(opts.min, opts.max), nil) } }
    end,

    fadeSaturation = function(self: FogVolumeHandle, opts: { from: number, to: number, periodMs: number }): {SetFogAnimationStep}
      return { { id = id, primitive = "setFogAnimation", args = saturationAnim(opts.periodMs, buildFadeSamples(opts.from, opts.to), 1) } }
    end,
  }

  return handle
end

return FogVolumesSdk
