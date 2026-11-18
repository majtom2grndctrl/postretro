# coop--character-and-run-saves — plan of record

mode: resumable
status: approved
read at: 6c3f41ed

## Corrections
- Research says the client builds its claim in `session/mod.rs` → `ConnectClaim` is built in `crates/postretro/src/session/net_entry.rs` (empty `display_name`); `player_id` is minted in `session/mod.rs` and stored on `PlayerOptions`. Planning around it by giving `net_entry.rs` the selected character's id and name.
- "`state.json` is written in full only at clean exit" → `coop--in-app-host-and-join` added a join save: `App::save_progress_for_join` (`startup/session_entry.rs`) calls `session/progress_save.rs::save_world_and_character`, the extracted save entry point the Path names. Planning around it by replacing that function's body with the save-point dispatcher. Clean exit and join both route through it.
- The lobby Decision needs joining players listed at the hosting frontend → `SessionPlayers::publish` clears `session.players` rows whenever no session level is installed (`session/players.rs`), as `scripting.md` §5 says ("Counts and rows hold no players … on a machine with no session level"). Planning around it by publishing **rows only** from the seat table while hosting at the frontend. Each seat shows status `loading` and its claim's display name. Counts stay 0, because gameplay reads counts and the frontend is not a session level. `scripting.md` §5 gains that one exception at landing. **Owner: confirm this reading of the lobby Decision on skim.**
- `SeatTable` has no setter for a seat's carry; only admission (`None`) and `release_seat` (remove) touch `carried`. Planning around it by adding `SeatTable::set_carried_state(seat, Option<CarriedState>)`. Seat 0 sets carry through that one call.
- A backdrop's unload harvests through the generic `App::unload_level` (`startup/lifecycle_net.rs`), which cannot tell a backdrop from a played level. Planning around it by gating `harvest_bound_pawns` on `SessionPlayers::session_level()`, read before `end_level` clears it. Only a played level is harvested.
- The unknown-section AC wants a rewrite to keep a section "byte-for-byte". `serde_json::Value` re-serializes numbers. Planning around it by holding unknown sections as `Box<serde_json::value::RawValue>`. Pretty-printing does not reformat raw values. This needs the `raw_value` feature on sim's `serde_json` dependency. It is already in the lockfile through `gltf`'s `extras`, so the build adds no new crate.
- Run scope has no home in `SlotSchema`, whose `persist: bool` is built at 189 sites in the `entities` chokepoint → planning around it by parsing `persist: true | "run"` into `PersistScope` in `scripting-core` and recording run-scoped slot names on `StoreDeclaration` (9 sites). `SlotSchema.persist` stays true for both scopes; persistence routes a slot to the run file when its declaration lists it. Reconcile compares `SlotSchema`, not `StoreDeclaration`, so `SlotTable` also keeps the committed run-scoped names and `namespace_schema_matches` compares them: a scope change on a live namespace rejects like any schema change.
- The migration AC expects values "recorded from loading it at acd5eae8". Since acd5eae8, `state_persistence.rs`'s load and overlay path has changed only by the added `StateStoreLifecycle::can_collect` (`git diff acd5eae8 HEAD`). Planning around it by recording the fixture's expected values from the current, pre-change loader before Task 6 replaces it. The test pins those literals.

## Delegated answers
- Engine caps on slot counts: **16 character slots, 16 run slots.** Catalog lists are fixed-capacity rows, so the cap sizes the engine slot table and the `forEach` unroll. 16 exceeds any menu page and keeps both small.
- `savedAt`: **wall-clock UTC seconds stored in the run file, published as a `YYYY-MM-DD HH:MM UTC` string.** The string is formatted engine-side with a civil-date conversion. A local-time zone needs a timezone dependency, so it is left as a follow-up. Mods show the string as-is.
- Bounded wait for a leaving client's final character: **2 s of wall time**, the same bound as the host's closing transport (`networking.md` §Leaving and ending a session). A client and its host then give up on the same scale.

## AC-to-proof

Proof names are the planned test names; every row is automated unless marked manual.

