--!strict
-- Reference vocabulary for authoring `LightAnimation` values in Luau.
--
-- These helpers are pure: they construct and return `LightAnimation`
-- tables without touching the engine. Modders can use them directly,
-- compose them, or read them as worked examples when rolling their own
-- curves.
--
-- None of the helpers below set `phase`. When staggering across a row
-- of lights, set `phase` at the call site (see `world.luau` for the
-- wave example).

-- `LightAnimation` and `Vec3` are provided ambiently by the generated
-- `sdk/types/postretro.d.luau` definitions file loaded via luau-lsp.

--- Per-channel keyframe format accepted by `timeline` and `sequence`.
--- First slot is a timestamp (absolute for `timeline`, delta for
--- `sequence`); remaining slots carry the value for that keyframe.
export type Keyframe = {number}

local LightAnimationSdk = {}

-- ---------------------------------------------------------------------
-- Brightness curves
-- ---------------------------------------------------------------------

-- Fixed 8-step irregular brightness pattern in [0, 1]. Reused for every
-- `flicker` call so the curve is deterministic across reloads and the
-- pattern is visually recognizable without importing a PRNG.
local FLICKER_PATTERN: {number} = {
  0.95, 0.40, 1.00, 0.72, 0.15, 0.88, 0.30, 0.65,
}

--- Returns an 8-sample irregular brightness curve flickering between
--- `minBrightness` and `maxBrightness`.
---
--- `rate` is the flicker frequency in Hz -- `periodMs` is `1000 / rate`.
--- Callers set `phase` at the call site if they need to stagger multiple
--- flickering lights.
function LightAnimationSdk.flicker(
  minBrightness: number,
  maxBrightness: number,
  rate: number
): LightAnimation
  local lo = math.min(minBrightness, maxBrightness)
  local hi = math.max(minBrightness, maxBrightness)
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

--- Returns a 16-sample sine-approximating brightness curve oscillating
--- between `minBrightness` and `maxBrightness` over one full `periodMs`.
---
--- Sample `i` is evaluated at `i / 16` of the period (0-indexed, matching
--- the TypeScript vocabulary).
function LightAnimationSdk.pulse(
  minBrightness: number,
  maxBrightness: number,
  periodMs: number
): LightAnimation
  local SAMPLES = 16
  local lo = math.min(minBrightness, maxBrightness)
  local hi = math.max(minBrightness, maxBrightness)
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

-- ---------------------------------------------------------------------
-- Color / direction curves
-- ---------------------------------------------------------------------

--- Cycles uniformly through the given RGB `colors` over `periodMs`.
---
--- Only valid on dynamic lights; the engine rejects color animation on
--- baked lights (the `world.luau` wrapper surfaces that with a clearer
--- error).
function LightAnimationSdk.colorShift(
  colors: {{number}},
  periodMs: number
): LightAnimation
  local color: {Vec3} = {}
  for i, rgb in ipairs(colors) do
    color[i] = { x = rgb[1], y = rgb[2], z = rgb[3] }
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

--- Sweeps the light's `direction` channel through `directions` over
--- `periodMs`. Direction samples are normalized defensively even though
--- the primitive also normalizes non-unit inputs -- zero-length samples
--- still error at the primitive seam.
function LightAnimationSdk.sweep(
  directions: {{number}},
  periodMs: number
): LightAnimation
  local direction: {Vec3} = {}
  for i, d in ipairs(directions) do
    local x, y, z = d[1], d[2], d[3]
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
-- Keyframe authoring helpers
-- ---------------------------------------------------------------------

