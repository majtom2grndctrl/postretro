# Production Pause Menu Through the UI SDK

> Prerequisite: `ready/production-gameplay-hud`.

## Goal

Replace the development JSON pause menu with a production pause menu authored
through the TypeScript UI SDK.

Use the menu as an end-to-end validation of interactive SDK UI: returned tree
registration, modal capture, focus navigation, button activation, reserved UI
actions, and retained rendering after the mod-init context drops.

Keep a minimal engine JSON fallback when no mod pause menu is registered.

## Scope

### In scope

- SDK-authored `pauseMenu` tree returned through `setupMod().uiTrees`.
- Capturing modal behavior with centered placement and explicit initial focus.
- Resume through an engine-reserved UI close action exposed by the SDK.
- Keyboard, pointer, and gamepad operation through the existing focus engine.
- Mod-tree shadowing of the engine fallback.
- Staged UI/theme replacement semantics from the HUD prerequisite.
- Removal of pause-menu-only demo state and controls.
- Deterministic CPU coverage of descriptor, registry, focus, activation, and
  modal-stack behavior.
- Manual runtime verification of the complete input, cursor, and render path.

### Out of scope

- Player settings and `settings.toml` mutation.
- Master-volume persistence.
- Quit-to-desktop UI.
- Nested pause submenus.
- Text entry from the pause menu.
- New widget kinds.
- Live mutation of an already-pushed menu instance.
- Freezing game simulation, animation clocks, particles, audio, or UI time.
- GPU framebuffer comparison.
- Window-system or GPU-backed automated tests.

## User Experience

- Escape or gamepad Start opens a centered pause menu.
- Opening the menu captures UI input, suppresses player controls, and releases
  the cursor. Game simulation continues.
- The menu shows a `PAUSED` heading and one `RESUME` button.
- Resume works through pointer click, keyboard confirm, and gamepad confirm.
- Escape, gamepad B, or Start closes the menu.
- Closing the menu restores player controls and cursor capture.
- A mod-authored `pauseMenu` shadows the engine fallback.
- Without mod registration, the engine fallback still opens and closes through
  Start, Escape, and gamepad B.

## Author Contract

Add `content/dev/scripts/pause-menu.ts`:

```ts
// Proposed design
import {
  Button,
  Text,
  Tree,
  VStack,
  CLOSE_DIALOG_ACTION,
} from "postretro";

export const PAUSE_MENU_TREE = "pauseMenu";

export function buildPauseMenu() {
  return Tree(
    {
      anchor: "center",
      offset: [0, 0],
      captureMode: "capture",
      initialFocus: "pauseResume",
      accessibleName: "Pause menu",
      role: "group",
    },
    VStack(
      {
        gap: "m",
        padding: "l",
        align: "stretch",
        focus: { policy: "linear", wrap: true },
        fill: "panel.default",
      },
      [
        Text({
          content: "PAUSED",
          font: "mono",
          color: "ok",
        }),
        Button({
          id: "pauseResume",
          label: "RESUME",
          onPress: CLOSE_DIALOG_ACTION,
        }),
      ],
    ),
  );
}
```

The mod entry returns the tree:

```ts
// Proposed design
const pauseMenu = buildPauseMenu();

return {
  name: "Development Content",
  entities,
  stores,
  uiTrees: [
    ...hud.uiTrees,
    { name: PAUSE_MENU_TREE, tree: pauseMenu, alwaysOn: false },
  ],
  theme,
};
```

`CLOSE_DIALOG_ACTION` is an SDK constant whose wire value is the reserved
`"ui.closeDialog"` action. Button activation intercepts that value App-side
and pops the active modal. It does not depend on a level data script or named
reaction registration.

The tree is plain returned data. Importing the module performs no registration
or FFI.

## Modal Contract

The registry name remains `pauseMenu`. Existing `nav.menu` handling continues
to call the engine push/pop path by that name.

The mod tree is pushed only. It is never `alwaysOn`.

The tree envelope owns:

- `captureMode: "capture"`;
- centered anchor and literal offset;
- `initialFocus: "pauseResume"`;
- tree-level accessible name.

The container owns linear wrapping focus. The button owns its stable id and
reserved UI action.

`nav.cancel` closes only the active `pauseMenu`. It must not pop the keyboard
or another dialog. `nav.menu` closes the pause menu when it is active and opens
it only when the modal stack is empty. When another modal is active,
`nav.menu` is ignored.

`CLOSE_DIALOG_ACTION` follows the existing `ui.commitTextEntry` precedent:
SDK-authored button data names a closed engine UI action that the App
intercepts before named-reaction dispatch. Add the TypeScript and Luau SDK
constant and a matching engine constant. Other `onPress` names keep the
existing named-reaction path.

