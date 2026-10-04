# E23--gamepad-input

Brief · resumable · Epic 23 (U3) · reads: `context/lib/input.md` §2, §5, §7 · `player_options.md` §2, §4–§6 · `ui.md` §4, §5 · `networking.md` §What gates, and what replicates instead · read at 2a4bd9eb3 (source at af1d3c1b5)

## Problem
This is a requested capability: epic U3 (D8), which the owner widened to include a game-author binding layer. Players cannot remap anything. Gamepad menus also miss console conventions:
- focus is not restored on return
- nothing repeats unless authored
- nested groups trap focus
- there is no scrolling
- there are no device-aware button prompts
- destructive exits are not confirmed

Game authors cannot choose their game's commands or defaults either. Every game gets `input/defaults.rs`, so a game without dash still binds F to a dead command, and once rebinding exists that dead binding blocks F for anything else. One key cannot drive both a tap command and a hold command.

When this is done:
- Authors pick their game's commands and default bindings, each with an activator.
- Relevance is derived from the mod's data.
- Players rebind keys, UI navigation included, saved as a per-mod diff over the author's defaults.
- Gamepad menus follow console conventions, and button prompts follow both the device and the bindings.

The owner keeps this as one brief with two stages (epic `research.md` §Brief pins U3).

## Decisions

**Prior commitments this brief diverges from**, each argued in its bullet below:
- `player_options.md` §5: bindings in the `[accessibility]` group.
- `player_options.md` §6: "mods do not extend". Authors now pick commands and set defaults; the set stays closed.
- `done/M13--input-breadth`: the bumpers as Next/Prev.
- U1's 0–1 range for the group's numeric fields.

### Commands and relevance
- **Commands are stable snake_case IDs.** Gameplay commands come from the closed `Action` set and UI commands from the nav intents, and both stay engine-closed (epic §Out of scope). Each axis action splits into digital per-direction commands (`move_forward`, `move_back`) and an analog command (`look_x`). A digital command accepts keys, buttons, and half-axis stick inputs (`left_stick_up`). An analog command accepts axes only, so a stick swap is an ordinary rebind. Engine IDs never contain `.`, which reserves `<mod_id>.<name>`, split at the last `.`, for later mod-defined commands. Saved data and the manifest never use enum indices.
- **Gameplay and UI commands are separate sets, and conflicts are checked among commands live in the same context.** South as both `jump` and `nav_confirm` stays legal. `nav_menu` is live only when no capturing tree is on top, and `nav_cancel` only when one is, so Escape on both is not a conflict; that is today's behavior (`input.md` §5). UI commands are always relevant and cannot be hidden. UI dispatch still runs first (`input.md` §7), reading the UI set's effective bindings.
- **Relevance is derived by an exhaustive match**, so an unclassified command fails to compile. This is the denylist recipe (`networking.md` §What gates); a "default relevant" rule fails open.
  - `dash` and `crouch` are relevant if any movement descriptor has them.
  - `reload` is relevant if any weapon's resource is the magazine kind.
  - `alt_fire` is relevant if any weapon declares `secondary`.
  - `move_up` is dev-only: it is bound to its engine defaults, hidden from the panel, and never part of a conflict (fly-cam).
  - Every other command is relevant by name; per-level relevance is a non-goal.
  - An irrelevant command is unbound, absent from the panel, never part of a conflict, and draws no glyph. The author's `show` overrides derivation.
- **In co-op, relevance follows the host.** While participating, relevance is the union of local derivation and the installed host tuning. Tuning sites keep no local fallback (`networking.md`), so a client whose registry lacks dash would otherwise leave the host's live dash unbound.

### Bindings and activators
- **Activators live on bindings:** `press` (the default), `release`, `tap` (a max time), and `hold` (a min time). The name `activator` (owner) avoids the weapon descriptor's `trigger`. **Authors set activators, and players rebind keys only** (owner): a rebound key inherits its slot's activator, and a slot past the author's list takes `press`.
  - A lone `press` fires on the press edge.
  - A lone `release` fires on key-up.
  - A lone `tap` fires on key-up when the key was held no longer than its max, and otherwise never.
  - A `hold` fires once its min elapses while the key is down.
