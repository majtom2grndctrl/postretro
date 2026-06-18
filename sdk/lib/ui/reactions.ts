// UI reaction vocabulary: pure builders for the HUD-dynamics reaction surface
// (M13 Goal E). `onStateCrossing` constructs a state-crossing watcher returned
// through `setupLevel().crossings` or `setupMod().crossings` — it never calls
// back into Rust; the FFI boundary is the `return` statement.
// See: context/lib/scripting.md §10.4

import type { ReadonlyStateRef, WritableStateRef } from "./widgets";

/**
 * Crossing condition: fires when the watched slot crosses the threshold in one
 * direction. Exactly one of `below`/`above` is given. `max` is the denominator
 * the threshold is a fraction of (`threshold / max` vs `value / max`); omit it
 * for a raw-value comparison (`max` defaults to `1.0`).
 */
export type CrossingCondition =
  | { below: number; above?: never; max?: number }
  | { above: number; below?: never; max?: number };

/**
 * A state-crossing watcher entry as it appears in `setupLevel().crossings` or
 * `setupMod().crossings`. `slot` is the dotted state-slot name; the condition
 * is flattened in (`below`/`above` plus optional `max`); `fire` is the list of
 * named reactions dispatched when the crossing occurs. `levels` scopes
 * mod-global crossings by map-catalog tags; omit it for every level.
 */
export type CrossingDescriptor = {
  slot: string;
  max?: number;
  fire: string[];
  levels?: string[];
} & ({ below: number } | { above: number });

function stateSlot(ref: ReadonlyStateRef<unknown>, helper: string): string {
  if (ref === null || typeof ref !== "object" || typeof ref.slot !== "string" || ref.slot.length === 0) {
    throw new Error(`${helper}: expected a state reference with a nonempty \`slot\``);
  }
  return ref.slot;
}

function reactionName(entry: import("../data_script").NamedReactionDescriptor | string): string {
  if (typeof entry === "string") {
    return entry;
  }
  if (entry !== null && typeof entry === "object" && typeof entry.name === "string" && entry.name.length > 0) {
    return entry.name;
  }
  throw new Error("onStateCrossing: `fire` entries must be reaction handles or strings");
}

function crossingThreshold(condition: CrossingCondition): { key: "below" | "above"; value: number; max?: number } {
  if (condition === null || typeof condition !== "object") {
    throw new Error("onStateCrossing: `condition` must be an object");
  }
  const hasBelow = Object.prototype.hasOwnProperty.call(condition, "below");
  const hasAbove = Object.prototype.hasOwnProperty.call(condition, "above");
  if (hasBelow === hasAbove) {
    throw new Error("onStateCrossing: `condition` must declare exactly one of `below` or `above`");
  }
  const key = hasBelow ? "below" : "above";
  const value = condition[key];
  if (typeof value !== "number" || !Number.isFinite(value)) {
    throw new Error(`onStateCrossing: \`${key}\` must be a finite number`);
  }
  if (condition.max !== undefined && (typeof condition.max !== "number" || !Number.isFinite(condition.max))) {
    throw new Error("onStateCrossing: `max` must be a finite number when provided");
  }
  return condition.max === undefined ? { key, value } : { key, value, max: condition.max };
}

/**
 * Build a state-crossing watcher. Pure — returns a plain object, no engine side
 * effect. Place the result in `setupLevel().crossings` or in
 * `setupMod().crossings` with optional `levels` scoping. The engine watches
 * `slot` after each frame's slot writes and, on a crossing in the condition's
 * direction (from at-or-past the threshold to across it), fires every reaction
 * in `fire` exactly once; it re-arms only after a crossing back. A registration
 * against a non-Number slot warns and is skipped at load.
 */
