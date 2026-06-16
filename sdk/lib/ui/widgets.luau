--!strict
-- UI widget factories (M13 G1a, Task 3): capitalized constructors for the seven
-- non-container widget kinds -- Text, Panel, Image, Button, Slider, Bar, Spacer.
-- (Containers -- VStack/HStack/Grid -- live in `layout.luau`.) Each mirrors the
-- `emitter()` precedent: a props table validated synchronously (throwing a
-- field-named error), documented defaults, a plain descriptor table whose keys
-- are the camelCase wire form of the matching `render/ui/descriptor.rs` `Widget`
-- variant. Exported as members of a returned module table (the `UiReactionsSdk`
-- / `reactions.luau` precedent) so a future loader installs them as globals.
--
-- Pure builders: constructing a widget has no engine side effect. Bound props
-- accept state-reference descriptors (`{ slot = "..." }`) or presentation-local
-- bind tables; `Button.onPress` accepts a reaction handle (a table with `.name`)
-- or a bare name string. Self-contained: the Luau prelude has no module loader, so the
-- validation helpers and shared types are redeclared here (not required across).
-- See: context/lib/ui.md · context/lib/scripting.md §7

--- The type of every user-facing text string (Task 2 alias; redeclared here as
--- the prelude has no cross-module require). `= string` today.
export type LocalizedText = string

--- A widget color slot: an inline `{r, g, b, a}` tuple (0-1) or a theme token.
export type WidgetColor = { number } | string

--- A spacing slot (gap/padding): an inline logical-px number or a theme token.
export type WidgetSpacing = number | string

--- A flat descriptor table a widget factory produces.
export type WidgetDescriptor = { [string]: any }

local UiWidgetsSdk = {}

-- --- Validation helpers -----------------------------------------------------

local function requireObject(props: any, factory: string)
  if type(props) ~= "table" then
    error(`{factory}: props must be a table`)
  end
end

local function requireString(value: any, field: string, factory: string)
  if type(value) ~= "string" then
    error(`{factory}: \`{field}\` must be a string`)
  end
end

local function requireNonemptyString(value: any, field: string, factory: string)
  if type(value) ~= "string" or #value == 0 then
    error(`{factory}: \`{field}\` must be a nonempty string`)
  end
end

local function requireFiniteNumber(value: any, field: string, factory: string)
  if type(value) ~= "number" or value ~= value or value == math.huge or value == -math.huge then
    error(`{factory}: \`{field}\` must be a finite number`)
  end
end

local function requireColor(value: any, field: string, factory: string)
  if type(value) == "string" then
    if #value == 0 then
      error(`{factory}: \`{field}\` color token must be a nonempty string`)
    end
    return
  end
  if type(value) ~= "table" or #value ~= 4 then
    error(`{factory}: \`{field}\` must be a \{r, g, b, a\} tuple or a theme-token string`)
  end
  for i = 1, 4 do
    requireFiniteNumber(value[i], `{field}[{i}]`, factory)
  end
end

local function requireSpacing(value: any, field: string, factory: string)
  if type(value) == "string" then
    if #value == 0 then
      error(`{factory}: \`{field}\` spacing token must be a nonempty string`)
    end
    return
  end
  requireFiniteNumber(value, field, factory)
end

local function validateEasing(value: any, field: string, factory: string)
  if value ~= "linear" and value ~= "easeIn" and value ~= "easeOut" and value ~= "easeInOut" then
    error(`{factory}: \`{field}\` must be one of "linear" | "easeIn" | "easeOut" | "easeInOut"`)
  end
end

-- Resolve a bind prop to its wire `{ slot, ... }` or `{ ["local"], ... }` table.
-- The bind source is either a `{ slot }` store binding or a `{ local }`
-- presentation-cell binding. `slot` wins when present; else `bind["local"]` is
-- required. `kind` selects the optional members: text accepts `format`; panel's
-- tween `from` is a color, others a number. Returns nil when no bind was
-- authored so the factory omits the key.
local function buildBind(bind: any, factory: string, kind: string): any?
  if bind == nil then
    return nil
  end
  if type(bind) ~= "table" then
    error(`{factory}: \`bind\` must be a table`)
  end
  local out: { [string]: any }
  if bind.slot ~= nil then
    requireNonemptyString(bind.slot, "bind.slot", factory)
    out = { slot = bind.slot }
  else
    requireNonemptyString(bind["local"], "bind.local", factory)
    out = { ["local"] = bind["local"] }
  end

  if kind == "text" and bind.format ~= nil then
    requireString(bind.format, "bind.format", factory)
    out.format = bind.format
  end

  if bind.tween ~= nil then
    if type(bind.tween) ~= "table" then
      error(`{factory}: \`bind.tween\` must be a table`)
    end
    local tw = bind.tween
    requireFiniteNumber(tw.durationMs, "bind.tween.durationMs", factory)
    validateEasing(tw.easing, "bind.tween.easing", factory)
    local tween: { [string]: any } = { durationMs = tw.durationMs, easing = tw.easing }
    if tw.from ~= nil then
      if kind == "panel" then
        requireColor(tw.from, "bind.tween.from", factory)
      else
        requireFiniteNumber(tw.from, "bind.tween.from", factory)
      end
      tween.from = tw.from
    end
    out.tween = tween
  end

  return out