- **Each command accepts a fixed set of activators**, and author defaults outside it are diagnosed.
  - `shoot` and `alt_fire` accept `press` only, because charge requires it (`entity_model.md` §Weapon activations).
  - `sprint` and `crouch` accept `press` or `hold`.
  - Analog and wheel-notch commands accept `press`.
  - Every other command accepts any kind.
- **Shared keys follow the Steam rule.** Within a context, a key carries at most one short binding (`press`, `release`, or `tap`) plus one `hold`; anything else is a conflict, and press-only commands never share.
  - On a shared key, the hold fires at its min and the short binding never does.
  - A release before the hold's min fires the short binding. For a `tap`, that release must also fall within the tap's max.
  - A release between a tap's max and the hold's min fires nothing. A tap max above the hold's min is diagnosed.
  - Release at exactly a threshold counts as the release.
  - Losing focus, or a capturing tree opening, cancels a pending resolution, and neither binding fires.
- **Activators emit phases.** Each resolution produces the Pressed → Held → Released sequence commands already read. Edges survive a zero-tick frame, and sprint reads a held state. `crouch_mode` and the new `sprint_mode` apply on top: the activator decides when the command goes down and up, and the mode decides whether that latches.
- **Activators resolve client-side**, before the movement input is built. The wire gains no field or message (`networking.md`: the wire carries resolved intent).
- **Thresholds are author-set per binding**, defaulting to 0.2 s. They are scaled by `hold_timing_scale` in the `[accessibility]` group, which gets a slot and a panel entry (I11, AC 2, 3a–3c, 10). Its range is 1–3, the first group field outside U1's 0–1 range: a motor accommodation lengthens thresholds, it does not shorten them.
- **The guard keeps `nav_confirm`, `nav_cancel`, and `nav_menu` bound** on each device class. The panel refuses any change that breaks this, a replace included, and author defaults that break it are diagnosed. `nav_menu` is guarded because it is the only input that opens the pause menu.

### Author surface
- **An optional `input` block on `ModManifest` in both SDKs** (Scripting surface).
  - **Per command:** label, category, order, `show`, and default bindings per device class.
  - **Defaults:** an absent device class keeps the engine default; an empty list is unbound. No block at all means the engine table, so the dev mod is unchanged until it opts in.
  - **Glyph art:** the block names glyph art per device family.
- **Validation degrades per command and device class.**
  - An unknown command ID, unknown input, refused activator, or guard violation is diagnosed, and that command and device class fall back to the engine default.
  - A fallback that would collide leaves the command unbound there, with a diagnostic.
  - Of two valid entries that conflict, the later one in manifest order is unbound.
  - Hot reload re-validates.

### Player data
- **Bindings are saved as a diff, scoped by mod id.**
  - Rows sit at `[game."<mod_id>".bindings.<device_class>]`: command ID keys, lists of input strings, keys only. They are not in `[accessibility]`, because a binding is a diff over one mod's defaults with no resolved value for a slot.
  - Rows are keyed by mod id because bare and xtask runs share the `postretro` directory even though each game has its own (`done/game-user-dirs`). This matches `state_persistence::state_path`.
  - A missing row follows the author default, so a changed default reaches players who never rebound that command.
  - An empty list is unbound. A row naming an unknown command is kept and ignored.
  - An unknown input string falls back to that slot's author default alone (I7, AC 21).
  - U3 builds the `[game."<mod_id>"]` reader, read at mod init. Later game-scoped settings join it.
