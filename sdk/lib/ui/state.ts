// Authoritative state-reference helpers. State refs are immutable descriptors
// with runtime shape `{ slot }`; readonly/writable and value type are type-level
// capabilities. Presentation-local cells below remain separate and keep their
// `.get()`/`.set()` handle behavior.

import type { PrimitiveReactionDescriptor } from "../data_script";
import type {
  ColorTween,
  LocalBindRef,
  NumberTween,
  NumericArrayStateValue,
  Predicate,
  PredicateValue,
  ReadonlyStateRef,
  ScalarStateValue,
} from "./widgets";

export type StateBindOptionsFor<T> =
  T extends number ? { format?: string; tween?: NumberTween; slot?: never; local?: never } :
  T extends NumericArrayStateValue ? { tween?: ColorTween; slot?: never; local?: never } :
  T extends ScalarStateValue ? { format?: string; slot?: never; local?: never } :
  never;

function stateSlot(ref: ReadonlyStateRef<unknown>, helper: string): string {
  if (ref === null || typeof ref !== "object" || typeof ref.slot !== "string" || ref.slot.length === 0) {
    throw new Error(`${helper}: expected a state reference with a nonempty \`slot\``);
  }
  return ref.slot;
}

/**
 * Compose bind-only options onto a state reference. Pure: returns the existing
 * retained bind wire shape `{ slot, ...options }`.
 */
export function bindState<T>(
  ref: ReadonlyStateRef<T>,
  options?: StateBindOptionsFor<T>,
): ReadonlyStateRef<T> & Omit<StateBindOptionsFor<T>, "slot" | "local"> {
  const slot = stateSlot(ref, "bindState");
  if (options !== undefined) {
    if (Object.prototype.hasOwnProperty.call(options, "slot")) {
      throw new Error("bindState: `options.slot` is reserved");
    }
    if (Object.prototype.hasOwnProperty.call(options, "local")) {
      throw new Error("bindState: `options.local` is reserved");
    }
  }
  return options === undefined
    ? ({ slot } as ReadonlyStateRef<T> & Omit<StateBindOptionsFor<T>, "slot" | "local">)
    : ({ slot, ...options } as ReadonlyStateRef<T> & Omit<StateBindOptionsFor<T>, "slot" | "local">);
}

/** Build an equality predicate against a readable scalar state reference. */
export function stateEquals<T extends PredicateValue>(
  ref: ReadonlyStateRef<T>,
  value: T,
): Predicate {
  return { slot: stateSlot(ref, "stateEquals"), equals: value };
}

// --- ui.createLocalState() --------------------------------------------------

/** A presentation-cell initial value: the `CellInit` wire shapes. */
type CellInit = number | boolean | string | [number, number, number, number];

/**
 * A presentation-cell handle returned by `ui.createLocalState()` for ONE cell.
 * Distinct from authoritative state references: this handle is presentation-only.
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
  /**
   * An equality `Predicate` against this cell (`{ local, equals: value }`).
   * `value` is typed to the cell's `T`, so a mismatched comparand is a TS error.
   * `Switch(cell, map)` uses this to inject each subtree's `visibleWhen`.
   */
  is(value: T): Predicate;
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
 * `ui.createLocalState(init)` — declare a presentation-cell scope. An SDK-lib
 * function (hand-authored, NOT a registered primitive, NOT auto-emitted): the
 * one stateful authoring primitive the static factory layer can't express. Pure
 * — no engine side effect; the FFI boundary stays the authored tree's `return`.
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
      is(value): Predicate {
        return { local: name as string, equals: value as number | boolean | string } as Predicate;
      },
    };
  }
  return { scope: { scope: scopeId, cells: init }, cells };
}

/**
 * The `ui` namespace object: state-helper SDK functions are namespaced
 * (reactions stay bare exports, state helpers are namespaced).
 * `ui.createLocalState` is the entry point.
 */
// --- Switch(cell, map) ------------------------------------------------------

/**
 * A widget subtree the `Switch` map associates with a cell value: a `kind`-tagged
 * descriptor (any factory's output). Re-declared structurally (no cyclic import).
 */
type SwitchSubtree = { kind: string; [field: string]: unknown };

/**
 * `Switch(cell, map)` — declarative reactive branching sugar. Reads the handle's
 * `{ local }` cell name and expands the `map`'s subtrees into an array, injecting
 * `visibleWhen: cell.is(key)` onto each (a shallow clone — the input subtree is
 * not mutated). Splice the result into a container's `children`: at runtime
 * exactly the subtree whose key equals the cell value is visible.
 *
 * Keys are string cell values; `cell.is(key)` carries the string key as the
 * `equals` comparand. Keys expand in LEXICOGRAPHICALLY-SORTED order so the emitted
 * array is byte-IDENTICAL between TS and Luau (Luau table iteration order is
 * undefined, so the sort is load-bearing for cross-runtime wire identity). A key
 * already carrying a `visibleWhen` is rejected (the injected predicate would be
 * lost).
 */
export function Switch(
  cell: LocalStateHandle<string>,
  map: Record<string, SwitchSubtree>,
): SwitchSubtree[] {
  if (cell === null || typeof cell !== "object" || typeof cell.is !== "function") {
    throw new Error("Switch: `cell` must be a `ui.createLocalState` cell handle");
  }
  if (map === null || typeof map !== "object") {
    throw new Error("Switch: `map` must be an object of value → subtree");
  }
  const keys = Object.keys(map).sort();
  if (keys.length === 0) {
    throw new Error("Switch: `map` must declare at least one case");
  }
  return keys.map((key) => {
    const subtree = map[key];
    if (subtree === null || typeof subtree !== "object" || typeof subtree.kind !== "string") {
      throw new Error(`Switch: case \`${key}\` must be a widget descriptor (a \`kind\`-tagged object)`);
    }
    if (subtree.visibleWhen !== undefined) {
      throw new Error(`Switch: case \`${key}\` already declares \`visibleWhen\`; Switch injects it`);
    }
    return { ...subtree, visibleWhen: cell.is(key) };
  });
}

/**
 * The `ui` namespace object: state-helper SDK functions are namespaced
 * (reactions stay bare exports, state helpers are namespaced).
 * `ui.createLocalState` is the entry point.
 */
export const ui = {
  createLocalState,
};
