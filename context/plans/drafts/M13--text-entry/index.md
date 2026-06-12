# M13 — Text Entry (On-Screen Keyboard + Hardware Keys)

## Goal

Players can enter text. Desktop players type on the hardware keyboard; gamepad
players get an on-screen keyboard as the accessibility accommodation — built
entirely from the wave's own primitives (F's grid/spatial focus/buttons/modal
stack, E's reactions, the store's string slots). Both paths drive one
engine-side text-edit surface, so the integration consumer for the E ‖ F wave
(the role the splash played for Goal A) also ships real text input, with only
IME composition deferred.

**Depends on:** E (system-reaction command path, `showDialog`/`closeDialog`)
and F (modal stack, focus, `button`, hold-to-repeat machinery, `setState`
write path). Draftable now; orchestrate after both.

## Scope

### In scope

- Text-edit reaction primitives over a writable string slot: `appendText`,
  `backspaceText`, `clearText` — engine-applied edits via the `setState`
  write path (no live VM, no string ops in script at fire time).
- Hardware-keyboard routing: while a text-entry modal is open, winit key text
  events append to the target slot through the same text-edit surface;
  Backspace deletes, Enter commits, Escape cancels. No IME composition —
  plain character input only.
- Per-button `repeatOnHold` opt-in (descriptor flag) reusing F's repeat
  timer, so held backspace repeats — F ships nav-only repeat; this is the
  one activation-repeat exception, opt-in per button.
- Engine-shipped keyboard descriptor: a JSON descriptor asset registered as a
  named tree (`keyboard`) — lowercase letters, digits, space, backspace,
  done — `grid` + `focus: "spatial"`, every key a `button` firing
  `appendText`/`backspaceText`; `done` fires `closeDialog` plus a commit
  reaction.
- Target-slot convention: text entry edits one writable string slot
  (`ui.textEntry`); openers copy in/out around `showDialog`/commit.
- Demo: a screen with a `text` widget bound to a string slot and a button
  that opens text entry; works typed on hardware keys and via gamepad on the
  on-screen keyboard.
- SDK + typedefs + docs for the new reactions and the open-text-entry
  pattern.

### Out of scope

- IME composition (CJK, dead keys beyond what winit text events already
  resolve) — deferred until a localization consumer exists.
- Caret/selection editing, an `input` widget with cursor rendering
  (append/backspace only; the bound `text` widget is the display).
- Shift / uppercase / symbol layers on the on-screen keyboard (single layer
  v1; layers need per-key label rebinding that is G1-component-shaped).
  Hardware keys are not layer-limited — shifted characters arrive as winit
  text.
- Localized on-screen layouts (QWERTY only; descriptor asset is
  mod-replaceable by construction).
- Masking/password fields, input validation.

## Acceptance criteria

- [ ] With a gamepad only: open the demo screen, open text entry, navigate
  keys spatially, type a word, press done — the bound text field shows the
  entered string; cancel (nav.cancel) closes without committing.
- [ ] With a hardware keyboard: open text entry, type (including shifted
  characters), Backspace deletes, Enter commits, Escape cancels — same
  observable result as the gamepad path; keystrokes never leak to game logic
  while the modal is open.
- [ ] `appendText` appends its string arg to the target slot;
  `backspaceText` removes the last char (no-op + no warning on empty);
  `clearText` empties it; all reject readonly slots with the `setState`
  warning; edits respect the N→N+1 frame contract.
- [ ] Holding on-screen backspace repeats at the declared rate via
  `repeatOnHold`; holding a letter key with the flag absent fires once.
  (Hardware-key repeat comes from the OS key-repeat events.)
- [ ] Unicode-safe: backspace removes one extended grapheme cluster (floor:
  one `char`, never splits a UTF-8 sequence — pin at implementation, test
  with a multi-byte string).
- [ ] The on-screen keyboard ships as a descriptor JSON asset that
  deserializes through the standard wire path — deleting one key from the
  JSON and reloading changes the keyboard with no Rust change.