- **The player wins collisions with changed defaults** (owner). A player binding and a later author default on one key would either conflict or newly delay the player's binding. In that case the player keeps the key, the author default is suppressed there, and the panel flags the displaced command. A player `press` and a later author `hold` on one key count as such a collision. Nothing the player chose changes silently.
- **Inputs are named by physical position:**
  - Keyboard: W3C `KeyboardEvent.code` (`KeyW`, `ShiftLeft`).
  - Mouse: `mouse_left`, `mouse_right`, `mouse_middle`, `mouse_back`, `mouse_forward`, `wheel_up`, `wheel_down`, `mouse_x`, `mouse_y`.
  - Gamepad positions: `south`, `left_shoulder`, `left_trigger`, `left_stick_press`, `left_stick_x`, `left_stick_up`, ….
  - Analog polarity is a fixed engine table per source, and players flip pitch through `invert_y` and `gamepad_invert_y`.
  - Labels and glyphs resolve at display time.
- **The effective binding** is the player override, else the author default, else the engine default, per (command, device class). The swap then applies to the gamepad UI bindings.
  - The table rebuilds at mod init, on hot reload, on rebind, and when host tuning installs or clears, keeping input state and preferences.
  - A pending resolution on a key whose bindings changed is cancelled. A held key the rebuild newly binds waits for a fresh press.
- **Settings scope.** Top-level keys are per app (one directory per app name), with no `[machine]` section and no migration. Game data lives under `[game."<mod_id>"]`, bindings first. The new fields are:
  - Top-level, beside their mouse and crouch siblings: `gamepad_look_sensitivity`, `gamepad_look_dead_zone` (it applies to whichever stick is bound to look; the move stick keeps its own), `gamepad_invert_y`, `sprint_mode`, and `swap_confirm_cancel`. They are tuning preferences with no OS value, so slots and panel entries would serve nothing.
  - In the group: only `hold_timing_scale`.

### Remapping UI
- **An engine-owned controls panel.** It is a `core/ui/` tree built from the effective command list and opened by the reserved `ui.openControls`. Its registry name is reserved like the accessibility panel's (`ui.md` §1.1), and it uses the mod's theme.
  - The engine owns it because mods cannot author rows over a list computed at mod init, and because the guard and conflict rules stay out of mod scripts.
  - It lists relevant commands by the author's category and order, showing each activator read-only, with per-command reset and reset-all.
  - Conflicts are reported before they apply, and the player either replaces or cancels.
- **Raw capture.** The prompt captures the next key, button, or axis, Escape, Start and Select/Back included.
  - It is decided App-side after that frame's activations, only while the prompt is the active tree (epic `research.md` §Brief pins).
  - It has no time limit, and every input can be captured.
  - With the swap on, a capture stores the counterpart button, so the captured button drives the command the row shows.

### Menu conventions (epic §U3; AC 19, 20, 23–25)
- **Restore on return is on by default.** It applies only to a pop that reveals the tree; a fresh push lands on initial focus (O14). `restoreOnReturn` moves to the tree's props, where an explicit `false` opts the tree out. Today the flag is ORed across containers, so `false` cannot be expressed.
- **Engine-default hold-to-repeat** applies where a container authors none. Authored repeat wins, and an authored zero delay means no repeat.
- **A held slider step repeats and accelerates**, clamping at its bounds without overshoot.
- **Nested groups.** A directional move a group cannot answer continues in the enclosing group, where each nested group is one candidate by its bounds. A linear group answers only its own axis.
  - Entering a group lands on its last-focused member, else its initial focus.
  - Next/Prev stay within the group, and `focusNeighbors` still overrides.
  - New containers admit only interactive widgets (I5).
- **Tabs use existing roles.** A `role: "tablist"` container holds `role: "tab"` buttons that carry `selected`. `nav_tab_next` and `nav_tab_prev` (LB/RB) activate the adjacent tab, wrapping, and move focus to it. A tree with no tablist keeps today's Next/Prev, so the bumpers act as tab keys only where tabs exist.
- **Scrolling is a container attribute** (owner): `scroll: { maxHeight }`, accepted on `VStack` and `Grid`.
  - Container behavior already lives in the shared props (`focus`, `restoreOnReturn`, `role`), and a wrapper widget would add a node and duplicate them.
  - The container sizes to its content up to `maxHeight`, then clips and scrolls vertically.
  - Focus outside the viewport scrolls into view by the minimum distance, and the pointer wheel scrolls it.
  - `scroll` creates no focus stop or group.
  - On an `HStack` it draws a diagnostic and is ignored.