| AC | Proof | Status |
|---|---|---|
| F1 Files round-trip every field, loadout and checkpoint | `save_files::tests::character_world_and_run_files_round_trip_every_field` | achievable as stated |
| F2 Multi-seat ledger round-trips into a fresh seat table; live state absent | `ledger_document_restores_every_durable_seat_field_and_omits_live_state` | achievable as stated |
| F3 Unknown section ignored and kept byte-for-byte | `unknown_ledger_section_is_ignored_and_rewritten_byte_for_byte` | achievable as stated |
| F4 Unsupported/unparsable file: warned, never written, occupied+unreadable, verbs refused, delete permitted; older supported version loads | `unreadable_slot_is_occupied_never_written_and_only_deletable` + `supported_older_version_loads` | achievable as stated |
| F5 Save killed before rename leaves previous file byte-identical | `save_killed_before_rename_keeps_previous_file` (per kind) | achievable as stated |
| F6 v4 `state.json` migration, `saves` block: recorded values, default sentiment, empty loadout, no run, id = `player_id`, source kept, no re-migration | `v4_state_migrates_to_character_and_world_once` | achievable as stated |
| F7 Migration killed after each write re-migrates to identical files (P9) | `migration_interrupted_after_each_write_converges` | achievable as stated |
| F8 Over-capacity character file loads in-range part, warns, active slot → 0 | `character_file_past_capacity_loads_in_range_part` | achievable as stated |
| F9 Two characters and two runs independent; delete leaves others byte-identical | `slots_are_independent_and_delete_touches_one_file` | achievable as stated |
| S1 Level start writes run (destination id) then character, after harvest, before install; failed load still resumes destination | `level_start_writes_run_then_character_before_install` + `failed_load_still_continues_at_destination` | achievable as stated |
| S2 Crash between the two writes never pairs earlier level with later gear (P5) | `crash_between_level_start_writes_never_duplicates_gear` | achievable as stated |
| S3 Non-initial life state at level start continues initial (P4) | `level_start_save_stores_initial_life_state` | achievable as stated |
| S4 Mid-level pickup, quit, Continue holds weapon at last save point | `quit_after_pickup_continues_with_weapon_at_last_save_point` | achievable as stated |
| S5 Crash (drop without exit path) loses only play since last save point | `dropped_app_loses_only_play_since_last_save_point` | achievable as stated |
| S6 Dev-picker `loadLevel` and path `restartLevel` after Continue+return leave run byte-identical, write character | `dev_picker_and_path_restart_leave_run_file_untouched` | achievable as stated |
| S7 Saved downed/dead continues initial at full health | `downed_character_continues_initial_at_full_health` | achievable as stated |
| S8 Only host/SP writes world and run | `client_never_writes_world_or_run_files` | achievable as stated |
| C1 `checkpoint` saves run+character; Continue arrives in set; co-op seats spread | `checkpoint_saves_and_continue_arrives_in_its_set` + `coop_continue_spreads_seats_over_checkpoint_set` | achievable as stated |
| C2 Unknown checkpoint falls back with warning; later level start clears it | `unknown_checkpoint_falls_back_and_level_start_clears` | achievable as stated |
| C3 After checkpoint fire and Continue arrival, respawn and late joiner use set | `respawn_and_late_joiner_use_active_checkpoint_set` | achievable as stated |
| C4 Continue at `bridge`, quit, Continue again → `bridge` (P11) | `continue_twice_arrives_at_same_checkpoint` | achievable as stated |
| C5 Level start skips keyed spawns; all-keyed level uses all and warns | `level_start_skips_keyed_spawns_unless_all_keyed` | achievable as stated |
| C6 Client `checkpoint` no-op; outside run writes character only; backdrop writes nothing | `checkpoint_scope_by_role_and_run` | achievable as stated |
| C7 `checkpoint`+`loadLevel` either order → destination start; two in one tick → later | `checkpoint_and_load_level_in_one_reaction_resume_destination` + `later_checkpoint_in_one_tick_wins` | achievable as stated |
| R1 Continue selects run's character, installs level with loadout, run slots, sentiment | `continue_installs_run_with_its_character` | achievable as stated |
| R2 Continue refused: deleted character, recreated slot, missing catalog id; delete permitted | `continue_refuses_orphaned_or_unknown_level_runs` | achievable as stated |
| R3 `newRun` into another character's run refused; empty or own permitted | `new_run_slot_ownership` | achievable as stated |
| R4 `newRun` resets run slots, sentiment, level; keeps world and character | `new_run_resets_run_scope_and_keeps_world_and_character` | achievable as stated |
| R5 `newRun` writes run before load; crash in first level resumes it (P7) | `new_run_writes_run_file_before_load` | achievable as stated |
| R6 SP with backdrop: new character carries gear level 1→2; second new character starts empty (P2) | `new_character_carries_gear_and_second_starts_empty` | achievable as stated |
| R7 Backdrop installed: select A (gear), B (none), Continue B → empty, A unchanged (P2) | `selecting_another_character_never_takes_backdrop_or_prior_gear` | achievable as stated |
| R8 Selecting B after A: per-owner slots B's or default, none A's | `select_character_replaces_per_owner_values` | achievable as stated |
| R9 Non-persisted mod slots default after `newRun`/Continue; engine slots unchanged; level change keeps both | `store_reload_resets_mod_slots_and_keeps_engine_slots` | achievable as stated |
| R10 `deleteCharacter` selected, `loadLevel` spawns empty, writes no character (P12) | `deleted_selection_spawns_empty_and_writes_nothing` | achievable as stated |
| R11 `persist: true` global set outside run survives return + select | `world_global_outside_run_survives_frontend_and_select` | achievable as stated |
| R12 Migrated global now `persist: "run"` → implicit run (no block) / dropped (block); never world | `migrated_run_scoped_global_routes_by_saves_block` | achievable as stated |
| R13 `persist: true` global written in run survives return + verbs + restart | `world_global_in_run_survives_verbs_and_restart` | achievable as stated |
| R14 Boot-restored selection then join/load sends and spawns that loadout; join save keeps it (P1) | `boot_selection_seeds_join_and_spawn_loadout` | achievable as stated |
| R15 Restart restores selection; options file holds neither; bad stored selection → -1 with warning, never written | `selection_restores_from_save_dir_and_rejects_bad_slots` | achievable as stated |
| R16 Deleting selected run/character → -1, kept across restart (P10) | `deleting_selection_stores_minus_one` | achievable as stated |
| R17 `createCharacter` into occupied refused | `create_character_refuses_occupied_slot` | achievable as stated |
| R18 Rows equal declared count; cap clamp warns; lowered count keeps files hidden | `saves_rows_match_declared_count_and_cap` | achievable as stated |
| R19 No `saves` block: implicit character+run keep progress and sentiment across restart, frontend and level changes; migration keeps sentiment | `mod_without_saves_block_keeps_progress_and_sentiment` | achievable as stated |
| V1 Every verb refused in played level/loading and as client; permitted at frontend, with backdrop, while backdrop loads | `save_verbs_gate_by_scene_and_role` | achievable as stated |
| V2 Hosting at frontend: own Continue and `newRun` permitted, clients follow; others refused; no selection blocks host/join/`newRun` | `hosting_frontend_verb_gate` + `no_selection_blocks_session_entry_and_new_run` | achievable as stated |
| V3 Guest admitted at lobby keeps per-owner and loadout through host Continue/`newRun` and final character | `lobby_guest_keeps_values_through_host_continue` | achievable as stated |
| V4 Second verb after queued run load refused (P6) | `verb_after_queued_run_load_is_refused` | achievable as stated |
| V5 `persist: "run"` refused per-owner, accepted global | `persist_run_scope_declaration_rules` | achievable as stated |
| N1 Host gear check: unknown parts dropped, magazine and health clamped, life state → initial, bad active slot → 0, valid restores whole | `join_seed_loadout_is_checked_against_host_mod` | achievable as stated |
| N2 Rejoin within hold spawns held seat's loadout | `rejoin_within_hold_keeps_held_loadout` | achievable as stated |
| N3 Guest pickup then host end saves weapon (P3); own leave after pickup saves it | `final_character_includes_gear_since_last_level_start` | achievable as stated |
| N4 Client writes character per loadout message; periodic save keeps it (P8) | `client_saves_each_loadout_message_and_periodic_keeps_it` | achievable as stated |
| N5 Leaving client with silent host closes at bound, saves last sent loadout | `leaving_client_gives_up_at_bound_with_last_loadout` | achievable as stated |
| N6 Loadout messages reach only own seat's client; roster carries none | `loadout_messages_reach_only_their_owner` | achievable as stated |
| N7 Roster shows character name truncated at claim cap | `roster_shows_character_name_truncated` | achievable as stated |
| N8 Host's files never hold a guest's rows or loadout | `host_save_files_hold_no_guest_state` | achievable as stated |
| N9 Two characters from one device each reclaim only own held seat | `two_characters_one_device_reclaim_own_seats` | achievable as stated |
| N10 Grep gate: `crates/net` no engine crate; loadout wire type strings+integers only | xtask surface gate `net_loadout_wire_is_names_only` | achievable as stated |
| D1 Join-then-leave seat-0 carry test replaced: carry equals selected character's saved loadout | `join_then_leave_restores_seat_zero_carry_from_selected_character` (replaces `join_then_leave_leaves_seat_zero_with_a_fresh_ledgers_carry`) | achievable as stated |
| D2 `docs/`, SDK comments cover `saves`, `persist: "run"`, verbs, `checkpoint`, `getGameState().saves`; FGD `checkpoint` key | doc review + SDK drift guards + FGD parse test | achievable as stated |
| D3 Scripting surface example runs as `content/dev` fixture in TS and Luau, slots minted | `saves_surface_fixture_ts_and_luau_load` | achievable as stated |
| M1 Dev mod SP: create, run, two levels, quit, Continue at level 2 with gear; checkpoint continue | owner, in-engine | manual |
| M2 Loopback co-op lobby, guest gear in and out, Cancel ends session | owner, in-engine | manual |
| M3 Gamepad-only character and run management | owner, in-engine | manual |

