# E23--gamepad-input — plan of record

mode: resumable
status: approved
read at: b9d0a4645

Source re-read at `b9d0a4645` (window-modes landed; research was against `af1d3c1b5`). No Decision premise was found false. Path corrections below.

## Corrections
- gilrs is **0.11.2** (Cargo.lock), not 0.11.1 → `Gamepad::vendor_id() -> Option<u16>` exists there too; no change to approach.
- "`build_sim_command` reads Dash/Shoot/Use/Drop as `tick_index == 0 && Pressed`" → those edges are computed in the tick loop (`main.rs` ~2656) and passed in as bools; `build_sim_command` reads Shoot only as a level. Its select path is dead (`select_pressed` is passed `false`); selection rides `gameplay_input_latch.wieldable_selection_mut().take_pending_commit` on tick 0. Activator phases feed the snapshot the loop already reads, so these sites need no change beyond the snapshot they consume.
- "`GameplayInputLatch` keeps Pressed plus Released for Shoot/AltFire" → the latch keeps Pressed only; Shoot/AltFire press and release latching is `ActivationInputCapture` (`input/activation.rs`) inside the latch. Activator output feeds both unchanged.
- "Key state is a level per key" → `physical_state: HashMap<PhysicalInput, bool>`, last-wins, and gamepad buttons are polled levels (`gamepad.is_pressed`). Buffered edges therefore come from winit key events and from the gilrs event stream that `GamepadSystem::update` already drains; the drain is shared between nav and gameplay rather than polled twice.
- `GamepadSystem::update` has **two** callers (Running `main.rs` ~2222, frontend ~5638); both route through the effective-binding lookup. The frontend path drops `Menu`.
- `InputSystem::clear_all` does not clear `prev_button_states` (a held action reads Released after a clear), and `crouch_toggle_active`, `nav_stick_tracker`, and focus repeat clocks survive focus changes. The activator resolver owns cancellation (Decision: losing focus or a capturing tree cancels a pending resolution); window blur clears input without changing `input_focus`, so blur is wired as its own cancel.
- "An `input` block follows the optional-block precedent: malformed warns and falls back" → only `render`, `audio`, `theme`, `movers` warn-and-fall-back; `switching` and `frontend` are fatal. `input` follows `drain_audio_profile_js` / `_lua`, drained at four sites (`mod_init_exec.rs` JS/Lua, `staged_manifest.rs` JS/Lua).
- "`commit_staged_manifest_result` gains the block" → that core commit covers entities, factions, maps, reactions, stores, identity; `audio`/`switching`/`theme`/`ui_trees` commit app-side in `startup/staged_manifest_lifecycle.rs`. The `input` block commits app-side beside `audio`, and the effective-table rebuild runs after both the core entity commit and the block commit of the same generation.
- `context/plans/drafts/player-descriptor-composition` has **not** landed: relevance reads flat `PlayerMovementDescriptor.dash` / `.crouch` (`foundation/.../movement.rs:37,45`). The exhaustive derivation is the single site that follows that draft when it lands.
- Co-op tuning has no accessor: the binary destructures `NetEndpoint::Client { tuning, tuning_generation, .. }`, and `demote_client_state` sets `tuning = None` without bumping `tuning_generation`. Relevance observes `(tuning.is_some(), tuning_generation)` once per frame and rebuilds on any change, which catches both install and clear (P5).
- No dev catalog map is pawnless (only two uncatalogued `.map`s lack `player_spawn`). The fly-cam row (AL2) is proven against the effective table and the snapshot's MoveUp axis with no pawn, not by loading a map.
- **UI images have no production loader**: `UiImageRegistry::register_uploaded` has no caller and the registry starts empty. Glyph art (Decision: "the block names glyph art per device family") therefore needs a mod UI-image load path: at mod init, PNGs at `<mod_root>/<dir>/<input>.png` for each declared family are decoded app-side and uploaded through the renderer into `UiImageRegistry` under key `<dir>/<input>`. Scoped to glyph directories; no general UI image manifest.
- **Nothing clips** in the UI pass (no scissor; glyphon `TextBounds` is the full viewport). Scroll containers add per-item clip rects to the draw list, a scissor in the renderer UI pass, clipped `TextBounds`, and clipped hit-test rects.
- **Focus groups are flat**: `FocusGroup { kind, wrap, repeat, members }` has no parent, bounds, or container id, and `FocusRect` carries `selected` but not `role`. The export gains group parent, group bounds, and a role/tablist tag.
- `restoreOnReturn` is a container prop ORed tree-wide by `any_restore_on_return`; restore fires on **any** stack change, a fresh push included. Moving it to tree props also removes the container fields (both SDKs, both parsers, serializer).
- P11 is real today: hardware Enter/Cancel pops the keyboard tree in `resolve_text_entry_intents` before `ui_focus.tick`, which then writes the keyboard's initial focus under the revealed tree's key. `FocusRectList.owner {name, tier}` already exists but the focus engine ignores it → the fix gates `tick` initialization on `rects.owner` matching the active key, rather than reordering `main.rs`.
- `RepeatTimer` semantics today: delay ≤ 0 never fires; delay > 0 with interval ≤ 0 fires once. No shipped tree authors nav `repeat`.
- A held slider never repeats (`apply_slider_nav_capture` consumes the edge intent); accessibility sliders route through `route_engine_accessibility_slider` with the same limit.
- `MoveUp` is an axis action, so it splits into `move_up` **and** `move_down` (Q/Z). Both are dev-only: bound to engine defaults, hidden, never in a conflict. Clarification of "`move_up` is dev-only"; meaning unchanged.
- Line counts: `main.rs` 14609, `input/ui_focus.rs` 2370, `input/mod.rs` 1332, `options/mod.rs` 1156, `options/bridge/mod.rs` 913 — all four split-first files still qualify.
- The dev options menu deliberately flattens its tab strip into one linear group, pinned by `production_title_and_options_trees_preserve_composed_control_contracts` (`main.rs` tests); the restructure updates that test's assertions.
- On-screen keyboard key reactions (`kbAppend_*`, `kbBackspace`) live in dev content (`arena-lights.ts`), not the engine. The text shortcuts activate the keyboard tree's own key buttons (`key_backspace`, the SPACE key, `key_done`), so they act through whatever the tree authors.
- "`nav_menu` is live only when no capturing tree is on top" → today gamepad Start also closes an open pause menu from inside it, an AL1 read. `nav_menu` stays live under a capturing tree too, and menu-plus-cancel on one input is exempt from conflicts: under a capturing tree that input acts as cancel, which closes an open menu anyway. Escape is still not a conflict; meaning unchanged.
- "The player wins collisions with changed defaults" → implemented as layer priority: player > author > engine, and within a layer the earlier entry keeps the input. A lower layer's `hold` on a player's short binding also yields (it would delay the press), per PD8. Only a player win flags the displaced command.
- Pre-existing bug found: notch counts came from the wheel alone, so the D-pad's default weapon-cycle bindings did nothing. A press edge from any non-wheel input on a notch-read command now counts one step (`a_dpad_press_steps_weapon_cycling`).
- Epic AC 33 binds only if U3 lands after U4; U4 (`drafts/E23--screen-reader`) has not started, so it is recorded as not applicable unless that changes before landing.

