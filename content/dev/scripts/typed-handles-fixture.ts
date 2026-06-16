// M13 G1a author-facing fixture — typed handle surfaces.
//
// This file is a documented REVIEW GATE, not a runtime script. The repo has no
// `tsc` CI, so the `@ts-expect-error` lines below assert the intended compile-
// time behavior for a human reviewer (and for an author who opens it in an
// IDE): each marked line MUST be a type error. If a future change makes one of
// them compile cleanly, `tsc --noEmit` would flag the now-unused
// `@ts-expect-error` — the contract has drifted and the review gate fails.
//
// See: context/lib/scripting.md §5, context/lib/ui.md §3, M13 G1a Task 1.

import {
  defineStore,
  defineReaction,
  getGameState,
  updateState,
  appendText,
  backspaceText,
  bindState,
  stateEquals,
  onStateCrossing,
  Text,
  Bar,
  Button,
  Slider,
  Tree,
  Panel,
  type StateValue,
  type LocalizedText,
} from "postretro";

// --- (1) Value-typed slot handles -------------------------------------------
// `defineStore` infers each slot's value type from its `type` discriminant.
const opts = defineStore("fixtureOpts", {
  volume: { type: "number", default: 0.8 },
  muted: { type: "boolean", default: false },
  preset: { type: "string", default: "default" },
  curve: { type: "array", default: [0.0, 0.5, 1.0] },
});

// Correct shape: declarations are returned from `setupMod().stores`, while
// state references are stable `{ slot }` objects keyed by schema field.
const _fixtureStoreDeclaration = opts.declaration;
const _volume: StateValue<number> = opts.state.volume;
const _muted: StateValue<boolean> = opts.state.muted;
const _preset: StateValue<string> = opts.state.preset;

// The documented mismatch: a `boolean` slot handle is NOT assignable to a
// numeric-typed binding. This is the `@ts-expect-error` fixture the AC requires.
// @ts-expect-error — `muted` is StateValue<boolean>, not StateValue<number>.
const _wrong: StateValue<number> = opts.state.muted;

// --- (2) Read-only engine-slot refs -----------------------------------------
// `getGameState().player.health` is directly bindable as a `{ slot }` ref.
const gameState = getGameState();
const _health = gameState.player.health;
const _healthText = Text({ content: "HP", bind: gameState.player.health });
const _healthBar = Bar({
  bind: gameState.player.health,
  max: gameState.player.maxHealth,
  fill: "ok",
  background: [0.1, 0.1, 0.1, 1],
});
const _formattedHealth = Text({
  content: "HP",
  bind: bindState(gameState.player.health, {
    format: "HP {}",
    tween: { durationMs: 120, easing: "easeOut" },
  }),
});
const _flashPanel = Panel({
  fill: "panel.default",
  bind: bindState(gameState.screen.flash, {
    tween: { durationMs: 80, easing: "linear", from: [0, 0, 0, 0] },
  }),
});

// Slider writes require a writable number ref; engine health is readonly.
// @ts-expect-error — readonly health cannot feed an interactive Slider.
Slider({ id: "hp", label: "HP", bind: gameState.player.health, min: 0, max: 100, step: 1 });
const _volumeSlider = Slider({
  id: "vol",
  label: "Volume",
  bind: opts.state.volume,
  min: 0,
  max: 1,
  step: 0.1,
});

// @ts-expect-error — sliders write numbers, not writable string refs.
Slider({ id: "preset", label: "Preset", bind: opts.state.preset, min: 0, max: 1, step: 1 });

// @ts-expect-error — array refs accept color tweens, not text `format`.
const _badArrayBind = bindState(gameState.screen.flash, { format: "RGBA {}" });

// Reaction state writes go through `updateState(ref, value)`: writable mod state
// is accepted and emits the existing `setState` wire descriptor.
const _volumeReset = defineReaction("fixtureResetVolume", updateState(opts.state.volume, 0.5));

// @ts-expect-error — readonly health cannot feed a state-write reaction.
const _badHealthWrite = updateState(gameState.player.health, 1);

// Text-entry targets and text-edit reactions require writable string refs.
const _entryTree = Tree(
  { anchor: "center", offset: [0, 0], textEntryTarget: gameState.ui.textEntry },
  Text({ content: "Name", bind: gameState.ui.textEntry }),
);
const _appendEntry = appendText(gameState.ui.textEntry, "a");
const _backspaceEntry = backspaceText(gameState.ui.textEntry);

const _badEntryTarget = Tree(
  {
    anchor: "center",
    offset: [0, 0],
    // @ts-expect-error — text-entry target must be a writable string ref.
    textEntryTarget: opts.state.volume,
  },
  Text({ content: "Bad" }),
);
// @ts-expect-error — text edits require writable string refs.
const _badAppend = appendText(opts.state.volume, "x");

// Crossings require numeric refs; equality predicates require scalar refs and a
// comparand matching the ref value type.
const _lowHealthCrossing = onStateCrossing(gameState.player.health, { below: 20, max: 100 }, [
  "lowHealth",
]);
const _modePredicate = stateEquals(gameState.input.mode, "focus");

// @ts-expect-error — crossings watch number refs, not writable strings.
const _badCrossing = onStateCrossing(gameState.ui.textEntry, { above: 1 }, ["bad"]);
// @ts-expect-error — equality predicates are scalar-only; arrays are excluded.
const _badArrayPredicate = stateEquals(gameState.screen.flash, [0, 0, 0, 0]);
// @ts-expect-error — equality comparand is typed to the ref's value type.
const _badHealthPredicate = stateEquals(gameState.player.health, "full");

// --- (3) Typed reaction handles ---------------------------------------------
// `defineReaction` accepts an optional `name`; omitted → deterministic auto-id.
// The returned handle is the typed reaction reference (go-to-definition, no
// silent name typos) a `Button.onPress` or crossing `fire` entry accepts.
const _named = defineReaction("explicitName", {
  primitive: "playSound",
  args: { sound: "click" },
});
const _auto = defineReaction({
  primitive: "playSound",
  args: { sound: "confirm" },
});

// --- (4) LocalizedText — the user-facing text-prop chokepoint ----------------
// Every widget text prop (`Text.content`, `Button.label`, …) is typed
// `LocalizedText` (= string today). The intent: a future localization swap
// (message keys, ICU handles) is one edit at the alias, and authoring code keeps
// type-checking. A bare string author surface stays ergonomic now.
const _greeting: LocalizedText = "Hello";
const _text = Text({ content: _greeting });
const _resume = Button({ id: "resume", label: "Resume", onPress: _named });

// The documented mismatch: a widget text prop is text, not an arbitrary value —
// a non-string (here a number) is a type error. When `LocalizedText` becomes a
// branded message-key type, this same line documents that a raw literal no
// longer satisfies the prop without going through the localization constructor.
// @ts-expect-error — `content` is LocalizedText (string), not a number.
const _badText = Text({ content: 42 });

void _volume;
void _muted;
void _preset;
void _wrong;
void _health;
void _healthText;
void _healthBar;
void _formattedHealth;
void _flashPanel;
void _volumeSlider;
void _badArrayBind;
void _named;
void _auto;
void _volumeReset;
void _badHealthWrite;
void _entryTree;
void _appendEntry;
void _backspaceEntry;
void _badEntryTarget;
void _badAppend;
void _lowHealthCrossing;
void _modePredicate;
void _badCrossing;
void _badArrayPredicate;
void _badHealthPredicate;
void _greeting;
void _text;
void _resume;
void _badText;
