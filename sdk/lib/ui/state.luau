--!strict
-- Authoritative state-reference helpers. State refs are immutable descriptors
-- with runtime shape `{ slot = "..." }`; readonly/writable and value type are
-- type-level capabilities. Presentation-local cells below remain separate and
-- keep their `:get()`/`:set()` handle behavior.

--- Primitive reaction body produced by `:set()`. Mirrors `data_script.luau`'s
--- `PrimitiveReactionDescriptor`; redeclared here to keep the module
--- self-contained (the Luau prelude has no module loader).
export type PrimitiveReactionDescriptor = {
  primitive: string,
  tag: string?,
  args: { [string]: any }?,
  onComplete: string?,
}

--- A reactive predicate (M13 G2): `{ slot }` or `{ local }` source + optional
--- `equals` comparand. Produced by `stateEquals(ref, v)` /
--- `LocalStateHandle:is(v)`.
export type Predicate = { slot: string?, ["local"]: string?, equals: any? }

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
      out[key] = value
    end
  end
  return out
end

--- Build an equality predicate against a readable scalar state reference.
function UiStateSdk.stateEquals(ref: any, value: any): Predicate
  return { slot = stateSlot(ref, "stateEquals"), equals = value }
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
  set: (self: LocalStateHandle<T>, value: T) -> PrimitiveReactionDescriptor,
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
      set = function(self, value): PrimitiveReactionDescriptor
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

--- The `ui` namespace object: state-helper SDK functions are namespaced
--- (reactions stay bare exports, state helpers are namespaced).
--- `ui.createLocalState` is the entry point.
UiStateSdk.ui = {
  createLocalState = UiStateSdk.createLocalState,
}

return UiStateSdk