export function onStateCrossing(
  ref: ReadonlyStateRef<number>,
  condition: CrossingCondition,
  fire: (import("../data_script").NamedReactionDescriptor | string)[],
): CrossingDescriptor {
  if (!Array.isArray(fire)) {
    throw new Error("onStateCrossing: `fire` must be an array of reaction handles or strings");
  }
  const threshold = crossingThreshold(condition);
  const descriptor: CrossingDescriptor = {
    slot: stateSlot(ref, "onStateCrossing"),
    fire: fire.map(reactionName),
    [threshold.key]: threshold.value,
  } as CrossingDescriptor;
  if (threshold.max !== undefined) {
    descriptor.max = threshold.max;
  }
  return descriptor;
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
 * Darken (or tint) the screen edges by writing the engine-owned
 * `screen.vignette` slot, which rises to peak then decays back to rest. Pure —
 * returns a primitive reaction body, no engine side effect. `strength` is the
 * peak edge-darken amount; `durationMs` is the total rise-plus-decay time in
 * milliseconds. The optional `color` is an `[r, g, b]` linear-RGB tint (omitted
 * when undefined ⇒ black, a pure strength-only edge-darken).
 */
export function vignette(
  strength: number,
  durationMs: number,
  color?: [number, number, number],
): import("../data_script").PrimitiveReactionDescriptor {
  const args: {
    color?: [number, number, number];
    strength: number;
    durationMs: number;
  } = { strength, durationMs };
  if (color !== undefined) args.color = color;
  return { primitive: "vignette", args };
}

/**
 * Shake the screen by writing the engine-owned `screen.shake` offset slot, a
 * decaying oscillation that fades to rest. Pure — returns a primitive reaction
 * body, no engine side effect. `amplitude` is the peak displacement in
 * logical-reference px; `durationMs` is the total decay time in milliseconds.
 * The optional `frequency` is the oscillation rate in Hz (omitted when
 * undefined ⇒ the engine applies its default frequency).
 */
export function screenShake(
  amplitude: number,
  durationMs: number,
  frequency?: number,
): import("../data_script").PrimitiveReactionDescriptor {
  const args: { amplitude: number; durationMs: number; frequency?: number } = {
    amplitude,
    durationMs,
  };
  if (frequency !== undefined) args.frequency = frequency;
  return { primitive: "screenShake", args };
}

/**
 * Push a dialog UI tree onto the modal stack. Pure — returns a primitive
 * reaction body, no engine side effect. `tree` names the UI tree to show; the
 * optional `onCommit` (omitted when undefined) names a reaction fired when the
 * dialog commits. An unknown tree name warns and no-ops at dispatch time.
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
 * The engine-shipped on-screen keyboard's registry name. `openTextEntry` opens
 * this tree; the engine loads its descriptor from `content/base/ui/keyboard.json`
 * at boot. The keyboard edits the `ui.textEntry` writable String slot.
 */
export const KEYBOARD_TREE = "keyboard";

/**
 * Reserved button `onPress` action that closes the active modal. The App
 * intercepts this exact wire value before named-reaction dispatch.
 */
export const CLOSE_DIALOG_ACTION = "ui.closeDialog";

/**
 * Reserved button `onPress` action that requests a clean app shutdown. The App
 * intercepts this exact wire value before named-reaction dispatch.
 */
export const EXIT_TO_DESKTOP_ACTION = "ui.exitToDesktop";

/**
 * Open the engine-shipped on-screen keyboard for text entry (M13 Text Entry).
 * Pure — returns a primitive reaction body wrapping `showDialog`. The keyboard is
 * a capturing modal that edits the `ui.textEntry` slot; bind a `text` widget to
 * `ui.textEntry` to show the live entry. The optional `onCommit` names a reaction
 * fired when the player commits (the on-screen `done` key or the hardware Enter
 * key); `nav.cancel` (Escape / B) closes without firing it.
 *
 * This is the canonical opener for the gamepad-accessible text-entry pattern: the
 * same `ui.textEntry` slot also receives the hardware-keyboard path's edits, so a
 * field bound to it reflects typing on either input path. Pass the result as the
 * descriptor of `defineReaction("name", openTextEntry(...))` and reference that
 * reaction from a `button`'s `onPress`.
 */
export function openTextEntry(
  onCommit?: string,
): import("../data_script").PrimitiveReactionDescriptor {
  return showDialog(KEYBOARD_TREE, onCommit);
}

/**
 * Push a menu UI tree onto the modal stack. Pure — returns a primitive
 * reaction body, no engine side effect. A v1 alias of `showDialog` (identical
 * push behavior) without the `onCommit` hook. An unknown tree name warns and
 * no-ops at dispatch time.
 */
export function openMenu(
  tree: string,
): import("../data_script").PrimitiveReactionDescriptor {
  return { primitive: "openMenu", args: { tree } };
}

/**
 * Pop the top UI tree off the modal stack. Pure — returns a primitive reaction
 * body, no engine side effect. An empty stack warns and no-ops at dispatch time.
 */
export function closeDialog(): import("../data_script").PrimitiveReactionDescriptor {
  return { primitive: "closeDialog", args: {} };
}

/**
 * Write `value` to the writable state reference at the game-logic stage.
 * Pure — returns the existing `setState` primitive reaction body, no engine
 * side effect. The write is readonly-gated at runtime: a readonly slot warns
 * and is left unchanged; an engine-owned writable slot is a valid target.
 * `value` is coerced to the slot's declared type by the write path.
 */
export function updateState<T extends number | boolean | string | ReadonlyArray<number>>(
  ref: WritableStateRef<T>,
  value: T,
): import("../data_script").PrimitiveReactionDescriptor {
  return { primitive: "setState", args: { slot: stateSlot(ref, "updateState"), value } };
}

/**
 * Append `text` to the current string value of the writable String slot `slot`
 * at the game-logic stage (M13 Text Entry). Pure — returns a primitive reaction
 * body, no engine side effect. Readonly-gated through the same writable-slot gate
 * as `setState`: a readonly slot warns and is left unchanged; an engine-owned
 * writable slot (e.g. `ui.textEntry`) is a valid target.
 */
export function appendText(
  ref: WritableStateRef<string>,
  text: string,
): import("../data_script").PrimitiveReactionDescriptor {
  return { primitive: "appendText", args: { slot: stateSlot(ref, "appendText"), text } };
}

/**
 * Remove the last character — one Unicode scalar value (the char-pop floor:
 * never splits a UTF-8 sequence, but does not segment grapheme clusters, so a
 * trailing combining mark pops on its own) — from the writable String slot
 * `slot` at the game-logic stage (M13 Text Entry). Pure — returns a primitive
 * reaction body, no engine side effect.
 * Empty is a no-op with no warning. Readonly-gated like `setState`.
 */
export function backspaceText(
  ref: WritableStateRef<string>,
): import("../data_script").PrimitiveReactionDescriptor {
  return { primitive: "backspaceText", args: { slot: stateSlot(ref, "backspaceText") } };
}

/**
 * Empty the writable String slot `slot` at the game-logic stage (M13 Text
 * Entry). Pure — returns a primitive reaction body, no engine side effect.
 * Readonly-gated like `setState`.
 */
export function clearText(
  ref: WritableStateRef<string>,
): import("../data_script").PrimitiveReactionDescriptor {
  return { primitive: "clearText", args: { slot: stateSlot(ref, "clearText") } };
}