## Tasks

Each task commits with its `plan.md` row update. Hot paths: none per-frame. Saves run at level starts, checkpoints and verbs. The client's periodic save stays on its 60 s timer, and a loadout message triggers one file write. No task allocates in the tick.

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | **Thin slice (riskiest premise).** New `sim::scripting::save_files` module: ledger document, seat record, names-only loadout record mirroring `CarriedState`, raw unknown sections, atomic character-file write/read. Add `SeatTable::set_carried_state`. Prove that a harvested carry written to a file and read back into a fresh seat table restores whole through `spawn_from_player_starts_with_carried_loadout`. Covers F1 (character part), F3. | integrating executor | — | done: `save_files::tests` (7), `character_file_slice_tests::harvested_carry_restores_whole_in_a_later_process_through_a_character_file` |
| 2 | **Behavior-preserving split.** Move save collection, path and file I/O out of `state_persistence.rs` and the save hooks out of `main.rs` into `session/` modules. No behavior change. Own commit. | integrating executor | 1 | done: `state_persistence/` split into core, `join_seed`, `client_session`, `tests` (34 tests unchanged, pass); `main.rs` client-save and join-seed helpers moved to `session/store_saves.rs`; `session_entry` + `session_membership` pass (64) |
| 3 | **File layer.** World, run and `selection.json` files; slot directory layout; version range; unreadable slots occupied and never written; capacity clamp on load; delete; kill-before-rename seam. F1, F2, F4, F5, F8, F9. | integrating executor | 2 | done (file level): `save_files::tests` (11), `seat::ledger_document_test` (F2); verb refusals on unreadable slots land with Task 8 |
| 4 | **Declarations.** `saves` manifest block (`SavesManifest`, 16/16 caps, clamp warning), `persist: "run"` (`PersistScope`) through `store_bridge` and schema, per-owner refusal. TS and Luau SDK types. V5, part of R18. | delegated worker, integrated | 2 | done: `store_bridge::tests::persist_run_scope_declaration_rules`, `staged_manifest::tests::staged_manifest_carries_run_scope_and_saves_in_both_vms`, `mod_init_exec::tests::mod_init_saves_match_in_both_runtimes`, `data_descriptors::tests::saves_manifest::*` (5), `core::tests::committed_run_scoped_slots_follow_each_declaration_commit`, `slot_table::tests::reconcile_rejects_changed_persist_scope_and_accepts_the_same_scope`; scripting-core lib 1131 pass; typedef drift 63 pass |
| 5 | **Engine slots.** `saves.characters` / `saves.runs` catalog lists and `saves.character` / `saves.run`, engine-written; `savedAt` formatting; generated SDK refs and drift guard. R18. | integrating executor | 3, 4 | done: catalog lists `saves.characters` / `saves.runs` (cap `SAVE_SLOT_CAPACITY` 16, which `MAX_*_SLOTS` now derive from) and `saves.character` / `saves.run` (-1 default); typedefs regenerated; script-compiler mirror updated; `saved_at_formats_utc_calendar_minutes`; entities `engine_state` 16, sim typedef 55, script-compiler 54, scripting-core 1131 pass. Row writer lands with Task 6, which owns slot state |
| 6 | **Save state service and boot.** App-side `SaveSlots` (selection, active run, implicit slots for a mod without a `saves` block). Boot load replaces `load_persisted_state`. One-way `state.json` migration, presence file last. Store reload (defaults, then world, character, run; seat 0 only while hosting). F6, F7, R11–R13, R15, R19. | integrating executor | 3, 4, 5 | |
| 7 | **Save points and seat-0 carry.** Level start (after harvest and life-state clear, before `begin_level_load`; run then character; destination id; path loads leave the run level). Clean exit, join, leave, return to frontend. World file at host and single-player save points. Backdrop never harvested. Seat 0 carries from the selected character. S1–S8, R10, R14, P1, P2, P4, P5, P12. | integrating executor | 6 | |
| 8 | **Save verbs.** `SystemReactionCommand::{ContinueRun, NewRun, DeleteRun, SelectCharacter, CreateCharacter, DeleteCharacter}`, TS primitives and Luau mirrors. Frontend/role/hosting gate with the P6 latch. Continue and `newRun` semantics; hosting, joining and `newRun` need a selection. R1–R9, R16, R17, V1, V2, V4. | integrating executor | 7 | |
| 9 | **Checkpoints.** `checkpoint(name)` system step (TS and Luau). `player_spawn` `checkpoint` key. Active spawn set threaded through first-player order, joinable sites, respawn and `spawn_pose`. Continue arrival keeps the checkpoint; FGD key. C1–C7. | integrating executor | 8 | |
| 10 | **Wire and host gear check.** Names-only loadout mirror in `net`; `JoinSeed` / `FinalCharacter` gain it. New server per-owner loadout message at level start and checkpoint; new client final-character request. `WIRE_VERSION` bump. Host-side gear check. A held seat wins on rejoin. `end_host_session` harvests before encoding. Claim = character id + name. N1, N2, N3 (host side), N6, N7, N9, N10. | delegated worker (wire + netcode), integrated by executor | 7 | |
| 11 | **Client character saves.** Writes on each loadout message; periodic save uses the last-sent loadout; leave waits up to 2 s for its final character, then sets seat 0 from the selected character. N3 (client side), N4, N5, N8, V3, D1. | integrating executor | 10 | |
| 12 | **Lobby rows and dev frontend.** Hosting-frontend `session.players` rows (Correction 3). Dev mod gets character select/create/delete, a run list with Continue/New game/delete, and a lobby screen with Cancel. The Scripting surface example becomes a TS and Luau fixture, with its slots minted. D3; enables M1–M3. | delegated worker (content + UI), integrated by executor | 8, 9 | |
| 13 | **Docs.** `docs/` and SDK doc comments for `saves`, `persist: "run"`, verbs, `checkpoint`, `getGameState().saves`; FGD `checkpoint`. D2. | delegated worker | 9, 12 | |
| 14 | Preflight, review panel, fix loop, full gate, results column. | integrating executor | 1–13 | |