- [ ] Text entry is a capturing modal tree: gameplay input freezes while
  open; the opener screen's focus restores on close.
- [ ] Typedefs + both-runtime SDK constructors for the three reactions;
  `docs/scripting-reference.md` shows the text-entry pattern end-to-end.

## Tasks

### Task 1: text-edit reactions

`appendText { slot, text }`, `backspaceText { slot }`, `clearText { slot }`
as system-reaction primitives on E's command path, applied at the game-logic
stage through the same writable-slot gate as `setState` (readonly → warn,
no-op). Grapheme-aware backspace. Both runtimes + typedefs.

### Task 2: `repeatOnHold` button flag

Additive `repeatOnHold: Option<RepeatPolicy>` on `button` (same wire shape as
F's container `repeat`); held confirm on a flagged focused button re-fires
`onPress` on F's repeat timer; absent flag keeps F's single-fire rule.
Round-trip-stable.

### Task 3: hardware-key routing

While the top stack tree declares an active text-entry target (descriptor
flag naming the slot), the input stage converts winit key text events into
Task 1 edit commands against that slot and maps Backspace/Enter/Escape to
backspace/commit/cancel; all other UI dispatch is unchanged. Keystrokes are
consumed (never forwarded to game logic) per the capture contract. No IME.

### Task 4: keyboard asset + demo + docs

The `keyboard` descriptor JSON asset (registered named tree), the
`ui.textEntry` slot convention, the demo screen (bound text field + open
button + commit reaction) exercising both input paths, SDK pattern helpers,
docs.

## Sequencing

**Phase 1 (concurrent):** Task 1, Task 2 — disjoint (scripting vs.
descriptor/focus).
**Phase 2 (sequential):** Task 3 — consumes Task 1's edit commands.
**Phase 3 (sequential):** Task 4 — consumes all three.
**Cross-plan:** entire plan runs after E Task 2 and F Tasks 2–4 (stack,
focus, button, setState). Run as the wave's trailing phase or the next
orchestrate immediately after.

## Rough sketch

- Reactions live beside `setState` in the system-command set; string edit
  applies where slot writes apply.
- Hardware routing in the input stage beside `UiDispatch` — winit
  `KeyEvent.text` is the character source (handles shift/dead-key resolution
  without IME).
- `repeatOnHold` rides F's repeat timer in the focus engine — no second
  timer.
- Asset under the engine content tree next to other engine-shipped
  descriptors (location per where C/F put registered trees); loaded/registered
  at boot with the built-in trees.

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau |
|---|---|---|---|---|
| append | `SystemReactionCommand::AppendText` | reaction `"appendText"` | `appendText` | `appendText` |
| backspace | `…::BackspaceText` | `"backspaceText"` | `backspaceText` | `backspaceText` |
| clear | `…::ClearText` | `"clearText"` | `clearText` | `clearText` |
| repeat flag | `repeat_on_hold` | `"repeatOnHold": { "initialDelayMs", "intervalMs" }` | `repeatOnHold` | `repeatOnHold` |
| text-entry target | `text_entry_target` (tree-level) | `"textEntryTarget": "<slot>"` | `textEntryTarget` | `textEntryTarget` |
| entry slot | n/a | n/a | `ui.textEntry` | `ui.textEntry` |

## Open questions

- Grapheme segmentation: add the `unicode-segmentation` crate vs. `char`-pop
  floor — decide at implementation against dependency policy; AC accepts the
  floor.
- Whether `done`'s commit reaction name is a fixed convention or a
  `showDialog` arg — lean: arg on the opener (`showDialog { tree, onCommit }`
  is additive to E's shape); confirm when wiring the demo.
- Whether the on-screen keyboard auto-opens on focus-mode activation of a
  text-entry opener vs. always explicit — ship explicit; auto-open is a BIS
  polish question.