- **Glyphs follow the last device family**: `keyboardMouse`, `xbox`, `playstation`, or `nintendo`.
  - One family is chosen per frame, and drift inside a dead zone never changes it.
  - `Glyph({ command })` draws the command's first effective binding on that family from the mod's art. Missing art draws the input's name; a command unbound on that family draws nothing.
  - Glyphs follow rebinding and the swap with no extra authoring.
- **The confirm/cancel swap** exchanges the gamepad bindings of `nav_confirm` and `nav_cancel`. The player chooses it; it is never automatic by family.
- **Confirmation is an authored pattern**: a dialog tree with `initialFocus` on the safe choice, pushed by `showDialog`, following the precedent of `core/ui/displayModeConfirm.json`. The dev EXIT and QUIT each gain one.
- **On-screen keyboard shortcuts.** `text_backspace`, `text_space`, and `text_commit` are live while a text-entry tree is on top. Each activates its key without moving focus. A held `text_backspace` repeats as the key does.
- **Dev-mod consumer** (epic §U3, D1):
  - The options menu is restructured with tabs and nested groups, and level select becomes spatial.
  - A CONTROLS entry opens the panel.
  - EXIT and QUIT get confirmations.
  - The mod opts into an `input` block and ships keyboard and gamepad glyph art.

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
VStack({ scroll: { maxHeight: 320 }, focus: { policy: "linear" } }, levelButtons);
Button({ id: "controls", label: "CONTROLS", onPress: OPEN_CONTROLS_ACTION });   // "ui.openControls"
```
The Luau mirror ships with the same names (Boundary inventory).

### Non-goals
- Mod-defined commands. Their ID form is reserved.
- Chord and modifier bindings, and double-tap. The activator set stays open to them.
- Players choosing activator kinds (owner).
- A press-then-hold activator (owner chose the Steam rule).
- Per-level relevance.
- Steam Input API integration (`input.md` §9).
- Migrating saved bindings. None exist today.
- Horizontal scrolling.
- Rebinding the diagnostic chords and the hardware text-entry keys (Enter commits, Backspace deletes). These stay fixed.

## Acceptance
These rows add to epic AC 19–25, as amended, and to AC 2, 3a–3c, and 10 for `hold_timing_scale`. AC 33 applies if U3 lands after U4. Pin ids refer to `research.md` §Pinned orderings.

### Automated
**Author layer**
- [ ] With no input block, effective bindings equal today's defaults: each default input produces the same action reads it does today, and the engine defaults raise no conflict (Escape opens the pause menu with no menu open and cancels inside one).
- [ ] With no input block, Q and Z still move the fly-cam in a pawnless map. move_up is absent from the controls panel and never conflicts with an author binding on Q.
- [ ] An author binding of ShiftLeft to dash, with sprint's keyboard list empty: Shift drives dash, F drives nothing, and the run state stays false.
- [ ] Every activator a command accepts is accepted, and every other is diagnosed. `tap` on shoot is diagnosed and `press` on shoot accepted; `hold` on sprint is accepted and `tap` on sprint diagnosed. A diagnosed entry falls back for that command and device class only.
- [ ] Author defaults that leave confirm, cancel, or menu unbound on a device class are diagnosed and fall back; defaults keeping each bound are accepted.
- [ ] A block with an unknown input on dash and a valid sprint entry diagnoses dash's keyboard entry and falls back for it alone; sprint's author default and dash's gamepad defaults apply; an unknown command ID is diagnosed and nothing else changes.
- [ ] A typo in sprint's keyboard entry, whose engine fallback is Shift while dash taps Shift, leaves sprint unbound on keyboard with a diagnostic. Of two valid entries that conflict, the later one is unbound.
- [ ] A command entry with only keyboardMouse keeps the engine's gamepad defaults; an empty gamepad list leaves it unbound on gamepad.
- [ ] show: false on nav_confirm is diagnosed, and nav_confirm stays bound and listed.
- [ ] A key on an analog command, or an axis on a digital one, is diagnosed and falls back. Binding look to the left stick and movement to the right swaps the sticks with no other change, and the look dead zone moves to the left stick.
- [ ] A manifest entry keyed postretro.dev.dash is diagnosed as an unknown command; saved rows are keyed by command ID strings.
- [ ] Hot-reloading the block recomputes effective bindings and keeps player overrides.

**Relevance**
- [ ] A mod with no dash descriptor: dash is unbound, missing from the controls panel, and does not conflict with an author binding on F. A mod where one of two descriptors has dash: dash is relevant.
- [ ] reload is relevant only for a mod with a magazine weapon, crouch only for one whose movement descriptor has crouch, and alt_fire only for one with a secondary activation.
- [ ] A hot reload that adds the first dash descriptor binds dash without a restart.
- [ ] Force-show and force-hide each flip the derived answer.
- [ ] Opening the controls panel from the frontend before any level loads lists dash for a mod whose descriptors have dash, and omits it otherwise (P6).
- [ ] A co-op client whose local registry lacks dash gets dash relevant and bound once host tuning with dash installs, and unbound again after it leaves participation (P5).
- [ ] Adding a command without classifying its relevance fails to compile. Review gate: no wildcard arm in the derivation.

**Player data**
- [ ] A command the player never rebound follows a changed author default after reload; a rebound command keeps the player's key.
- [ ] A player row for dash is kept across a session in which dash is irrelevant and applies again when dash becomes relevant.
- [ ] An empty-list row stays unbound across save and load.
- [ ] An unknown command row survives, value unchanged, a save that rewrites another row in the same bindings table.
- [ ] A row whose first entry is an unknown input binds the author default on that slot and keeps its second entry on the second slot's activator; every other binding and setting loads.
- [ ] Every input string the engine writes parses back to the same input, across keyboard, mouse, and gamepad names.
- [ ] A player binding of Q to dash, then an author default of Q for reload: Q drives dash only, and reload is flagged in the panel. Without the player row, Q drives reload.
- [ ] A player press on Q, then an author hold on Q for another command: Q still fires the player's command on the press frame, and the author's command is flagged.
- [ ] Two mod ids in one shared settings file keep separate rows, and A's overrides do not apply while B is loaded.
- [ ] A rebound key keeps its slot's author activator.
- [ ] gamepad_look_sensitivity, gamepad_look_dead_zone, gamepad_invert_y, sprint_mode and swap_confirm_cancel save as top-level keys with no accessibility.* slot or panel entry (catalog assertion); hold_timing_scale has both. An unrecognized sprint_mode value falls back to hold for that field alone.

**Activators**
- [ ] Tap and hold on one key:
  - released within the tap's max, the tap fires once and the hold never;
  - held past the hold's min, the hold fires and the tap never;
  - released at exactly the tap's max, the tap fires;
  - released between the tap's max and the hold's min, nothing fires;
  - a tap max above the hold's min is diagnosed;
  - losing focus or a capturing menu opening mid-hold: neither fires.
- [ ] A key pressed and released between two frames fires its tap once and its hold never, and a lone press fires once (P1). Two keys bound to dash and tapped on one frame fire dash once (P2).
- [ ] A tap still reaches the simulation on a frame with zero ticks. A hold that crosses its threshold on a zero-tick frame and releases before the next tick still latches toggle-mode sprint, and its tap partner never fires (P3).
- [ ] A lone press binding fires on the press frame. A lone tap fires on its release frame within its max and never past it. A lone release binding fires on key-up.
- [ ] A dash tap authored at 0.1 s does not fire on a release at 0.15 s; a binding with no threshold uses 0.2 s.
- [ ] Hold-Shift sprint shows a held state every frame past the threshold and releases on key-up. In sprint_mode toggle it latches on the hold resolution and releases on the next.
- [ ] Raising hold_timing_scale to 2 doubles every threshold. The panel steps it within 1–3, and a key already down keeps the threshold it started with (P23).
- [ ] A tuning install that binds dash to a Shift tap while Shift is held for sprint keeps sprint held, and Shift's release fires no dash. A key held through a rebuild that newly binds it fires only on its next press (P4).
- [ ] A key held while a capturing menu opens and closes does nothing until released and pressed again (P24).
- [ ] Conflicts:
  - South on both jump and confirm is not a conflict.
  - Two gameplay commands on one key with the same activator are a conflict, reported before the change applies.
  - A press and a tap on one key are a conflict.
  - Shoot sharing a key with a hold is a conflict.
  - Rebinding confirm to East with replace, when East is cancel's only gamepad binding, is refused and both bindings stay.
- [ ] Review gate: no new wire field or message carries activator state.

**Capture**
- [ ] The confirm press that opens the capture prompt is not captured, nor its OS key repeat, nor any input pressed on the frame the prompt opens; pressing the same button again captures it. A stick held off rest when the prompt opens is captured only after it returns to rest and moves again (P7).
- [ ] Capturing South, Escape, Start, or Select binds it and does nothing else: no prompt reopens, no conflict question is answered, and no menu opens or closes, on the press or its release (P8).
- [ ] With the swap on, capturing South for confirm makes South confirm.
- [ ] A capture prompt open for a command that becomes irrelevant closes without binding or saving anything (P25).

**Menu conventions**
- [ ] A tree authored with restoreOnReturn: false, in either SDK, lands on its initial focus on return; the same tree without the field restores.
- [ ] Committing or cancelling the on-screen keyboard returns focus to the field that opened it (P11).
- [ ] A confirmation closed and reopened on one frame lands on its safe choice (P12). Two confirms on EXIT on consecutive frames leave the game running with the confirmation closed (P27).
- [ ] On return, a saved focus rebuilt away or now disabled lands on initial focus. Re-entering a nested group whose last-focused member is gone or disabled lands on its first enabled member (P13).
- [ ] A direction held while a confirmation opens never moves focus off its safe choice until pressed again (P14).
- [ ] A held direction across a 1 s frame moves focus at most one step on that frame, and a held slider at most one step (P15).
- [ ] A container authoring a zero repeat delay never repeats; one authoring no repeat repeats at the engine default; one authoring its own delay and interval uses them (P16).
- [ ] A held slider step and an external write to its slot on one frame never leave the slot at the old value plus a step (P17).
- [ ] A nav direction rebound to W repeats while W is held and stops on W's release; releasing an arrow no longer bound to it does not stop it; a direction rebound to a face button repeats while held. With the swap on, releasing East stops a held confirm repeat; toggling the swap with confirm acts once and the next South press cancels (P9, P10).
- [ ] Binding nav_down to right_stick_down navigates menus with the right stick.
- [ ] Leaving a nested group and moving back into it lands on the member last focused there. Next at the end of a non-wrapping nested group does nothing. A focusNeighbors target in another group wins over the escape. A Text inside a scroll or tab container is never focused.
- [ ] In a tabbed menu whose strip is a nested linear group, Down from any tab enters the panel group, and Right on the last tab wraps within the strip when it wraps (P21).
- [ ] Two bumper presses resolving on one frame advance two tabs, and a disabled tab is skipped. With one tab, the bumpers move focus to it without activating. With none selected, RB activates the first tab and LB the last. With a dialog over a tabbed menu, the bumpers act on the dialog only (P20).
- [ ] In a tree with no tablist, the bumpers still step Next/Prev.
- [ ] A scroll container whose content fits under maxHeight sizes to its content and does not scroll. The pointer wheel scrolls one that overflows. Its children belong to the enclosing focus group unless it declares focus. scroll on an HStack draws a diagnostic and is ignored, in both SDKs.
- [ ] Moving focus one row below a scroll viewport scrolls by one row, so that row's bottom meets the viewport's bottom; moving above aligns tops.
- [ ] A scroll container scrolled to its end whose content shrinks below its viewport draws from its top with no empty band (P18). A click on its clipped area activates nothing hidden; a restored focus outside the viewport scrolls into view by the minimum distance (P19).
- [ ] A text_space shortcut on the frame text entry commits adds nothing, and the shortcuts do nothing with no text-entry tree on top (P26). Holding the backspace shortcut repeats as holding the on-screen backspace key does. With a text-entry tree on top, a key rebound to a nav command types its character and moves no focus (P28).
- [ ] A mod tree registered under the controls panel's reserved name, at mod scope, at level scope, or on a staged reload, is rejected with a load-time diagnostic, and ui.openControls still opens the engine panel.

**Glyphs and family**
- [ ] Glyphs follow the last device family and a rebinding: rebinding confirm to West changes the confirm glyph on the next frame.
- [ ] A pad with Sony's vendor id draws PlayStation glyphs, Nintendo's draws Nintendo glyphs, and any other draws Xbox glyphs. Drift inside a dead zone never changes the family, and a frame with keyboard and pad input sets one family (P22).
- [ ] A Glyph whose art is missing draws its input's name. A Glyph for an irrelevant command, or for one unbound on the current family, draws nothing.
- [ ] Switching to a PlayStation or Nintendo pad leaves confirm and cancel unswapped until the player turns the swap on. The swap applies after player overrides.
- [ ] The Scripting surface example runs as a `content/dev` fixture in both SDKs.

### Manual
- [ ] Playtest tap-Shift dash and hold-Shift sprint on keyboard and gamepad: dash timing feels right at 0.2 s, and sprint starts without a stutter.
- [ ] The controls panel shows only relevant commands, with author labels, categories, and order, and each binding's activator read-only.
- [ ] Epic AC 25: a gamepad-only pass completes every dev menu, the controls panel included, on Xbox and on PlayStation or Nintendo layouts, with matching glyph art.

## Path
- **Stage order** is set at plan review. Suggested:
  1. the binding layer: commands, relevance, effective bindings, activators, persistence, the panel, capture;
  2. the menu conventions;
  3. glyphs and the swap, which read effective bindings.
- **First slice:** the activator resolver on Shift (tap-dash, hold-sprint) through `GameplayInputLatch`. This falsifies the riskiest assumption: that phases can ride the latch without disturbing `build_sim_command`'s tick-0 edge reads. The latch keeps only Pressed today, plus Released for Shoot and AltFire.
- **Seams:**
  - `InputSystem` gains a rebuild that refreshes `unique_actions`.
  - Key state is a level per key today (`input/mod.rs`), so same-frame press and release (P1) needs buffered edges.
  - `ui_nav.rs`'s mappers and the hardcoded repeat-release sites (`app/keyboard_input.rs`, `input/gamepad.rs`) become effective-binding lookups (P10).
  - `UiIntentPayload` (`input/ui_dispatch.rs`) gains raw capture.
  - `commit_staged_manifest_result` gains the block.
  - The `[game]` reader sits beside `FieldReader`.
  - Relevance reads `DataRegistry.entities` and the installed `TuningPayload`, and `demote_client_state` triggers a rebuild.
  - Device family comes from gilrs `Gamepad::vendor_id` (0.11.1, all default backends).
  - The panel reserves its name in `crates/ui/src/modal_stack/registry.rs`.
  - The text-entry pop runs before the focus tick in `main.rs` (P11).
  - The accessibility numeric table clamps to 0–1 (`options/panel_actions.rs`, `options/accessibility.rs`, SDK `AccessibilityNumericField`) and needs a per-field range.
  - The focus export carries `selected` but not `role`.
- **Rejected rival:** activators on commands (action-level triggers). A tap/hold pair on one key needs two bindings.
- **Split first,** behavior-preserving, each in its own commit: `input/ui_focus.rs` (shared with U4), `input/mod.rs`, `options/mod.rs`, and `options/bridge/mod.rs`. Extend `main.rs` only through U1's `app/` seams.
- Source map, pins, and precedent survey: `research.md`.

## Open questions
- The full command-ID table, the gamepad position names, the half-axis inputs, the analog polarity table, and the gilrs mapping. **Delegated:** pinned in the plan for the owner's review, then recorded in `input.md`.
- Repeat delay and interval, the slider acceleration curve, the ranges of the new top-level numeric fields, and whether a zero authored repeat interval keeps today's single repeat. **Delegated.**
- How a player abandons a capture. **Delegated**, under two constraints: no time limit, and every input stays capturable.
- Default keys for the keyboard tab commands and the on-screen keyboard shortcuts. **Delegated**: conflict-free within their context.
- GAG tier labels for hold-to-toggle and multiple input devices (an epic open question). **Delegated**: confirm on the live pages before the docs cite a tier.

## Boundary inventory
Rust ↔ TS/Luau ↔ TOML. Both SDKs ship every modder-facing row.

| Name | Manifest / SDK | TOML | Slot | Notes |
|---|---|---|---|---|
| Input block | `ModManifest.input` (`input` in Luau) | — | — | optional; per-command diagnostics |
| Command entry fields | `label`, `category`, `order`, `show`, `keyboardMouse`, `gamepad` | — | — | `show`: `true` forces shown, `false` hidden |
| Binding entry | `{ input, activator?, threshold? }` | — | — | `threshold` in seconds; a tap's max or a hold's min |
| Activator values | `"press"`, `"release"`, `"tap"`, `"hold"` | — | — | — |
| Glyph art | `input.glyphs.{keyboardMouse,xbox,playstation,nintendo}` | — | — | asset id `<dir>/<input>` |
| Command IDs | snake_case, e.g. `dash`, `nav_confirm`, `look_x` | row keys | — | table pinned at plan review |
| Device classes | `keyboardMouse`, `gamepad` | `keyboard_mouse`, `gamepad` | — | — |
| Input strings | W3C codes, mouse names, gamepad position and half-axis names | row values | — | same strings in manifest and TOML |
| Per-game bindings | — | `[game."<mod_id>".bindings.<device_class>]` | — | keys only |
| Hold timing scale | — | `accessibility.hold_timing_scale` | `options.holdTimingScale`, `accessibility.holdTimingScale` | range 1–3; panel entry |
| Gamepad look | — | `gamepad_look_sensitivity`, `gamepad_look_dead_zone`, `gamepad_invert_y` | `options.gamepadLookSensitivity`, `options.gamepadLookDeadZone`, `options.gamepadInvertY` | top-level |
| Sprint mode | — | `sprint_mode` (`hold`/`toggle`) | `options.sprintMode` | top-level |
| Confirm/cancel swap | — | `swap_confirm_cancel` | `options.swapConfirmCancel` | top-level |
| Controls panel | `OPEN_CONTROLS_ACTION` = `ui.openControls` | — | — | reserved registry name |
| Glyph widget | `Glyph({ command })` | — | — | — |
| Scroll attribute | `scroll: { maxHeight }` on `VStack` and `Grid` | — | — | vertical; ignored with a diagnostic on `HStack` |
| Restore on return | `restoreOnReturn` on tree props; `false` honored | — | — | moves from containers |
| Tab commands | `nav_tab_next`, `nav_tab_prev` | row keys | — | UI set |
| On-screen keyboard shortcuts | `text_backspace`, `text_space`, `text_commit` | row keys | — | UI set |
