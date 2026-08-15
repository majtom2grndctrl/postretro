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
}
export type PresentationTemplate = {
  id: string,
  root: { [string]: any },
  lifetimeMs: number,
  motion: { rise: number, easing: WidgetEasing },
  fade: { startMs: number },
  spawnScatter: { radius: number },
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
end

--- Luau has no compiler binding sugar, so it uses the explicit stable id form.
function PresentationSdk.definePresentationTemplate(id: string, props: PresentationTemplateProps): PresentationTemplate
  if type(id) ~= "string" or #id == 0 then
    error("definePresentationTemplate: `id` must be nonempty", 2)
  end
  validate(props)
  return table.freeze({
    id = id,
    root = props.root,
    lifetimeMs = props.lifetimeMs,
    motion = table.freeze({ rise = props.motion.rise, easing = props.motion.easing }),
    fade = table.freeze({ startMs = props.fade.startMs }),
    spawnScatter = table.freeze({ radius = props.spawnScatter.radius }),
  })
end

return table.freeze(PresentationSdk)
