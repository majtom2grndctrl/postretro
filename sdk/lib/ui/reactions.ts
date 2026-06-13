// UI reaction vocabulary: pure builders for the HUD-dynamics reaction surface
// (M13 Goal E). `onStateCrossing` constructs a state-crossing watcher returned
// through `setupLevel`'s manifest in the `crossings` field — it never calls
// back into Rust; the FFI boundary is the `return` statement.
// See: context/lib/scripting.md §10.4

/**
 * Crossing condition: fires when the watched slot crosses the threshold in one
 * direction. Exactly one of `below`/`above` is given. `max` is the denominator
 * the threshold is a fraction of (`threshold / max` vs `value / max`); omit it
 * for a raw-value comparison (`max` defaults to `1.0`).
 */
export type CrossingCondition =
  | { below: number; max?: number }
  | { above: number; max?: number };

/**
 * A state-crossing watcher entry as it appears in `setupLevel`'s manifest
 * `crossings` array. `slot` is the dotted state-slot name; the condition is
 * flattened in (`below`/`above` plus optional `max`); `fire` is the list of
 * named reactions dispatched (through the shared named-reaction vocabulary)
 * when the crossing occurs.
 */
export type CrossingDescriptor = {
  slot: string;
  max?: number;
  fire: string[];
} & ({ below: number } | { above: number });

/**
 * Build a state-crossing watcher. Pure — returns a plain object, no engine side
 * effect. Place the result in `setupLevel`'s returned `crossings` array. The
 * engine watches `slot` after each frame's slot writes and, on a crossing in
 * the condition's direction (from at-or-past the threshold to across it), fires
 * every reaction in `fire` exactly once; it re-arms only after a crossing back.
 * A registration against a non-Number slot warns and is skipped at load.
 */
export function onStateCrossing(
  slot: string,
  condition: CrossingCondition,
  fire: string[],
): CrossingDescriptor {
  return { slot, ...condition, fire } as CrossingDescriptor;
}
