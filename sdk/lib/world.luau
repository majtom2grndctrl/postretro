--!strict
-- World-query vocabulary: typed wrapper around `worldQuery` plus the
-- `LightEntity` handle methods. The primitive does the actual work;
-- this file exists so modders import `world` and call `:setAnimation`
-- /`:setIntensity`/`:setColor` on the returned handles the same way
-- they use other vocabulary helpers.
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
--   -- returns LightEntityHandle values with :setAnimation / :setIntensity / :setColor.
--   local lights = world:query({ component = "light", tag = "hallway_wave" })
--   table.sort(lights, function(a, b)
--     return a.transform.position.x < b.transform.position.x
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

-- `EntityId`, `LightAnimation`, `LightComponent`, `LightEntity` (the
-- generated snapshot shape), `Vec3`, `WorldQueryFilter` and the
-- `getComponent`/`setLightAnimation`/`worldQuery` primitives are
-- provided ambiently by the generated `sdk/types/postretro.d.luau`
-- definitions file.

--- Easing family used by `setIntensity` / `setColor` transitions.
export type EasingCurve = "linear" | "easeIn" | "easeOut" | "easeInOut"

--- Typed handle returned by `world:query` for a light entity. Carries
--- the query-time snapshot fields plus convenience methods that call
--- the underlying scripting primitives. Methods are called with `:`
--- (e.g. `light:setAnimation(anim)`).
export type LightEntityHandle = {
  id: EntityId,
  transform: EntityTransform,
  isDynamic: boolean,
  tag: string?,
  component: LightComponent,

  --- Replace the light's animation. Pass `nil` to clear it. Last call
  --- wins -- lights are always interruptible.
  setAnimation: (self: LightEntityHandle, anim: LightAnimation?) -> (),

  --- Transition the light's intensity to `target` over `transitionMs`
  --- milliseconds (default 0, which applies the target instantly).
  ---
  --- Re-reads the live `LightComponent.intensity` from the registry at
  --- call time rather than the handle's query-time snapshot, so chained
  --- transitions compose correctly. `easing` defaults to `"easeInOut"`
  --- when `transitionMs > 0`; it is ignored for instant transitions.
  ---
  --- Internally constructs a one-cycle `LightAnimation` (`playCount = 1`)
  --- and hands it to `setAnimation`.
  setIntensity: (
    self: LightEntityHandle,
    target: number,
    transitionMs: number?,
    easing: EasingCurve?
  ) -> (),

  --- Transition the light's RGB color to `target` over `transitionMs`
  --- milliseconds (default 0). Same live-read / one-cycle pattern as
  --- `setIntensity`.
  ---
  --- Raises a descriptive error on non-dynamic lights (baked lights
  --- cannot have their color animated because their indirect SH was
  --- baked with the compile-time color).
  setColor: (
    self: LightEntityHandle,
    target: {number},
    transitionMs: number?,
    easing: EasingCurve?
  ) -> (),
}

--- Base entity handle returned by `world:query` when the component type
--- is not "light". Carries id, transform, and tag; use `getComponent`
--- to access the component data for non-light entities.
export type EntityHandle = {
  id: EntityId,
  transform: EntityTransform,
  tag: string?,
}

--- Typed vocabulary object returned from `world:query`.
--- Pass `{ component = "light", tag = "..." }` to get `LightEntityHandle`
--- values; any other component filter returns `EntityHandle` values.
export type World = {
  query: ((self: World, filter: { component: "light", tag: string? }) -> {LightEntityHandle})
       & ((self: World, filter: WorldQueryFilter) -> {EntityHandle}),
}

-- ---------------------------------------------------------------------
-- Easing + one-cycle animation builders
-- ---------------------------------------------------------------------

-- 8-sample resolution for transitions. Fine enough for a smooth ease
-- without bloating the primitive call payload.
local EASE_SAMPLES = 8

local function resolveEasing(transitionMs: number, easing: EasingCurve?): EasingCurve
  if transitionMs <= 0 then
    -- Irrelevant for step transitions, but pick a stable default.
    return "linear"
  end
  return easing or "easeInOut"
end

local function easeAt(curve: EasingCurve, t: number): number
  -- t is the normalized sample position in [0, 1].
  if curve == "linear" then
    return t
  elseif curve == "easeIn" then
    return t * t
  elseif curve == "easeOut" then
    return 1 - (1 - t) * (1 - t)
  else
    -- Smoothstep-style ease-in-out.
    if t < 0.5 then
      return 2 * t * t
    else
      return 1 - ((-2 * t + 2) ^ 2) / 2
    end
  end
end

local function buildIntensityAnimation(
  from: number,
  to: number,
  transitionMs: number,
  easing: EasingCurve?
): LightAnimation
  if transitionMs <= 0 then
    return {
      periodMs = 1,
      phase = nil,
      playCount = 1,
      brightness = { to },
      color = nil,
      direction = nil,
    }
  end
  local curve = resolveEasing(transitionMs, easing)
  local brightness: {number} = {}
  for i = 1, EASE_SAMPLES do
    local t = (i - 1) / (EASE_SAMPLES - 1)
    brightness[i] = from + (to - from) * easeAt(curve, t)
  end
  return {
    periodMs = transitionMs,
    phase = nil,
    playCount = 1,
    brightness = brightness,
    color = nil,
    direction = nil,
  }
end

local function buildColorAnimation(
  from: Vec3,
  to: Vec3,
  transitionMs: number,
  easing: EasingCurve?
): LightAnimation
  if transitionMs <= 0 then
    return {
      periodMs = 1,
      phase = nil,
      playCount = 1,
      brightness = nil,
      color = { { x = to.x, y = to.y, z = to.z } },
      direction = nil,
    }
  end
  local curve = resolveEasing(transitionMs, easing)
  local color: {Vec3} = {}
  for i = 1, EASE_SAMPLES do
    local t = (i - 1) / (EASE_SAMPLES - 1)
    local k = easeAt(curve, t)
    color[i] = {
      x = from.x + (to.x - from.x) * k,
      y = from.y + (to.y - from.y) * k,
      z = from.z + (to.z - from.z) * k,
    }
  end
  return {
    periodMs = transitionMs,
    phase = nil,
    playCount = 1,
    brightness = nil,
    color = color,
    direction = nil,
  }
end

-- ---------------------------------------------------------------------
-- Handle construction
-- ---------------------------------------------------------------------

local function idDebug(id: EntityId): string
  -- `EntityId` is a branded number -- print the underlying value for
  -- error messages.
  return tostring(id)
end

local function readLightComponent(id: EntityId): LightComponent
  local c = getComponent(id, "Light")
  if c.kind ~= "Light" then
    error(`expected Light component on entity {idDebug(id)}, got {c.kind}`)
  end
  return c.value
end

local function wrapLightEntity(snapshot: LightEntity): LightEntityHandle
  local id: EntityId = snapshot.id
  local isDynamic = snapshot.isDynamic

  local handle: LightEntityHandle = {
    id = id,
    transform = snapshot.transform,
    isDynamic = isDynamic,
    tag = snapshot.tag,
    component = snapshot.component,

    setAnimation = function(self: LightEntityHandle, anim: LightAnimation?)
      if anim ~= nil and anim.color ~= nil and not isDynamic then
        error(`setAnimation: light {idDebug(id)} is not dynamic; color animation is only valid on dynamic lights`)
      end
      setLightAnimation(id, anim)
    end,

    setIntensity = function(
      self: LightEntityHandle,
      target: number,
      transitionMs: number?,
      easing: EasingCurve?
    )
      local ms = transitionMs or 0
      local live = readLightComponent(id)
      local anim = buildIntensityAnimation(live.intensity, target, ms, easing)
      setLightAnimation(id, anim)
    end,

    setColor = function(
      self: LightEntityHandle,
      target: {number},
      transitionMs: number?,
      easing: EasingCurve?
    )
      if not isDynamic then
        error(`setColor: light {idDebug(id)} is not dynamic; color can only be animated on dynamic lights`)
      end
      local ms = transitionMs or 0
      local live = readLightComponent(id)
      local to: Vec3 = { x = target[1], y = target[2], z = target[3] }
      local anim = buildColorAnimation(live.color, to, ms, easing)
      setLightAnimation(id, anim)
    end,
  }

  return handle
end

-- ---------------------------------------------------------------------
-- Exported `world` singleton
-- ---------------------------------------------------------------------

local world: World = {
  query = function(self: World, filter: WorldQueryFilter): {EntityHandle}
    local normalized: WorldQueryFilter = {
      component = filter.component,
      tag = filter.tag,
    }
    local raw = worldQuery(normalized)
    if filter.component == "light" then
      local out: {LightEntityHandle} = {}
      for i, snapshot in ipairs(raw) do
        out[i] = wrapLightEntity(snapshot :: LightEntity)
      end
      return out :: any
    end
    local out: {EntityHandle} = {}
    -- Cast at the array level: the primitive returns per-component snapshots
    -- that are supersets of `Entity`, so individual field access is safe.
    local snapshots = raw :: {any}
    for i, snapshot in ipairs(snapshots) do
      out[i] = { id = snapshot.id, transform = snapshot.transform, tag = snapshot.tag }
    end
    return out
  end,
}

return world
