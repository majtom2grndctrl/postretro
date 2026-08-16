--!strict
-- Passive manifest templates for world-anchored transient presentation. The
-- renderer owns their layout; this builder only validates/returns plain data.

export type WidgetEasing = "linear" | "easeIn" | "easeOut" | "easeInOut"
export type PresentationTemplateProps = {
  root: { [string]: any },
  lifetimeMs: number,
  motion: { rise: number, easing: WidgetEasing },
  fade: { startMs: number },
  spawnScatter: { radius: number },
  worldAnchor: { socket: string, offsetY: number }?,
}
export type PresentationTemplate = {
  id: string,
  root: { [string]: any },
  lifetimeMs: number,
  motion: { rise: number, easing: WidgetEasing },
  fade: { startMs: number },
  spawnScatter: { radius: number },
  worldAnchor: { socket: string, offsetY: number }?,
}

export type OverlayEntity = {
  state: (self: OverlayEntity, name: string) -> RuntimeValue,
}
export type DamagedEnemiesProps = {
  lingerMs: number,
  hideAtFull: boolean,
  shield: {
    value: (entity: OverlayEntity) -> RuntimeValue,
    max: (entity: OverlayEntity) -> RuntimeValue,
  }?,
}
export type DamagedEnemiesSource = {
  kind: "damagedEnemies",
  lingerMs: number,
  hideAtFull: boolean,
  shield: { value: RuntimeValue, max: RuntimeValue }?,
}
export type PresentationOverlay = {
  over: DamagedEnemiesSource,
  template: string,
  maxVisible: number,
}

local PresentationSdk = {}

local function finite(value: any): boolean
  return type(value) == "number" and value == value and value ~= math.huge and value ~= -math.huge
end

local function validate(props: any)
  if type(props) ~= "table" then error("definePresentationTemplate: props must be a table", 3) end
  if type(props.root) ~= "table" or type(props.root.kind) ~= "string" then
    error("definePresentationTemplate: `root` must be a kind-tagged widget descriptor", 3)
  end
  if not finite(props.lifetimeMs) or props.lifetimeMs < 0 or props.lifetimeMs % 1 ~= 0 then
    error("definePresentationTemplate: `lifetimeMs` must be a non-negative integer", 3)
  end
  if type(props.motion) ~= "table" or not finite(props.motion.rise) then
    error("definePresentationTemplate: `motion.rise` must be finite", 3)
  end
  if props.motion.easing ~= "linear" and props.motion.easing ~= "easeIn" and props.motion.easing ~= "easeOut" and props.motion.easing ~= "easeInOut" then
    error("definePresentationTemplate: `motion.easing` is invalid", 3)
  end
  if type(props.fade) ~= "table" or not finite(props.fade.startMs) or props.fade.startMs < 0 or props.fade.startMs % 1 ~= 0 or props.fade.startMs > props.lifetimeMs then
    error("definePresentationTemplate: `fade.startMs` must be an integer within the lifetime", 3)
  end
  if type(props.spawnScatter) ~= "table" or not finite(props.spawnScatter.radius) or props.spawnScatter.radius < 0 then
    error("definePresentationTemplate: `spawnScatter.radius` must be non-negative", 3)
  end
  if props.worldAnchor ~= nil then
    if type(props.worldAnchor) ~= "table" or type(props.worldAnchor.socket) ~= "string" or #props.worldAnchor.socket == 0 or not finite(props.worldAnchor.offsetY) then
      error("definePresentationTemplate: `worldAnchor` requires nonempty socket and finite offsetY", 3)
    end
  end
end

--- Luau has no compiler binding sugar, so it uses the explicit stable id form.
function PresentationSdk.definePresentationTemplate(id: string, props: PresentationTemplateProps): PresentationTemplate
  if type(id) ~= "string" or #id == 0 then
    error("definePresentationTemplate: `id` must be nonempty", 2)
  end
  validate(props)
  local output: any = {
    id = id,
    root = props.root,
    lifetimeMs = props.lifetimeMs,
    motion = table.freeze({ rise = props.motion.rise, easing = props.motion.easing }),
    fade = table.freeze({ startMs = props.fade.startMs }),
    spawnScatter = table.freeze({ radius = props.spawnScatter.radius }),
  }
  if props.worldAnchor ~= nil then
    output.worldAnchor = table.freeze({ socket = props.worldAnchor.socket, offsetY = props.worldAnchor.offsetY })
  end
  return table.freeze(output)
end

local OverlayEntityImpl: OverlayEntity = table.freeze({
  state = function(_self: OverlayEntity, name: string): RuntimeValue
    if type(name) ~= "string" or #name == 0 then
      error("damagedEnemies: state name must be nonempty", 2)
    end
    return { op = "input", name = "@state." .. name }
  end,
})

function PresentationSdk.damagedEnemies(props: DamagedEnemiesProps): DamagedEnemiesSource
  if type(props) ~= "table" then error("damagedEnemies: props must be a table", 2) end
  if not finite(props.lingerMs) or props.lingerMs < 0 or props.lingerMs % 1 ~= 0 then
    error("damagedEnemies: `lingerMs` must be a non-negative integer", 2)
  end
  if type(props.hideAtFull) ~= "boolean" then
    error("damagedEnemies: `hideAtFull` must be a boolean", 2)
  end
  local output: any = { kind = "damagedEnemies", lingerMs = props.lingerMs, hideAtFull = props.hideAtFull }
  if props.shield ~= nil then
    if type(props.shield) ~= "table" or type(props.shield.value) ~= "function" or type(props.shield.max) ~= "function" then
      error("damagedEnemies: `shield` must provide value and max expressions", 2)
    end
    output.shield = table.freeze({
      value = props.shield.value(OverlayEntityImpl),
      max = props.shield.max(OverlayEntityImpl),
    })
  end
  return table.freeze(output)
end

function PresentationSdk.defineOverlay(props: { over: DamagedEnemiesSource, template: PresentationTemplate, maxVisible: number }): PresentationOverlay
  if type(props) ~= "table" then error("defineOverlay: props must be a table", 2) end
  if type(props.over) ~= "table" or props.over.kind ~= "damagedEnemies" then
    error("defineOverlay: `over` must come from damagedEnemies", 2)
  end
  if type(props.template) ~= "table" or type(props.template.id) ~= "string" then
    error("defineOverlay: `template` must come from definePresentationTemplate", 2)
  end
  if props.template.worldAnchor == nil then
    error("defineOverlay: `template.worldAnchor` is required for an overlay", 2)
  end
  if not finite(props.maxVisible) or props.maxVisible < 1 or props.maxVisible % 1 ~= 0 then
    error("defineOverlay: `maxVisible` must be a positive integer", 2)
  end
  return table.freeze({ over = props.over, template = props.template.id, maxVisible = props.maxVisible })
end

return table.freeze(PresentationSdk)