end

-- Validate + clone a `styleRanges` prop, or nil when absent.
local function buildStyleRanges(value: any, factory: string): any?
  if value == nil then
    return nil
  end
  if type(value) ~= "table" then
    error(`{factory}: \`styleRanges\` must be a table`)
  end
  requireFiniteNumber(value.max, "styleRanges.max", factory)
  if type(value.entries) ~= "table" then
    error(`{factory}: \`styleRanges.entries\` must be an array`)
  end
  local entries: { any } = {}
  for i, raw in ipairs(value.entries) do
    if type(raw) ~= "table" then
      error(`{factory}: \`styleRanges.entries[{i}]\` must be a table`)
    end
    local entry: { [string]: any } = {}
    if raw.upTo ~= nil then
      requireFiniteNumber(raw.upTo, `styleRanges.entries[{i}].upTo`, factory)
      entry.upTo = raw.upTo
    end
    if raw.color ~= nil then
      requireColor(raw.color, `styleRanges.entries[{i}].color`, factory)
      entry.color = raw.color
    end
    if raw.pulse ~= nil then
      if type(raw.pulse) ~= "table" then
        error(`{factory}: \`styleRanges.entries[{i}].pulse\` must be a table`)
      end
      requireFiniteNumber(raw.pulse.periodMs, `styleRanges.entries[{i}].pulse.periodMs`, factory)
      entry.pulse = { periodMs = raw.pulse.periodMs }
    end
    if raw.flash ~= nil then
      if type(raw.flash) ~= "table" then
        error(`{factory}: \`styleRanges.entries[{i}].flash\` must be a table`)
      end
      requireFiniteNumber(raw.flash.durationMs, `styleRanges.entries[{i}].flash.durationMs`, factory)
      entry.flash = { durationMs = raw.flash.durationMs }
    end
    entries[i] = entry
  end
  return { max = value.max, entries = entries }
end

-- Validate + clone an optional `focusNeighbors` prop, or nil when empty.
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

local function buildRepeatPolicy(value: any, field: string, factory: string): any
  if type(value) ~= "table" then
    error(`{factory}: \`{field}\` must be a table`)
  end
  requireFiniteNumber(value.initialDelayMs, `{field}.initialDelayMs`, factory)
  requireFiniteNumber(value.intervalMs, `{field}.intervalMs`, factory)
  return { initialDelayMs = value.initialDelayMs, intervalMs = value.intervalMs }
end

-- Append optional `id`/`focusNeighbors` to a leaf descriptor in place.
local function applyFocusFields(out: { [string]: any }, props: any, factory: string)
  if props.id ~= nil then
    requireNonemptyString(props.id, "id", factory)
    out.id = props.id
  end
  local neighbors = buildFocusNeighbors(props.focusNeighbors, factory)
  if neighbors ~= nil then
    out.focusNeighbors = neighbors
  end
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

-- Validate an optional `role` prop, appending it to `out` when present.
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

-- Validate + clone a `Predicate` prop (`visibleWhen`/`selected`/`checked`, or a
-- Button `bind`), or nil when absent. The source is `{ slot }` (a store
-- handle/string) or `{ local }`; `equals` is an optional scalar comparand. `slot`
-- wins when present.
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

local function buildBarMax(value: any, factory: string): any
  if type(value) == "number" then
    requireFiniteNumber(value, "max", factory)
    return value
  end
  if type(value) ~= "table" then
    error(`{factory}: \`max\` must be a finite number or a state reference`)
  end
  requireNonemptyString(value.slot, "max.slot", factory)
  return { slot = value.slot }
end

