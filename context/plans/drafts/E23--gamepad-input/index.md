# E23--gamepad-input

Brief · resumable · Epic 23 (U3) · reads: `context/lib/input.md` §2, §5, §7 · `player_options.md` §2, §4–§6 · `ui.md` §4, §5 · `networking.md` §What gates, and what replicates instead · read at 2a4bd9eb3 (source at af1d3c1b5)

## Problem
A requested capability: epic U3 (D8), widened by the owner to add a game-author binding layer. Players cannot remap anything. Gamepad menus also miss console conventions:
- focus is not restored on return
- nothing repeats unless the author sets it up
- nested groups trap focus
- there is no scrolling
- there are no device-aware button prompts
- destructive exits are not confirmed

Game authors cannot choose their game's commands or defaults. Every game gets the bindings in `input/defaults.rs`, so a game without dash still has F bound to a dead command, and once rebinding exists that dead binding blocks F for anything else. One key cannot drive both a tap command and a hold command.

When this is done:
- Authors pick their game's commands and its default keyboard/mouse and gamepad bindings, each with an activator.
- Relevance is derived from the mod's data, and authors can override it.
- Players rebind keys, UI navigation included, and the result is saved as a diff over the author's defaults for that mod.
- Gamepad menus follow console conventions, and button prompts follow both the device and the bindings.

## Decisions

**Prior commitments this brief diverges from.** Each is argued in its bullet below.
- `player_options.md` §5 puts remapped bindings in the `[accessibility]` group.
- `player_options.md` §6 says mods do not extend the action set. Authors still cannot add commands, but they now pick commands and set defaults.
- `done/M13--input-breadth` binds LB/RB to Next/Prev and makes `restoreOnReturn` and repeat opt-in (epic Prior commitments already argue the latter two).
- The epic's "persisted `[bindings]`" table.

### Commands and relevance
- **Commands are stable snake_case IDs.** They map one-to-one to the closed `Action` set. Engine IDs never contain `.`. Later mod-defined commands take the form `<mod_id>.<name>` and are split at the last `.`, because mod ids contain dots (`postretro.dev`). Saved data and the manifest never use enum indices. The command set stays engine-closed (epic §Out of scope), so mod-defined commands are a non-goal and only their ID space is reserved.
- **Axis actions split by input kind.** Each axis action exposes one digital command per direction (`move_forward`, `move_back`) and one analog command for axis sources (`look_x`). A digital command accepts keys and buttons; an analog command accepts axes only. A stick swap is therefore an ordinary rebind.
- **Two command sets: gameplay and UI nav.** Conflicts are checked only within a set, so South as both `jump` and `nav_confirm` stays legal. Nav commands are always relevant and cannot be hidden. Layer placement: UI dispatch still runs first (`input.md` §7) and resolves nav intents from the nav set's effective bindings.
- **Relevance is derived at mod init by an exhaustive match over the commands.** A new command fails to compile until someone classifies it, the denylist recipe of `networking.md` §What gates. A "default to relevant" rule would fail open.
  - `dash` and `crouch` are relevant if any movement descriptor in the registry has them.
  - `reload` is relevant if any weapon's resource is the magazine kind.
  - `alt_fire` is relevant if any weapon declares a `secondary` activation.
  - `move_up` is never relevant (fly-cam only).
  - Every other command is classified relevant by name. Per-level relevance is a non-goal: use, drop, and slot selection depend on level content.
  - Relevance is recomputed when entity descriptors hot-reload.
- **In co-op, relevance follows the host.** While participating, relevance is the union of local derivation and the installed host tuning, recomputed when tuning installs. Tuning sites keep no local fallback (`networking.md`), so a client whose registry lacks dash would otherwise leave the host's live dash unbound.
- **An author override beats derivation.** The `input` block can force a command shown or hidden. An irrelevant command is unbound, absent from the rebind panel, never part of a conflict, and draws no glyph.

