--!strict
-- UI placement-envelope factory (M13 G1a, Task 4): `Tree(...)` wraps a root
-- widget descriptor (from the `widgets`/`layout` factories) in the `AnchoredTree`
-- shape -- `anchor`, `offset`, `root`, and the optional `captureMode`/
-- `initialFocus`/`textEntryTarget`. Mirrors `render/ui/descriptor.rs`'s
-- `AnchoredTree` serde wire form: camelCase keys, and `captureMode` omitted when
-- passthrough (the default) so a HUD/pre-F tree round-trips byte-identically.
-- Exported as a member of a returned module table (the `UiReactionsSdk` /
-- `reactions.luau` precedent).
--
-- Pure builder: constructing a tree has no engine side effect. `children`-style
-- nesting is the `root` widget's own concern; this factory only places the whole
-- tree once.
-- See: context/lib/ui.md · context/lib/scripting.md §7

--- The nine placement anchors a tree may be pinned to. Mirrors `descriptor.rs`
--- `layout::Anchor` (camelCase wire literals).
export type WidgetAnchor =
  "topLeft" | "top" | "topRight" | "left" | "center" | "right" | "bottomLeft" | "bottom" | "bottomRight"

--- Whether a tree captures input or passes it through. Mirrors `CaptureMode`.
--- `"passthrough"` is the default and round-trips to omission; only `"capture"`
--- is emitted.
export type WidgetCaptureMode = "capture" | "passthrough"

--- A flat descriptor table the `Tree` factory produces (the `AnchoredTree` wire
--- shape). Optional keys appear only when authored.
export type WidgetDescriptor = { [string]: any }

local UiTreeSdk = {}

local ANCHORS: { [string]: boolean } = {
  topLeft = true,
  top = true,
  topRight = true,
  left = true,
  center = true,
  right = true,
  bottomLeft = true,
  bottom = true,
  bottomRight = true,
}

local TREE_ROLES: { [string]: boolean } = {
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

local function requireObject(value: any, factory: string)
  if type(value) ~= "table" then
    error(`{factory}: props must be a table`)
  end
end

local function requireNonemptyString(value: any, field: string, factory: string)
  if type(value) ~= "string" or #value == 0 then
    error(`{factory}: \`{field}\` must be a nonempty string`)
  end
end

local function isFiniteNumber(value: any): boolean
  return type(value) == "number" and value == value and value ~= math.huge and value ~= -math.huge
end

--- Build a placement envelope wrapping `root`. Props come first, the root widget
--- descriptor (from `widgets`/`layout`) is a POSITIONAL second argument. `offset`
--- is an `{x, y}` logical-reference px tuple (+x right, +y down).
---
--- Field emission order matches the Rust struct declaration (anchor, offset,
--- root, captureMode, initialFocus, textEntryTarget) so the JSON re-serializes
--- byte-identically. `captureMode` is emitted ONLY for `"capture"`:
--- `"passthrough"` (and an omitted value) drop the key, matching the Rust
--- `skip_serializing_if = "is_passthrough"`.
function UiTreeSdk.Tree(props: any, root: any): WidgetDescriptor
  requireObject(props, "Tree")
  if not ANCHORS[props.anchor] then
    error(
      'Tree: `anchor` must be one of "topLeft" | "top" | "topRight" | "left" | "center" | "right" | "bottomLeft" | "bottom" | "bottomRight"'
    )
  end
  local offset = props.offset
  if
    type(offset) ~= "table"
    or #offset ~= 2
    or not isFiniteNumber(offset[1])
    or not isFiniteNumber(offset[2])
  then
    error("Tree: `offset` must be an {x, y} tuple of finite numbers")
  end
  if type(root) ~= "table" or type(root.kind) ~= "string" then
    error("Tree: `root` must be a widget descriptor (a `kind`-tagged table)")
  end

  local out: { [string]: any } = {
    anchor = props.anchor,
    offset = { offset[1], offset[2] },
    root = root,
  }

  -- captureMode skip-serializes when passthrough (the default), so emit the key
  -- ONLY for "capture"; "passthrough" and an omitted value drop the key.
  if props.captureMode ~= nil then
    if props.captureMode ~= "capture" and props.captureMode ~= "passthrough" then
      error('Tree: `captureMode` must be "capture" or "passthrough"')
    end
    if props.captureMode == "capture" then
      out.captureMode = "capture"
    end
  end
  if props.initialFocus ~= nil then
    requireNonemptyString(props.initialFocus, "initialFocus", "Tree")
    out.initialFocus = props.initialFocus
  end
  if props.textEntryTarget ~= nil then
    if type(props.textEntryTarget) ~= "table" or type(props.textEntryTarget.slot) ~= "string" then
      error("Tree: `textEntryTarget` must be a writable string state reference")
    end
    requireNonemptyString(props.textEntryTarget.slot, "textEntryTarget.slot", "Tree")
    out.textEntryTarget = props.textEntryTarget.slot
  end
  if props.accessibleName ~= nil then
    requireNonemptyString(props.accessibleName, "accessibleName", "Tree")
    out.accessibleName = props.accessibleName
  end
  if props.role ~= nil then
    if type(props.role) ~= "string" or not TREE_ROLES[props.role] then
      error(
        'Tree: `role` must be one of "tab" | "tablist" | "checkbox" | "radio" | "listitem" | "button" | "slider" | "progressbar" | "image" | "group" | "none"'
      )
    end
    out.role = props.role
  end
  return out
end

return UiTreeSdk