## Delegated answers
- **Command-ID table.** Gameplay (digital unless noted): `move_forward`, `move_back`, `move_left`, `move_right` (digital; a half-axis input carries its stick magnitude, so analog movement survives); `move_up`, `move_down` (dev-only); `look_x`, `look_y` (analog, axes only); `sprint`, `jump`, `dash`, `crouch`, `use`, `drop`, `shoot`, `alt_fire`, `reload`, `select_wieldable_1`…`select_wieldable_10`, `cycle_wieldable_next`, `cycle_wieldable_previous` (wheel-notch), `toggle_last_wieldable`. UI: `nav_up`, `nav_down`, `nav_left`, `nav_right`, `nav_next`, `nav_prev`, `nav_tab_next`, `nav_tab_prev`, `nav_confirm`, `nav_cancel`, `nav_menu`, `nav_options`, `text_backspace`, `text_space`, `text_commit`. — Movement axes split into digital per-direction commands and look axes into analog commands; adding digital look or analog move commands would add keyboard-look, a capability nobody asked for.
- **Gamepad position names (gilrs mapping).** `south`/`east`/`west`/`north` ← South/East/West/North; `left_shoulder`/`right_shoulder` ← LeftTrigger/RightTrigger; `left_trigger`/`right_trigger` ← LeftTrigger2/RightTrigger2 (digital at today's 0.5 threshold); `select`, `start` ← Select/Start; `left_stick_press`/`right_stick_press` ← LeftThumb/RightThumb; `dpad_up`/`dpad_down`/`dpad_left`/`dpad_right`. Axes: `left_stick_x`, `left_stick_y`, `right_stick_x`, `right_stick_y`. Half-axes: `left_stick_up`/`_down`/`_left`/`_right`, `right_stick_up`/`_down`/`_left`/`_right`, active past the stick's dead zone, digital edge at 0.5 for nav. Mode/C/Z are unmapped. — Positional names follow the brief; triggers stay digital because no analog-trigger command exists.
- **Analog polarity table (per source, fixed, physical).** Superseded on 2026-10-06 (owner): today's pad defaults are inverted on all three stick axes against keyboard and mouse (gilrs reports stick up as +1, yet `MoveForward = −LeftStickY`, `LookYaw = +RightStickX` vs `−MouseX`, `LookPitch = −RightStickY`). The table is physical: a source reads + for right and up — `mouse_x` +1, `mouse_y` −1 (the OS reports down as +), every stick axis +1. Analog commands are physical too: `look_x` + is look right → `LookYaw` ×−1; `look_y` + is look up → `LookPitch` ×+1. Mouse reads are unchanged; the sticks now move and look in the keyboard and mouse directions. Half-axis names are physical (`left_stick_up` = raw +Y) and carry magnitude.
- **Keyboard names.** W3C `KeyboardEvent.code` strings via an explicit winit `KeyCode` table (letters, digits, F1–F24, arrows, modifiers, punctuation, numpad, navigation and editing keys); unmapped `KeyCode`s are not bindable. Mouse names as in the brief. Every written string round-trips (PD9).
- **Repeat defaults.** Engine default nav repeat: 400 ms delay, 100 ms interval. A zero authored interval keeps today's single repeat after the delay; a zero authored delay never repeats (P16). — Matches the on-screen keyboard's existing 400 ms backspace delay; 100 ms is a common console menu cadence.
- **Slider acceleration.** A held slider repeats at the nav default (400 ms / 100 ms). Step multiplier ×1 for the first 1.0 s of repeating, ×2 to 2.0 s, ×4 after; clamped at bounds, at most one step per frame (P15).
- **New numeric ranges.** `gamepad_look_sensitivity` 0.5–8.0 rad/s at full deflection, default 2.5 (today's const), step 0.25. `gamepad_look_dead_zone` 0.0–0.5, default 0.15 (today's const), step 0.05; the move stick keeps a fixed 0.15. `hold_timing_scale` 1.0–3.0, default 1.0, step 0.25. Activator thresholds must be finite and > 0, else diagnosed and the entry falls back.
- **Abandoning a capture.** Pressing the slot's current input again closes the prompt unchanged (a same-key rebind is a no-op anyway); losing window focus also closes it unchanged, as does the command becoming irrelevant (P25). No timeout. For an empty slot, the capture binds and the per-command reset undoes it. — Keeps every input capturable and adds no time limit.
- **Default keys for tab and text commands.** Keyboard: `nav_tab_prev` KeyQ, `nav_tab_next` KeyE; `text_backspace`, `text_space`, `text_commit` unbound on keyboard (hardware Backspace/Space/Enter already do it and stay fixed). Gamepad: `nav_tab_prev` `left_shoulder`, `nav_tab_next` `right_shoulder` (so `nav_next`/`nav_prev` are unbound on gamepad, and the tab commands fall back to Next/Prev in a tree with no tablist); `text_backspace` `west`, `text_space` `north`, `text_commit` `start`. Conflict-free per context: Start is `nav_menu` only with no capturing tree, and `text_commit` only under a text-entry tree.
- **GAG tiers** (gameaccessibilityguidelines.com full list, read 2026-10-05). Motor **Basic**: "Allow controls to be remapped / reconfigured". Motor **Intermediate**: "Avoid / provide alternatives to requiring buttons to be held down" (hold-to-toggle) and "Support more than one input device". Motor **Advanced**: "Do not make precise timing essential to gameplay" (relevant to `hold_timing_scale`).
- **Glyph asset convention.** PNG at `<mod_root>/<dir>/<input>.png`, registry key `<dir>/<input>`; keyboard/mouse art uses the input strings (`KeyW`, `mouse_left`).

## AC-to-proof

Row ids number the brief's Acceptance bullets in order within each heading. Planned test names are indicative; final names land in the Status column.

| AC | Proof | Status |
|---|---|---|
| AL1 no block = today's defaults, no conflicts, Escape menu/cancel | `effective_table_without_block_matches_legacy_defaults` + conflict check over engine table | restated (owner, 2026-10-06): every default key, mouse, and button input produces the same action reads as today; the pad sticks move and look in the keyboard and mouse directions instead of today's inverted signs |
| AL2 Q/Z fly-cam with no block; move_up hidden, no conflict on Q | effective-table + no-pawn snapshot test; panel row list test | achievable as stated |
| AL3 ShiftLeft→dash, sprint kbm empty | manifest-validation + resolver test | achievable as stated |
| AL4 accepted activators per command | `activator_acceptance_per_command` (exhaustive over commands) | achievable as stated |
| AL5 guard on author defaults | validation test | achievable as stated |
| AL6 unknown input on dash / unknown command id | validation test with log assertions | achievable as stated |
| AL7 typo fallback collides → unbound; later conflicting entry unbound | validation test | achievable as stated |
| AL8 absent device class keeps engine default; empty unbinds | validation test | achievable as stated |
| AL9 show:false on nav_confirm diagnosed | validation test | achievable as stated |
| AL10 key on analog / axis on digital diagnosed; stick swap moves dead zone | validation + gamepad look test | achievable as stated |
| AL11 `postretro.dev.dash` unknown; rows keyed by ID strings | validation + settings round-trip test | achievable as stated |
| AL12 hot reload recomputes, keeps overrides | staged-commit rebuild test | achievable as stated |
| R1 no dash descriptor → unbound, unlisted, no conflict on F; one of two → relevant | relevance derivation test | achievable as stated |
| R2 reload / crouch / alt_fire relevance | relevance derivation test | achievable as stated |
| R3 hot reload adding dash binds it | staged-commit rebuild test | achievable as stated |
| R4 force-show / force-hide flip | relevance test | achievable as stated |
| R5 panel from frontend before level load (P6) | panel row-list test after mod-init commit | achievable as stated |
| R6 co-op tuning union and clear (P5) | relevance + endpoint observation test | achievable as stated |
| R7 unclassified command fails to compile; no wildcard | exhaustive match + review gate | review gate |
| PD1 changed author default reaches non-rebinders | layering test | achievable as stated |
| PD2 dash row kept while irrelevant, reapplies | settings + relevance test | achievable as stated |
| PD3 empty-list row round-trips | settings round-trip test | achievable as stated |
| PD4 unknown command row survives a save | settings round-trip test | achievable as stated |
| PD5 unknown first entry → author default on slot 1, slot 2 kept | settings + layering test | achievable as stated |
| PD6 every written input string parses back | `every_input_name_round_trips` | achievable as stated |
| PD7 player Q→dash vs later author Q→reload | collision test + panel flag | achievable as stated |
| PD8 player press vs later author hold on Q | collision test + panel flag | achievable as stated |
| PD9 two mod ids keep separate rows | settings test | achievable as stated |
| PD10 rebound key keeps slot activator | layering test | achievable as stated |
| PD11 top-level keys without accessibility slot (catalog assertion); hold_timing_scale has both; bad sprint_mode falls back alone | options + catalog tests | achievable as stated |
| AV1 tap+hold on one key, all six cases | resolver unit tests (deterministic time) | achievable as stated |
| AV2 P1, P2 | resolver buffered-edge tests | achievable as stated |
| AV3 P3 zero-tick tap and hold | latch + resolver test | achievable as stated |
| AV4 lone press / tap / release | resolver tests | achievable as stated |
| AV5 tap 0.1 s vs release 0.15 s; default 0.2 s | resolver test | achievable as stated |
| AV6 hold sprint held state; toggle latch | resolver + sprint-mode test | achievable as stated |
| AV7 hold_timing_scale doubles; panel 1–3; P23 | resolver + panel action test | achievable as stated |
| AV8 tuning install while Shift held; P4 | rebuild test | achievable as stated |
| AV9 P24 key held across menu | resolver cancel test | achievable as stated |
| AV10 conflict cases incl. guarded replace refusal | conflict checker tests | achievable as stated |
| AV11 no wire field for activators | review gate (netcode wire types diff) | review gate |
| CAP1 P7 | capture-prompt tests | achievable as stated |
| CAP2 P8 | capture routing test | achievable as stated |
| CAP3 swap-aware capture | capture test | achievable as stated |
| CAP4 P25 | capture + rebuild test | achievable as stated |
| MC1 restoreOnReturn:false in both SDKs | SDK drain tests (JS + Luau) + focus test | achievable as stated |
| MC2 P11 | focus owner-gating test | achievable as stated |
| MC3 P12, P27 | focus + dev-dialog test | achievable as stated |
| MC4 P13 | focus tests | achievable as stated |
| MC5 P14 | focus test | achievable as stated |
| MC6 P15 | repeat test | achievable as stated |
| MC7 P16 | repeat test | achievable as stated |
| MC8 P17 | slider capture test | achievable as stated |
| MC9 P9, P10 | nav-binding repeat-release tests | achievable as stated |
| MC10 nav_down on right_stick_down | gamepad nav test | achievable as stated |
| MC11 nested group escape / re-entry / Next at end / focusNeighbors / Text never focused | focus tests | achievable as stated |
| MC12 P21 tab strip | focus test | achievable as stated |
| MC13 P20 | tab intent tests | achievable as stated |
| MC14 no tablist → bumpers Next/Prev | tab intent test | achievable as stated |
| MC15 scroll fits / wheel / group membership / HStack diagnostic both SDKs | layout + SDK drain tests | achievable as stated |
| MC16 scroll-into-view by one row | layout test | achievable as stated |
| MC17 P18, P19 | layout + hit-test tests | achievable as stated |
| MC18 P26, P28, backspace repeat | text-entry shortcut tests | achievable as stated |
| MC19 reserved controls-panel name at all paths | registry tests with log assertions | achievable as stated |
| GF1 glyph follows family and rebinding | glyph resolve test | achievable as stated |
| GF2 vendor id → family; dead-zone drift; one family per frame (P22) | family tracker tests | achievable as stated |
| GF3 missing art → input name; irrelevant/unbound → nothing | glyph resolve test | achievable as stated |
| GF4 no auto-swap by family; swap after overrides | layering + glyph test | achievable as stated |
| GF5 Scripting-surface example as a `content/dev` fixture in both SDKs | fixture scripts drained by JS and Luau tests | achievable as stated |
| MN1 tap-dash / hold-sprint feel | owner, in-engine, keyboard + gamepad | manual |
| MN2 controls panel content | owner, in-engine | manual |
| MN3 epic AC 25 gamepad-only pass, Xbox + PS/Nintendo | owner, real pads | manual |
| E2 (hold_timing_scale) unrecognized value falls back alone | options test | achievable as stated |
| E3a hold_timing_scale slot live, readonly | catalog + bridge test | achievable as stated |
| E3b working copy write path | bridge test | achievable as stated |
| E3c engine write reseeds | panel action test | achievable as stated |
| E10 panel carries hold_timing_scale | `the_panel_descriptor_carries_every_accessibility_field` | achievable as stated |
| E13 replication clause covers new entries | existing catalog-derived test | achievable as stated |
| E19–E24 | covered by MC/GF/AV/CAP rows above; cross-checked at report | achievable as stated |
| E25 | = MN3 | manual |
| E33 | only if U4 lands first | not applicable unless U4 lands first |

## Tasks

Split-first files are split before they are extended, each split its own behavior-preserving commit. Task 2 is the riskiest premise (phases ride the latch without disturbing tick-0 edge reads); task 1 is the split it extends.

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | Split `input/mod.rs` by responsibility (latch, physical state, axis resolution) — no behavior change | integrating executor | — | done: `snapshot.rs`, `scroll.rs`, `latch.rs`, `system.rs` (+ `system_tests.rs`); moved code byte-identical but one `pub(super)`; `input::` 199 passed |
| 2 | **Riskiest slice.** Activator resolver (press/release/tap/hold, Steam rule, buffered key and gilrs edges, thresholds captured at key-down, cancel on blur/capture) emitting phases into the snapshot via `GameplayInputLatch`; hardcoded Shift tap-dash / hold-sprint fixture. Proves AV1–AV5, AV9 and the tick-0 premise | integrating executor | 1 | done: premise holds — phases ride `GameplayInputLatch` unchanged (`a_tap_on_a_zero_tick_frame_reaches_the_next_tick_as_a_press`, `a_hold_crossing_on_a_zero_tick_frame_reaches_the_next_tick_and_its_tap_never_fires`). `input/activator.rs` + 20 tests in `activator_tests.rs` (AV1, AV2, AV3, AV4, AV5, AV6 hold part, AV7 resolver part, AV9). Keyboard OS repeats no longer reach gameplay; gilrs button events are timestamped edges, the poll reconciles. Full bin suite 1232 passed; clippy clean |
| 3 | Command and input vocabulary: command IDs, contexts, accepted activators, input names + parse/format, polarity table, engine default table; no-block equivalence | integrating executor (kept in-house: the polarity question needed the owner) | 2 | done: `input/commands.rs`, `input/input_names.rs`, half-axis `PhysicalInput::GamepadAxisHalf`, `input/defaults.rs` keyed by command (legacy tables kept as the test reference in `defaults_tests.rs`). Pad stick polarity fixed per owner (AL1 restated). Proofs: `engine_defaults_keep_every_legacy_key_mouse_and_button_read` + `pad_sticks_move_in_the_keyboard_directions` + `pad_look_turns_and_pitches_in_the_mouse_directions` (AL1 reads; conflict half lands in task 4), `activator_acceptance_per_command` (AL4 table), `every_input_name_round_trips` (PD6), `command_ids_round_trip_and_never_contain_a_dot` (AL11 ID half). `commands`/`input_names` carry a not-test dead-code allow until task 4 consumes them. Bin suite 1243 passed; clippy clean |
| 4 | Effective table + conflict checker + guard; `InputSystem` rebuild refreshing `unique_actions`; held-key rebuild rules (P4); relevance derivation (exhaustive) with tuning observation | integrating executor | 3 | done: `input/{relevance,binding_table,binding_state}.rs`, `InputSystem::set_bindings` + `ActivatorResolver::rebind`, `app/bindings.rs` (`refresh_effective_bindings` per frame in gameplay and frontend, and after mod init). Proofs in `input/binding_table_tests.rs` and `app/bindings_tests.rs`: AL1 conflict half, AL2, AL3, AL8, AL10 stick swap (dead-zone half in task 8), R1, R2, R4, R6 (table level; P5 key includes tuning presence), PD1, PD5, PD7, PD8, PD10, AV8, AV10 (panel replace-refusal in task 10), GF4. Bin suite 1277 passed; clippy clean |
| 5 | Author surface: `input` block in `ModManifestResult`, JS + Luau drains at four sites, per-command degradation, SDK `.d.ts`/`.d.luau` + typegen + parity test, app-side staged commit and rebuild | worker (scripting-core + SDK), integrator owns commit seam | 4 | |
| 6 | Split `options/mod.rs` and `options/bridge/mod.rs` — no behavior change (two commits) | integrating executor | — | done: `options/{persist,tests}.rs`; `options/bridge/{top_level,tests}.rs` (top-level working copies separate from the orchestrating bridge). Moved code identical but `pub(super)` visibility; bin suite 1277 passed (unchanged count); clippy clean |
| 7 | Player data: `[game."<mod_id>"]` reader/writer beside `FieldReader`, diff rows, unknown rows kept, per-slot fallback, player-wins collisions with displaced flags | integrating executor | 4, 6 | done: `options/game.rs` (rows read from the loaded document plus session writes; writes go through `DocumentWriter::put_at`, so unknown rows and other mods' rows keep their text), `input/player_rows.rs` (rows → player layer; unknown command ignored, unreadable/wrong-device/wrong-kind input → `None` slot), loaded at mod init by `App::load_player_bindings`. Proofs: PD2 (`a_player_dash_row_waits_out_an_irrelevant_session_and_applies_when_dash_returns`), PD3, PD4, PD9, malformed-row fallback (`options/tests.rs`), PD5 + AL11 row half (`player_rows` tests). Player-wins collisions landed in task 4; the save on rebind lands with the panel (task 10). Bin suite 1287 passed; clippy clean |
| 8 | New options: gamepad look trio, `sprint_mode`, `swap_confirm_cancel` (top-level + slots), `hold_timing_scale` (group, per-field range, slot, panel entry, SDK `AccessibilityNumericField`); sprint toggle on resolved phases; look dead zone follows the look stick | integrating executor | 2, 6 | |
| 9 | UI nav from effective bindings: `ui_nav` mappers become lookups in both `GamepadSystem::update` callers and keyboard input; repeat release by binding (P9, P10); swap; text-entry context (P28); stick half-axis nav | integrating executor | 4, 8 | |
| 10 | Controls panel: generated `core/ui` tree, reserved name, `ui.openControls` (Rust + both SDKs), rows by category/order, read-only activators, reset / reset-all, conflict replace/cancel, guard refusal, displaced flags | integrating executor | 7, 9 | |
| 11 | Raw capture: `UiIntentPayload` capture path decided App-side after activations while the prompt is active (P7, P8, P25), swap-aware, abandon rules | integrating executor | 10 | |
| 12 | Split `input/ui_focus.rs` — no behavior change | integrating executor | — | done: `input/ui_focus/{mod,repeat,traversal,slider,tests}.rs` (engine 468 lines; 38 focus tests unchanged); clippy clean |
| 13 | Restore on return: tree prop (both SDKs, parsers, serializer), push vs pop, owner-gated tick (P11), P12, P13 | integrating executor | 12 | |
| 14 | Engine default repeat; slider hold-repeat with acceleration (P14–P17) | integrating executor | 12 | |
| 15 | Nested groups: export group parent + bounds, directional escape, last-focused re-entry, linear axis rule (P21) | integrating executor | 12 | |
| 16 | Tabs: role in export, `nav_tab_*` activation and fallback (P20) | integrating executor | 9, 15 | |
| 17 | Scroll container: descriptor + both SDKs, layout clip and offset, renderer scissor and text bounds, clipped hit-test, scroll-into-view, wheel (P18, P19) | worker (renderer UI pass clip), integrator owns layout/focus seam | 15 | |
| 18 | Confirmation dialogs (dev EXIT/QUIT, P27) and on-screen keyboard shortcuts (P26) | integrating executor | 9, 13 | |
| 19 | Glyphs: mod glyph-art load into `UiImageRegistry`, device-family tracker (vendor id, P22), `Glyph({ command })` descriptor + both SDKs + build, swap-aware resolve | worker (image load path), integrator owns glyph resolve | 4, 9 | |
| 20 | Dev-mod consumer: options restructure (tabs, nested groups), spatial level select, CONTROLS entry, EXIT/QUIT confirms, `input` block, glyph art for keyboard/xbox/playstation/nintendo, new option rows; Scripting-surface fixture in both SDKs | integrating executor | 10, 16–19 | |
| 21 | Author docs (`docs/`) for the input block, Glyph, scroll, tabs, restoreOnReturn, `ui.openControls` | integrating executor | 20 | |