### Bindings and activators
- **Activators live on bindings.** The kinds are `press` (the default), `release`, `tap` (a max time), and `hold` (a min time). The field is named `activator` (owner) so it doesn't clash with the weapon descriptor's `trigger`. Authors set activators. **Players rebind keys only** (owner). A rebound key inherits the activator of its slot; a slot past the end of the author's list takes `press`.
- **Each command accepts a fixed set of activator kinds.**
  - `shoot` and `alt_fire` accept `press` only, because charge requires `press` (`entity_model.md` §Weapon activations).
  - `sprint` and `crouch` accept `press` or `hold`.
  - Analog and wheel-notch commands accept `press`.
  - Every other command accepts any kind.
- **Shared keys follow the Steam rule.** Within a set, one key may carry at most one short binding (`press` or `tap`) plus one `hold`; any other combination is a conflict.
  - On a shared key, neither binding fires until the key is released before the threshold (the short binding fires) or the threshold passes (the hold fires and the short binding never does).
  - A press-only command never joins a shared key.
  - A `tap` with no hold partner on its key fires on the press edge, so a single-binding key never adds delay.
  - If focus is lost or the UI captures input mid-resolution, the pending resolution is cancelled and neither binding fires.
  - Threshold edge: release at exactly the threshold counts as the tap.
- **Activators emit phases, not one-shots.** A resolution produces the Pressed → Held → Released sequence the snapshot already carries, through the existing render-rate latch (`input.md` §2). Edge commands read Pressed and sprint reads the held state. An edge resolved on release still survives a frame with zero ticks. `crouch_mode` and the new `sprint_mode` apply on top: the activator decides when a command goes down and up, and the mode decides whether that latches.
- **Thresholds are author-set and player-scaled.** Each binding's threshold is set by the author; the engine default is 0.2 s for both tap and hold. One `hold_timing_scale` field in the `[accessibility]` group scales every threshold. Being in the group, it gets the U1 obligations: an `accessibility.*` slot, a panel entry, and epic AC 2, 3a–3c, and 10.
- **The guard covers `nav_confirm`, `nav_cancel`, and `nav_menu`.** Each keeps at least one binding per device class. The rebind panel refuses a change that breaks this, and author defaults that break it are diagnosed. Epic AC 21 names only confirm and cancel; `nav_menu` is added because it is the only route to the pause menu and its exits.

### Author surface
- **An optional `input` block on `ModManifest`, in both SDKs.**
  - **Per command:** a label, category, order, `show` override, and default bindings per device class (`keyboardMouse`, `gamepad`). Each binding gives an input, an activator, and an optional threshold.
  - **Fallbacks:** a device class that is absent keeps the engine default, and an empty list means unbound. With no block at all, the engine default table applies, so the dev mod is unchanged until it opts in.
  - **Glyph art:** the block also names glyph art per device family.
- **Validation at mod init degrades per command.** Precedent: a malformed optional manifest block warns and falls back. Each of these gets a diagnostic, and that command's defaults for that device class fall back to the engine default while the rest of the block applies:
  - an unknown command ID
  - an unknown input string
  - an activator the command doesn't accept
  - a conflict within a set
  - a guard violation

  Hot reload re-validates the block and recomputes effective bindings.

### Player data
- **Bindings are saved as a diff, scoped by mod id.** Rows live at `[game."<mod_id>".bindings.<device_class>]`, keyed by command ID, each a list of input strings. Rows store keys only, never activators.
  - **Not in `[accessibility]`:** this diverges from `player_options.md` §5. A binding is a diff over one mod's defaults, and it has no resolved value to put in an `accessibility.*` slot.
  - **Keyed by mod id:** each game already has its own directory (`done/game-user-dirs`), but bare and xtask runs share the `postretro` directory. The key matches `state_persistence::state_path`.
  - **Row rules:**
    - A missing row follows the author default, so a later change to a default reaches players who never rebound that command.
    - An empty list means unbound.
    - A row naming an unknown command is kept on disk and ignored.
    - An unknown input string drops that entry alone (I7).
  - **The reader:** U3 builds the `[game."<mod_id>"]` reader, read at mod init once the id is known. Later game-scoped settings join the same section.
