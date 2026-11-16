--!strict
-- Authoritative state-reference helpers. State refs carry an SDK-only `kind`
-- tag alongside `slot`; descriptor wire includes only `slot`. Readonly/writable
-- remains a type-level capability. Presentation-local cells below remain
-- separate and keep their `:get()`/`:set()` handle behavior.

--- System reaction produced by `:set()`. Mirrors `data_script.luau`'s
--- `SystemReactionStep`; redeclared here to keep the module self-contained
--- (the Luau prelude has no module loader).
export type SystemReactionStep = {
  primitive: string,
  args: { [string]: any }?,
}

--- The ordered relation a numeric predicate tests (mirrors `CompareOp`).
export type PredicateCompareOp = "greaterThan" | "lessThan" | "atLeast" | "atMost"

--- A numeric comparison against a Number source: `{ op, value }`.
export type PredicateCompare = { op: PredicateCompareOp, value: number }

--- A reactive predicate (M13 G2): `{ slot }` or `{ local }` source + optional
--- `equals` comparand or numeric `compare` (never both). Produced by
--- `stateEquals(ref, v)` / `stateCompare(ref, op, v)` / `LocalStateHandle:is(v)`.
export type Predicate = { slot: string?, ["local"]: string?, equals: any?, compare: PredicateCompare? }

local UiStateSdk = {}

local function stateSlot(ref: any, helper: string): string
  if type(ref) ~= "table" or type(ref.slot) ~= "string" or #ref.slot == 0 then
    error(`{helper}: expected a state reference with a nonempty \`slot\``)
  end
  return ref.slot
end

--- Compose bind-only options onto a state reference. Pure: returns the existing
--- retained bind wire shape `{ slot, ...options }`.
function UiStateSdk.bindState(ref: any, options: { [string]: any }?): any
  local out: { [string]: any } = { slot = stateSlot(ref, "bindState") }
  if options ~= nil then
    for key, value in options do
      if key == "slot" then
        error("bindState: `options.slot` is reserved")
      end
      if key == "local" then
        error("bindState: `options.local` is reserved")
      end
      if key == "kind" then
        error("bindState: `options.kind` is reserved")
      end
      out[key] = value
    end
  end
  -- Keep `kind` inherited so it stays out of descriptor serialization. Freezing
  -- the raw wire table prevents an author assignment (including `rawset`) from
  -- shadowing that SDK-only metadata.
  return table.freeze(setmetatable(out, { __index = { kind = ref.kind } }))
end

--- Build an equality predicate against a readable scalar state reference.
function UiStateSdk.stateEquals(ref: any, value: any): Predicate
  return { slot = stateSlot(ref, "stateEquals"), equals = value }
end

local COMPARE_OPS = { greaterThan = true, lessThan = true, atLeast = true, atMost = true }

--- Build a numeric comparison predicate against a readable Number state
--- reference: true while the slot's value stands in `op` relation to `value`
--- (`stateCompare(player.assistProgress, "greaterThan", 0)`). Rejects a
--- non-number ref, an unknown op, and a non-finite `value`.
function UiStateSdk.stateCompare(ref: any, op: PredicateCompareOp, value: number): Predicate
  local slot = stateSlot(ref, "stateCompare")
  if ref.kind ~= "number" then
    error(`stateCompare: \`{slot}\` is not a number state reference`)
  end
  if type(op) ~= "string" or not COMPARE_OPS[op] then
    error("stateCompare: `op` must be one of greaterThan, lessThan, atLeast, atMost")
  end
  -- NaN fails `value == value`; ±inf fails the magnitude bound.
  if type(value) ~= "number" or value ~= value or value == math.huge or value == -math.huge then
    error("stateCompare: `value` must be a finite number")
  end
  return { slot = slot, compare = { op = op, value = value } }
end

-- --- ui.createLocalState() -------------------------------------------------

--- The `{ ["local"] = name }` bind reference a widget's `bind` prop accepts
--- (`local` is a Luau keyword, so the field is written in bracket form).
export type LocalBindRef = { ["local"]: string }

