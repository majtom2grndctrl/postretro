--!strict
-- Fog volume vocabulary: the `FogVolumeHandle` returned by
-- `world:query({ component = "fog_volume" })`, plus the `pulseDensity`
-- animation constructor. Mirrors `entities/lights.luau` shape; the
-- handle wraps the generated `FogVolumeEntity` snapshot with mutating
-- methods that call `setComponent`. Density and color tweens are
-- driven from per-handle `tick` callbacks registered with
-- `registerHandler` -- there is no engine-side fog animation primitive,
-- unlike lights.
--
-- ---------------------------------------------------------------------
-- Canonical modder example -- pulse a tagged fog cloud.
--
-- ```lua
-- registerHandler("levelLoad", function()
--   local clouds = world:query({ component = "fog_volume", tag = "neon_haze" })
--   for _, cloud in ipairs(clouds) do
--     pulseDensity(cloud, { min = 0.2, max = 0.6, period = 4000 })
--   end
-- end)
-- ```
-- ---------------------------------------------------------------------

-- `EntityId`, `FogVolumeComponent`, `FogVolumeEntity`,
-- `ScriptCallContext`, `getComponent`, `setComponent`, and
-- `registerHandler` are provided ambiently by the generated
-- `sdk/types/postretro.d.luau` definitions file.

--- Typed handle returned by `world:query` for a fog-volume entity.
--- Carries the query-time snapshot fields plus four mutating methods.
--- Methods are called with `:` (e.g. `cloud:setDensity(0.4, 1000)`).
export type FogVolumeHandle = {
  id: EntityId,
  position: Vec3,
  tags: {string},
  component: FogVolumeComponent,

  --- Set fog density. Instant when `durationMs` is 0 or omitted;
  --- otherwise lerped each tick from the live density value over
  --- `durationMs` milliseconds. Last call wins -- the previous tween
  --- (if any) is cancelled.
  setDensity: (self: FogVolumeHandle, density: number, durationMs: number?) -> (),

  --- Set fog color. Instant when `durationMs` is 0 or omitted;
  --- otherwise lerped each tick from the live color over `durationMs`
  --- milliseconds. Last call wins.
  setColor: (self: FogVolumeHandle, color: {number}, durationMs: number?) -> (),

  --- Set the scatter fraction. Instant.
  setScatter: (self: FogVolumeHandle, scatter: number) -> (),

  --- Set the edge falloff. Instant.
  setFalloff: (self: FogVolumeHandle, falloff: number) -> (),
}

--- Controller returned by `pulseDensity` to cancel the running animation.
export type AnimationController = {
  --- Stop the animation. Idempotent -- subsequent calls are no-ops.
  stop: (self: AnimationController) -> (),
}

local FogVolumesSdk = {}

local function idDebug(id: EntityId): string
  return tostring(id)
end

local function readFogVolumeComponent(id: EntityId): FogVolumeComponent
  local c = getComponent(id, "fog_volume")
  if c.kind ~= "fog_volume" then
    error(`expected FogVolume component on entity {idDebug(id)}, got {c.kind}`)
  end
  -- Flat ComponentValue: kind + FogVolumeComponent fields are siblings.
  return (c :: any) :: FogVolumeComponent
end

-- A cancelable tick subscription. `registerHandler` itself has no
-- unregister primitive, so we wrap the user callback in a closure that
-- checks a `cancelled` flag on each tick -- the handler stays installed
-- for the life of the level but becomes a no-op once stopped.
local function tickSubscription(fn: (ScriptCallContext) -> ()): AnimationController
  local cancelled = false
  registerHandler("tick", function(ctx: ScriptCallContext?)
    if cancelled then return end
    if ctx == nil then return end
    fn(ctx :: ScriptCallContext)
  end)
  local ctrl: AnimationController
  ctrl = {
    stop = function(self: AnimationController)
      cancelled = true
    end,
  }
  return ctrl
end

local function writeFogVolume(
  id: EntityId,
  density: number,
  color: {number},
  scatter: number,
  falloff: number
)
  setComponent(id, "fog_volume", {
    kind = "fog_volume",
    density = density,
    color = { color[1], color[2], color[3] },
    scatter = scatter,
    falloff = falloff,
  } :: any)
end

-- Per-handle slots holding the active density / color tween (if any).
-- A new tween cancels any in-flight tween on the same field; an
-- "instant" setter also cancels its respective tween before writing.
type Slots = {
  density: AnimationController?,
  color: AnimationController?,
}

local function cancelTween(slots: Slots, field: "density" | "color")
  local existing = slots[field]
  if existing ~= nil then
    existing:stop()
    slots[field] = nil
  end
end