- **On a collision with a changed default, the player wins** (owner). If a player's binding and a later author default land on the same key with the same activator, the player's binding keeps the key. The author default is suppressed on that key, and the rebind panel flags the displaced command as needing a key. Nothing the player chose changes silently.
- **Inputs are named by physical position.**
  - **Keyboard:** W3C `KeyboardEvent.code` names (`KeyW`, `ShiftLeft`), which winit `KeyCode` mirrors.
  - **Mouse:** `mouse_left`, `mouse_right`, `mouse_middle`, `mouse_back`, `mouse_forward`, `wheel_up`, `wheel_down`, `mouse_x`, `mouse_y`.
  - **Gamepad:** named by position, e.g. `south`, `left_shoulder`, `left_trigger`, `left_stick_press`, `left_stick_x`.
  - Labels and glyphs are resolved at display time.
- **Effective binding** is resolved per (command, device class): player override, else author default, else engine default. The confirm/cancel swap is then applied to the gamepad nav bindings. The binding table is rebuilt at mod init, on hot reload, on rebind, and when tuning installs, and the rebuild keeps input state and preferences.
- **Settings scope.** Top-level `settings.toml` keys are per app, because each app has its own directory. There is no `[machine]` section and no migration. Game-scoped data lives under `[game."<mod_id>"]`, with bindings as the first content. New fields:
  - `gamepad_look_sensitivity`, `gamepad_look_dead_zone` (look stick only; the move stick keeps its dead zone), and `gamepad_invert_y` are top-level, beside `mouse_sensitivity` and `invert_y`.
  - `sprint_mode` is top-level, beside `crouch_mode`.
  - `swap_confirm_cancel` is top-level.

  Only `hold_timing_scale` joins the `[accessibility]` group. The gamepad look fields are tuning preferences with no OS value; putting them in the group would add panel entries and slots that serve nothing.

### Remapping UI
- **An engine-owned controls panel.** It is a `core/ui/` tree generated from the effective command list, opened by the reserved action `ui.openControls`. Its registry name is reserved the same way the accessibility panel's is (`ui.md` §1.1), and it uses the mod's theme tokens.
  - **Why the engine owns it:** mods cannot author rows over a command list that is computed at mod init, and owning the panel keeps the guard and conflict rules out of mod scripts.
  - **Contents:** relevant commands grouped by author category and ordered by the author's order, with each binding's activator shown read-only.
  - **Actions:** per-command reset and reset-all.
  - **Conflicts** are reported before they apply. The player either replaces the binding (unbinding the other command) or cancels.
- **Raw capture.** The capture prompt receives the next key, button, or axis, including inputs the UI otherwise swallows: Escape, Start, Select/Back.
  - The capture is decided App-side after that frame's activations, and only while the prompt is the active tree (epic `research.md` §Brief pins, raw-capture decision stage).
  - The prompt has no time limit, and every input can be captured.

### Menu conventions (epic §U3; AC 19, 20, 23–25)
- **Restore on return** is on by default. It applies only on a return, meaning a pop that reveals the tree again. A fresh push lands on initial focus (O14). `restoreOnReturn: false` opts a tree out.
- **Engine-default hold-to-repeat** applies where a container authors none. An authored repeat wins, and an authored zero delay still means no repeat.
- **A held slider step repeats and accelerates** with hold time, then clamps at its bounds without overshoot. If an external write to the slot lands during a hold, the next step continues from the new value.
- **Nested groups.** When a directional move finds no target in the current group, it continues in the enclosing group, which treats each nested group as one candidate by its bounds. Entering a group lands on its last-focused member, else its initial focus. Linear Next/Prev stay within the group, and `focusNeighbors` still overrides. I5 holds: the new containers admit only interactive widgets.
- **Tabs are an authored pattern on existing roles.** A tab strip is a container with `role: "tablist"` whose `role: "tab"` buttons carry `selected`.
  - `nav_tab_next` and `nav_tab_prev` (LB/RB) activate the adjacent tab in the top tree's tablist, wrapping, and move focus to it.
  - In a tree with no tablist they keep today's Next/Prev behavior, so existing trees are unchanged. This is the divergence from `done/M13--input-breadth`, argued here: the bumpers are console tab keys.
