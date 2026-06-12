# M13 Goal E — HUD Dynamics

## Goal

HUD reacts to game state without script at runtime: continuous value→style
mapping (`styleRanges`), discrete threshold crossings firing pre-registered
reaction lists (`onStateCrossing`), and the UI reaction helpers (`flashScreen`,
`playSound`, `rumble`, `showDialog`, `closeDialog`, `openMenu`). Realizes
`ui-layer.md` §10–§11, §15 on the shipped C/D/TW foundation.

## Scope

### In scope

- `styleRanges` on `text` and `panel` widgets: ordered value→style entries
  resolving theme tokens, with `pulse` and `flash` effect overlays.
- Engine-side crossing detector: `onStateCrossing` registrations (load-time,
  VM-dropped) watched against the live slot table after game-logic writes.
- System-reaction command path: reaction primitives that target engine
  services (audio, rumble, UI stack, store) instead of entities.
- Helper primitives: `flashScreen`, `playSound`, `rumble`, `showDialog`,
  `closeDialog`, `openMenu` in both runtimes + typedefs + SDK
  `sdk/lib/ui/reactions.{ts,luau}`.
- Engine-owned `screen.flash` slot (RGBA, engine-decayed) as the published
  surface SE later consumes.

### Out of scope

- `setState` (slot-write reaction) — owned by Goal F (slider write-back).
- `bar` widget — Goal F; the styleRanges evaluator is widget-agnostic so bar
  adopts it there.
- New effect kinds beyond `pulse` / `flash` (no `fade`/`scale`/`spin`).
- Screen-space compositor effects (vignette, shake) — Goal SE.
- styleRanges on containers, images, or unbound widgets.
- Crossing conditions beyond `below` / `above` (no `equals`, no hysteresis
  bands).

## Acceptance criteria

- [ ] A `text` or `panel` widget with `styleRanges` bound to `player.health`
  changes color at the declared fractions; first matching entry wins; the
  trailing entry (no `upTo`) is the default; an unknown token degrades per the
  existing theme rules (magenta + warning).
- [ ] `pulse` modulates the resolved color's alpha sinusoidally at `periodMs`;
  `flash` spikes alpha once on entry into the range and decays over
  `durationMs`; both pause when game logic pauses (dt-accumulated time).
- [ ] When a tween (TW) is active on the bind, styleRanges evaluate the eased
  display value; the crossing detector evaluates the authoritative slot — a
  contract test demonstrates the two diverging mid-tween.
- [ ] `onStateCrossing("player.health", { below: 0.2, max: 100 }, [...])`
  fires its reaction list exactly once when the value transitions from
  ≥ threshold to < threshold, fires again only after re-crossing back, and
  does not fire for a slot that already starts below threshold at
  registration.
- [ ] Crossing reactions dispatch through the existing named-reaction
  vocabulary: a crossing can fire the same named reaction an entity event
  fires, with identical effect.
- [ ] `playSound` plays a registered sound on a named bus through the M12
  audio module; `rumble` vibrates the active gamepad for `durationMs` and is
  a warn-once no-op when force feedback is unavailable; `flashScreen` writes
  `screen.flash` and a full-screen panel bound to it renders the flash
  decaying to transparent.
- [ ] `showDialog` / `openMenu` push a named registered tree onto the modal
  stack and `closeDialog` pops it (verified once F's stack lands; warn-once
  "no stack" sink until then); unknown tree name warns, no panic.
- [ ] Both runtimes (TS + Luau) declare identical `onStateCrossing` + helper
  surfaces as SDK-authored types in `sdk/lib/ui/reactions.{ts,luau}`
  (registry-driven typedefs only where a primitive is actually registered);
  `styleRanges` is wire-format + typedef only — script-facing registration
  arrives with G1; the evaluator is live for Rust-built trees; round-trip
  wire test: a descriptor without `styleRanges` keeps its pre-E wire form
  byte-identical.