The HUD prerequisite changes the registry to retain engine and mod tiers. A
mod `pauseMenu` shadows the engine fallback without deleting it.

## Fallback

Reduce `content/base/ui/pauseMenu.json` to a minimal capturing fallback:

- centered `PAUSED` text;
- short `PRESS ESC OR B TO RESUME` instruction;
- no button, reserved-action, or named-reaction dependency;
- no volume, input-mode, or text-entry controls.

The fallback relies on engine `nav.menu` and `nav.cancel` handling, so it
remains usable when no mod or level reaction manifest exists.

## Demo Cleanup

Remove pause-menu-only E13 demo plumbing:

- `content/dev/scripts/pause-menu-store.ts`;
- the `registerPauseMenuStore()` call;
- the mod-owned `audio.master` declaration;
- `AudioMasterConsumer` and its App wiring;
- pause-menu input-mode text;
- pause-menu text-entry readout and opener;
- the old `resumePauseMenu` demo reaction.

Do not remove:

- the engine-owned `input.mode` slot;
- the on-screen keyboard asset or text-entry runtime;
- generic text-edit reactions used by keyboard coverage;
- audio bus volume APIs;
- `PlayerOptions`;
- `screen.flash` or other HUD behavior.

Real master volume belongs in the future settings-menu path backed by
`PlayerOptions`, not a replacement mod-state demo slot.

## Staged Reload

Use the HUD prerequisite's staged replacement contract:

- successful current staged results replace the complete mod UI-tree tier and
  complete mod theme override;
- failed or stale results preserve the current registry and theme;
- omitting `pauseMenu` from a committed mod snapshot reveals the engine
  fallback;
- an already-pushed pause menu keeps its cloned descriptor until closed;
- reopening resolves the current registry entry.

The reserved close action is engine-owned and unaffected by staged mod-init.
Reload may change layout and theme without creating a cross-manifest action
dependency.

## Validation

Automated tests run without a window or GPU context. Split coverage across the
CPU seams the engine already exposes:

1. Bundle the development TypeScript mod entry, run mod init, and parse the
   returned `pauseMenu` envelope.
2. Drop the mod-init context and prove the retained descriptor still builds
   layout, draw data, and focus rectangles.
3. Register engine and mod trees in the tiered registry. Prove the mod tree
   shadows the fallback and staged omission reveals it.
4. Push the tree by name. Prove capture mode, active name, and initial focus
   metadata reach the modal/focus seams.
5. Feed keyboard/gamepad confirm and pointer click through focus-engine
   fixtures. Prove each resolves the Resume button's same
   `CLOSE_DIALOG_ACTION`.
6. Exercise the App-side reserved-action router as pure CPU logic. Prove
   `ui.closeDialog` pops the active modal and ordinary names retain named
   reaction dispatch.
7. Exercise the `nav.menu` policy: open on an empty stack, close an active
   pause menu, and ignore the action while another modal is active.

Assert:

- the returned mod tree shadows the fallback marker;
- the active tree captures input and selects `pauseResume`;
- keyboard/gamepad confirm and pointer click resolve the same reserved action;
- the reserved action pops the active pause menu;
- ordinary button action names still follow named-reaction dispatch;
- retained tree data remains usable after the mod-init context drops;
- staged omission reveals the fallback on the next open;
- a menu already open during staged replacement remains stable until reopened.

These tests do not claim to prove winit cursor capture, real device input,
frame-loop suppression of player controls, or GPU composition.

Manual debug launch verifies:

- Escape and gamepad Start open the menu from gameplay;
- keyboard, pointer, and gamepad activate Resume;
- Escape, gamepad B, and Start close the active pause menu;
- Start does not stack the pause menu over another modal;
- player controls are suppressed while the menu captures input;
- cursor release/recapture and pointer/focus mode switching behave correctly;
- final layout, focus ring, text, and panel render correctly;
- world simulation continues while the menu is open.

## Boundary Inventory

| Boundary | Producer | Consumer | Contract |
| --- | --- | --- | --- |
| `pauseMenu` | mod manifest or engine fallback | named UI registry | pushed-only capturing tree |
| `setupMod().uiTrees` | mod-init context | UI registry | returned owned tree envelopes |
| `ui.closeDialog` | SDK button descriptor | App activation router | reserved action that pops active modal |
| focus rectangles | UI draw-data build | app focus engine | stable node ids and device rects |
| UI intent queue | input phase | game-logic focus engine | consumed events arrive no earlier than N+1 |
| staged mod UI snapshot | successful current staged result | tiered UI registry | complete mod tree replacement |

