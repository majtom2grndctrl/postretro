--!strict
-- Light entity vocabulary: the `LightEntityHandle` returned by
-- `world:query({ component = "light" })`. The handle composes the
-- generated `LightEntity` snapshot with capability methods (`pulse`,
-- `fade`, `flicker`, `colorShift`, `sweep`) that return
-- `{SetLightAnimationStep}` arrays suitable for splicing into a
-- `registerReaction("...", { sequence = ... })` body.
--
-- `wrapLightEntity` is included in the returned table; the Luau prelude
-- evaluator (`luau.rs`) extracts it and installs it as a temporary
-- global before `world.luau` evaluates. It is nil'd out by the prelude
-- after `world.luau` has been evaluated. No other fields from this
-- module are promoted to bare globals — the old free constructors
-- (`flicker`, `pulse`, `colorShift`, `sweep`) are now handle methods.
--
-- ---------------------------------------------------------------------

-- `EntityId`, `LightAnimation`, `LightEntity` (the generated snapshot
-- shape), `Vec3`, and `SetLightAnimationStep` are provided ambiently by
-- the generated `sdk/types/postretro.d.luau` definitions file.

--- Typed handle returned by `world:query` for a light entity. Carries
--- the query-time snapshot fields plus capability methods called with
--- `:` (e.g. `light:pulse({ min = 0.2, max = 1.0, periodMs = 1500 })`).
export type LightEntityHandle = {
  id: EntityId,
  position: Vec3,
  isDynamic: boolean,
  tags: {string},
  component: LightComponent,

  pulse: (self: LightEntityHandle, opts: { min: number, max: number, periodMs: number }) -> {SetLightAnimationStep},
  fade: (self: LightEntityHandle, opts: { from: number, to: number, periodMs: number }) -> {SetLightAnimationStep},
  flicker: (self: LightEntityHandle, opts: { min: number, max: number, rate: number }) -> {SetLightAnimationStep},
  colorShift: (self: LightEntityHandle, opts: { values: {Vec3}, periodMs: number }) -> {SetLightAnimationStep},
  sweep: (self: LightEntityHandle, opts: { values: {Vec3}, periodMs: number }) -> {SetLightAnimationStep},
}

local LightsSdk = {}

-- ---------------------------------------------------------------------
-- Curve builders
-- ---------------------------------------------------------------------

-- Fixed 8-step irregular brightness pattern in [0, 1]. Reused for every
-- `flicker` call so the curve is deterministic across reloads and the
-- pattern is visually recognizable without importing a PRNG.
local FLICKER_PATTERN: {number} = {
  0.95, 0.40, 1.00, 0.72, 0.15, 0.88, 0.30, 0.65,
}

local function buildPulse(min: number, max: number, periodMs: number): LightAnimation
  local SAMPLES = 16
  local lo = math.min(min, max)
  local hi = math.max(min, max)
  local mid = (lo + hi) * 0.5
  local amp = (hi - lo) * 0.5
  local brightness: {number} = {}
  for i = 1, SAMPLES do
    -- `(i - 1) / SAMPLES` reproduces the 0-indexed TypeScript sampling.
    local theta = ((i - 1) / SAMPLES) * math.pi * 2
    brightness[i] = mid + amp * math.sin(theta)
  end
  return {
    periodMs = periodMs,
    phase = nil,
    playCount = nil,
    brightness = brightness,
    color = nil,
    direction = nil,
  }
end

local function buildFade(from: number, to: number, periodMs: number): LightAnimation
  local SAMPLES = 16
  local brightness: {number} = {}
  for i = 1, SAMPLES do
    local t = (i - 1) / (SAMPLES - 1)
    brightness[i] = from + (to - from) * t
  end
  return {
    periodMs = periodMs,
    phase = nil,
    playCount = 1,
    brightness = brightness,
    color = nil,
    direction = nil,
  }
end

local function buildFlicker(min: number, max: number, rate: number): LightAnimation
  local lo = math.min(min, max)
  local hi = math.max(min, max)
  local span = hi - lo
  local brightness: {number} = {}
  for i, t in ipairs(FLICKER_PATTERN) do
    brightness[i] = lo + t * span
  end
  return {
    periodMs = 1000 / rate,
    phase = nil,
    playCount = nil,
    brightness = brightness,
    color = nil,
    direction = nil,
  }
end

local function buildColorShift(values: {Vec3}, periodMs: number): LightAnimation
  local color: {Vec3} = {}
  for i, v in ipairs(values) do
    color[i] = { x = v.x, y = v.y, z = v.z }
  end
  return {
    periodMs = periodMs,
    phase = nil,
    playCount = nil,
    brightness = nil,
    color = color,
    direction = nil,
  }
end

local function buildSweep(values: {Vec3}, periodMs: number): LightAnimation
  local direction: {Vec3} = {}
  for i, d in ipairs(values) do
    local x, y, z = d.x, d.y, d.z
    local len = math.sqrt(x * x + y * y + z * z)
    if len > 0 then
      direction[i] = { x = x / len, y = y / len, z = z / len }
    else
      -- Pass zero-length through untouched; the primitive will reject it
      -- with `InvalidArgument` and a specific error message.
      direction[i] = { x = x, y = y, z = z }
    end
  end
  return {
    periodMs = periodMs,
    phase = nil,
    playCount = nil,
    brightness = nil,
    color = nil,
    direction = direction,
  }
end

-- ---------------------------------------------------------------------
-- Handle construction
-- ---------------------------------------------------------------------

function LightsSdk.wrapLightEntity(snapshot: LightEntity): LightEntityHandle
  local id: EntityId = snapshot.id

  local handle: LightEntityHandle = {
    id = id,
    position = snapshot.position,
    isDynamic = snapshot.isDynamic,
    tags = snapshot.tags,
    component = snapshot.component,

    pulse = function(self: LightEntityHandle, opts: { min: number, max: number, periodMs: number }): {SetLightAnimationStep}
      return { { id = id, primitive = "setLightAnimation", args = buildPulse(opts.min, opts.max, opts.periodMs) } }
    end,

    fade = function(self: LightEntityHandle, opts: { from: number, to: number, periodMs: number }): {SetLightAnimationStep}
      return { { id = id, primitive = "setLightAnimation", args = buildFade(opts.from, opts.to, opts.periodMs) } }
    end,

    flicker = function(self: LightEntityHandle, opts: { min: number, max: number, rate: number }): {SetLightAnimationStep}
      return { { id = id, primitive = "setLightAnimation", args = buildFlicker(opts.min, opts.max, opts.rate) } }
    end,

    colorShift = function(self: LightEntityHandle, opts: { values: {Vec3}, periodMs: number }): {SetLightAnimationStep}
      return { { id = id, primitive = "setLightAnimation", args = buildColorShift(opts.values, opts.periodMs) } }
    end,

    sweep = function(self: LightEntityHandle, opts: { values: {Vec3}, periodMs: number }): {SetLightAnimationStep}
      return { { id = id, primitive = "setLightAnimation", args = buildSweep(opts.values, opts.periodMs) } }
    end,
  }

  return handle
end

return LightsSdk
