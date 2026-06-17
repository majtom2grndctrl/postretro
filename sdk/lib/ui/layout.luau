--!strict
-- UI layout factories (M13 G1a, Task 3): capitalized constructors for the three
-- container widget kinds -- VStack, HStack, Grid. Compose/SwiftUI lineage: the
-- props table comes first and `children` is a POSITIONAL second argument, not a
-- prop. Each mirrors `emitter()`: synchronous field-named validation, documented
-- defaults, a plain descriptor table whose keys are the camelCase wire form of
-- the matching `render/ui/descriptor.rs` container variant. Exported as members
-- of a returned module table (the `UiReactionsSdk` precedent).
--
-- Containers take NO state bind. They may carry a backdrop `fill`/`border`
-- (stacks only), a `focus` policy, and focus fields. Self-contained: the Luau
-- prelude has no module loader, so the validation helpers are redeclared here
-- rather than required from `widgets.luau`.
-- See: context/lib/ui.md · context/lib/scripting.md §7

export type ColorToken = { __postretroToken: "color", token: string }
export type SpacingToken = { __postretroToken: "spacing", token: string }

--- A widget color slot: an inline `{r, g, b, a}` tuple (0-1) or a color token.
export type WidgetColor = { number } | ColorToken

--- A spacing slot (gap/padding): an inline logical-px number or a spacing token.
export type WidgetSpacing = number | SpacingToken

export type Predicate = { slot: string?, ["local"]: string?, equals: any? }
export type WidgetRole = "tab" | "tablist" | "checkbox" | "radio" | "listitem" | "button" | "slider" | "progressbar" | "image" | "group" | "none"

--- A flat descriptor table a container factory produces.
export type WidgetDescriptor = { [string]: any }

local UiLayoutSdk = {}
local unwrapThemeToken = rawget(_G, "__postretroUnwrapThemeToken")
if type(unwrapThemeToken) ~= "function" then
  error("layout.luau: missing internal theme-token validator")
end
local unwrapThemeToken = unwrapThemeToken :: (any, string) -> string?

-- --- Validation helpers -----------------------------------------------------

local function requireObject(props: any, factory: string)
  if type(props) ~= "table" then
    error(`{factory}: props must be a table`)
  end
end

local function requireFiniteNumber(value: any, field: string, factory: string)
  if type(value) ~= "number" or value ~= value or value == math.huge or value == -math.huge then
    error(`{factory}: \`{field}\` must be a finite number`)
  end
end

local function requireNonemptyString(value: any, field: string, factory: string)
  if type(value) ~= "string" or #value == 0 then
    error(`{factory}: \`{field}\` must be a nonempty string`)
  end
end

local function requireString(value: any, field: string, factory: string)
  if type(value) ~= "string" then
    error(`{factory}: \`{field}\` must be a string`)
  end
end

local function unwrapToken(value: any, category: string, field: string, factory: string): string?
  if type(value) ~= "table" or value.__postretroToken == nil then
    return nil
  end
  local token = unwrapThemeToken(value, category)
  if token == nil then
    error(`{factory}: \`{field}\` must be a {category} token returned by getDesignTokens(theme)`)
  end
  return token
end

local function validateSpacing(value: any, field: string, factory: string): any
  local token = unwrapToken(value, "spacing", field, factory)
  if token ~= nil then
    return token
  end
  if type(value) == "string" then
    error(`{factory}: \`{field}\` must be a spacing token record, not a raw string`)
  end
  if type(value) == "table" and value.__postretroToken ~= nil then
    error(`{factory}: \`{field}\` must be a spacing token returned by getDesignTokens(theme)`)
  end
  requireFiniteNumber(value, field, factory)
  return value
end

local function validateAlign(value: any, factory: string): string
  if value ~= "start" and value ~= "center" and value ~= "end" and value ~= "stretch" then
    error(`{factory}: \`align\` must be one of "start" | "center" | "end" | "stretch"`)
  end
  return value
end

local function validateColor(value: any, field: string, factory: string): any
  local token = unwrapToken(value, "color", field, factory)
  if token ~= nil then
    return token
  end
  if type(value) == "string" then
    error(`{factory}: \`{field}\` must be a color token record, not a raw string`)
  end
  if type(value) == "table" and value.__postretroToken ~= nil then
    error(`{factory}: \`{field}\` must be a color token returned by getDesignTokens(theme)`)
  end
  if type(value) ~= "table" or #value ~= 4 then
    error(`{factory}: \`{field}\` must be a \{r, g, b, a\} tuple or a color token`)
  end
  for i = 1, 4 do
    requireFiniteNumber(value[i], `{field}[{i}]`, factory)
  end
  return value
end

-- Validate a 9-slice border prop (redeclared from `widgets.luau`: no cross-module
-- require in the Luau prelude). Returns the wire table.
local function validateBorder(value: any, factory: string): any
  if type(value) ~= "table" then
    error(`{factory}: \`border\` must be a table`)
  end
  requireString(value.texture, "border.texture", factory)
  if type(value.slice) ~= "table" or #value.slice ~= 4 then
    error(`{factory}: \`border.slice\` must be a \{left, top, right, bottom\} tuple`)
  end
  for i = 1, 4 do
    requireFiniteNumber(value.slice[i], `border.slice[{i}]`, factory)
  end
  return { texture = value.texture, slice = value.slice, tint = validateColor(value.tint, "border.tint", factory) }
