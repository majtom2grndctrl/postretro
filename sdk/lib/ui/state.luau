--!strict
-- Store-slot handle ergonomics: `:get()`/`:set()` accessor wrappers over
-- value-typed store-slot handles (`StateValue<T>`). The runtime representation
-- of a `defineStore` slot handle is a plain branded string carrying the dotted
-- slot name -- it carries no methods, so these namespaced wrappers add `:get()`
-- / `:set()` over it. Exported as a member of a returned module table.
--
-- `:set(v)` produces a `setState` reaction descriptor BYTE-IDENTICAL to the
-- shipped `reactions.luau` `setState(slot, v)` builder. The Luau prelude has no
-- module loader (so this file cannot `require` `reactions.luau`); the descriptor
-- construction is inlined to match `UiReactionsSdk.setState` exactly. `:get()`
-- yields the typed bind reference a widget binds to (`{ slot = name }`), accepted
-- by `Text`/`Panel`/`Slider`/`Bar`.
--
-- Engine-owned slots (`postretro/game-state`) are read-only to mods and expose
-- `:get()` ONLY -- their handle type (`ReadonlyStateValue<T>`) omits `:set()`.
-- This module is the WRITABLE store-handle wrapper; do NOT confuse it with the
-- distinct `ui.createLocalState()` presentation handle.
-- See: context/lib/scripting.md §6.9 · context/lib/ui.md

--- Primitive reaction body produced by `:set()`. Mirrors `data_script.luau`'s
--- `PrimitiveReactionDescriptor`; redeclared here to keep the module
--- self-contained (the Luau prelude has no module loader).
export type PrimitiveReactionDescriptor = {
  primitive: string,
  tag: string?,
  args: { [string]: any }?,
  onComplete: string?,
}

--- The typed bind reference a widget's `bind` prop accepts (`{ slot = name }`).
export type SliderBindProp = { slot: string, tween: any? }

--- A reactive predicate (M13 G2): `{ slot }` or `{ local }` source + optional
--- `equals` comparand. Produced by `StoreHandle:is(v)` / `LocalStateHandle:is(v)`.
export type Predicate = { slot: string?, ["local"]: string?, equals: any? }

--- A `:get()`/`:set()`/`:is()` accessor wrapper over a writable, value-typed
--- store-slot handle (`StateValue<T>`). `:get()` yields the `{ slot }` bind
--- reference a widget binds to; `:set(v)` produces a `setState` reaction
--- descriptor (typed to the slot's `T`); `:is(v)` produces an equality
--- `Predicate` (typed to `T`). Read-only engine slots use `ReadonlyStateValue<T>`
--- instead -- `:get()` only, no `:set()`.
export type StoreHandle<T> = {
  get: (self: StoreHandle<T>) -> SliderBindProp,
  set: (self: StoreHandle<T>, value: T) -> PrimitiveReactionDescriptor,
  is: (self: StoreHandle<T>, value: T) -> Predicate,
}

local UiStateSdk = {}

--- Wrap a value-typed store-slot handle (`defineStore`'s `StateValue<T>` return)
--- in a `:get()`/`:set()` accessor. Pure: no engine side effect -- `:set(...)`
--- returns a descriptor, it does not write. `slot` is the branded dotted-name
--- handle; its runtime form is the slot-name string read straight through to the
--- bind reference and the `setState` descriptor.
function UiStateSdk.storeHandle<T>(slot: T): StoreHandle<T>
  -- `StateValue<T>` is a branded value carrying the dotted slot name; its runtime
  -- form is the slot-name string. Read it through unchanged.
  local name: string = (slot :: any) :: string
  return {
    get = function(self: StoreHandle<T>): SliderBindProp
      return { slot = name }
    end,
    set = function(self: StoreHandle<T>, value: T): PrimitiveReactionDescriptor
      -- Byte-identical to `UiReactionsSdk.setState(name, value)`.
      return { primitive = "setState", args = { slot = name, value = value } }
    end,
    is = function(self: StoreHandle<T>, value: T): Predicate
      return { slot = name, equals = value }
    end,
  }
end

-- --- ui.createLocalState() -------------------------------------------------

--- The `{ ["local"] = name }` bind reference a widget's `bind` prop accepts
--- (`local` is a Luau keyword, so the field is written in bracket form).
export type LocalBindRef = { ["local"]: string }

--- A presentation-cell handle for ONE cell, returned inside `createLocalState`'s
--- bundle. Distinct from `StoreHandle` (which writes the authoritative store):
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
