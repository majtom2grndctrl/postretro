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
- **Device families.** Sony's USB vendor id is `0x054C` and Nintendo's is `0x057E`. Any other vendor gets the Xbox layout. Not yet verified: that the pinned gilrs version exposes `Gamepad::vendor_id` on every platform. The executor confirms it.