--- A presentation-cell handle for ONE cell, returned inside `createLocalState`'s
--- bundle. Distinct from authoritative state references:
--- `:get()` yields the `{ local }` bind reference; `:set(v)` emits a `cellWrite`
--- reaction (NEVER `setState`) updating the cell at the game-logic stage; `:is(v)`
--- produces an equality `Predicate` (typed to the cell's `T`).
export type LocalStateHandle<T> = {
  get: (self: LocalStateHandle<T>) -> LocalBindRef,
  set: (self: LocalStateHandle<T>, value: T) -> SystemReactionStep,
  is: (self: LocalStateHandle<T>, value: T) -> Predicate,
}

--- Monotonic counter stabilizing each `createLocalState` call's scope id.
--- Deterministic per script-evaluation order (single-threaded registration
--- pass), so the same authoring code yields the same scope id the app stage
--- (writes) and render stage (`{ local }` resolution) both address by.
local localStateCounter = 0

--- `ui.createLocalState(init)` -- declare a presentation-cell scope. An
--- SDK-lib function (hand-authored, NOT a registered primitive, NOT
--- auto-emitted). Pure -- no engine side effect. Returns a `{ scope, cells }`
--- bundle: splice `bundle.scope` onto the declaring container's `localState`, and
--- bind descendant widgets to `bundle.cells.<name>:get()`.
--- `bundle.cells.<name>:set(v)` emits a `cellWrite` reaction; the authoritative
--- store is never written. `State`-named because cells are stored across frames.
function UiStateSdk.createLocalState(init: { [string]: any }): { scope: any, cells: any }
  local scopeId = "localState." .. tostring(localStateCounter)
  localStateCounter += 1
  local cells: { [string]: any } = {}
  for name in init do
    local cellName = name
    cells[name] = {
      get = function(self): LocalBindRef
        return { ["local"] = cellName }
      end,
      set = function(self, value): SystemReactionStep
        -- A `cellWrite` reaction (NEVER `setState`): presentation cell, scoped.
        return { primitive = "cellWrite", args = { scope = scopeId, cell = cellName, value = value } }
      end,
      is = function(self, value): Predicate
        return { ["local"] = cellName, equals = value }
      end,
    }
  end
  return { scope = { scope = scopeId, cells = init }, cells = cells }
end

-- --- Switch(cell, map) -----------------------------------------------------

--- `Switch(cell, map)` -- declarative reactive branching sugar. Reads the
--- handle's `{ local }` cell name and expands the `map`'s subtrees into an array,
--- injecting `visibleWhen = cell:is(key)` onto a shallow clone of each (the input
--- subtree is not mutated). Splice the result into a container's `children`: at
--- runtime exactly the subtree whose key equals the cell value is visible.
---
--- Keys expand in LEXICOGRAPHICALLY-SORTED order so the emitted array is
--- byte-IDENTICAL to the TS `Switch` (Luau table iteration order is undefined, so
--- the sort is load-bearing). A key already carrying `visibleWhen` errors.
function UiStateSdk.Switch(cell: any, map: { [string]: any }): { any }
  if type(cell) ~= "table" or type(cell.is) ~= "function" then
    error("Switch: `cell` must be a `ui.createLocalState` cell handle")
  end
  if type(map) ~= "table" then
    error("Switch: `map` must be a table of value -> subtree")
  end
  -- Collect keys, then sort lexicographically -- Luau pair iteration order is
  -- undefined, so this sort is what makes the output match TS byte-for-byte.
  local keys: { string } = {}
  for key in map do
    table.insert(keys, key)
  end
  if #keys == 0 then
    error("Switch: `map` must declare at least one case")
  end
  table.sort(keys)

  local out: { any } = {}
  for i, key in ipairs(keys) do
    local subtree = map[key]
    if type(subtree) ~= "table" or type(subtree.kind) ~= "string" then
      error(`Switch: case \`{key}\` must be a widget descriptor (a \`kind\`-tagged table)`)
    end
    if subtree.visibleWhen ~= nil then
      error(`Switch: case \`{key}\` already declares \`visibleWhen\`; Switch injects it`)
    end
    -- Shallow clone the subtree, then inject the predicate.
    local clone: { [string]: any } = {}
    for k, v in subtree do
      clone[k] = v
    end
    clone.visibleWhen = cell:is(key)
    out[i] = clone
  end
  return out
end

local function isWidgetNode(value: any): boolean
  return type(value) == "table" and type(value.kind) == "string"
end

local function isSequence(value: any): boolean
  return type(value) == "table" and not isWidgetNode(value) and (#value > 0 or next(value) == nil)
end

local function collectRowIds(node: { [string]: any }, ids: { [string]: boolean })
  if type(node.id) == "string" then
    ids[node.id] = true
  end
  for _, value in node do
    if isWidgetNode(value) then
      collectRowIds(value, ids)
    elseif isSequence(value) then
      for _, item in ipairs(value) do
        if isWidgetNode(item) then
          collectRowIds(item, ids)
        end
      end
    end
  end
end

local function suffixRow(node: { [string]: any }, ids: { [string]: boolean }, suffix: string): { [string]: any }
  local function follow(target: any): any
    if type(target) == "string" and ids[target] then
      return target .. suffix
    end
    return target
  end
  local out: { [string]: any } = {}
  for key, value in node do
    if isWidgetNode(value) then
      out[key] = suffixRow(value, ids, suffix)
    elseif isSequence(value) and key ~= "focusNeighbors" then
      local items = {}
      -- Index to the border, as children validation does, so a `nil` hole
      -- does not cut the rows short.
      for i = 1, #value do
        local item = value[i]
        items[i] = if isWidgetNode(item) then suffixRow(item, ids, suffix) else item
      end
      out[key] = items
    else
      out[key] = value
    end
  end
  if type(node.id) == "string" then
    out.id = node.id .. suffix
  end
  if node.labelledBy ~= nil then
    out.labelledBy = follow(node.labelledBy)
  end
  if type(node.focusNeighbors) == "table" then
    local neighbors = {}
    for direction, target in node.focusNeighbors do
      neighbors[direction] = follow(target)
    end
    out.focusNeighbors = neighbors
  end
  return out
end

--- `forEach(list, function(row, i) ... end)` — unroll a widget once per row of
--- an engine catalog list (`getGameState().session.players`), up to the list's
--- capacity, and return the rows as an array to splice into a container's
--- `children`. `i` counts rows from 1, as `ipairs` does; row `i` reads slot row
--- `i - 1`.
---
--- A row shows only while its `present` flag holds and any `visibleWhen` it
--- declares holds too. Every authored id in a row gets the suffix `.<slot row>`
--- (the same ids TypeScript emits), and a `labelledBy` or `focusNeighbors`
--- naming an id inside the same row follows it. The tree stays static: rows
--- appearing or leaving never rebuild it.
function UiStateSdk.forEach(list: any, build: (any, number) -> any): { any }
  if type(list) ~= "table" or type(list.__list) ~= "string" then
    error("forEach: `list` must be an engine state list such as `getGameState().session.players`")
  end
  if type(build) ~= "function" then
    error("forEach: `build` must be a function `(row, i) -> widget`")
  end
  local out: { any } = {}
  for i, row in ipairs(list) do
    local widget = build(row, i)
    if not isWidgetNode(widget) then
      error(`forEach: row {i} must build a widget descriptor (a \`kind\`-tagged table)`)
    end
    local ids: { [string]: boolean } = {}
    collectRowIds(widget, ids)
    local suffixed = suffixRow(widget, ids, "." .. tostring(i - 1))
    local present = { slot = stateSlot(row.present, "forEach"), equals = true }
    if suffixed.visibleWhen == nil then
      suffixed.visibleWhen = present
      out[i] = suffixed
    else
      -- One predicate per node: the row keeps its own, and a zero-spacing
      -- stack around it shows only while the row is present.
      out[i] = { kind = "vstack", gap = 0, padding = 0, align = "stretch", visibleWhen = present, children = { suffixed } }
    end
  end
  return out
end

--- The `ui` namespace object: state-helper SDK functions are namespaced
--- (reactions stay bare exports, state helpers are namespaced).
--- `ui.createLocalState` is the entry point.
UiStateSdk.ui = {
  createLocalState = UiStateSdk.createLocalState,
}

return UiStateSdk
