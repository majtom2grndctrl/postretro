--!strict
-- Store-slot handle ergonomics (M13 G1a, Task 4): `:get()`/`:set()` accessor
-- wrappers over Task 1's value-typed store-slot handles (`StateValue<T>`). The
-- runtime representation of a `defineStore` slot handle is a plain branded string
-- carrying the dotted slot name (`scripting.md` §6.9) -- it carries no methods,
-- so these namespaced wrappers are the SDK layer that adds `:get()` / `:set()`
-- over it. Exported as a member of a returned module table (the `UiReactionsSdk`
-- precedent).
--
-- `:set(v)` produces a `setState` reaction descriptor BYTE-IDENTICAL to the
-- shipped `reactions.luau` `setState(slot, v)` builder. The Luau prelude has no
-- module loader (so this file cannot `require` `reactions.luau`); the descriptor
-- construction is inlined to match `UiReactionsSdk.setState` exactly. `:get()`
-- yields the typed bind reference a widget binds to (`{ slot = name }`), accepted
-- by `Text`/`Panel`/`Slider`/`Bar`.
--
-- Engine-owned slots (`postretro/game-state`) are read-only to mods and expose
-- `:get()` ONLY -- their handle type (`ReadonlyStateValue<T>`, Task 1) omits
-- `:set()`. This module is the WRITABLE store-handle wrapper; do NOT confuse it
-- with G1b's distinct `ui.createLocalState()` presentation handle.
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

--- A `:get()`/`:set()` accessor wrapper over a writable, value-typed store-slot
--- handle (Task 1's `StateValue<T>`). `:get()` yields the `{ slot }` bind
--- reference a widget binds to; `:set(v)` produces a `setState` reaction
--- descriptor (typed to the slot's `T`). Read-only engine slots use
--- `ReadonlyStateValue<T>` (Task 1) instead -- `:get()` only, no `:set()`.
export type StoreHandle<T> = {
  get: (self: StoreHandle<T>) -> SliderBindProp,
  set: (self: StoreHandle<T>, value: T) -> PrimitiveReactionDescriptor,
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
  }
end

-- --- ui.createLocalState() (M13 G1b, Task 5) -------------------------------

--- The `{ ["local"] = name }` bind reference a widget's `bind` prop accepts
--- (`local` is a Luau keyword, so the field is written in bracket form).
export type LocalBindRef = { ["local"]: string }

--- A presentation-cell handle for ONE cell, returned inside `createLocalState`'s
--- bundle. Distinct from `StoreHandle` (which writes the authoritative store):
--- `:get()` yields the `{ local }` bind reference; `:set(v)` emits a `cellWrite`
--- reaction (NEVER `setState`) updating the cell at the game-logic stage.
export type LocalStateHandle<T> = {
  get: (self: LocalStateHandle<T>) -> LocalBindRef,
  set: (self: LocalStateHandle<T>, value: T) -> PrimitiveReactionDescriptor,
}

--- Monotonic counter stabilizing each `createLocalState` call's scope id.
--- Deterministic per script-evaluation order (single-threaded registration
--- pass), so the same authoring code yields the same scope id the app stage
--- (writes) and render stage (`{ local }` resolution) both address by.
local localStateCounter = 0

--- `ui.createLocalState(init)` -- declare a presentation-cell scope (M13 G1b,
--- Task 5). An SDK-lib function (hand-authored, NOT a registered primitive, NOT
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
    }
  end
  return { scope = { scope = scopeId, cells = init }, cells = cells }
end

--- The `ui` namespace object: state-helper SDK functions are namespaced (per
--- G1a's locked decision). `ui.createLocalState` is the G1b entry.
UiStateSdk.ui = {
  createLocalState = UiStateSdk.createLocalState,
}

return UiStateSdk
