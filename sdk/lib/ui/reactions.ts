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

/**
 * Play a sound through the M12 audio module. Pure — returns a primitive
 * reaction body, no engine side effect. Pass the result as the descriptor of
 * `defineReaction("name", playSound(...))`. `sound` is an audio asset id; the
 * optional `bus` routes to a named mixer bus (omitted when undefined, falling
 * back to the engine's default bus).
 */
export function playSound(
  sound: string,
  bus?: string,
): import("../data_script").PrimitiveReactionDescriptor {
  const args: { sound: string; bus?: string } = { sound };
  if (bus !== undefined) args.bus = bus;
  return { primitive: "playSound", args };
}

/**
 * Drive gamepad force feedback through gilrs. Pure — returns a primitive
 * reaction body, no engine side effect. `strong` and the optional `weak`
 * (omitted when undefined) are 0–1 motor intensities; `durationMs` is the
 * rumble length in milliseconds. A warn-once no-op on hardware without force
 * feedback.
 */
export function rumble(
  strong: number,
  durationMs: number,
  weak?: number,
): import("../data_script").PrimitiveReactionDescriptor {
  const args: { strong: number; weak?: number; durationMs: number } = {
    strong,
    durationMs,
  };
  if (weak !== undefined) args.weak = weak;
  return { primitive: "rumble", args };
}

/**
 * Flash the screen by writing the engine-owned `screen.flash` RGBA slot, which
 * decays back to transparent. Pure — returns a primitive reaction body, no
 * engine side effect. `color` is an `[r, g, b, a]` tuple (0–1); `durationMs`
 * is the decay time in milliseconds.
 */
export function flashScreen(
  color: [number, number, number, number],
  durationMs: number,
): import("../data_script").PrimitiveReactionDescriptor {
  return { primitive: "flashScreen", args: { color, durationMs } };
}

/**
 * Push a dialog UI tree onto the modal stack. Pure — returns a primitive
 * reaction body, no engine side effect. `tree` names the UI tree to show; the
 * optional `onCommit` (omitted when undefined) names a reaction fired when the
 * dialog commits. Warn-once "no stack" until Goal F's modal stack lands.
 */
export function showDialog(
  tree: string,
  onCommit?: string,
): import("../data_script").PrimitiveReactionDescriptor {
  const args: { tree: string; onCommit?: string } = { tree };
  if (onCommit !== undefined) args.onCommit = onCommit;
  return { primitive: "showDialog", args };
}

/**
 * Push a menu UI tree onto the modal stack. Pure — returns a primitive
 * reaction body, no engine side effect. A v1 alias of `showDialog` (identical
 * push behavior) without the `onCommit` hook. Warn-once "no stack" until
 * Goal F's modal stack lands.
 */
export function openMenu(
  tree: string,
): import("../data_script").PrimitiveReactionDescriptor {
  return { primitive: "openMenu", args: { tree } };
}

/**
 * Pop the top UI tree off the modal stack. Pure — returns a primitive reaction
 * body, no engine side effect. Warn-once "no stack" until Goal F's modal stack
 * lands.
 */
export function closeDialog(): import("../data_script").PrimitiveReactionDescriptor {
  return { primitive: "closeDialog", args: {} };
}

/**
 * Write `value` to the writable store slot `slot` at the game-logic stage
 * (M13 Goal F). Pure — returns a primitive reaction body, no engine side effect.
 * The write is readonly-gated: a readonly slot warns and is left unchanged; an
 * engine-owned writable slot is a valid target. `value` is coerced to the slot's
 * declared type by the write path. The slider widget emits this on a captured
 * nav step; scripts fire it as a named reaction.
 */
export function setState(
  slot: string,
  value: number | boolean | string | number[],
): import("../data_script").PrimitiveReactionDescriptor {
  return { primitive: "setState", args: { slot, value } };
}
