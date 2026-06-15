# Production Pause Menu Through the UI SDK

> Prerequisites: `drafts/game-state-sdk-surface` and
> `drafts/production-gameplay-hud`.

## Goal

Replace the development JSON pause menu with a production pause menu authored
through the TypeScript UI SDK.

Use the menu as an end-to-end validation of interactive SDK UI: returned tree
registration, modal capture, focus navigation, button activation, named
reaction dispatch, and retained rendering after the mod-init context drops.

Keep a minimal engine JSON fallback when no mod pause menu is registered.

## Scope

### In scope

- SDK-authored `pauseMenu` tree returned through `setupMod().uiTrees`.
- Capturing modal behavior with centered placement and explicit initial focus.
- Resume through a named reaction returned by `setupLevel()`.
- Keyboard, pointer, and gamepad operation through the existing focus engine.
- Mod-tree shadowing of the engine fallback.
- Staged UI/theme replacement semantics from the HUD prerequisite.
- Removal of pause-menu-only demo state and controls.
- Headless validation through retained draw data and the real input/reaction
  path.

### Out of scope

- Player settings and `settings.toml` mutation.
- Master-volume persistence.
- Quit-to-desktop UI.
- Nested pause submenus.
- Text entry from the pause menu.
- New widget kinds.
- Live mutation of an already-pushed menu instance.
- GPU framebuffer comparison.

## User Experience

- Escape or gamepad Start opens a centered pause menu.
- Opening the menu freezes gameplay and releases the cursor.
- The menu shows a `PAUSED` heading and one `RESUME` button.
- Resume works through pointer click, keyboard confirm, and gamepad confirm.
- Escape, gamepad B, or Start closes the menu.
- Closing the menu restores gameplay input and cursor capture.
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
  closeDialog,
  defineReaction,
} from "postretro";

export const PAUSE_MENU_TREE = "pauseMenu";
export const RESUME_PAUSE_MENU = "pauseMenu.resume";

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
          onPress: RESUME_PAUSE_MENU,
        }),
      ],
    ),
  );
}

