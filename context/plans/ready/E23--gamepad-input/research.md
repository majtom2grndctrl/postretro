# E23--gamepad-input: Research

Derivation and source map behind the brief. Source was read at `af1d3c1b5`, the window-modes branch: main `2a4bd9eb3` plus uncommitted window-mode work in `app/window_modes/`, `options`, and `main.rs`. Recheck anything in those areas once window-modes lands.

## 1. Binding substrate today

- `Action` (`input/types.rs`) is a closed enum with no serde and no exhaustive match anywhere. `Action::is_axis` uses `matches!`, and the hand-written `common_actions()` test list in `input/defaults.rs` omits the wieldable-select slots.
- `PhysicalInput` has these variants:
  - `Key(winit KeyCode)` and `MouseButton`
  - `MouseWheelUp` / `MouseWheelDown`
  - `MouseAxisX` / `MouseAxisY`
  - `GamepadButton(gilrs::Button)` and `GamepadAxis(gilrs::Axis)`
- In gilrs, `LeftTrigger`/`RightTrigger` are the bumpers and `LeftTrigger2`/`RightTrigger2` are the analog triggers. The positional vocabulary maps from those names.
- `InputSystem::new(Vec<Binding>)` stores a flat list. There is no rebind setter. The `unique_actions` cache comment asks a future write site to refresh it, and `prev_button_states` is sized from it.
- `ButtonState` is Pressed / Held / Released / Inactive, stepped by `advance(active)`.
- `GameplayInputLatch` (`input/mod.rs`) records render-rate Pressed actions. `snapshot_for_ticks` re-applies them into the first tick snapshot and returns `None` on a zero-tick frame, so a press is not lost. `gameplay_snapshot_for_capture_state` (`main.rs`) clears the latch while UI captures gameplay.
- Command reads in `build_sim_command` and the tick loop:
  - Sprint: `running = button(Sprint).is_active()`, a held read.
  - Dash, Shoot, Use, Drop: `tick_index == 0 && Pressed`.
  - Crouch: `resolve_crouch_intent(crouch_mode, …)` before the tick loop.
  - Reload → `SimCommand.reload`.
  - AltFire → `secondary_button { pressed, active }`.
  - MoveUp: the fly-cam path only (`!has_player_pawn && !pre_arm_client`).
  - Cycle-next/previous and toggle-last are read by `input/wieldable_selection.rs`.
- No `sprint_mode` exists.
- UI nav is hardcoded in `input/ui_nav.rs`:
  - `nav_intent_for_key`: arrows, Tab → Next, Enter, Escape → Menu/Cancel.
  - `nav_intent_for_gamepad_button`: DPad; bumpers → Next/Prev; South → Confirm; East → Cancel; Start → Menu; Select → Options.
  - `StickNavTracker`.
  - Callers: `app/keyboard_input.rs` and `GamepadSystem::update` (`input/gamepad.rs`), which drains gilrs events directly.
  - `NavIntent` is a closed set.
- There is no raw-capture path. `text_entry_key` is logical-key text entry. `ActivationInputCapture` is weapon-edge latching and unrelated.

## 2. Author surface and relevance inputs

- **Manifest types.** `ModManifestResult` lives in `scripting-core/src/runtime/types.rs`. The SDK `ModManifest` is registered in `sim/src/scripting/primitives/manifest.rs`, with a parity test, and is emitted to both `.d.ts` and `.d.luau`.
- **Optional block precedent.** `render`, `audio`, `switching`, `theme`, and `frontend` are all optional blocks. A malformed optional value warns and falls back.
- **Hot reload.** It is debug-only. `ScriptRuntime::commit_staged_manifest_result` (`runtime/core.rs`) commits a subset that includes `entities` and identity, and identity stays frozen. An `input` block has to join that subset to hot-reload.
- **Descriptors.** Movement and weapons sit on entity types: `EntityTypeDescriptor.movement: Option<PlayerMovementDescriptor>` and `.weapon`, in `DataRegistry.entities`. That registry is filled after `run_mod_init`, survives level unload, and carries `entity_types_generation()`.
  - `PlayerMovementDescriptor.dash` and `.crouch` are `Option`.
  - `WeaponResource` is `Ammo` (magazine), `Heat`, or `Cell`.
  - `WeaponDescriptor.secondary: Option<WeaponActivationDescriptor>`.
