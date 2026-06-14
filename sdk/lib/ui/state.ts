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
import type { SliderBindProp } from "./widgets";
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
