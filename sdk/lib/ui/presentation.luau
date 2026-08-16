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
export type NumberFactOptions = { format: string?, tween: { durationMs: number, easing: WidgetEasing, from: number? }? }
export type ScalarFactOptions = { format: string? }
export type FactBindRef = { fact: string, format: string?, tween: { durationMs: number, easing: WidgetEasing, from: number? }? }

local PresentationSdk = {}
local MAX_OVERLAY_VISIBLE = 64
local MAX_U32 = 4294967295
local MAX_F32 = 3.4028234663852886e38

local function finite(value: any): boolean
  return type(value) == "number" and value == value and value ~= math.huge and value ~= -math.huge
end

local function storedF32(value: any): boolean
  return finite(value) and math.abs(value) <= MAX_F32
end

local function requireStoredF32(value: any, field: string, factory: string)
  if not storedF32(value) then
    error(`{factory}: \`{field}\` must be a finite number representable as f32`, 3)
  end
end

local function requireStoredU32(value: any, field: string, factory: string)
  if not finite(value) or value < 0 or value > MAX_U32 or value % 1 ~= 0 then
    error(`{factory}: \`{field}\` must be an integer in [0, {MAX_U32}]`, 3)
  end
end

local function factRef(name: string, options: any?): FactBindRef
  if type(name) ~= "string" or #name == 0 then
    error("fact: name must be nonempty", 3)
  end
  if options ~= nil and type(options) ~= "table" then
    error("fact: options must be a table", 3)
  end
  local source = options or {}
  if source.format ~= nil and type(source.format) ~= "string" then
    error("fact: `format` must be a string", 3)
  end
  if source.tween ~= nil then
    local tween = source.tween
    if type(tween) ~= "table" then
      error("fact: `tween` must be a table", 3)
    end
    requireStoredF32(tween.durationMs, "tween.durationMs", "fact")
    if tween.durationMs < 0 then error("fact: `tween.durationMs` must be non-negative", 3) end
    if tween.easing ~= "linear" and tween.easing ~= "easeIn" and tween.easing ~= "easeOut" and tween.easing ~= "easeInOut" then
      error("fact: `tween.easing` is invalid", 3)
    end
    if tween.from ~= nil then requireStoredF32(tween.from, "tween.from", "fact") end
  end
  local out: any = { fact = name }
  if source.format ~= nil then out.format = source.format end
  if source.tween ~= nil then
    local tween = source.tween
    local cloned: any = { durationMs = tween.durationMs, easing = tween.easing }
    if tween.from ~= nil then cloned.from = tween.from end
    out.tween = table.freeze(cloned)
  end
  return table.freeze(out)
end

PresentationSdk.fact = table.freeze({
  number = function(name: string, options: NumberFactOptions?): FactBindRef
    return factRef(name, options)
  end,
  text = function(name: string, options: ScalarFactOptions?): FactBindRef
    return factRef(name, options)
  end,
  bool = function(name: string, options: ScalarFactOptions?): FactBindRef
    return factRef(name, options)
  end,
})

local function validateWidgetF32(value: any, field: string, positive: boolean?)
  requireStoredF32(value, field, "definePresentationTemplate")
  if (positive and value <= 0) or (not positive and value < 0) then
    error(`definePresentationTemplate: \`{field}\` must be {if positive then "greater than zero" else "non-negative"}`, 3)
  end
end

local function validateColor(value: any, field: string)
  if type(value) ~= "table" then return end
  if #value ~= 4 then error(`definePresentationTemplate: \`{field}\` must contain four f32 components`, 3) end
  for index = 1, 4 do requireStoredF32(value[index], `{field}[{index}]`, "definePresentationTemplate") end
end

local function validateBorder(value: any, field: string)
  if value == nil then return end
  if type(value) ~= "table" then error(`definePresentationTemplate: \`{field}\` must be a table`, 3) end
  if type(value.texture) ~= "string" or #value.texture == 0 then
    error(`definePresentationTemplate: \`{field}.texture\` must be nonempty`, 3)
  end
  if type(value.slice) ~= "table" or #value.slice ~= 4 then
    error(`definePresentationTemplate: \`{field}.slice\` must contain four f32 dimensions`, 3)
  end
  for index = 1, 4 do validateWidgetF32(value.slice[index], `{field}.slice[{index}]`) end
  validateColor(value.tint, `{field}.tint`)
end

local function validateTween(value: any, field: string)
  if value == nil then return end
  if type(value) ~= "table" then error(`definePresentationTemplate: \`{field}\` must be a table`, 3) end
  validateWidgetF32(value.durationMs, `{field}.durationMs`)
  if value.from ~= nil then requireStoredF32(value.from, `{field}.from`, "definePresentationTemplate") end
end