- **Effect of `drafts/player-descriptor-composition`.** It moves dash to `modifiers.dash` plus `states.ground.dash?` / `states.air.dash?`, and crouch to `states.crouch?`. "The mod has dash" becomes "any state block carries dash". The exhaustive derivation is the place that follows the change.
- **Co-op tuning.**
  - `ServerControlMessage::Tuning` → `NetEndpoint::install_tuning_payload` (`netcode/src/endpoint.rs`).
  - `TuningPayload { movement: Option<PlayerMovementDescriptor>, wieldables: [Option<WieldableTuningPayload>; …] }` (`combat-model/src/carried_loadout.rs`). Each wieldable row carries `resource` and `secondary`, taken from the pawn's live inventory, not from every registered weapon.
  - `demote_client_state` clears it.
  - No code rebuilds solo relevance on disconnect: a return to solo goes through a session or level reinstall (medium confidence).
  - `networking.md` §What gates forbids any fallback to the local registry for a replicated value.

## 3. Settings storage

- U1 landed tolerant per-field storage:
  - `PlayerOptions::from_table` / `load_with_status` read through `FieldReader::read` / `read_in` (`options/document.rs`).
  - `DocumentWriter::new` clones the loaded table, so unknown tables survive a save.
  - `mark_written` clears a field's unrecognized flag.
  - `save` does nothing on an unreadable or invalid file.
- No `[game…]` section exists.
- The `[accessibility]` group is declared in `options/accessibility.rs`. U1's projection works like this:
  - Each field gets a readonly `accessibility.<field>` slot and an `accessibility.<field>FollowsSystem` slot.
  - `options.<field>` working copies sit behind a table-driven bridge (`options/bridge/accessibility.rs`).
  - The panel field-action family is `ui.accessibility.<op>.<field>` (`crates/ui/src/actions.rs`, `options/panel_actions.rs`), and the `panel_actions` tests enforce that the panel carries every field.
- `player_options.md` §5 says remapped bindings "join the same group". The brief diverges (Decisions).
- Per-game directories landed with game-user-dirs (merge `12e48599e`): `startup/app_dirs.rs` resolves `ProjectDirs::from("", "", app_name)`. Bare engine and xtask runs share the `postretro` directory.

## 4. Menu conventions today

- **Focus restore.** `UiFocusEngine.trees` keys saved focus by tree name for the session and never removes an entry. `tick` detects any active-key change and does not distinguish push from pop. `ensure_initialized` restores on any change when `restore_on_return` is set. A saved node that was rebuilt away falls back to initial focus. `restoreOnReturn` defaults to false in both parsers.
  - Caveat (unverified): the first frame after a push may still read the previous tree's focus export.
- **Repeat.** `FocusPolicy::Detailed { repeat }` is per container, with no engine default; `advance_repeat` reads the group's policy.
  - Repeat stops on release in `app/keyboard_input.rs` and in the gamepad path in `main.rs`.
  - An authored zero delay disables repeat (`RepeatTimer::advance`, threshold ≤ 0).
- **Sliders.** `capture_slider_step` applies one fixed step per captured intent. `apply_slider_nav_capture` (`app/ui_actions.rs`) removes the intent before the focus tick, so a held slider never repeats.
- **Nesting.** `collect_focus_node` puts each stop into its nearest policy ancestor only. The `move_focus` paths walk only the current group, which is why the dev options menu flattens its tab strip into one group.
- **Tabs.** The dev options menu is already tabbed (`role: "tablist"`, tab buttons with `selected`, a `Switch` over local state). LB/RB already produce Next/Prev (linear steps), and LB is also gameplay toggle-last.
- **Missing pieces.**
  - No scroll widget exists (`Widget` enum in `scripting-core/src/ui/descriptor/widgets.rs`), and nothing clips.
  - Device family isn't tracked: `InputMode` is Pointer/Focus, and `ModeSignal` merges keyboard and gamepad.
- **Confirmation and the on-screen keyboard.** `core/ui/displayModeConfirm.json` is an engine confirmation with initial focus on the safe choice, and `showDialog(tree, onCommit)` exists. Dev EXIT/QUIT act immediately (`frontend-menu.ts`, `pause-menu.ts`, handled in `app/ui_actions.rs`). `core/ui/keyboard.json` has no shortcut buttons, and the gamepad path has no text-entry branch.
- **Gamepad look.**
  - `GAMEPAD_LOOK_SENSITIVITY` is a const (`input/look.rs`).
  - One private `DEAD_ZONE` serves both sticks (`input/gamepad.rs`).
  - `invert_y` affects the mouse only. Gamepad pitch inversion is a binding scale of -1.

## 5. File sizes (lines, at `af1d3c1b5`)

| File | Lines | Brief touches |
|---|---|---|
| `crates/postretro/src/main.rs` | ~14.5k | command reads, gamepad nav release, capture routing |
| `input/ui_focus.rs` | ~2.4k | restore-on-return, repeat default, nested groups, slider repeat |
| `input/mod.rs` | ~1.3k | rebind, activator phases |
| `options/mod.rs` | ~1.2k | new fields, `[game]` reader |
| `options/bridge/mod.rs` | ~910 | working copies |

