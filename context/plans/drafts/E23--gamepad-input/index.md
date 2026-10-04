# E23--gamepad-input

Seed · `/draft-session` complete; next step is `/draft-brief` for E23 U3 · read at 1527f5b26

This is the session handoff for the input-binding half of U3 (`ready/E23--accessibility/index.md` §U3), recorded so a later session can pick it up. The owner folded a game-author binding layer and tap/hold triggers into U3. U3's menu-conventions stage is unchanged and is not covered here. U3 also owns settings scope (machine vs game), taken over from `drafts/window-modes` (§Settings scope).

## Problem
Every game built on PostRetro gets the same hardcoded keys (`input/defaults.rs`: Shift→Sprint, F→Dash). A game author can't pick which commands their game uses or set its default keyboard and gamepad bindings, and nothing skips the commands a game doesn't use. A dash-less game still has F bound to a dead command, and that dead binding can still collide with other keys. Player rebinding is planned in U3 but has no author layer under it, and today one tap or hold of a key can't drive two commands.

## Outcome
Game authors pick their game's commands and set default bindings (keyboard/mouse and gamepad), including tap/hold triggers. Which commands are relevant is derived from the mod's data, and authors can override it. Players rebind key and trigger on top. Their changes are saved per game, as a diff over the author's defaults. The API is shaped so mod-defined custom commands can be added later without a migration.

## Verified facts
- Commands are already separate from keys. `Action` (closed enum, `input/types.rs`) is read via `ActionSnapshot`; `Binding{input: PhysicalInput, action, scale}` maps keys to commands. Bindings are fixed at `InputSystem::new` (no setter; the `unique_actions` cache comment expects one).
- `Action` is used only inside the postretro crate; no serde on `Action` or `PhysicalInput`.
- The wire carries resolved intent, not commands: `net::wire::InputCommand` → `WireMovementInput{dash_pressed, running, crouch_intent, …}`, built by `build_sim_command`. Bindings and triggers are client-local; the wire is unchanged.
- UI nav bypasses the binding table: `nav_intent_for_key`, `nav_intent_for_gamepad_button`, and `StickNavTracker` (`input/ui_nav.rs`). South is both Jump and Confirm.
- Dash reads only the press edge, on `tick_index == 0`; Sprint is a held-state read (`main.rs`). Crouch hold/toggle is resolved upstream of the simulation (`resolve_crouch_intent`). Input-layer timing precedent: the `WieldableSelection` dwell timer, tuned from `SwitchingDescriptor`.
- There's no author input surface. `postretro.toml` is read only by the tooling (`crates/tool/src/manifest.rs`). `ModManifest` → `ModManifestResult` has no input field; it's committed at mod init and hot-reloads.
- Capability switches:
  - `PlayerMovementDescriptor.dash` / `.crouch: Option<..>`: absent means disabled.
  - Reload is meaningful only for magazine-type `WeaponResource`.
  - `ground.speed.run` is required, so Sprint has no switch.
  - AltFire has no consumer (`FireMode` is Semi|Auto).
  - MoveUp is read only by the fly-cam when no pawn exists.
- Movement descriptors resolve at spawn via `player_spawn` `entity_class`. A mod may define several, and the rebind menu can open from the frontend before any spawn.
- `ModManifestResult.id` is required (`validate_mod_manifest_id`) and frozen at first commit across hot reload. Persisted mod state is already keyed by it (`state_persistence::state_path`). What happens when two games share an id is a §Settings scope question.
- Settings are one file per OS user: `ProjectDirs("", "", "postretro")` → `settings.toml`, shared by every game built on the engine.

## Not verified
- Whether `air.jumps = 0` disables jumping (Jump is treated as always relevant).
- Use/Drop semantics beyond the `sim/touch.rs` names.

## Decisions
1. **Fold into U3** (owner). U3's remapping stage grows to cover the author layer, tap/hold triggers, and per-game player overrides. Amend the U3 contents and acceptance criteria in the epic index.
2. **Authors pick commands and set defaults; no custom commands yet** (owner). The API assumes custom commands are coming:
   - Commands are identified by stable snake_case string IDs (`dash`, `wieldable_select_3`); the engine enum converts to and from them.
   - Engine commands are built-in entries in a command registry. Mod commands later take a `<mod_id>.<name>` ID in the same key space.
   - Saved files and the manifest never use enum indices.
