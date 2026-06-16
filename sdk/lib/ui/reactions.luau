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
export type CrossingCondition =
  { below: number, above: nil?, max: number? }
  | { below: nil?, above: number, max: number? }

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

local function stateSlot(ref: any, helper: string): string
  if type(ref) ~= "table" or type(ref.slot) ~= "string" or #ref.slot == 0 then
    error(`{helper}: expected a state reference with a nonempty \`slot\``)
  end
  return ref.slot
end

local function isFiniteNumber(value: any): boolean
  return type(value) == "number" and value == value and value ~= math.huge and value ~= -math.huge
end

local function crossingThreshold(condition: any): (string, number, number?)
  if type(condition) ~= "table" then
    error("onStateCrossing: `condition` must be a table")
  end
  local hasBelow = condition.below ~= nil
  local hasAbove = condition.above ~= nil
  if hasBelow == hasAbove then
    error("onStateCrossing: `condition` must declare exactly one of `below` or `above`")
  end
  local key = if hasBelow then "below" else "above"
  local value = if hasBelow then condition.below else condition.above
  if not isFiniteNumber(value) then
    error(`onStateCrossing: \`{key}\` must be a finite number`)
  end
  if condition.max ~= nil and not isFiniteNumber(condition.max) then
    error("onStateCrossing: `max` must be a finite number when provided")
  end
  return key, value, condition.max
end

--- Build a state-crossing watcher. Pure: returns a plain table, does not
--- register anything in the engine. The engine consumes the table when
--- `setupLevel` returns its `crossings` array. On a crossing in the condition's
--- direction the engine fires every reaction in `fire` exactly once, re-arming
--- only after a crossing back. A registration against a non-Number slot warns
--- and is skipped at load.
function UiReactionsSdk.onStateCrossing(
  ref: any,
  condition: CrossingCondition,
  fire: {any}
): CrossingDescriptor
  if type(fire) ~= "table" then
    error("onStateCrossing: `fire` must be an array of reaction handles or strings")
  end
  local fireNames: { string } = {}
  for i, entry in ipairs(fire) do
    if type(entry) == "string" then
      fireNames[i] = entry
    elseif type(entry) == "table" and type(entry.name) == "string" and #entry.name > 0 then
      fireNames[i] = entry.name
    else
      error("onStateCrossing: `fire` entries must be reaction handles or strings")
    end
  end
  local key, value, max = crossingThreshold(condition)
  local descriptor: CrossingDescriptor = {
    slot = stateSlot(ref, "onStateCrossing"),
    fire = fireNames,
  }
  descriptor[key] = value
  if max ~= nil then
    descriptor.max = max
  end
  return descriptor
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

--- Darken (or tint) the screen edges by writing the engine-owned
--- `screen.vignette` slot, which rises to peak then decays back to rest. Pure:
--- returns a primitive reaction body, no engine side effect. `strength` is the
--- peak edge-darken amount; `durationMs` is the total rise-plus-decay time in
--- milliseconds. The optional `color` is an `{r, g, b}` linear-RGB tint (omitted
--- when nil ⇒ black, a pure strength-only edge-darken).
function UiReactionsSdk.vignette(
  strength: number,
  durationMs: number,
  color: {number}?
): PrimitiveReactionDescriptor
  local args: { [string]: any } = { strength = strength, durationMs = durationMs }
  if color ~= nil then
    args.color = color
  end
  return { primitive = "vignette", args = args }
end

--- Shake the screen by writing the engine-owned `screen.shake` offset slot, a
--- decaying oscillation that fades to rest. Pure: returns a primitive reaction
--- body, no engine side effect. `amplitude` is the peak displacement in
--- logical-reference px; `durationMs` is the total decay time in milliseconds.
--- The optional `frequency` is the oscillation rate in Hz (omitted when nil ⇒
--- the engine applies its default frequency).
function UiReactionsSdk.screenShake(
  amplitude: number,
  durationMs: number,
  frequency: number?
): PrimitiveReactionDescriptor
  local args: { [string]: any } = { amplitude = amplitude, durationMs = durationMs }
  if frequency ~= nil then
    args.frequency = frequency
  end
  return { primitive = "screenShake", args = args }
end