U1 split `main.rs` by seam into `app/{ui_actions,keyboard_input,options_menu}.rs`, but did not split `ui_focus.rs`. U4 also extends `ui_focus.rs` (epic §Concurrent landing).

## 6. Direction review (validate-plan, 2026-10-04)

The verdict on the seed was *Reshape (narrow)*, and the owner accepted it. The findings that shaped Decisions:
- AltFire has a consumer since E16.
- Settings are no longer shared across games.
- Relevance has to fail closed (an exhaustive match) and follow host tuning in co-op.
- Triggers must emit phases, not one-shots, because sprint is a held read.
- Per-command trigger limits are needed because charge requires `press`.
- An override can collide with a later-changed author default.
- The binding vocabulary clashes with the weapon `trigger` vocabulary.

The owner also reopened seed D8: authors fix activator kinds, and players rebind keys only.

## 7. Precedent survey (carried from the seed)

- **Where triggers live.** Unreal Enhanced Input puts Triggers on mappings and also on actions; Unity puts Interactions on bindings and on actions. Per-command accepted kinds sit at the action level.
- **Saved data.** Unity's `SaveBindingOverridesAsJson` stores a diff. Unreal's `EnhancedInputUserSettings` keys mappings by mapping name and slot.
- **Shared keys.** The shared-key rule comes from Steam Input activators: an interruptible press is held back until release or the long-press threshold.
- **Physical key names.** Godot `physical_keycode`, Unity `<Keyboard>/a`, Bevy `KeyCode`. W3C `KeyboardEvent.code` names match winit `KeyCode`.
- **Rebind menus.** They list a declared, labeled subset: Source `kb_act.lst`, Steam In-Game Actions manifest.
- **Thresholds.** Unreal and Unity tap ≤ 0.2 s. Their holds (0.4 s and 1.0 s) are too slow for sprint in a fast shooter.
- **Device families.** Sony's USB vendor id is `0x054C` and Nintendo's is `0x057E`. Any other vendor gets the Xbox layout. gilrs 0.11.1 exposes `Gamepad::vendor_id` on macOS, Linux, and Windows WGI (the default backend); only the non-default xinput backend returns `None`.

## 8. Detail review (review-brief, 2026-10-04)

The premise, rows, and subtract lenses ran once. The owner kept one brief and all four activators, and accepted the remaining recommendations. Decisions changed as a result: context-scoped conflicts (Escape), `move_up` as dev-only, a 1–3 range for `hold_timing_scale`, a fixed analog polarity, per-slot fallback for unknown inputs, a tree-level `restoreOnReturn`, half-axis stick inputs, swap-aware capture, collision and fallback rules, no `input.deviceFamily` slot, and a client-side activator wire decision. Source facts behind them:
- `nav_intent_for_key` maps Escape to Cancel under a capturing tree and to Menu otherwise.
- The accessibility numeric fields clamp to 0–1 in `options/accessibility.rs` and `options/panel_actions.rs`.
- `any_restore_on_return` ORs the flag across containers, and both SDKs and the Rust serializer drop `false`.
- The default bindings carry polarity in `Binding.scale`: mouse X is -1 for yaw, and right stick X is +1.
- The dev level select is a linear-focus `VStack` of sections.
- The text-entry commit pops the tree before the focus tick reads the layout.
- Key state is a level per key, so a press and a release between two frames leave no edge.

## Pinned orderings

Each row is cited by an Acceptance row in the brief.

