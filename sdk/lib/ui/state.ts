// Store-slot handle ergonomics (M13 G1a, Task 4): `.get()`/`.set()` accessor
// wrappers over Task 1's value-typed store-slot handles (`StateValue<T>`). The
// runtime representation of a `defineStore` slot handle is a plain branded
// string carrying the dotted slot name (`scripting.md` §6.9) — it carries no
// methods, so these namespaced wrappers are the SDK layer that adds `.get()` /
// `.set()` over it.
//
// `.set(v)` delegates to the shipped `setState` reaction builder so the produced
// descriptor is byte-identical to calling `setState(slot, v)` directly. `.get()`
// yields the typed bind reference a widget binds to (`{ slot }`), accepted by
// `Text` (TextBind), `Panel` (PanelBind), and `Slider`/`Bar` (SliderBind).
//
// Engine-owned slots (`postretro/game-state`) are read-only to mods and expose
// `.get()` ONLY — their handle type (`ReadonlyStateValue<T>`, Task 1) omits
// `.set()`, so a `.set(...)` on an engine slot is a type error. This module is
// the WRITABLE store-handle wrapper; do NOT confuse it with G1b's distinct
// `ui.createLocalState()` presentation handle (which never writes the store).
// See: context/lib/scripting.md §6.9 · context/lib/ui.md

import type { StateValue } from "postretro";
import type { PrimitiveReactionDescriptor } from "../data_script";
import type { LocalBindRef, SliderBindProp } from "./widgets";
import { setState } from "./reactions";

/**
 * A `.get()`/`.set()` accessor wrapper over a writable, value-typed store-slot
 * handle (Task 1's `StateValue<T>`). `T` is the slot's declared value type, so
 * `.set(v)` is typed to it and the bind reference carries it through.
 *
 * - `.get()` yields the typed bind reference a widget binds to — the `{ slot }`
 *   wire shape (`SliderBindProp`) that `Text`/`Panel`/`Slider`/`Bar` accept as
 *   their `bind` prop. Adding a `format`/`tween` is the widget's concern.
 * - `.set(v)` produces a `setState` reaction descriptor (typed to the slot's
 *   `T`), byte-identical to calling `setState(slot, v)` directly.
 *
 * Read-only engine slots use `ReadonlyStateValue<T>` (Task 1) instead — `.get()`
 * only, no `.set()`.
 */
export type StoreHandle<T> = {
  /** The typed bind reference a widget's `bind` prop accepts (`{ slot }`). */
  get(): SliderBindProp;
  /** A `setState` reaction descriptor writing `value` to this slot. */
  set(value: T): PrimitiveReactionDescriptor;
};

/**
 * Wrap a value-typed store-slot handle (`defineStore`'s `StateValue<T>` return)
 * in a `.get()`/`.set()` accessor. Pure: no engine side effect — `.set(...)`
 * returns a descriptor, it does not write. `slot` is the branded dotted-name
 * handle; its runtime form is the slot-name string read straight through to the
 * bind reference and the `setState` descriptor.
 */
export function storeHandle<T extends number | boolean | string | number[]>(
  slot: StateValue<T>,
): StoreHandle<T> {
  // `StateValue<T>` is a branded value carrying the dotted slot name; its runtime
  // form is the slot-name string. Read it through unchanged.
  const name = slot as unknown as string;
  return {
    get(): SliderBindProp {
      return { slot: name };
    },
    set(value: T): PrimitiveReactionDescriptor {
      // Delegate to the shipped builder so the descriptor is byte-identical to a
      // direct `setState(slot, value)` call.
      return setState(name, value as number | boolean | string | number[]);
    },
  };
}

// --- ui.createLocalState() (M13 G1b, Task 5) --------------------------------

/** A presentation-cell initial value: the `CellInit` wire shapes. */
type CellInit = number | boolean | string | [number, number, number, number];

/**
 * A presentation-cell handle returned by `ui.createLocalState()` for ONE cell.
 * Distinct from `StoreHandle` (which writes the authoritative store): this handle
 * is presentation-only.
 *
 * - `.get()` yields the `{ local: name }` bind reference a descendant widget's
 *   `bind` prop accepts — resolved app-side against the cell store, scoped to the
 *   nearest enclosing `localState` declaration.
 * - `.set(v)` emits a `cellWrite` reaction (NEVER `setState`) writing the cell at
 *   the game-logic stage. The authoritative store is untouched.
 */
export type LocalStateHandle<T extends CellInit> = {
  /** The `{ local }` bind reference a widget's `bind` prop accepts. */
  get(): LocalBindRef;
  /** A `cellWrite` reaction descriptor writing `value` to this cell. */
  set(value: T): PrimitiveReactionDescriptor;
};

/**
 * The bundle `ui.createLocalState(init)` returns: a `scope` descriptor to splice
 * into the declaring container's `localState`, plus a per-cell handle map keyed by
 * the `init` keys. `State`-named because the cells are STORED across frames (in
 * the app-side cell store), not recomputed each frame.
 */
export type LocalStateBundle<I extends Record<string, CellInit>> = {
  /** Splice this onto the declaring container's `localState` prop. */
  scope: { scope: string; cells: I };
  /** Per-cell presentation handles, keyed by the `init` keys. */
  cells: { [K in keyof I]: LocalStateHandle<I[K]> };
};

/**
 * Monotonic counter stabilizing the scope id of each `createLocalState` call.
 * Deterministic per script-evaluation order (the registration pass is single-
 * threaded and runs once), so the same authoring code yields the same scope id —
 * which the app stage (writes) and the render stage (`{ local }` resolution) both
 * address by. NOT engine-registered; a pure SDK-side construct.
 */
let localStateCounter = 0;

/**
 * `ui.createLocalState(init)` — declare a presentation-cell scope (M13 G1b, Task
 * 5). An SDK-lib function (hand-authored, NOT a registered primitive, NOT
 * auto-emitted): the one stateful authoring primitive the static factory layer
 * can't express. Pure — no engine side effect; the FFI boundary stays the
 * authored tree's `return`.
 *
 * Returns a `{ scope, cells }` bundle: splice `bundle.scope` onto the declaring
 * container's `localState` (so its initials seed the app-side cell store and the
 * scope id keys the store + reconcile sweep), and bind descendant widgets to
 * `bundle.cells.<name>.get()`. `bundle.cells.<name>.set(v)` emits a `cellWrite`
 * reaction that updates the cell at the game-logic stage — the authoritative
 * store is never written. Follows the `create*` convention (inline-constructed).
 */
export function createLocalState<I extends Record<string, CellInit>>(
  init: I,
): LocalStateBundle<I> {
  const scopeId = `localState.${localStateCounter++}`;
  const cells = {} as { [K in keyof I]: LocalStateHandle<I[K]> };
  for (const name of Object.keys(init) as (keyof I)[]) {
    cells[name] = {
      get(): LocalBindRef {
        return { local: name as string };
      },
      set(value): PrimitiveReactionDescriptor {
        // A `cellWrite` reaction (NEVER `setState`): presentation cell, scoped.
        return {
          primitive: "cellWrite",
          args: { scope: scopeId, cell: name as string, value },
        };
      },
    };
  }
  return { scope: { scope: scopeId, cells: init }, cells };
}

/**
 * The `ui` namespace object: state-helper SDK functions are namespaced (per
 * G1a's locked decision — reactions stay bare exports, state helpers are
 * namespaced). `ui.createLocalState` is the G1b entry.
 */
export const ui = {
  createLocalState,
};