- **A new `Scroll` widget** is a vertical container with a fixed viewport height that clips its children. It is neither a focus stop nor a focus group. Focusing a child outside the viewport scrolls it into view by the minimum distance, and the pointer wheel scrolls it.
- **Device family follows the last input.** The families are `keyboardMouse`, `xbox`, `playstation`, and `nintendo`; a gamepad's family comes from its vendor id, and unknown vendors are `xbox`. A readonly slot `input.deviceFamily` exposes it.
  - **Glyph widget.** `Glyph({ command })` draws the glyph for the command's first effective binding on the current family, using the mod's glyph art.
  - **Fallback:** missing art draws the input's name as text.
  - Glyphs follow rebinding and the swap with no extra authoring.
- **The confirm/cancel swap** exchanges the gamepad bindings of `nav_confirm` and `nav_cancel`. Glyphs follow because they read effective bindings. A swap chosen automatically by device family is a non-goal: the player chooses.
- **A confirmation dialog is an authored pattern, not a new primitive.** It is a dialog tree with `initialFocus` on the safe choice, pushed by `showDialog`; the precedent is `core/ui/displayModeConfirm.json`. AC 19's fresh-push rule makes a reopened dialog land on the safe choice. The dev EXIT and QUIT actions each gain one.
- **On-screen keyboard shortcuts.** The UI commands `text_backspace`, `text_space`, and `text_commit` are live while a text-entry tree is on top. Each activates its key button without moving focus to it.

### Scripting surface
```ts
export default defineMod({
  id: "acme.neon",
  input: {
    commands: {
      dash: {
        label: "Dash", category: "Movement", order: 30,
        keyboardMouse: [{ input: "ShiftLeft", activator: "tap", threshold: 0.2 }],
        gamepad: [{ input: "left_stick_press" }],          // activator defaults to "press"
      },
      sprint: { keyboardMouse: [{ input: "ShiftLeft", activator: "hold" }], gamepad: [] },
      alt_fire: { show: false },                              // force-hide a derived-relevant command
    },
    glyphs: { keyboardMouse: "ui/glyphs/kbm", xbox: "ui/glyphs/xbox",
              playstation: "ui/glyphs/ps", nintendo: "ui/glyphs/nx" },  // asset = <dir>/<input>
  },
  // ...
});

HStack({ gap: 8 }, [Glyph({ command: "nav_confirm" }), Text({ content: "SELECT" })]);
Scroll({ height: 320 }, levelButtons);
Button({ id: "controls", label: "CONTROLS", onPress: OPEN_CONTROLS_ACTION });   // "ui.openControls"
```
The Luau mirror ships with the same names (Boundary inventory).

### Non-goals
- **Mod-defined commands.** Their ID form is reserved.
- **Chord and modifier bindings, and double-tap.** The activator set stays open to them.
- **Players choosing activator kinds** (owner).
- **A press-then-hold activator** (owner chose the Steam rule).
- **Per-level relevance.**
- **Steam Input API integration** (`input.md` §9).
- **Migrating saved bindings.** None exist today.

## Acceptance
Epic AC 19–25, as amended with this brief, are the unit's rows, together with AC 2, 3a–3c, and 10 for `hold_timing_scale`. AC 33 applies if U3 lands after U4. The rows below add what the epic's rows do not pin.

### Automated
**Author layer**
- [ ] With no `input` block, effective bindings equal today's defaults, and the dev mod's command reads are unchanged.
- [ ] An author binding of `ShiftLeft` to dash, with no default for sprint: Shift drives dash, F drives nothing, and the run state stays false.
- [ ] Every activator a command accepts is accepted, and every other is diagnosed. Pinned pairs: `tap` on shoot is diagnosed and `press` on shoot is accepted; `hold` on sprint is accepted and `tap` on sprint is diagnosed. A diagnosed entry falls back to the engine default for that command and device class only.
- [ ] Author defaults that leave confirm, cancel, or menu unbound on a device class are diagnosed and fall back. Defaults that keep each one bound are accepted.
- [ ] Hot-reloading the block recomputes effective bindings and keeps player overrides.