end

local function buildFocusNeighbors(value: any, factory: string): any?
  if value == nil then
    return nil
  end
  if type(value) ~= "table" then
    error(`{factory}: \`focusNeighbors\` must be a table`)
  end
  local out: { [string]: any } = {}
  local any = false
  for _, dir in ipairs({ "up", "down", "left", "right" }) do
    if value[dir] ~= nil then
      requireNonemptyString(value[dir], `focusNeighbors.{dir}`, factory)
      out[dir] = value[dir]
      any = true
    end
  end
  if not any then
    return nil
  end
  return out
end

local function buildFocusPolicy(value: any, factory: string): any?
  if value == nil then
    return nil
  end
  if value == "linear" or value == "spatial" then
    return value
  end
  if type(value) ~= "table" then
    error(`{factory}: \`focus\` must be "linear" | "spatial" or a \{ policy, wrap?, repeat? \} table`)
  end
  if value.policy ~= "linear" and value.policy ~= "spatial" then
    error(`{factory}: \`focus.policy\` must be "linear" | "spatial"`)
  end
  local out: { [string]: any } = { policy = value.policy }
  -- `wrap` skip-serializes only when true (its default), so only an authored
  -- `false` is emitted -- matching the Rust `skip_serializing_if = "is_true"`.
  if value.wrap ~= nil then
    if type(value.wrap) ~= "boolean" then
      error(`{factory}: \`focus.wrap\` must be a boolean`)
    end
    if value.wrap == false then
      out.wrap = false
    end
  end
  -- `repeat` is a Lua keyword, so the field is read/written via bracket access.
  local repeatPolicy = value["repeat"]
  if repeatPolicy ~= nil then
    if type(repeatPolicy) ~= "table" then
      error(`{factory}: \`focus.repeat\` must be a table`)
    end
    requireFiniteNumber(repeatPolicy.initialDelayMs, "focus.repeat.initialDelayMs", factory)
    requireFiniteNumber(repeatPolicy.intervalMs, "focus.repeat.intervalMs", factory)
    out["repeat"] = {
      initialDelayMs = repeatPolicy.initialDelayMs,
      intervalMs = repeatPolicy.intervalMs,
    }
  end
  return out
end

local ROLES: { [string]: boolean } = {
  tab = true,
  tablist = true,
  checkbox = true,
  radio = true,
  listitem = true,
  button = true,
  slider = true,
  progressbar = true,
  image = true,
  group = true,
  none = true,
}

local function applyRole(out: { [string]: any }, role: any, factory: string)
  if role == nil then
    return
  end
  if type(role) ~= "string" or not ROLES[role] then
    error(
      `{factory}: \`role\` must be one of "tab" | "tablist" | "checkbox" | "radio" | "listitem" | "button" | "slider" | "progressbar" | "image" | "group" | "none"`
    )
  end
  out.role = role
end

local function buildPredicate(value: any, field: string, factory: string): any?
  if value == nil then
    return nil
  end
  if type(value) ~= "table" then
    error(`{factory}: \`{field}\` must be a table`)
  end
  local out: { [string]: any }
  if value.slot ~= nil then
    requireNonemptyString(value.slot, `{field}.slot`, factory)
    out = { slot = value.slot }
  else
    requireNonemptyString(value["local"], `{field}.local`, factory)
    out = { ["local"] = value["local"] }
  end
  if value.equals ~= nil then
    local e = value.equals
    if type(e) ~= "number" and type(e) ~= "boolean" and type(e) ~= "string" then
      error(`{factory}: \`{field}.equals\` must be a number, boolean, or string`)
    end
    if type(e) == "number" and (e ~= e or e == math.huge or e == -math.huge) then
      error(`{factory}: \`{field}.equals\` number must be finite`)
    end
    out.equals = e
  end
  return out
end

local function validateLocalState(value: any, factory: string): any?
  if value == nil then
    return nil
  end
  if type(value) ~= "table" then
    error(`{factory}: \`localState\` must be a table`)
  end
  requireNonemptyString(value.scope, "localState.scope", factory)
  if type(value.cells) ~= "table" then
    error(`{factory}: \`localState.cells\` must be a table`)
  end
  for name, cell in pairs(value.cells) do
    if type(name) ~= "string" or #name == 0 then
      error(`{factory}: \`localState.cells\` keys must be nonempty strings`)
    end
    if type(cell) == "number" then
      requireFiniteNumber(cell, `localState.cells.{name}`, factory)
    elseif type(cell) ~= "boolean" and type(cell) ~= "string" then
      if type(cell) ~= "table" or #cell ~= 4 then
        error(`{factory}: \`localState.cells.{name}\` must be a number, boolean, string, or color tuple`)
      end
      for i = 1, 4 do
        requireFiniteNumber(cell[i], `localState.cells.{name}[{i}]`, factory)
      end
    end
  end
  return value