## Acceptance Criteria

- [ ] Development pause menu is authored with public TypeScript UI SDK
  factories and returned as a pushed-only `pauseMenu` tree.
- [ ] Importing the pause-menu module performs no registration or FFI.
- [ ] The tree declares capture, centered placement, accessible name, initial
  focus, and linear wrapping navigation through public SDK props.
- [ ] Resume uses the SDK-exported `CLOSE_DIALOG_ACTION`. TypeScript and Luau
  emit the reserved `"ui.closeDialog"` wire name.
- [ ] The App intercepts `"ui.closeDialog"` before named-reaction dispatch and
  pops the active modal. Ordinary button names retain named-reaction behavior.
- [ ] Mod `pauseMenu` shadows the engine fallback without deleting it.
- [ ] Start or Escape opens the menu from gameplay. While `pauseMenu` is active,
  Start, Escape, gamepad B, or Resume closes it without popping unrelated
  dialogs.
- [ ] `nav.menu` opens only when the modal stack is empty, closes an active
  `pauseMenu`, and is ignored while another modal is active.
- [ ] CPU tests prove the returned descriptor survives mod-init context drop,
  builds layout/draw data/focus rectangles, shadows the fallback, resolves
  initial focus, and routes all three activation sources to the same reserved
  action.
- [ ] CPU tests prove reserved-action pop, ordinary named-action routing, staged
  fallback reveal, and pushed-instance stability. They make no window, GPU, or
  real-device-input claim.
- [ ] The fallback has no script reaction dependency and remains openable and
  closable without mod or level registrations.
- [ ] Successful staged replacement updates the menu on its next open.
  Omission reveals the fallback. Failed and stale results preserve the current
  registry.
- [ ] An already-pushed menu remains stable across staged replacement and uses
  the new descriptor after close and reopen.
- [ ] Pause-menu-only `audio.master`, input-mode, and text-entry demo plumbing
  is removed without removing generic audio, input-mode, keyboard, or
  text-entry systems.
- [ ] UI and scripting documentation shows the final returned tree plus
  reserved-action pattern and documents fallback, modal policy, simulation,
  and staged-instance behavior.
- [ ] Manual launch verifies player-control suppression, continued world
  simulation, final composition, focus ring, cursor release/recapture,
  non-stacking Start behavior, and keyboard/mouse/gamepad Resume.
- [ ] No tracked generated bundle, new `unsafe`, or renderer ownership
  violation is introduced.

## Tasks

### Task 1: Add the reserved close action and author the pause menu

Add the `ui.closeDialog` reserved button action beside the existing text-entry
commit sentinel. Export `CLOSE_DIALOG_ACTION` in TypeScript and Luau. Route it
App-side to modal pop before ordinary named-reaction dispatch.

Create the pure TypeScript pause-menu module and return its tree from
`setupMod()`. Use only public SDK factories and constants. Add SDK parity,
compiler, manifest, and activation-routing coverage.

### Task 2: Replace the fallback and remove demo plumbing

Reduce the engine JSON pause menu to the script-independent fallback. Remove
the mod-owned volume store, App-side demo consumer, input-mode display,
text-entry controls, and old resume demo reaction.

Keep generic keyboard, text-entry, input-mode, audio, and player-options
infrastructure intact. Update stale demo comments and focused fixtures.

### Task 3: Add interactive end-to-end coverage

Add deterministic CPU integration coverage for source bundle, mod manifest,
tiered registration, named push, retained layout, draw data, focus rectangles,
activation routing, modal pop, and staged replacement.

Cover keyboard/gamepad confirm and pointer click as equivalent activation
sources. Cover fallback-only operation, ordinary named-action preservation,
pushed-instance stability, and the complete `nav.menu` policy.

### Task 4: Document and verify

Update durable UI and scripting docs for SDK-authored pushed menus, split
reserved/ordinary button actions, fallback behavior, input capture versus true
pause, modal stacking policy, and pushed-instance reload semantics.

Run formatting, focused CPU tests, workspace clippy, and workspace tests.
Perform normal and fallback debug-launch checklists for behavior that requires
a window, devices, or GPU output.

## Sequencing

**Phase 1 (sequential):** Task 1 establishes the authored tree and reserved
action contract.

**Phase 2 (sequential):** Task 2 replaces demo content and establishes the
fallback behavior Task 3 validates.

**Phase 3 (sequential):** Task 3 validates the complete interactive path.

**Phase 4 (sequential):** Task 4 documents and manually verifies the completed
path.

## Open Questions

None.