**Relevance**
- [ ] A mod with no dash descriptor: dash is unbound, missing from the controls panel, and does not conflict with an author binding on F. A mod where one of two descriptors has dash: dash is relevant.
- [ ] alt_fire is relevant for a mod with a weapon that declares a secondary activation, and irrelevant for a mod with none.
- [ ] Force-show and force-hide each flip the derived answer.
- [ ] A co-op client whose local registry lacks dash gets dash relevant and bound once host tuning that has dash installs.
- [ ] Adding a command without classifying its relevance fails to compile. Review gate: no wildcard arm in the derivation.

**Player data**
- [ ] A command the player never rebound follows a changed author default after reload; a rebound command keeps the player's key.
- [ ] An empty-list row stays unbound across save and load.
- [ ] An unknown command row survives a save byte-identical.
- [ ] An unknown input string drops that entry alone, and every other binding and setting loads.
- [ ] A player rebinding of Q to dash, followed by an author default of Q for reload: Q drives dash only, and reload is flagged in the panel. Without the player row, Q drives reload.
- [ ] Two mod ids in one shared settings file keep separate rows, and A's overrides do not apply while B is loaded.
- [ ] A rebound key keeps its slot's author activator.

**Activators**
- [ ] Tap and hold on one key:
  - released before the threshold, the tap fires once and the hold never fires;
  - held past the threshold, the hold fires and the tap never fires;
  - released at exactly the threshold, the tap fires;
  - released and pressed again within one frame, both cycles resolve;
  - the tap still reaches the simulation on a frame with zero ticks;
  - losing focus or opening a capturing menu mid-hold means neither fires.
- [ ] A dash key with a single `press` or unpartnered `tap` binding fires on the press frame.
- [ ] Hold-Shift sprint shows a held state every frame past the threshold and releases on key-up. In `sprint_mode` toggle, it latches on the hold resolution and releases on the next one.
- [ ] Raising `hold_timing_scale` moves both thresholds.
- [ ] Conflicts: South bound to both jump and confirm is not a conflict. Two gameplay commands on one key with the same activator are a conflict, reported before the change applies. Shoot sharing a key with a hold binding is a conflict.

**Menus, glyphs, and wire**
- [ ] In a tree with no tablist, the bumpers still step Next/Prev.
- [ ] Glyphs follow the last device family and a rebinding: rebinding confirm to West changes the confirm glyph on the next frame.
- [ ] The confirm/cancel swap applies after player overrides, so a player who rebinds confirm still has the swap honored.
- [ ] Encoding of the movement input on the wire is unchanged. Review gate: no diff to the wire module.
- [ ] The Scripting surface example runs as a `content/dev` fixture in both SDKs.

### Manual
- [ ] Playtest tap-Shift dash and hold-Shift sprint on keyboard and on gamepad: dash timing feels right at 0.2 s, and sprint starts without a stutter.
- [ ] The controls panel shows only relevant commands, with author labels, categories, and order, and each binding's activator read-only.
- [ ] Epic AC 25: a gamepad-only pass completes every dev menu, the controls panel included, on Xbox and on PlayStation or Nintendo layouts, with the matching glyph art.

## Path
- **Stage order** is set at plan review. A suggested order:
  1. The binding layer: command IDs, relevance, effective bindings, activators, persistence, the controls panel, and raw capture.
  2. The menu conventions.
  3. Glyphs and the swap, which read effective bindings.
- **First slice:** the activator resolver on one shared key (tap-dash and hold-sprint on Shift) through `GameplayInputLatch`. This falsifies the riskiest assumption, that phases can ride the latch without disturbing the tick-0 edge reads in `build_sim_command`.
- **Seams:**
  - `InputSystem` gains a rebuild that also refreshes the `unique_actions` cache, as its comment asks.
  - `ui_nav.rs`'s hardcoded mappers become lookups over the nav set.
  - `UiIntentPayload` (`input/ui_dispatch.rs`) gains the raw-capture path.
  - `commit_staged_manifest_result` gains the `input` block.
  - The `[game]` reader sits beside `FieldReader` (`options/document.rs`).
  - Relevance reads `DataRegistry.entities` and the installed `TuningPayload`.
  - Device family is read from the gilrs vendor id.
  - The controls panel follows the accessibility panel's registration and reservation in `crates/ui/src/modal_stack/registry.rs`.