end

local function validateChildren(children: any, factory: string): { WidgetDescriptor }
  if type(children) ~= "table" then
    error(`{factory}: \`children\` must be an array of widget descriptors`)
  end
  for i, c in ipairs(children) do
    if type(c) ~= "table" or type(c.kind) ~= "string" then
      error(`{factory}: \`children[{i}]\` must be a widget descriptor (a \`kind\`-tagged table)`)
    end
  end
  return children
end

-- Append the shared container tail fields (`id`, `focusNeighbors`, `focus`,
-- `restoreOnReturn`, then `children`) in place. `children` is always present.
local function applyCommonTail(
  out: { [string]: any },
  props: any,
  children: { WidgetDescriptor },
  factory: string,
  localState: any?
)
  if props.id ~= nil then
    requireNonemptyString(props.id, "id", factory)
    out.id = props.id
  end
  local neighbors = buildFocusNeighbors(props.focusNeighbors, factory)
  if neighbors ~= nil then
    out.focusNeighbors = neighbors
  end
  local focus = buildFocusPolicy(props.focus, factory)
  if focus ~= nil then
    out.focus = focus
  end
  if props.restoreOnReturn ~= nil then
    if type(props.restoreOnReturn) ~= "boolean" then
      error(`{factory}: \`restoreOnReturn\` must be a boolean`)
    end
    -- Skip-serializes when false (the default) -- emit only true.
    if props.restoreOnReturn then
      out.restoreOnReturn = true
    end
  end
  if localState ~= nil then
    out.localState = localState
  end
  local visibleWhen = buildPredicate(props.visibleWhen, "visibleWhen", factory)
  if visibleWhen ~= nil then
    out.visibleWhen = visibleWhen
  end
  applyRole(out, props.role, factory)
  out.children = children
end

local function buildStack(kind: string, factory: string, props: any, children: any): WidgetDescriptor
  requireObject(props, factory)
  local gap = validateSpacing(if props.gap ~= nil then props.gap else 0.0, "gap", factory)
  local padding = validateSpacing(if props.padding ~= nil then props.padding else 0.0, "padding", factory)
  local align = validateAlign(if props.align ~= nil then props.align else "start", factory)
  local kids = validateChildren(children, factory)

  local out: { [string]: any } = { kind = kind, gap = gap, padding = padding, align = align }
  -- fill/border come before the common tail in the Rust struct order.
  if props.fill ~= nil then
    out.fill = validateColor(props.fill, "fill", factory)
  end
  if props.border ~= nil then
    out.border = validateBorder(props.border, factory)
  end
  applyCommonTail(out, props, kids, factory, validateLocalState(props.localState, factory))
  return out
end

-- --- Stacks -----------------------------------------------------------------

--- A vertical stack (`vstack`): lays `children` top-to-bottom with `gap` between
--- them, `padding` inside, cross-axis `align`. `gap`/`padding` default to 0,
--- `align` to `"start"`. `children` is a positional second argument. May carry a
--- backdrop `fill`/`border`. Mirrors `descriptor.rs` `ContainerWidget`.
function UiLayoutSdk.VStack(props: any?, children: any?): WidgetDescriptor
  return buildStack("vstack", "VStack", if props ~= nil then props else {}, if children ~= nil then children else {})
end

--- A horizontal stack (`hstack`): lays `children` left-to-right. Same props and
--- defaults as `VStack`. `children` is a positional second argument. Mirrors
--- `descriptor.rs` `ContainerWidget`.
function UiLayoutSdk.HStack(props: any?, children: any?): WidgetDescriptor
  return buildStack("hstack", "HStack", if props ~= nil then props else {}, if children ~= nil then children else {})
end

-- --- Grid -------------------------------------------------------------------

--- A `grid` container: flows `children` across `cols` columns with `gap`,
--- `padding`, and cross-axis `align`. `cols` is required (an integer >= 1). No
--- backdrop fill/border. `children` is a positional second argument. Mirrors
--- `descriptor.rs` `GridWidget`.
function UiLayoutSdk.Grid(props: any, children: any?): WidgetDescriptor
  requireObject(props, "Grid")
  local gap = validateSpacing(if props.gap ~= nil then props.gap else 0.0, "gap", "Grid")
  local padding = validateSpacing(if props.padding ~= nil then props.padding else 0.0, "padding", "Grid")
  local align = validateAlign(if props.align ~= nil then props.align else "start", "Grid")
  if
    type(props.cols) ~= "number"
    or props.cols ~= props.cols
    or props.cols < 1
    or math.floor(props.cols) ~= props.cols
  then
    error("Grid: `cols` must be an integer >= 1")
  end
  local kids = validateChildren(if children ~= nil then children else {}, "Grid")

  local out: { [string]: any } = { kind = "grid", gap = gap, padding = padding, align = align, cols = props.cols }
  applyCommonTail(out, props, kids, "Grid")
  return out
end

return UiLayoutSdk