- [ ] The health-bar panel with `styleRanges` plus the 20% crossing firing
  flash + sound runs in the dev map (manual verification of Task 4's demo).
- [ ] `docs/scripting-reference.md` covers the new surface.

## Tasks

### Task 1: styleRanges descriptor + evaluator

Add `styleRanges: Option<StyleRanges>` to `TextWidget` and `PanelWidget`
(sibling of `bind`; warn once per tree build when present without `bind`,
per the theme-fallback precedent). `StyleRanges { max,
entries }`; entry `{ upTo: Option<f32>, color: Option<ColorValue>, pulse,
flash }`, ordered, first match on `value/max`, trailing no-`upTo` entry is
default. Evaluator is a pure function (value in → resolved style out, no
widget knowledge) living beside the theme resolver so F's `bar` calls it
unchanged. Per-node effect state (pulse phase anchor, flash entry clock) lives
in `NodeContext` next to TW's tween state; clocked by snapshot `time_seconds`.
Evaluates the display value when a tween is active. Appearance-only — drives
redraw, never relayout. Ships the byte-identical round-trip test for
descriptors without `styleRanges`.

### Task 2: system-reaction command path

Today's `ReactionPrimitiveRegistry` handlers mutate `EntityRegistry` only. Add
a command-queue arm: system reaction handlers push typed
`SystemReactionCommand` values (PlaySound, Rumble, FlashScreen, PushTree,
PopTree) onto a queue owned by the scripting context, drained once per frame
by the app after the post-tick event drains — audio/input/UI subsystems
consume their commands without threading `&mut` engine services into
scripting. Registration, named-event resolution (`fire_named_event` /
`fire_named_event_with_sequences`), and the descriptor vocabulary
(`ReactionDescriptor::Primitive`) are shared with entity reactions — one event
namespace, two execution arms. System reactions target no entities: make
`tag` optional on `PrimitiveDescriptor` (absent = system-targeted) and update
both manifest parse paths (JS + Luau) and the SDK descriptor type in the same
pass — no mandatory dummy tag in the author API.

### Task 3: crossing detector

Engine-side watcher declared at load: `onStateCrossing(slot, condition,
reactions)` in both runtimes is a builder whose results return through the
setup-function manifest (the `scripting.md` §12 FFI rule — no side-effect
registration); the manifest gains a `crossings` field beside `reactions`
(entry shape: `{ slot, below|above, max?, fire: [reaction names] }`), and
`DataRegistry` widens to carry it. After the frame's slot writes (concretely:
after `ui_proxy.tick`, before the snapshot build), the detector compares each
watched slot's
current value to its previous: `below: x` fires on `prev >= x && cur < x`,
`above: x` on `prev <= x && cur > x` (thresholds are fractions of the
registration's `max`, carried in the condition object, default 1.0 for
raw-value comparison). Previous value initializes to the slot value at level
start — no fire on initial state; a slot with no value yet initializes its
previous value on first observed value and cannot fire before one exists.
Registration against a non-`Number` slot warns and skips at registration time.
Firing dispatches the reaction list synchronously via Task 2's path. Watchers
clear on level unload with the rest of the data registry. Ships the AC-3
contract test driving the styleRanges evaluator and the detector against the
same tweened slot.

### Task 4: helper primitives + SDK + demo