3. **Author surface.** An optional `input` block on `ModManifest`, validated at mod init and hot-reloadable. Per command it carries an optional label, category, menu order, a forced show/hide, and default bindings per device class, each with a trigger. With no block, the engine default table applies, so the dev mod keeps working.
4. **Relevance is derived, with an author override** (owner):
   - At mod init, take the union over all of the mod's descriptors: Dash if any descriptor has dash, Crouch likewise, Reload if any weapon has a magazine resource.
   - AltFire and MoveUp are never relevant in shipped play (no consumer, and fly-cam only).
   - Every other command is relevant.
   - The input block can force a command shown or hidden.
   - An irrelevant command is unbound, absent from the rebind menu, and never part of a conflict.
   - Mod-global descriptors are the intended authoring pattern (owner).
5. **Effective binding.** Player override, else author default, else engine default; resolved per (command, device class). A runtime `set_bindings`-style rebuild runs at mod init, on hot reload, and on rebind, keeping input state and preferences.
6. **Saved data is a diff, scoped per game:** `[game."<mod_id>".bindings.<device_class>]` rows keyed by command ID. The mod id is quoted because ids may contain `.` (the dev mod's is `postretro.dev`). U3 builds the `[game."<mod_id>"]` reader, read at mod init once the id is known; later game-scoped settings join the same section.
   - A missing row follows the author's default, so a later change to a default reaches players who never rebound that command.
   - An empty list means explicitly unbound.
   - Rows naming unknown commands are kept on disk and ignored.
   - An unknown key string falls back to the default for that binding only (I7).
7. **Keys are saved by physical position.** Keyboard keys use W3C `KeyboardEvent.code` (`KeyW`, `ShiftLeft`, which winit `KeyCode` mirrors). Gamepad buttons are named by position (`south`, `east`, `left_shoulder`). Glyphs and labels are worked out at display time (U3 glyph work).
8. **Triggers live on bindings, not commands:** `press` (default), `release`, `tap{max}`, `hold{min}`.
   - Authors set defaults.
   - Players can change key and trigger kind per binding (owner).
   - Thresholds come from author defaults, scaled by one global accessibility setting for hold timing (a field in the `[accessibility]` group).
   - Starting defaults: tap ≤ 0.2 s, hold ≥ 0.2 s.
9. **Sharing a key uses the Steam rule** (owner). When tap and hold bindings share a key, neither fires until release (under the threshold: the tap) or the threshold passes (the hold, never the tap).
   - A `press` sharing a key with a `hold` is held back the same way.
   - A key with a single binding fires on the press edge with no added delay.
   - A trigger resolves to a one-shot that `build_sim_command` reads, so a dash resolved on release survives a frame with zero ticks.
   - A focus change mid-hold cancels the pending resolution: neither command fires.
10. **Command sets.** Gameplay and UI nav are separate sets, and conflicts are checked only within a set, so South as both Jump and Confirm stays legal. Nav commands are always relevant, can't be hidden, and keep U3's confirm/cancel guard. Author defaults are validated against that guard at mod init.
11. **Docs.** In the same change, amend `player_options.md` §6 ("mods do not extend" becomes "authors pick and set defaults; custom commands later") and `input.md` §2/§7 with the layering, trigger, and command-set contracts.

## Non-goals
- Mod-defined custom commands (the shape is reserved by decisions 2 and 6).
- Chord/modifier bindings and double-tap. The trigger enum stays open for them; double-tap adds a second delay window.
- A press-then-hold trigger (owner chose the Steam rule only).
- Per-level relevance. Use, Drop, and weapon slots depend on level content and stay always relevant.
- Steam Input API integration (`input.md` §9).

## Settings scope
Taken over from `drafts/window-modes`, which kept only window modes; per-game user directories went to `drafts/game-user-dirs`. Candidates, not decided:
- Top-level keys stay the implicit machine scope; no `[machine]` section, no migration. Saving already preserves unknown tables (`DocumentWriter`).
- Classification. Machine/person: graphics quality and `render_resolution`, `window_mode`, the `[accessibility]` group with `view_feel_scale`, `mouse_sensitivity`, `invert_y`, `scroll_notch_pixels`, `crouch_mode` and the planned `sprint_mode`, `player_id`, `accessibility_panel_shown`, `switch_cycle_dwell_ms` (a player preference; the window-modes validation agreed). Game: bindings. U2's `theme_variant` is machine-scoped but stores a per-mod variant id.
- Mod-declared gameplay settings are persisted mod-state slots, not `settings.toml` entries.
- Two games sharing a mod id share per-game data; changing an id drops it. Once `drafts/game-user-dirs` gives each shipped game its own directory, this matters only where projects share a directory: the `postretro` directory of bare engine and `xtask` runs, and a future multi-mod hub. Authoring and SDK runs get their project's own directory.
- Record the outcome in `player_options.md`.

## Proof
Automated:
- No input block: effective bindings equal `default_bindings()`, and the dev mod's behavior is unchanged.
- Author binds `ShiftLeft`→dash and gives sprint no default: Shift drives `dash_pressed`, F drives nothing, `running` stays false.
- Relevance:
  - A mod with no dash descriptor: Dash is unbound, missing from the rebind list, and its engine default key doesn't conflict with an author binding on F.
  - A mod where one of two descriptors has dash: Dash is relevant.
  - Force-show and force-hide overrides each flip the derived answer.
- Diff layering:
  - An unrebound command follows a changed author default after reload; a rebound one keeps the player's key.
  - An empty-list row stays unbound across save and load.
  - An unknown command row survives a save untouched.
  - An unknown key string falls back for that binding only.
- Per-game scope: game A's overrides don't apply when game B loads from the same `settings.toml`.
- Tap/hold on one key:
  - Released before the threshold: the tap fires once, the hold never.
  - Held past the threshold: the hold fires, the tap never.
  - Release exactly at the threshold: pin the inclusive side.
  - Release and re-press within one frame.
  - A frame with zero ticks still delivers the tap one-shot.
  - A focus change mid-hold: neither fires.
- A single-binding dash key fires on the press frame, with no delay.
- The global accessibility hold-timing scale moves both thresholds.
- Player trigger-kind changes round-trip through `settings.toml`.
- Hot reload of the input block recomputes effective bindings and keeps player overrides.
- Command sets: South bound to both Jump and Confirm is not a conflict; two gameplay commands on one key with the same trigger is a conflict, reported before it applies.
- Mod-init validation rejects author defaults that leave confirm or cancel unbound.
- Encoding of `InputCommand` and `WireMovementInput` is unchanged (no `wire.rs` diff).

Manual:
- Playtest tap-Shift dash plus hold-Shift sprint on keyboard and gamepad: dash timing feel at 0.2 s, and the sprint start doesn't stutter.
- The rebind menu shows only relevant commands, with author labels and categories, and lets the player pick a trigger kind.
- Gamepad-only rebinding pass (folds into U3 AC 25).

## Open owner questions
None for the binding layer.

## Route
Brief: the U3 resumable brief at this path, replacing this seed. The owner folded this work into U3. The saved `[game."<mod_id>".bindings]` shape, the manifest `input` block, and the command-ID vocabulary are surfaces players and modders depend on, and they're one-way doors, so they need review before code. One integrating executor owns how the work splits into tasks.

## Grounding: precedent survey
- Every engine surveyed puts triggers on the binding. Unreal Enhanced Input uses Triggers on mappings; Unity uses Interactions on bindings.
- Saved player changes:
  - Unity `SaveBindingOverridesAsJson` stores only the diff.
  - Unreal `EnhancedInputUserSettings` keys mappings by (mapping name, slot).
- Shared-key rule: from Steam Input activators. An interruptible press is held back until release or the long-press threshold.
- Physical key names: Godot `physical_keycode`, Unity `<Keyboard>/a`, and Bevy `KeyCode`.
- The rebind menu lists a declared subset with labels in Source `kb_act.lst` and Steam's In-Game Actions (IGA) manifest.
- Default thresholds: Unreal and Unity both use tap ≤ 0.2 s. Unity's hold is 0.4 s and Unreal's 1.0 s; both are too slow for a sprint in a fast-paced shooter.