local function validateStyleRanges(value: any, field: string)
  if value == nil then return end
  if type(value) ~= "table" then error(`definePresentationTemplate: \`{field}\` must be a table`, 3) end
  validateWidgetF32(value.max, `{field}.max`, true)
  if type(value.entries) ~= "table" then error(`definePresentationTemplate: \`{field}.entries\` must be an array`, 3) end
  for index, entry in ipairs(value.entries) do
    if type(entry) ~= "table" then error(`definePresentationTemplate: \`{field}.entries[{index}]\` must be a table`, 3) end
    if entry.upTo ~= nil then requireStoredF32(entry.upTo, `{field}.entries[{index}].upTo`, "definePresentationTemplate") end
    if entry.color ~= nil then validateColor(entry.color, `{field}.entries[{index}].color`) end
    if entry.pulse ~= nil then validateWidgetF32(entry.pulse.periodMs, `{field}.entries[{index}].pulse.periodMs`, true) end
    if entry.flash ~= nil then validateWidgetF32(entry.flash.durationMs, `{field}.entries[{index}].flash.durationMs`) end
  end
end

local function validatePassiveWidget(value: any, path: string)
  if type(value) ~= "table" or type(value.kind) ~= "string" then
    error(`definePresentationTemplate: \`{path}\` must be a kind-tagged widget descriptor`, 3)
  end
  local kind = value.kind
  if kind ~= "text" and kind ~= "bar" and kind ~= "image" and kind ~= "vstack" and kind ~= "hstack" then
    error(`definePresentationTemplate: \`{path}.kind\` "{kind}" is not supported; passive templates allow text, bar, image, vstack, and hstack`, 3)
  end
  if kind == "text" then
    validateWidgetF32(value.fontSize, `{path}.fontSize`, true)
    validateColor(value.color, `{path}.color`)
  elseif kind == "bar" then
    if type(value.max) == "number" then validateWidgetF32(value.max, `{path}.max`, true) end
    if value.width ~= nil then validateWidgetF32(value.width, `{path}.width`, true) end
    if value.height ~= nil then validateWidgetF32(value.height, `{path}.height`, true) end
    validateColor(value.fill, `{path}.fill`)
    validateColor(value.background, `{path}.background`)
    if value.exitFade ~= nil then
      if type(value.exitFade) ~= "table" then
        error(`definePresentationTemplate: \`{path}.exitFade\` must be a table`, 3)
      end
      validateWidgetF32(value.exitFade.durationMs, `{path}.exitFade.durationMs`, true)
    end
  elseif kind == "vstack" or kind == "hstack" then
    if type(value.gap) == "number" then validateWidgetF32(value.gap, `{path}.gap`) end
    if type(value.padding) == "number" then validateWidgetF32(value.padding, `{path}.padding`) end
    if value.fill ~= nil then validateColor(value.fill, `{path}.fill`) end
    validateBorder(value.border, `{path}.border`)
    if type(value.children) ~= "table" then error(`definePresentationTemplate: \`{path}.children\` must be an array`, 3) end
    for index, child in ipairs(value.children) do validatePassiveWidget(child, `{path}.children[{index}]`) end
  end
  if value.bind ~= nil then validateTween(value.bind.tween, `{path}.bind.tween`) end
  validateStyleRanges(value.styleRanges, `{path}.styleRanges`)
end

local function validate(props: any)
  if type(props) ~= "table" then error("definePresentationTemplate: props must be a table", 3) end
  if type(props.root) ~= "table" or type(props.root.kind) ~= "string" then
    error("definePresentationTemplate: `root` must be a kind-tagged widget descriptor", 3)
  end
  validatePassiveWidget(props.root, "root")
  requireStoredU32(props.lifetimeMs, "lifetimeMs", "definePresentationTemplate")
  if type(props.motion) ~= "table" then error("definePresentationTemplate: `motion` must be a table", 3) end
  requireStoredF32(props.motion.rise, "motion.rise", "definePresentationTemplate")
  if props.motion.easing ~= "linear" and props.motion.easing ~= "easeIn" and props.motion.easing ~= "easeOut" and props.motion.easing ~= "easeInOut" then
    error("definePresentationTemplate: `motion.easing` is invalid", 3)
  end
  if type(props.fade) ~= "table" then error("definePresentationTemplate: `fade` must be a table", 3) end
  requireStoredU32(props.fade.startMs, "fade.startMs", "definePresentationTemplate")
  if props.fade.startMs > props.lifetimeMs then error("definePresentationTemplate: `fade.startMs` must not exceed `lifetimeMs`", 3) end
  if type(props.spawnScatter) ~= "table" then error("definePresentationTemplate: `spawnScatter` must be a table", 3) end
  validateWidgetF32(props.spawnScatter.radius, "spawnScatter.radius")
  if props.worldAnchor ~= nil then
    if type(props.worldAnchor) ~= "table" or type(props.worldAnchor.socket) ~= "string" or #props.worldAnchor.socket == 0 then
      error("definePresentationTemplate: `worldAnchor.socket` must be nonempty", 3)
    end
    requireStoredF32(props.worldAnchor.offsetY, "worldAnchor.offsetY", "definePresentationTemplate")
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
  requireStoredU32(props.lingerMs, "lingerMs", "damagedEnemies")
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
  if not finite(props.maxVisible) or props.maxVisible < 1 or props.maxVisible > MAX_OVERLAY_VISIBLE or props.maxVisible % 1 ~= 0 then
    error("defineOverlay: `maxVisible` must be an integer between 1 and 64", 2)
  end
  return table.freeze({ over = props.over, template = props.template.id, maxVisible = props.maxVisible })
end

return table.freeze(PresentationSdk)