- **Rejected rival:** activators on commands instead of bindings (action-level triggers). A tap/hold pair on one key needs two bindings with different activators.
- **Split first,** behavior-preserving and in its own commit: `input/ui_focus.rs` (shared with U4, epic §Concurrent landing), `input/mod.rs`, `options/mod.rs`, and `options/bridge/mod.rs`. Extend `main.rs` only through the `app/` seams U1 opened.
- Source map and the precedent survey: `research.md`.

## Open questions
- The full command-ID table, the gamepad position names, and the gilrs mapping — **delegated**: pinned in the plan for the owner's plan review, then recorded in `input.md`.
- Repeat delay and interval, slider acceleration curve, and the ranges for the new numeric fields — **delegated**.
- How the player abandons a capture without binding anything — **delegated**, under the constraints that the prompt has no time limit and every input can be captured.
- Default buttons for the keyboard tab commands and the on-screen keyboard shortcuts — **delegated**, conflict-free within the UI set.
- GAG tier labels for hold-to-toggle and multiple input devices (epic open question) — **delegated**: confirm on the live pages before docs cite a tier.

## Boundary inventory
Rust ↔ TS/Luau ↔ TOML. Both SDKs ship every modder-facing row.

| Name | Manifest / SDK | TOML | Slot | Notes |
|---|---|---|---|---|
| Input block | `ModManifest.input` (`input` in Luau) | — | — | optional; per-command diagnostics |
| Command entry fields | `label`, `category`, `order`, `show`, `keyboardMouse`, `gamepad` | — | — | `show`: `true` forces shown, `false` forces hidden |
| Binding entry | `{ input, activator?, threshold? }` | — | — | `threshold` in seconds |
| Activator values | `"press"`, `"release"`, `"tap"`, `"hold"` | — | — | — |
| Glyph art | `input.glyphs.{keyboardMouse,xbox,playstation,nintendo}` | — | — | asset id `<dir>/<input>` |
| Command IDs | snake_case strings, e.g. `dash`, `nav_confirm`, `look_x` | row keys | — | table pinned at plan review |
| Device classes | `keyboardMouse`, `gamepad` | `keyboard_mouse`, `gamepad` | — | — |
| Input strings | W3C codes, mouse names, gamepad position names | row values | — | same strings in manifest and TOML |
| Per-game bindings | — | `[game."<mod_id>".bindings.<device_class>]` | — | keys only |
| Hold timing scale | — | `accessibility.hold_timing_scale` | `options.holdTimingScale`, `accessibility.holdTimingScale` | in the group; panel entry |
| Gamepad look | — | `gamepad_look_sensitivity`, `gamepad_look_dead_zone`, `gamepad_invert_y` | `options.gamepadLookSensitivity`, `options.gamepadLookDeadZone`, `options.gamepadInvertY` | top-level |
| Sprint mode | — | `sprint_mode` (`hold`/`toggle`) | `options.sprintMode` | top-level |
| Confirm/cancel swap | — | `swap_confirm_cancel` | `options.swapConfirmCancel` | top-level |
| Device family | — | — | `input.deviceFamily` (readonly) | `keyboardMouse`/`xbox`/`playstation`/`nintendo` |
| Controls panel | `OPEN_CONTROLS_ACTION` = `ui.openControls` | — | — | reserved registry name |
| Widgets | `Glyph({ command })`, `Scroll({ height }, children)` | — | — | — |
| Tab commands | `nav_tab_next`, `nav_tab_prev` | row keys | — | UI set |
| On-screen keyboard shortcuts | `text_backspace`, `text_space`, `text_commit` | row keys | — | UI set |