| id | Scenario | Ordering | Expected outcome |
|---|---|---|---|
| P1 | A key is pressed and released between two frames | press → release → frame snapshot | On a shared tap/hold key, the tap fires once and the hold never fires. A lone `press` or `tap` binding fires once. |
| P2 | Two bindings of one command resolve on one frame | key A resolves → key B resolves → snapshot | The command fires once. |
| P3 | A hold crosses its threshold on a frame with zero ticks, and the key is released before the next tick | threshold passes (0 ticks) → key up → next tick | The hold reaches the simulation once: toggle-mode sprint latches, hold-mode sprint is active for that tick, and the shared tap never fires. |
| P4 | The binding table rebuilds while a key is held or a resolution is pending (hot reload, rebind, tuning install, swap toggle) | key down → rebuild → threshold or release | A resolution already made stands until release. A pending resolution on a key whose bindings changed is cancelled, and neither binding fires. A held key that the rebuild newly binds fires nothing until it is pressed again. A key whose bindings did not change resolves as if no rebuild happened. |
| P5 | A co-op client leaves participation | tuning installed → demote or disconnect (tuning cleared) → binding table | Commands that were relevant only through host tuning are unbound again. The table rebuilds when tuning clears, not only when it installs. |
| P6 | The frontend opens the controls panel before any level loads | mod init commits the registry → derivation → panel opens | The panel lists the commands derived from the committed registry. Derivation never runs against an empty registry. |
| P7 | The capture prompt opens | confirm at N → prompt opens at N+1 → presses at N+1 and later | The press that opened the prompt is never captured, and neither is its OS key repeat or anything pressed on the frame the prompt opens. The first new press on a later frame is captured. An axis already off rest when the prompt opens is captured only after it returns to rest and moves again. |
| P8 | A captured press would also be a UI input | capture decided at N → the same press's nav intent resolves at N+1; Start/Escape set the menu toggle at N | A press the capture consumes reaches no other consumer, on its press frame or its release: no nav intent, no pause toggle, no options open, and no answer to a conflict question the capture raised. |
| P9 | The confirm/cancel swap is toggled while a confirm repeat is held, or with the button that toggles it | toggle press → swap applies → release → next press | The toggling press acts once, as confirm. Its release neither confirms nor cancels. The new mapping applies from the next press. A confirm repeat stops on the release of whichever button confirm is bound to now. |
| P10 | A nav direction or confirm is remapped, then held | remap → press → hold → release | The hold repeats on the remapped input and stops on its release. Releasing an input no longer bound to that command does not stop it. |
| P11 | A tree is popped earlier in the frame than focus resolution (text-entry commit or cancel) | pop → focus resolution against the popped tree's layout → next frame | The revealed tree keeps its saved focus. The stale layout neither resets nor overwrites it. |
| P12 | A tree is closed and reopened on one frame | pop → push of the same name → focus resolution | It lands on its initial focus, as a fresh push does (O14). |
| P13 | A saved focus was rebuilt away or is now disabled | save focus → rebuild → return, or re-entry of a nested group | On return the tree lands on its initial focus. Re-entering a group whose last-focused member is gone or disabled lands on the group's first enabled member. |
| P14 | A direction is held while a tree is pushed or popped | direction down → push or pop → still held | Focus does not move in the newly active tree until the direction is pressed again. A held stick has to return to rest first. |
| P15 | A held direction or slider step spans a long frame | hold → 1 s frame → next frame | At most one repeat fires per frame, so the step never lands past what the player saw. |
| P16 | An authored repeat has a zero delay or a zero interval | the container authors repeat → hold | A zero delay never repeats, even though an engine default exists. A container that authors no repeat repeats at the engine default. |
| P17 | A slider is held while an external write lands on its slot | step computed from v → external write w → both drain | The slot never ends at v plus a step. It ends at w, or at w plus the steps taken after the write. |
| P18 | A scroll container's content shrinks while the container is scrolled | scrolled to end → content shrinks → draw | The offset clamps on that frame, with no empty band. Under `maxHeight`, the container shrinks to its content. The focused child stays visible. |
| P19 | Pointer input or a return lands on a scroll container's clipped region | scroll → click or return on a clipped child | A click on the clipped area activates nothing hidden there. A restored focus outside the viewport scrolls into view by the minimum distance. |
| P20 | Tab intents meet an edge-case tablist | RB/LB with: one tab; none selected; a disabled tab; two intents on one frame; a dialog above the tabbed tree | One tab: no activation, and focus moves to that tab. None selected: RB activates the first tab and LB the last. A disabled tab is skipped. Two intents on one frame advance two tabs. With a dialog on top, the bumpers act on the dialog only. |
| P21 | A directional move off-axis in a nested linear group | focus in a horizontal linear strip → Down | A linear group answers only its own axis. A cross-axis move leaves the group, and wrap applies only along the axis. |
| P22 | Input from two device families arrives in one frame, or a resting device drifts | key + pad in one frame; stick drift inside its dead zone; small mouse motion | One family per frame. Input inside a dead zone never changes the family. Mouse motion changes it only past the pointer-mode debounce. |
| P23 | `hold_timing_scale` changes while a key is held | key down → scale changes → release | The pending resolution keeps the threshold that applied when the key went down. |
| P24 | A key is held across a capturing menu's close | key down in gameplay → menu opens (cancels) → menu closes, key still down | The key does nothing until it is released and pressed again. |
| P25 | A rebuild or demote removes the command a capture prompt is waiting on | prompt open for dash → dash becomes irrelevant | The prompt closes, binds nothing, and saves nothing. The panel keeps focus on the nearest remaining row. |
| P26 | A text-entry commit and a shortcut land on one frame | commit pops the tree → `text_space` resolves | The shortcut adds nothing to the committed text. Shortcuts do nothing while no text-entry tree is on top. |
| P27 | Confirm is pressed twice on the dev EXIT or QUIT | confirm at N → dialog opens → confirm at N+1 | The second confirm lands on the safe choice. The game never exits. |
| P28 | A key is remapped while a text-entry tree is on top | remap Q to `nav_down` → open text entry → press Q | Q types `q`. No nav intent is produced. |