-- Append an optional `visibleWhen` predicate + `role` shared by every widget.
local function applyA11yFields(out: { [string]: any }, props: any, factory: string)
  local visibleWhen = buildPredicate(props.visibleWhen, "visibleWhen", factory)
  if visibleWhen ~= nil then
    out.visibleWhen = visibleWhen
  end
  applyRole(out, props.role, factory)
end

-- Validate a 9-slice border prop, returning the wire table. Exported for reuse
-- by `layout.luau` stacks (which redeclare the helper -- see that file).
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
  requireColor(value.tint, "border.tint", factory)
  return { texture = value.texture, slice = value.slice, tint = value.tint }
end
UiWidgetsSdk.validateBorder = validateBorder

-- Read a reaction name from a handle (`.name`) or accept a bare name string.
local function resolveReactionName(value: any, factory: string): string
  if type(value) == "string" then
    if #value == 0 then
      error(`{factory}: \`onPress\` must be a nonempty reaction name`)
    end
    return value
  end
  if type(value) == "table" and type(value.name) == "string" then
    if #value.name == 0 then
      error(`{factory}: \`onPress\` handle has an empty \`.name\``)
    end
    return value.name
  end
  error(`{factory}: \`onPress\` must be a reaction handle or a reaction-name string`)
end
UiWidgetsSdk.resolveReactionName = resolveReactionName

-- --- Text -------------------------------------------------------------------

--- A `text` leaf. `content` is the literal/fallback string; `fontSize` defaults
--- to 12 (logical px); `color` defaults to opaque white. An optional `bind`
--- resolves the rendered string from a store slot; `styleRanges` recolors by
--- value. Mirrors `descriptor.rs` `TextWidget`.
function UiWidgetsSdk.Text(props: any): WidgetDescriptor
  requireObject(props, "Text")
  requireString(props.content, "content", "Text")
  local fontSize = if props.fontSize ~= nil then props.fontSize else 12.0
  requireFiniteNumber(fontSize, "fontSize", "Text")
  local color = if props.color ~= nil then props.color else { 1.0, 1.0, 1.0, 1.0 }
  requireColor(color, "color", "Text")

  local out: { [string]: any } = { kind = "text", content = props.content, fontSize = fontSize, color = color }
  applyFocusFields(out, props, "Text")
  if props.font ~= nil then
    requireNonemptyString(props.font, "font", "Text")
    out.font = props.font
  end
  local bind = buildBind(props.bind, "Text", "text")
  if bind ~= nil then
    out.bind = bind
  end
  local styleRanges = buildStyleRanges(props.styleRanges, "Text")
  if styleRanges ~= nil then
    out.styleRanges = styleRanges
  end
  applyA11yFields(out, props, "Text")
  return out
end

-- --- Panel ------------------------------------------------------------------

--- A `panel` leaf: a solid `fill` with an optional 9-slice `border`. An optional
--- `bind` resolves the fill from a length-4 RGBA slot. A border-less panel OMITS
--- the `border` key (the lua->json walker drops nil-valued keys, so an explicit
--- `null` cannot be carried). The Rust `PanelWidget.border` is `#[serde(default)]`,
--- so an absent key deserializes to `None` and re-serializes as `border: null` --
--- byte-identical to the shipped fixtures, and identical to the TS factory's
--- output. Mirrors `PanelWidget`.
function UiWidgetsSdk.Panel(props: any): WidgetDescriptor
  requireObject(props, "Panel")
  requireColor(props.fill, "fill", "Panel")

  local out: { [string]: any } = { kind = "panel", fill = props.fill }
  if props.border ~= nil then
    out.border = validateBorder(props.border, "Panel")
  end
  applyFocusFields(out, props, "Panel")
  local bind = buildBind(props.bind, "Panel", "panel")
  if bind ~= nil then
    out.bind = bind
  end
  local styleRanges = buildStyleRanges(props.styleRanges, "Panel")
  if styleRanges ~= nil then
    out.styleRanges = styleRanges
  end
  applyA11yFields(out, props, "Panel")
  return out
end

-- --- Image ------------------------------------------------------------------

