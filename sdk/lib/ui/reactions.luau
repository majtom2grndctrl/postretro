--!strict
-- UI reaction vocabulary: pure builders for the HUD-dynamics reaction surface
-- (M13 Goal E). `onStateCrossing` constructs a state-crossing watcher returned
-- through `setupLevel`'s manifest in the `crossings` field. It never calls back
-- into Rust -- the FFI boundary is the `return` statement of the script entry
-- point.
-- See: context/lib/scripting.md §10.4

--- Crossing condition: fires when the watched slot crosses the threshold in one
--- direction. Exactly one of `below`/`above` is given. `max` is the denominator
--- the threshold is a fraction of; omit it for a raw-value comparison (`max`
--- defaults to `1.0`).
export type CrossingCondition = {
  below: number?,
  above: number?,
  max: number?,
}

--- A state-crossing watcher entry as it appears in `setupLevel`'s manifest
--- `crossings` array. The condition fields are flattened in beside `slot` and
--- `fire`; `fire` is the list of named reactions dispatched (through the shared
--- named-reaction vocabulary) when the crossing occurs.
export type CrossingDescriptor = {
  slot: string,
  below: number?,
  above: number?,
  max: number?,
  fire: {string},
}

--- Primitive reaction body produced by the system-reaction constructors below.
--- Mirrors `data_script.luau`'s `PrimitiveReactionDescriptor`; redeclared here
--- to keep this module self-contained (the Luau prelude has no module loader).
export type PrimitiveReactionDescriptor = {
  primitive: string,
  tag: string?,
  args: { [string]: any }?,
  onComplete: string?,
}

local UiReactionsSdk = {}

--- Build a state-crossing watcher. Pure: returns a plain table, does not
--- register anything in the engine. The engine consumes the table when
--- `setupLevel` returns its `crossings` array. On a crossing in the condition's
--- direction the engine fires every reaction in `fire` exactly once, re-arming
--- only after a crossing back. A registration against a non-Number slot warns
--- and is skipped at load.
function UiReactionsSdk.onStateCrossing(
  slot: string,
  condition: CrossingCondition,
  fire: {string}
): CrossingDescriptor
  return {
    slot = slot,
    below = condition.below,
    above = condition.above,
    max = condition.max,
    fire = fire,
  }
end

--- Play a sound through the M12 audio module. Pure: returns a primitive
--- reaction body, no engine side effect. Pass the result as the descriptor of
--- `defineReaction("name", playSound(...))`. `sound` is an audio asset id; the
--- optional `bus` routes to a named mixer bus (omitted when nil).
function UiReactionsSdk.playSound(sound: string, bus: string?): PrimitiveReactionDescriptor
  local args: { [string]: any } = { sound = sound }
  if bus ~= nil then
    args.bus = bus
  end
  return { primitive = "playSound", args = args }
end

--- Drive gamepad force feedback through gilrs. Pure: returns a primitive
--- reaction body, no engine side effect. `strong` and the optional `weak`
--- (omitted when nil) are 0-1 motor intensities; `durationMs` is the rumble
--- length in milliseconds. A warn-once no-op when force feedback is
--- unsupported.
function UiReactionsSdk.rumble(strong: number, durationMs: number, weak: number?): PrimitiveReactionDescriptor
  local args: { [string]: any } = { strong = strong, durationMs = durationMs }
  if weak ~= nil then
    args.weak = weak
  end
  return { primitive = "rumble", args = args }
end

--- Flash the screen by writing the engine-owned `screen.flash` RGBA slot,
--- which decays back to transparent. Pure: returns a primitive reaction body,
--- no engine side effect. `color` is an `{r, g, b, a}` tuple (0-1); `durationMs`
--- is the decay time in milliseconds.
function UiReactionsSdk.flashScreen(color: {number}, durationMs: number): PrimitiveReactionDescriptor
  return { primitive = "flashScreen", args = { color = color, durationMs = durationMs } }
end

--- Push a dialog UI tree onto the modal stack. Pure: returns a primitive
--- reaction body, no engine side effect. `tree` names the UI tree to show; the
--- optional `onCommit` (omitted when nil) names a reaction fired when the
--- dialog commits. Warn-once "no stack" until Goal F's modal stack lands.
function UiReactionsSdk.showDialog(tree: string, onCommit: string?): PrimitiveReactionDescriptor
  local args: { [string]: any } = { tree = tree }
  if onCommit ~= nil then
    args.onCommit = onCommit
  end
  return { primitive = "showDialog", args = args }
end

--- Push a menu UI tree onto the modal stack. Pure: returns a primitive
--- reaction body, no engine side effect. A v1 alias of `showDialog` (identical
--- push behavior) without the `onCommit` hook. Warn-once "no stack" until
--- Goal F's modal stack lands.
function UiReactionsSdk.openMenu(tree: string): PrimitiveReactionDescriptor
  return { primitive = "openMenu", args = { tree = tree } }
end

--- Pop the top UI tree off the modal stack. Pure: returns a primitive reaction
--- body, no engine side effect. Warn-once "no stack" until Goal F's modal
--- stack lands.
function UiReactionsSdk.closeDialog(): PrimitiveReactionDescriptor
  return { primitive = "closeDialog", args = {} }
end

--- Write `value` to the writable store slot `slot` at the game-logic stage
--- (M13 Goal F). Pure: returns a primitive reaction body, no engine side effect.
--- The write is readonly-gated: a readonly slot warns and is left unchanged; an
--- engine-owned writable slot is a valid target. `value` is coerced to the slot's
--- declared type by the write path. The slider widget emits this on a captured
--- nav step; scripts fire it as a named reaction.
function UiReactionsSdk.setState(
  slot: string,
  value: number | boolean | string | {number}
): PrimitiveReactionDescriptor
  return { primitive = "setState", args = { slot = slot, value = value } }
end

--- Append `text` to the current string value of the writable String slot `slot`
--- at the game-logic stage (M13 Text Entry). Pure: returns a primitive reaction
--- body, no engine side effect. Readonly-gated through the same writable-slot gate
--- as `setState`: a readonly slot warns and is left unchanged; an engine-owned
--- writable slot (e.g. `ui.textEntry`) is a valid target.
function UiReactionsSdk.appendText(slot: string, text: string): PrimitiveReactionDescriptor
  return { primitive = "appendText", args = { slot = slot, text = text } }
end

--- Remove the last grapheme cluster (char-pop floor: never splits a UTF-8
--- sequence) from the writable String slot `slot` at the game-logic stage (M13
--- Text Entry). Pure: returns a primitive reaction body, no engine side effect.
--- Empty is a no-op with no warning. Readonly-gated like `setState`.
function UiReactionsSdk.backspaceText(slot: string): PrimitiveReactionDescriptor
  return { primitive = "backspaceText", args = { slot = slot } }
end

--- Empty the writable String slot `slot` at the game-logic stage (M13 Text
--- Entry). Pure: returns a primitive reaction body, no engine side effect.
--- Readonly-gated like `setState`.
function UiReactionsSdk.clearText(slot: string): PrimitiveReactionDescriptor
  return { primitive = "clearText", args = { slot = slot } }
end

return UiReactionsSdk