--- Push a dialog UI tree onto the modal stack. Pure: returns a primitive
--- reaction body, no engine side effect. `tree` names the UI tree to show; the
--- optional `onCommit` (omitted when nil) names a reaction fired when the
--- dialog commits. An unknown tree name warns and no-ops at dispatch time.
function UiReactionsSdk.showDialog(tree: string, onCommit: string?): PrimitiveReactionDescriptor
  local args: { [string]: any } = { tree = tree }
  if onCommit ~= nil then
    args.onCommit = onCommit
  end
  return { primitive = "showDialog", args = args }
end

--- The engine-shipped on-screen keyboard's registry name. `openTextEntry` opens
--- this tree; the engine loads its descriptor from `content/base/ui/keyboard.json`
--- at boot. The keyboard edits the `ui.textEntry` writable String slot.
UiReactionsSdk.KEYBOARD_TREE = "keyboard"

--- Open the engine-shipped on-screen keyboard for text entry (M13 Text Entry).
--- Pure: returns a primitive reaction body wrapping `showDialog`. The keyboard is
--- a capturing modal that edits the `ui.textEntry` slot; bind a `text` widget to
--- `ui.textEntry` to show the live entry. The optional `onCommit` (omitted when
--- nil) names a reaction fired on commit (the on-screen `done` key or hardware
--- Enter); `nav.cancel` (Escape / B) closes without firing it. The same
--- `ui.textEntry` slot also receives the hardware-keyboard path's edits.
function UiReactionsSdk.openTextEntry(onCommit: string?): PrimitiveReactionDescriptor
  return UiReactionsSdk.showDialog(UiReactionsSdk.KEYBOARD_TREE, onCommit)
end

--- Push a menu UI tree onto the modal stack. Pure: returns a primitive
--- reaction body, no engine side effect. A v1 alias of `showDialog` (identical
--- push behavior) without the `onCommit` hook. An unknown tree name warns and
--- no-ops at dispatch time.
function UiReactionsSdk.openMenu(tree: string): PrimitiveReactionDescriptor
  return { primitive = "openMenu", args = { tree = tree } }
end

--- Pop the top UI tree off the modal stack. Pure: returns a primitive reaction
--- body, no engine side effect. An empty stack warns and no-ops at dispatch time.
function UiReactionsSdk.closeDialog(): PrimitiveReactionDescriptor
  return { primitive = "closeDialog", args = {} }
end

--- Write `value` to the writable state reference at the game-logic stage.
--- Pure: returns the existing `setState` primitive reaction body, no engine
--- side effect. The write is readonly-gated at runtime: a readonly slot warns
--- and is left unchanged; an engine-owned writable slot is valid. `value` is
--- coerced to the slot's declared type by the write path.
function UiReactionsSdk.updateState(ref: any, value: any): PrimitiveReactionDescriptor
  return { primitive = "setState", args = { slot = stateSlot(ref, "updateState"), value = value } }
end

--- Append `text` to the current string value of the writable String slot `slot`
--- at the game-logic stage (M13 Text Entry). Pure: returns a primitive reaction
--- body, no engine side effect. Readonly-gated through the same writable-slot gate
--- as `setState`: a readonly slot warns and is left unchanged; an engine-owned
--- writable slot (e.g. `ui.textEntry`) is a valid target.
function UiReactionsSdk.appendText(ref: any, text: string): PrimitiveReactionDescriptor
  return { primitive = "appendText", args = { slot = stateSlot(ref, "appendText"), text = text } }
end

--- Remove the last character -- one Unicode scalar value (the char-pop floor:
--- never splits a UTF-8 sequence, but does not segment grapheme clusters) --
--- from the writable String slot `slot` at the game-logic stage (M13 Text
--- Entry). Pure: returns a primitive reaction body, no engine side effect.
--- Empty is a no-op with no warning. Readonly-gated like `setState`.
function UiReactionsSdk.backspaceText(ref: any): PrimitiveReactionDescriptor
  return { primitive = "backspaceText", args = { slot = stateSlot(ref, "backspaceText") } }
end

--- Empty the writable String slot `slot` at the game-logic stage (M13 Text
--- Entry). Pure: returns a primitive reaction body, no engine side effect.
--- Readonly-gated like `setState`.
function UiReactionsSdk.clearText(ref: any): PrimitiveReactionDescriptor
  return { primitive = "clearText", args = { slot = stateSlot(ref, "clearText") } }
end

return UiReactionsSdk