--- An `image` leaf referencing a texture asset by key; sizes from the asset's
--- natural pixel dimensions. No bind capability. Name-XOR-decorative (M13 G2):
--- exactly one of `label` (accessible name) or `decorative = true` (hidden from
--- a11y) is required -- neither or both errors. Mirrors `ImageWidget`.
function UiWidgetsSdk.Image(props: any): WidgetDescriptor
  requireObject(props, "Image")
  requireNonemptyString(props.asset, "asset", "Image")
  local hasLabel = props.label ~= nil
  if props.decorative ~= nil and type(props.decorative) ~= "boolean" then
    error("Image: `decorative` must be a boolean")
  end
  local decorative = props.decorative == true
  if hasLabel and decorative then
    error("Image: set exactly one of `label` or `decorative: true`, not both")
  end
  if not hasLabel and not decorative then
    error("Image: needs an accessible name: set exactly one of `label` or `decorative: true`")
  end
  local out: { [string]: any } = { kind = "image", asset = props.asset }
  applyFocusFields(out, props, "Image")
  if hasLabel then
    requireNonemptyString(props.label, "label", "Image")
    out.label = props.label
  end
  if decorative then
    out.decorative = true
  end
  applyA11yFields(out, props, "Image")
  return out
end

-- --- Spacer -----------------------------------------------------------------

--- A `spacer` leaf claiming a proportional share of leftover space (`flexGrow`,
--- default 1). No bind capability. Mirrors `SpacerWidget`.
function UiWidgetsSdk.Spacer(props: any?): WidgetDescriptor
  local p = if props ~= nil then props else {}
  requireObject(p, "Spacer")
  local flexGrow = if p.flexGrow ~= nil then p.flexGrow else 1.0
  requireFiniteNumber(flexGrow, "flexGrow", "Spacer")
  local out: { [string]: any } = { kind = "spacer", flexGrow = flexGrow }
  if p.id ~= nil then
    requireNonemptyString(p.id, "id", "Spacer")
    out.id = p.id
  end
  applyA11yFields(out, p, "Spacer")
  return out
end

-- --- Button -----------------------------------------------------------------

--- An interactive `button`. `id` is required. `onPress` accepts a reaction
--- handle (the `.name` is read) or a bare reaction-name string, emitting the
--- unchanged `onPress: string` wire form. Name-XOR (M13 G2): exactly one of
--- `label` / `labelledBy` is required -- neither or both errors. `selected`/
--- `checked` are reactive predicates; `bind`+`styleRanges` drive the reactive
--- highlight; `disabled` makes it non-interactive.
function UiWidgetsSdk.Button(props: any): WidgetDescriptor
  requireObject(props, "Button")
  requireNonemptyString(props.id, "id", "Button")
  local hasLabel = props.label ~= nil
  local hasLabelledBy = props.labelledBy ~= nil
  if hasLabel and hasLabelledBy then
    error("Button: set exactly one of `label` or `labelledBy`, not both")
  end
  if not hasLabel and not hasLabelledBy then
    error("Button: needs an accessible name: set exactly one of `label` or `labelledBy`")
  end

  local onPress = resolveReactionName(props.onPress, "Button")
  local out: { [string]: any } = { kind = "button", id = props.id, onPress = onPress }
  if hasLabel then
    requireString(props.label, "label", "Button")
    out.label = props.label
  else
    requireNonemptyString(props.labelledBy, "labelledBy", "Button")
    out.labelledBy = props.labelledBy
  end
  if props.focusNeighbors ~= nil then
    local neighbors = buildFocusNeighbors(props.focusNeighbors, "Button")
    if neighbors ~= nil then
      out.focusNeighbors = neighbors
    end
  end
  if props.repeatOnHold ~= nil then
    out.repeatOnHold = buildRepeatPolicy(props.repeatOnHold, "repeatOnHold", "Button")
  end
  local selected = buildPredicate(props.selected, "selected", "Button")
  if selected ~= nil then
    out.selected = selected
  end
  local checked = buildPredicate(props.checked, "checked", "Button")
  if checked ~= nil then
    out.checked = checked
  end
  local bind = buildPredicate(props.bind, "bind", "Button")
  if bind ~= nil then
    out.bind = bind
  end
  local styleRanges = buildStyleRanges(props.styleRanges, "Button")
  if styleRanges ~= nil then
    out.styleRanges = styleRanges
  end
  if props.disabled ~= nil then
    if type(props.disabled) ~= "boolean" then
      error("Button: `disabled` must be a boolean")
    end
    if props.disabled then
      out.disabled = true
    end
  end
  applyA11yFields(out, props, "Button")
  return out
end

-- --- Slider -----------------------------------------------------------------

