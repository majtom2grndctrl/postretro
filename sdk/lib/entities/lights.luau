--!strict
-- Light entity vocabulary: the `LightEntityHandle` returned by
-- `world:query({ component = "light" })`, plus the pure animation
-- constructors (`flicker`, `pulse`, `colorShift`, `sweep`) that
-- produce `LightAnimation` tables for `light:setAnimation`.
--
-- The handle wraps the generated `LightEntity` snapshot with a single
-- convenience method (`setAnimation`) that calls the underlying
-- scripting primitive. `wrapLightEntity` is included in the returned
-- table; the Luau prelude evaluator (`luau.rs`) extracts it and installs
-- it as a temporary global before `world.luau` evaluates. It is nil'd
-- out by the prelude after `world.luau` has been evaluated.
--
-- `setIntensity` / `setColor` transition helpers were removed alongside
-- the Live VM tick API — they relied on `getComponent` to read live
-- state. Callers that need a transition build a one-cycle
-- `LightAnimation` themselves and hand it to `setAnimation`.
--
-- ---------------------------------------------------------------------
-- Canonical modder example -- rolling pulse down a hallway, 10s loop.
--
-- Map authors tag the hallway lights `"hallway_wave"` in TrenchBroom.
-- The behavior script below queries them at level load, sorts along
-- the x axis, and staggers `phase` across the row so the pulse travels.
--
-- ```lua
-- registerHandler("levelLoad", function()
--   -- world:query dispatches on component; passing component = "light"
--   -- returns LightEntityHandle values with :setAnimation.
--   local lights = world:query({ component = "light", tag = "hallway_wave" })
--   table.sort(lights, function(a, b)
--     return a.position.x < b.position.x
--   end)
--
--   local pulse = {
--     periodMs = 10000,
--     brightness = {
--       0.1, 0.1, 0.1, 0.1, 0.1,
--       0.3, 0.8, 1.0, 0.8, 0.3,
--       0.1, 0.1, 0.1, 0.1, 0.1,
--       0.1, 0.1, 0.1, 0.1, 0.1,
--     },
--   }
--
--   for i, light in ipairs(lights) do
--     light:setAnimation({
--       periodMs = pulse.periodMs,
--       brightness = pulse.brightness,
--       phase = (i - 1) / #lights,
--     })
--   end
-- end)
-- ```
-- ---------------------------------------------------------------------

-- `EntityId`, `LightAnimation`, `LightEntity` (the generated snapshot
-- shape), `Vec3`, and the `setLightAnimation` primitive are provided
-- ambiently by the generated `sdk/types/postretro.d.luau` definitions
-- file.

--- Typed handle returned by `world:query` for a light entity. Carries
--- the query-time snapshot fields plus a convenience method that calls
--- the underlying scripting primitive. Methods are called with `:`
--- (e.g. `light:setAnimation(anim)`).
export type LightEntityHandle = {
  id: EntityId,
  position: Vec3,
  isDynamic: boolean,
  tags: {string},
  component: LightComponent,

  --- Replace the light's animation. Pass `nil` to clear it. Last call
  --- wins -- lights are always interruptible.
  setAnimation: (self: LightEntityHandle, anim: LightAnimation?) -> (),
}

local LightsSdk = {}

-- ---------------------------------------------------------------------
-- Handle construction
-- ---------------------------------------------------------------------

local function idDebug(id: EntityId): string
  -- `EntityId` is a branded number -- print the underlying value for
  -- error messages.
  return tostring(id)
end

function LightsSdk.wrapLightEntity(snapshot: LightEntity): LightEntityHandle
  local id: EntityId = snapshot.id
  local isDynamic = snapshot.isDynamic

  local handle: LightEntityHandle = {
    id = id,
    position = snapshot.position,
    isDynamic = isDynamic,
    tags = snapshot.tags,
    component = snapshot.component,

    setAnimation = function(self: LightEntityHandle, anim: LightAnimation?)
      if anim ~= nil and anim.color ~= nil and not isDynamic then
        error(`setAnimation: light {idDebug(id)} is not dynamic; color animation is only valid on dynamic lights`)
      end
      setLightAnimation(id, anim)
    end,
  }

  return handle
end

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
function LightsSdk.flicker(
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
function LightsSdk.pulse(
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
--- baked lights (the `wrapLightEntity` handle wrapper in this file surfaces that with a clearer
--- error).
function LightsSdk.colorShift(
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
function LightsSdk.sweep(
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

return LightsSdk