local function validateKeyframes(keyframes: {Keyframe}, isSequence: boolean)
  local label = if isSequence then "sequence" else "timeline"
  if type(keyframes) ~= "table" or #keyframes == 0 then
    error(`{label}: keyframes must be a non-empty array`)
  end
  local first = keyframes[1]
  if type(first) ~= "table" or #first == 0 then
    error(`{label}: entry 0 is empty`)
  end
  local arity = #first

  local prevT: number = -math.huge
  for i, kf in ipairs(keyframes) do
    -- Entry indices reported in the error messages are 0-based to match
    -- the TypeScript vocabulary (so tests comparing messages line up).
    local idx = i - 1
    if type(kf) ~= "table" then
      error(`{label}: entry {idx} is not an array`)
    end
    if #kf ~= arity then
      error(`{label}: entry {idx} has arity {#kf}, expected {arity}`)
    end
    for s = 1, #kf do
      local v = kf[s]
      -- Luau: non-finite is NaN or +/-inf. `v ~= v` catches NaN.
      if type(v) ~= "number" or v ~= v or v == math.huge or v == -math.huge then
        error(`{label}: entry {idx} slot {s - 1} is not a finite number`)
      end
    end
    local t = kf[1]
    if i > 1 and t <= prevT then
      error(`{label}: entry {idx} timestamp {t} is not strictly greater than previous {prevT}`)
    end
    prevT = t
  end
end

--- Validates a list of `{absolute_ms, ...value}` keyframes and returns
--- it unchanged. Raises a descriptive `error` naming the offending entry
--- if:
---
--- - The list is empty.
--- - Any entry is empty or has a different arity from the first.
--- - Any slot is a non-finite number.
--- - Timestamps are not strictly increasing.
---
--- The engine consumes `{absolute_ms, ...value}` directly; `timeline`
--- exists purely for shape validation so authoring mistakes surface
--- instead of being silently dropped.
function LightAnimationSdk.timeline(keyframes: {Keyframe}): {Keyframe}
  validateKeyframes(keyframes, false)
  return keyframes
end

--- Accepts `{delta_ms, ...value}` keyframes and returns the canonical
--- `{absolute_ms, ...value}` form by accumulating deltas. The first
--- entry is passed through verbatim; subsequent timestamps are the
--- running sum of all preceding deltas plus the current delta.
---
--- Validates the accumulated timeline with the same rules as `timeline`,
--- so non-positive deltas (which would produce non-monotonic absolute
--- timestamps after the first keyframe) raise a descriptive error.
function LightAnimationSdk.sequence(keyframes: {Keyframe}): {Keyframe}
  if type(keyframes) ~= "table" or #keyframes == 0 then
    error("sequence: keyframes must be a non-empty array")
  end
  local first = keyframes[1]
  if type(first) ~= "table" or #first == 0 then
    error("sequence: entry 0 is empty")
  end
  local arity = #first

  local out: {Keyframe} = {}
  -- Copy the first entry so we don't alias the caller's input table.
  local firstCopy: Keyframe = {}
  for s = 1, arity do
    firstCopy[s] = first[s]
  end
  out[1] = firstCopy

  for i = 2, #keyframes do
    local kf = keyframes[i]
    local idx = i - 1 -- 0-based for error parity with TypeScript.
    if type(kf) ~= "table" then
      error(`sequence: entry {idx} is not an array`)
    end
    if #kf ~= arity then
      error(`sequence: entry {idx} has arity {#kf}, expected {arity}`)
    end
    for s = 1, #kf do
      local v = kf[s]
      if type(v) ~= "number" or v ~= v or v == math.huge or v == -math.huge then
        error(`sequence: entry {idx} slot {s - 1} is not a finite number`)
      end
    end
    local delta = kf[1]
    local prevT = out[i - 1][1]
    local absT = prevT + delta
    if absT <= prevT then
      error(`sequence: entry {idx} delta {delta} produces non-monotonic timestamp (prev={prevT}, next={absT})`)
    end
    local copy: Keyframe = {}
    for s = 1, arity do
      copy[s] = kf[s]
    end
    copy[1] = absT
    out[i] = copy
  end

  -- Defensive re-validation of the accumulated output. Catches any
  -- inconsistency we didn't already flag (e.g. the first entry's fields
  -- being non-finite).
  validateKeyframes(out, true)
  return out
end

return LightAnimationSdk