`playSound { sound, bus? }` → audio module `play()` (volume is deferred —
the audio `play()` path has no per-voice volume today; adding it later is
additive). `rumble
{ strong, weak?, durationMs }` → gilrs force-feedback on the active gamepad
(gilrs 0.11 `ff` API; warn-once no-op when unsupported), duration timeout
tracked in the input stage. `flashScreen { color, durationMs }` → engine-owned
`screen.flash` RGBA slot, written via the engine write path; the drained
`FlashScreen` command feeds an App-side flash-decay state (start color,
`durationMs`, start time) that writes the slot each tick at the game-logic
stage. `showDialog { tree, onCommit? }` / `openMenu { tree }` /
`closeDialog {}` → PushTree/PopTree commands (consumed by F's stack; inert
warn until it lands). `PushTree` carries only the registered tree name plus
the optional `onCommit` reaction name (fired by a commit on the pushed tree —
the text-entry plan's consumer); the tree's capture mode and other behavior
travel on its registered `AnchoredTree` envelope (F's named-tree registry),
not on the command. `showDialog` and `openMenu` are aliases in v1 —
identical `PushTree` behavior apart from `showDialog`'s optional `onCommit`,
both names kept per research-§15 API; semantic divergence (e.g. pause
policy) is deferred to BIS. SDK constructors in `sdk/lib/ui/reactions.{ts,luau}`,
typedef emission, docs, and a demo: health-bar panel with styleRanges + a
crossing at 20% firing flash + sound (demo homes: the hardcoded gameplay tree
in `render/ui/demo.rs`, a `setupLevel` data script and registered sound in
the dev map).

## Sequencing

**Phase 1 (concurrent):** Task 1 (renderer-side, `descriptor.rs`/`tree.rs`),
Task 2 (scripting-side) — disjoint files.
**Phase 2 (sequential):** Task 3 — consumes Task 2's dispatch arm.
**Phase 3 (sequential):** Task 4 — consumes Tasks 2 + 3; demo consumes Task 1.
**Cross-plan gate:** Task 4's `showDialog` / `closeDialog` / `openMenu` AC
verifies only after F's modal-stack task ships; if E runs first, they land as
commands with a warn-once "no stack" sink and the AC closes when F lands.
Coordinate `descriptor.rs` / `tree.rs` edits with F (see
`drafts/M13--input-breadth/`).

## Rough sketch

- Evaluator + types: `render/ui/style_ranges.rs` (new), consumed from
  `tree.rs` draw-data build; effect state in `NodeContext`.
- Commands: `scripting/reactions/system_commands.rs` (new) —
  `SystemReactionCommand` enum + queue handle on `ScriptCtx`; drain call in
  the app tick after reaction dispatch.
- Detector: `scripting/state_crossings.rs` (new); crossing descriptors are
  parsed in `data_descriptors.rs` alongside `NamedReaction` from the widened
  setup-manifest return (`{ reactions, crossings }`).
- `screen.flash` registered like the `player.*` engine-owned slots; decay
  mechanism per Task 4 (the App-side flash-decay state).

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau |
|---|---|---|---|---|
| style ranges field | `style_ranges: Option<StyleRanges>` | `"styleRanges"` | `styleRanges` | `styleRanges` |
| range entry bound | `up_to` | `"upTo"` | `upTo` | `upTo` |
| pulse effect | `Pulse { period_ms }` | `"pulse": { "periodMs" }` | `pulse.periodMs` | `pulse.periodMs` |
| flash effect | `Flash { duration_ms }` | `"flash": { "durationMs" }` | `flash.durationMs` | `flash.durationMs` |
| crossing primitive | `CrossingDescriptor` (DataRegistry) | manifest `"crossings"`: `{ "slot", "below"\|"above", "max"?, "fire": [names] }` | `onStateCrossing` | `onStateCrossing` |
| crossing condition | `CrossingCondition::{Below, Above}` | n/a (flattened into the crossings manifest entry above) | `{ below, max? }` / `{ above, max? }` (SDK arg shape) | same |
| styleRanges max | `StyleRanges { max }` | `"max"` | `max` | `max` |
| styleRanges entries | `StyleRanges { entries }` | `"entries"` | `entries` | `entries` |
| helpers | command variants | reaction args camelCase | `flashScreen` `playSound` `rumble` `showDialog` `closeDialog` `openMenu` | same |
| flashScreen args | — | `{ "color", "durationMs" }` | `flashScreen { color, durationMs }` | same |
| playSound args | — | `{ "sound", "bus"? }` | `playSound { sound, bus? }` | same |
| rumble args | — | `{ "strong", "weak"?, "durationMs" }` | `rumble { strong, weak?, durationMs }` | same |
| showDialog/openMenu args | — | `{ "tree", "onCommit"? }` / `{ "tree" }` | `showDialog { tree, onCommit? }` / `openMenu { tree }` | same |
| flash slot | n/a | n/a | `screen.flash` | `screen.flash` |

## Open questions

- gilrs 0.11 force-feedback support varies by platform/backend — Task 4
  verifies the exact `ff` API shape at implementation; the warn-once no-op is
  the floor, not a risk.
- Whether crossing `max` should default to the slot schema's range when one
  exists (nicety; default-1.0 ships either way).
