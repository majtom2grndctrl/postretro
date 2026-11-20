--!strict
-- Emissive volume entity vocabulary: the `EmissiveVolumeHandle` returned by
-- `getMapEntities("emissive")`. The handle composes the generated
-- `EmissiveVolumeEntity` snapshot with capability methods (`pulse`, `fade`,
-- `flicker`) that return `{SetEmissiveAnimationStep}` arrays, usable directly
-- in an array body or a `sequence` (nested tables flatten).
--
-- Values are brightness in [0, 1] across the volume's authored
-- `emissive_min`..`emissive_max`, unlike lights and fog, whose methods take
-- absolute values. Out-of-range values clamp.
--
-- `wrapEmissiveVolumeEntity` is included in the returned table; the Luau
-- prelude evaluator installs it as a temporary global before
-- `map_entities.luau` evaluates, and nils it afterwards.
-- See: context/lib/scripting.md §10.2a (emissive reaction primitives).
-- ---------------------------------------------------------------------

-- `EntityId`, `EmissiveAnimation`, `EmissiveVolumeComponent`,
-- `EmissiveVolumeEntity`, `SetEmissiveAnimationStep`, and `Vec3` are
-- provided ambiently by the generated `sdk/types/postretro.d.luau`
-- definitions file.

--- Emissive member returned by `getMapEntities("emissive")`.
--- Carries the query-time snapshot fields plus capability methods called
--- with `:` (e.g. `sign:flicker({ min = 0.3, max = 1, rate = 6 })`).
export type EmissiveVolumeHandle = {
  id: EntityId,
  position: Vec3,
  tags: {string},
  component: EmissiveVolumeComponent,

  pulse: (self: EmissiveVolumeHandle, opts: { min: number, max: number, periodMs: number }) -> {SetEmissiveAnimationStep},
  fade: (self: EmissiveVolumeHandle, opts: { from: number, to: number, periodMs: number }) -> {SetEmissiveAnimationStep},
  flicker: (self: EmissiveVolumeHandle, opts: { min: number, max: number, rate: number }) -> {SetEmissiveAnimationStep},
}

local EmissiveVolumesSdk = {}

-- ---------------------------------------------------------------------
-- Curve builders
-- ---------------------------------------------------------------------

-- Same 8-step irregular pattern fog_volumes.luau and lights.luau flicker with.
local FLICKER_PATTERN: {number} = {
  0.95, 0.40, 1.00, 0.72, 0.15, 0.88, 0.30, 0.65,
}

-- 17 samples (16 + 1 wrap): the bridge samples linearly, treating N samples
-- as N-1 intervals on [0, 1]; the wrap sample closes the last interval.
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

local function brightnessAnim(periodMs: number, brightness: {number}, playCount: number?): EmissiveAnimation
  return {
    periodMs = periodMs,
    playCount = playCount,
    brightness = brightness,
  }
end

-- ---------------------------------------------------------------------
-- Handle construction
-- ---------------------------------------------------------------------

function EmissiveVolumesSdk.wrapEmissiveVolumeEntity(snapshot: EmissiveVolumeEntity): EmissiveVolumeHandle
  local id: EntityId = snapshot.id

  local handle: EmissiveVolumeHandle = {
    id = id,
    position = snapshot.position,
    tags = snapshot.tags,
    component = snapshot.component,

    pulse = function(self: EmissiveVolumeHandle, opts: { min: number, max: number, periodMs: number }): {SetEmissiveAnimationStep}
      return { { id = id, primitive = "setEmissiveAnimation", args = brightnessAnim(opts.periodMs, buildPulseSamples(opts.min, opts.max), nil) } }
    end,

    fade = function(self: EmissiveVolumeHandle, opts: { from: number, to: number, periodMs: number }): {SetEmissiveAnimationStep}
      return { { id = id, primitive = "setEmissiveAnimation", args = brightnessAnim(opts.periodMs, buildFadeSamples(opts.from, opts.to), 1) } }
    end,

    flicker = function(self: EmissiveVolumeHandle, opts: { min: number, max: number, rate: number }): {SetEmissiveAnimationStep}
      return { { id = id, primitive = "setEmissiveAnimation", args = brightnessAnim(1000 / opts.rate, buildFlickerSamples(opts.min, opts.max), nil) } }
    end,
  }

  return handle
end

return EmissiveVolumesSdk