local function startDensityTween(
  id: EntityId,
  slots: Slots,
  target: number,
  durationMs: number
)
  cancelTween(slots, "density")
  local startDensity = readFogVolumeComponent(id).density
  local elapsedMs = 0.0
  local ctrl: AnimationController
  ctrl = tickSubscription(function(ctx: ScriptCallContext)
    elapsedMs += ctx.delta * 1000
    local t = math.min(1.0, elapsedMs / durationMs)
    local value = startDensity + (target - startDensity) * t
    -- Re-read live for the other fields so a concurrent color tween or
    -- scripted setScatter/setFalloff is not clobbered.
    local live = readFogVolumeComponent(id)
    writeFogVolume(id, value, (live.color :: any) :: {number}, live.scatter, live.falloff)
    if t >= 1.0 then
      ctrl:stop()
      slots.density = nil
    end
  end)
  slots.density = ctrl
end

local function startColorTween(
  id: EntityId,
  slots: Slots,
  target: {number},
  durationMs: number
)
  cancelTween(slots, "color")
  local liveStart = readFogVolumeComponent(id)
  local startColor = (liveStart.color :: any) :: {number}
  local from = { startColor[1], startColor[2], startColor[3] }
  local elapsedMs = 0.0
  local ctrl: AnimationController
  ctrl = tickSubscription(function(ctx: ScriptCallContext)
    elapsedMs += ctx.delta * 1000
    local t = math.min(1.0, elapsedMs / durationMs)
    local value = {
      from[1] + (target[1] - from[1]) * t,
      from[2] + (target[2] - from[2]) * t,
      from[3] + (target[3] - from[3]) * t,
    }
    local live = readFogVolumeComponent(id)
    writeFogVolume(id, live.density, value, live.scatter, live.falloff)
    if t >= 1.0 then
      ctrl:stop()
      slots.color = nil
    end
  end)
  slots.color = ctrl
end

--- Wrap a `FogVolumeEntity` snapshot returned by `worldQuery` as a
--- mutating handle. Used by `world.luau` for `world:query({ component
--- = "fog_volume" })`. The returned table preserves all snapshot fields
--- and adds the four mutating methods.
function FogVolumesSdk.wrapFogVolumeEntity(snapshot: FogVolumeEntity): FogVolumeHandle
  local id: EntityId = snapshot.id
  local slots: Slots = { density = nil, color = nil }

  local handle: FogVolumeHandle = {
    id = id,
    position = snapshot.position,
    tags = snapshot.tags,
    component = snapshot.component,

    setDensity = function(self: FogVolumeHandle, density: number, durationMs: number?)
      local ms = durationMs or 0
      if ms <= 0 then
        cancelTween(slots, "density")
        local live = readFogVolumeComponent(id)
        writeFogVolume(id, density, (live.color :: any) :: {number}, live.scatter, live.falloff)
        return
      end
      startDensityTween(id, slots, density, ms)
    end,

    setColor = function(self: FogVolumeHandle, color: {number}, durationMs: number?)
      local ms = durationMs or 0
      if ms <= 0 then
        cancelTween(slots, "color")
        local live = readFogVolumeComponent(id)
        writeFogVolume(id, live.density, color, live.scatter, live.falloff)
        return
      end
      startColorTween(id, slots, color, ms)
    end,

    setScatter = function(self: FogVolumeHandle, scatter: number)
      local live = readFogVolumeComponent(id)
      writeFogVolume(id, live.density, (live.color :: any) :: {number}, scatter, live.falloff)
    end,

    setFalloff = function(self: FogVolumeHandle, falloff: number)
      local live = readFogVolumeComponent(id)
      writeFogVolume(id, live.density, (live.color :: any) :: {number}, live.scatter, falloff)
    end,
  }

  return handle
end

--- Oscillates `handle.density` sinusoidally between `min` and `max`
--- with the given `period` (milliseconds). Returns a controller whose
--- `:stop()` cancels the animation. Multiple `pulseDensity` calls on
--- the same handle stack -- call `:stop()` on the previous controller
--- before starting a new one if you do not want overlap.
function FogVolumesSdk.pulseDensity(
  handle: FogVolumeHandle,
  opts: { min: number, max: number, period: number }
): AnimationController
  local minV = opts.min
  local maxV = opts.max
  local period = opts.period
  if not (period > 0) then
    error("pulseDensity: `period` must be a positive number")
  end
  local lo = math.min(minV, maxV)
  local hi = math.max(minV, maxV)
  local mid = (lo + hi) * 0.5
  local amp = (hi - lo) * 0.5
  local id: EntityId = handle.id
  local elapsedMs = 0.0
  return tickSubscription(function(ctx: ScriptCallContext)
    elapsedMs += ctx.delta * 1000
    local phase = (elapsedMs % period) / period
    local value = mid + amp * math.sin(phase * math.pi * 2)
    local live = readFogVolumeComponent(id)
    writeFogVolume(id, value, (live.color :: any) :: {number}, live.scatter, live.falloff)
  end)
end

return FogVolumesSdk