export function pauseMenuReactions() {
  return [
    defineReaction(RESUME_PAUSE_MENU, closeDialog()),
  ];
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

The level data entry returns `pauseMenuReactions()` with its other reactions.
The tree and reaction are reconstructed independently in short-lived contexts.
Their shared contract is the stable `pauseMenu.resume` name, not a retained
script object or function.

Every engine-bound value remains reachable from a setup return. Importing the
module performs no registration or FFI.

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
reaction name.

`nav.cancel` closes only the active `pauseMenu`. It must not pop the keyboard
or another dialog. `nav.menu` closes the pause menu when it is active and opens
it from gameplay.

The HUD prerequisite changes the registry to retain engine and mod tiers. A
mod `pauseMenu` shadows the engine fallback without deleting it.

## Fallback

Reduce `content/base/ui/pauseMenu.json` to a minimal capturing fallback:

- centered `PAUSED` text;
- short `PRESS ESC OR B TO RESUME` instruction;
- no button or named reaction dependency;
- no volume, input-mode, or text-entry controls.

The fallback relies on engine `nav.menu` and `nav.cancel` handling, so it
remains usable when no mod or level reaction manifest exists.

## Demo Cleanup

Remove pause-menu-only M13 demo plumbing:

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

The reaction name stays `pauseMenu.resume`. Staged mod-init reload may change
layout and theme, but does not rename the per-level reaction contract.

## End-to-End Validation

Add a headless regression using the production script-loading path:

1. Bundle the development TypeScript mod entry.
2. Run mod init and retain the returned pause tree.
3. Run the level data script and retain the returned resume reaction.
4. Drop both short-lived authoring contexts.
5. Register the engine fallback and returned mod tree.
6. Push `pauseMenu` through the same named engine path used by `nav.menu`.
7. Build retained layout, draw data, and focus rectangles.
8. Advance the UI dispatch queue through its N-to-N+1 boundary.
9. Activate Resume through focus confirm.
10. Dispatch `pauseMenu.resume` and drain its `closeDialog` command.

Assert:

- the returned mod tree shadows the fallback marker;
- the active tree captures input and selects `pauseResume`;
- gameplay input is frozen and menu focus releases the cursor;
- keyboard/gamepad confirm and pointer click reach the same named reaction;
- activation does not pop until the queued reaction reaches game logic;
- `closeDialog` pops the pause menu;
- gameplay focus and cursor capture return after reconciliation;
- retained tree and reaction data remain usable after both contexts drop;
- staged omission reveals the fallback on the next open;
- a menu already open during staged replacement remains stable until reopened.

Use CPU layout, focus, registry, command, and draw-data assertions. Manual
launch verifies final GPU composition and OS cursor behavior.

## Boundary Inventory

| Boundary | Producer | Consumer | Contract |
| --- | --- | --- | --- |
| `pauseMenu` | mod manifest or engine fallback | named UI registry | pushed-only capturing tree |
| `pauseMenu.resume` | level data manifest | button activation | named reaction containing `closeDialog` |
| `setupMod().uiTrees` | mod-init context | UI registry | returned owned tree envelopes |
| `setupLevel().reactions` | data context | per-level reaction registry | returned named reactions |
| focus rectangles | UI draw-data build | app focus engine | stable node ids and device rects |
| UI intent queue | input phase | game-logic focus engine | consumed events arrive no earlier than N+1 |
| `PopTree` | `closeDialog` system reaction | modal stack | pop active tree during game logic |
| staged mod UI snapshot | successful current staged result | tiered UI registry | complete mod tree replacement |

## Acceptance Criteria

- [ ] Development pause menu is authored with public TypeScript UI SDK
  factories and returned as a pushed-only `pauseMenu` tree.
- [ ] Importing the pause-menu module performs no registration or FFI.
- [ ] The tree declares capture, centered placement, accessible name, initial
  focus, and linear wrapping navigation through public SDK props.
- [ ] Resume is a stable named reaction returned by `setupLevel()` and built
  from `closeDialog()`.
- [ ] Mod `pauseMenu` shadows the engine fallback without deleting it.
- [ ] Start or Escape opens the menu from gameplay. While `pauseMenu` is active,
  Start, Escape, gamepad B, or Resume closes it without popping unrelated
  dialogs.
- [ ] Opening freezes gameplay and releases the cursor; closing restores both
  after the game-logic reconciliation point.
- [ ] Pointer click, keyboard confirm, and gamepad confirm on Resume produce the
  same named-reaction effect through the N-to-N+1 UI dispatch contract.
- [ ] Headless coverage reaches retained layout, focus rectangles, draw data,
  named reaction dispatch, system-command drain, and modal pop after both
  authoring contexts drop.
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
  returned reaction pattern and documents fallback and staged-instance
  behavior.
- [ ] Manual launch verifies final composition, focus ring, cursor release, and
  keyboard/mouse/gamepad Resume behavior.
- [ ] No tracked generated bundle, new `unsafe`, or renderer ownership
  violation is introduced.

## Tasks

### Task 1: Author the pause menu and reaction module

Create the pure TypeScript module with stable tree and reaction names. Return
the tree from `setupMod()` and the reaction from `setupLevel()`.

Use only public SDK factories and reaction builders. Add compiler and manifest
coverage proving both returned wire values parse through the production path.

### Task 2: Replace the fallback and remove demo plumbing

Reduce the engine JSON pause menu to the script-independent fallback. Remove
the mod-owned volume store, App-side demo consumer, input-mode display,
text-entry controls, and old resume demo reaction.

Keep generic keyboard, text-entry, input-mode, audio, and player-options
infrastructure intact. Update stale demo comments and focused fixtures.

### Task 3: Add interactive end-to-end coverage

Exercise source bundle, mod manifest, level manifest, tiered registration,
named push, retained layout, focus, input latency, reaction dispatch, command
drain, modal pop, and focus reconciliation.

Cover keyboard/gamepad confirm and pointer click as equivalent activation
sources. Cover fallback-only operation and staged replacement behavior.

### Task 4: Document and verify

Update durable UI and scripting docs for SDK-authored pushed menus, split
tree/reaction publication, fallback behavior, and pushed-instance reload
semantics.

Run formatting, focused tests, workspace clippy, workspace tests, normal debug
launch, and fallback debug launch.

## Sequencing

**Phase 1 (sequential):** Task 1 establishes the authored tree and reaction
contract.

**Phase 2 (sequential):** Task 2 replaces demo content and establishes the
fallback behavior Task 3 validates.

**Phase 3 (sequential):** Task 3 validates the complete interactive path.

**Phase 4 (sequential):** Task 4 documents and manually verifies the completed
path.

## Open Questions

None.