--- An interactive `slider`. Nav wires in `capturesNav` step the bound value by
--- `step` within `[min, max]`. `id` and `bind` are required. Name-XOR (M13 G2):
--- exactly one of `label` / `labelledBy` is required. `disabled` makes it
--- non-interactive.
function UiWidgetsSdk.Slider(props: any): WidgetDescriptor
  requireObject(props, "Slider")
  requireNonemptyString(props.id, "id", "Slider")
  local hasLabel = props.label ~= nil
  local hasLabelledBy = props.labelledBy ~= nil
  if hasLabel and hasLabelledBy then
    error("Slider: set exactly one of `label` or `labelledBy`, not both")
  end
  if not hasLabel and not hasLabelledBy then
    error("Slider: needs an accessible name: set exactly one of `label` or `labelledBy`")
  end
  requireFiniteNumber(props.min, "min", "Slider")
  requireFiniteNumber(props.max, "max", "Slider")
  requireFiniteNumber(props.step, "step", "Slider")
  local bind = buildBind(props.bind, "Slider", "slider")
  if bind == nil then
    error("Slider: `bind` is required")
  end

  local out: { [string]: any } = { kind = "slider", id = props.id }
  if hasLabel then
    requireString(props.label, "label", "Slider")
    out.label = props.label
  else
    requireNonemptyString(props.labelledBy, "labelledBy", "Slider")
    out.labelledBy = props.labelledBy
  end
  out.bind = bind
  out.min = props.min
  out.max = props.max
  out.step = props.step
  if props.capturesNav ~= nil then
    if type(props.capturesNav) ~= "table" then
      error("Slider: `capturesNav` must be a string array")
    end
    for i, n in ipairs(props.capturesNav) do
      requireNonemptyString(n, `capturesNav[{i}]`, "Slider")
    end
    if #props.capturesNav > 0 then
      local copy: { string } = {}
      for i, n in ipairs(props.capturesNav) do
        copy[i] = n
      end
      out.capturesNav = copy
    end
  end
  if props.focusNeighbors ~= nil then
    local neighbors = buildFocusNeighbors(props.focusNeighbors, "Slider")
    if neighbors ~= nil then
      out.focusNeighbors = neighbors
    end
  end
  if props.disabled ~= nil then
    if type(props.disabled) ~= "boolean" then
      error("Slider: `disabled` must be a boolean")
    end
    if props.disabled then
      out.disabled = true
    end
  end
  applyA11yFields(out, props, "Slider")
  return out
end

-- --- Bar --------------------------------------------------------------------

--- A passive `bar`: fill fraction is `value/max` clamped to `[0, 1]`.
--- `styleRanges` recolors the fill from that displayed fraction. `bind` is required.
function UiWidgetsSdk.Bar(props: any): WidgetDescriptor
  requireObject(props, "Bar")
  local bind = buildBind(props.bind, "Bar", "slider")
  if bind == nil then
    error("Bar: `bind` is required")
  end
  local max = buildBarMax(props.max, "Bar")
  requireColor(props.fill, "fill", "Bar")
  requireColor(props.background, "background", "Bar")

  local out: { [string]: any } = {
    kind = "bar",
    bind = bind,
    max = max,
    fill = props.fill,
    background = props.background,
  }
  if props.id ~= nil then
    requireNonemptyString(props.id, "id", "Bar")
    out.id = props.id
  end
  local styleRanges = buildStyleRanges(props.styleRanges, "Bar")
  if styleRanges ~= nil then
    out.styleRanges = styleRanges
  end
  applyA11yFields(out, props, "Bar")
  return out
end

-- --- Announce ---------------------------------------------------------------

--- A non-visual `announce` widget (M13 G2): lays out as nothing; its `text` is a
--- live-region message routed to the platform a11y layer at the declared
--- `priority`. `text` is the POSITIONAL second argument. `priority` is emitted
--- ONLY for `"assertive"`; `"polite"` (and an omitted value) drop the key,
--- matching the Rust `skip_serializing_if = "is_polite"`. Mirrors `AnnounceWidget`.
function UiWidgetsSdk.Announce(props: any, text: any): WidgetDescriptor
  requireObject(props, "Announce")
  requireString(text, "text", "Announce")
  local out: { [string]: any } = { kind = "announce", text = text }
  if props.priority ~= nil then
    if props.priority ~= "polite" and props.priority ~= "assertive" then
      error('Announce: `priority` must be "polite" or "assertive"')
    end
    if props.priority == "assertive" then
      out.priority = "assertive"
    end
  end
  local visibleWhen = buildPredicate(props.visibleWhen, "visibleWhen", "Announce")
  if visibleWhen ~= nil then
    out.visibleWhen = visibleWhen
  end
  return out
end

return UiWidgetsSdk
