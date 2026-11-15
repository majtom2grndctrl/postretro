# E18--coop-activation-policy — plan of record

mode: resumable
status: landed
read at: 01d5db4f

## Corrections
Every Decision premise holds at `01d5db4f`. The sequencing prerequisite, the first-command presence fix, merged after the brief's read point (`84e9bac7`, `28084927`, `16b5f693`): each host tick's player set is every owned remote pawn plus the host's own. The corrections below change the Path only.

**Sim and status**
- `player_is_alive_for_trigger_occupancy` lives in `crates/sim/src/sim/mod.rs:275`, `#[cfg(test)]` only → the live rule is inlined in `TriggerStageCapabilities::capture` (`sim/player_life.rs:52-93`). Status reads the capture, not the helper.
- The capture is a local of `simulate_started_tick_with_presentation_aim` (`sim/mod.rs:801`); its `pawns` / `occupants` are `pub(crate)` and `TriggerSystem::effective_occupants` is private → the sim aggregates per-tag occupancy and the tick's counted-player set while the capture lives, and returns both on `TickEvents`. Recomputing after the tick would read post-damage health and break P7.
- `PlayerId::Remote` carries a client id, not a seat → map through `registry.seat_for_pawn`.
- `SeatTable.pawn_bindings` keeps a stale id after demotion (`netcode/src/host.rs:437-460` never clears it) → a seat's live pawn is `registry.pawn_for_seat(seat)`, as the brief's Path already says. A held seat is `seat != 0 && client_for_seat(seat).is_none()`.
- No flag distinguishes a session level from a frontend backdrop; the host-backdrop check exists only during install (`lifecycle_install.rs:216`), and a backdrop install still binds seat 0 → add a session-level flag set at non-backdrop install and cleared at unload and leave. Ticks do not run between levels, so unload and `apply_role` zero counts and rows explicitly.
- The headless wrapper `sim/tick_start.rs` still builds presence from commands only → puzzle harnesses use the production presence path.
- `trigger_system.rs` is 2549 lines, about 703 of them non-test (tests are inline from :704). Occupant state is the `occupants` field, `clear`, `occupancy`, the retain, the enter/leave edits in the edge loop and `effective_occupants`. The split still goes in its own behavior-preserving commit; the enter `insert` also decides `entered`, so the new owner returns it.

**Catalog, families and lists**
- The catalog tree is `Object` / `Leaf` only, `validate_sdk_path` admits only identifier segments, and the build-time mirror keys static leaves by slot name, so `session.players.0.present` cannot be a static leaf → the catalog gains `Family` and `List` nodes. Each gets runtime installers in `game_state_refs.rs` (both VMs), a build-time twin beside `MIRRORED_GAME_STATE_LEAVES` (`script-compiler/src/light_membership.rs`), typedef emit, and a drift guard modeled on `build_time_by_player_installers_match_runtime_bridge`. The path = slot mirror test excludes them. Luau's `occupancy` interior gets a frozen metatable for `:tag`.
- `SlotTable` has no remove, `insert_namespace` rejects a repeat namespace, and `replace_player_defense_slots` drops values → a new value-preserving prefix replace for families (P22), in a `slot_table` submodule (the file is at its size threshold). Families insert `SlotOwnership::Engine`, `SharedGlobal`, `default: Some(0.0)`. A `None` value arms a crossing without firing (`state_crossings.rs:164`), which would break P1, and Mod ownership would be filtered from the replicated schema for lack of a durable key.
- Families register no namespace record, so the overlap check alone would not catch a mod `occupancy` store → reserve explicitly in `StoreDeclarationSet::add` (`slot_table.rs:179`) and `plan_reconcile` (:302).
- `MAX_CLIENTS` is a private `const` in `netcode/src/lib.rs:395` → it moves to `foundation`, which `entities` and `netcode` both depend on. List capacity is `MAX_CLIENTS + 1`.
- `engine_state_catalog.rs` (about 1240 non-test lines, mostly data) and `light_membership.rs` (about 1660 non-test) are past the split threshold → family and list code goes in new sibling modules, not those files.

**Lifecycle points**
- `rebuild_active_reaction_subscribers` does not reset the state schema; `staged_manifest_lifecycle.rs:302` does, before the rebuild at :306 → staged-commit mod families are declared before :302.
- Mod UI binds warn only at first init (`splash_lifecycle.rs:507`, after `drain_manifest_registrations` :487); a staged commit never warns → first-init mod families go between :487 and :507. At first init no accumulators bind; they bind at the next level install.
- The level UI bind warning (`lifecycle_world_cpu.rs:250`) precedes `populate_level_with_trigger_events` (:268) → level families go between `validate_sequence_primitives` (:245) and :250, collected before `ui_trees` is taken (:255) and `reactions` moved (:268). Volume tags exist from :163, so the absent-tag warning is decided there. Unload removal goes in `unload_level` (`lifecycle_net.rs:167`); `lifecycle_unload_tests.rs:78`, which asserts the table survives unload, is updated.
- Crossing detection runs once per frame after the tick loop (`frame_loop/mod.rs:1564`), not per tick. This matches the brief's catch-up caveat. The write goes in `frame_loop/mod.rs` after the sim tick is absorbed (~:1041) and before `evaluate_slot_accumulators` (:1078), on host and single player.

**Roster and replication**
- A second privacy test, `assert_claim_is_absent_from_roster` (`netcode/src/seat/roster_harness_test.rs:148`), and doc comments on `RosterEntry` (`net/src/wire/control.rs:394`), `ClientSessionStatus` (`endpoint.rs:186`) and `roster_entries` (`seat.rs:476`) state the name-free roster → all rewritten with the contract. `WIRE_VERSION` is pinned in three tests (`handshake.rs:1063`, `transport.rs:3150`, `defense_slots_test.rs:212`); the ignored xtask `wire_version_matches_main` fails by design until merge.
- `ClientSessionStatus` exposes no `entries` → add an accessor.
- `HostStateReplication::ingest_frame` is `#[cfg(test)]`; production is `ingest_frame_and_collect_sampled_weapons` (`state_slots.rs:856`) with SharedGlobal values from `shared_source_value` (`owner_source.rs:10`). Engine SharedGlobal slots need no replication change. A late joiner's first snapshot is all `FullBaseline` records, not "baseline 0".

**UI**
- `Predicate` is one source plus `equals` / `compare` (`descriptor/values.rs:188`), with no conjunction → `forEach` sets `visibleWhen` to the row's `present` when the row declares none, and otherwise wraps the row in a container gated on `present`. A hidden container skips its subtree (`ui_tree.rs:216`), so both conditions hold without a wire change.
- `repeat` is `focus.repeat`, not a container prop. It is a name to avoid, nothing more.
- Same-row id references to rewrite: `id`, `focusNeighbors.{up,down,left,right}`, Button / Slider `labelledBy`. Six export inventories list `postretro/ui` members (`luau_prelude.rs:315`, :346; `luau_require.rs:510`; `light_membership.rs:768`; `sdk/lib/prelude.ts:108`; `typedef/tests/surface.rs:183`).

**Harnesses and content**
- The observability batch driver passes no trigger context (`observability/driver.rs:313-332`), and no test drives `redraw`. `ghost_links_surface_tests.rs` `Level` (:203) runs the production sim tick on a compiled dev map with a remote partner but skips accumulators and is file-private → a shared headless harness (production sim tick, the new write, accumulators, crossings) is built from it. The two-endpoint `presentation_conditioned_fixture.rs` `Harness` (:514) drives a standalone `TriggerSystem`, not the sim tick → it is extended to run the sim tick and the new write.
- `content/dev/scripts/coop-two-button-puzzles.ts` uses a store workaround and no map loads it; its WALL notes are stale → it is rewritten on the families and loaded by the new dev map.

## Delegated answers
- **Status-only roster rate limit:** 250 ms, measured on `hold_clock`, one gate for the whole roster. Any status-only change publishes at most once per 250 ms, and the publish carries every seat's current status, so the last flip and another seat's change both land within one interval (P19). Bind, admit, hold and release still publish at once. 250 ms is invisible on a display list and caps a flapping state at four reliable frames a second.
- **Dev map:** a new `content/dev/maps/coop-activation-test.map` with its TS and Luau scripts. `ghost-links` and `coop-presentation-test` each carry surface tests pinned to their layouts, and all four consumers need dedicated geometry: two plates, N separated plates, a room, a hill.
- **Empty `occupancy` namespace for a mod with no reference:** no. Families register no namespace record, and the reservation is an explicit name check, so nothing needs a placeholder. Peers with the same scripts derive the same (possibly empty) family set either way.

## Session notes
- Build environment: the workspace needs rustc ≥ 1.99 (`rustup update stable`) and `libasound2-dev libudev-dev` for `cargo check`. The session disk allowance is tight: build with `CARGO_INCREMENTAL=0`.
- Committed typedefs regenerate with `cargo run -p postretro-sim --bin gen-script-types`; copy both outputs to `crates/sim/src/scripting/typedef/tests/fixtures/expected.d.{ts,luau}`, which the full-registry snapshot tests compare.
- Netcode replication fixtures that observe only their own records call `localize_engine_shared_slots`, since the counts are the first shared engine slots.

## Clarifications
- `session.players` is a frozen array of row refs in QuickJS and a frozen sequence in Luau (rows 1..capacity, row `i` holding slot row `i - 1`); both carry a non-enumerable `__list` marker so `forEach` accepts only engine lists. Typed `StateList<Row>`.
- Luau `forEach` passes `i` from 1, as `ipairs` does; row ids carry the slot row (`i - 1`), so both runtimes emit identical descriptors. A row with its own `visibleWhen` is wrapped in a zero-spacing `vstack` gated on `present`.
- `occupancy` family members insert as engine slots under no namespace record; `RESERVED_ENGINE_NAMESPACES` rejects `occupancy` and overlaps on both mod declaration paths with `EngineReservedNamespace`.

## AC-to-proof
Test names are proposals; final names land with each task. `two-endpoint` means real `NetServer` / `NetClient` endpoints in-process.

| AC | Proof | Status | Result |
|---|---|---|---|
| S1 host+2 clients `inPlay` 3; loading, down, reconnecting counts | `seat_status_reads_live_pawn_capture_and_hold`, `player_counts_replicate_to_every_client_and_late_joiner`, `rows_by_status_match_counts_on_host_and_clients` | achievable as stated; passing (in-process replication) | pass (in-process replication, no real transport) |
| S2 never-loading client stays `loading`; disconnect while loading leaves `loading`/`connected` next tick (P15) | `seat_status_reads_live_pawn_capture_and_hold` (a seat with no pawn reads loading; a held seat reads reconnecting, outside `connected`) | achievable as stated; passing | pass |
| S3 joiner counts `inPlay` from spawn, before first command (P16) | `joiner_counts_in_play_from_spawn_before_first_command` | achievable as stated; passing (task 1) | pass |
| S4 undeclared zero-health client `down`, no occupant; no-health pawn counts; declared by capability (P17) | `trigger_stage_pawns_count_client_pawns_by_the_effective_rule`, `seat_status_reads_live_pawn_capture_and_hold`, `occupied_volumes_ignore_arm_and_latch_and_count_use_volumes` | achievable as stated; passing | pass |
| S5 single player `inPlay` 1 / `down`; leaver reads 0 at frontend (P14) | `single_player_counts_follow_install_tick_death_and_unload`, `client_counts_read_zero_once_its_level_unloads`, `frontend_backdrop_is_not_a_session_level` | achievable as stated; passing (task 1) | pass |
| S6 `occupants <= inPlay` every tick | assertion in every `coop_activation_tests` harness tick | achievable as stated; passing (task 8) | pass |
| S7 demoted client and host seat read `loading` without a live pawn (P26) | `seat_without_live_pawn_reads_loading` | achievable as stated; passing (task 1) | pass |
| O1 two on one plate 1/2; one on each of two plates 2/2 | `occupancy_counts_volumes_and_distinct_players` | achievable as stated; passing (task 7) | pass |
| O2 overlapping volumes: once in `occupants`, twice in `occupiedCount` | `occupancy_counts_volumes_and_distinct_players`, overlap case | achievable as stated; passing (task 7) | pass |
| O3 disarmed or latched volume counts; `switch` press volume counts | `occupied_volumes_ignore_arm_and_latch_and_count_use_volumes` | achievable as stated; passing (task 7) | pass |
| O4 repeated tag counts once (P11) | `occupancy_counts_volumes_and_distinct_players`, repeated-tag case | achievable as stated; passing (task 7) | pass |
| O5 player killed inside leaves both on the same later tick; gate never crosses (P7) | `death_inside_leaves_counts_together_and_gate_holds` | achievable as stated; passing (task 8) | pass |
| P1 two-button door: both held, early release, walking plate to plate | `two_button_door_solves_only_with_both_plates_held` | achievable as stated; passing (task 8) | pass |
| P2 N plates solve only when all held on one tick | `n_plates_solve_only_when_all_held_on_one_tick` | achievable as stated; passing (task 8) | pass |
| P3 all-players gate with 1, 2, 3 players opens in the last entry's frame | `all_players_gate_opens_in_last_entry_frame` | achievable as stated; passing (task 8) | pass |
| P4 downed player outside; nobody in play (P12) | `all_players_gate_ignores_downed_and_stays_closed_with_none_in_play` | achievable as stated; passing (task 8) | pass |
| P5 level change and same-map restart with real clients: closed while loading (P13) | `party_gate_waits_for_every_client_after_level_change_and_restart` (headless host, client demotion and re-promotion; no transport) | achievable as stated; passing headless, real-endpoint run outstanding | pass on the headless host; real-endpoint run outstanding |
| P6 player admitted after open turns gate false, fires again on entry (P18) | `late_admission_reopens_all_players_gate` | achievable as stated; passing (task 8) | pass |
| P7 one tick per frame crosses that frame; lone host inside at install crosses first frame (P1) | `completed_set_crosses_in_its_frame_and_at_install` | achievable as stated; passing (task 8) | pass |
| P8 King-of-the-Hill drains `dt`, `2dt`, `0` | `king_of_the_hill_drains_by_occupants` (evaluated, not descriptor-only) | achievable as stated; passing (task 8) | pass |
| P9 two identical runs give identical count timelines (P6) | `scripted_runs_produce_identical_count_timelines` on the trigger-driving harness | achievable as stated; passing (task 8) | pass |
| L1 rows match seats on every machine; shift on leave; `isLocal` follows (P25) | `player_rows_follow_seats_names_and_local_seat`, `rows_by_status_match_counts_on_host_and_clients` | achievable as stated; passing | pass (in-process replication, no real transport) |
| L2 empty name `Player N`, stable after an earlier seat leaves; sent name reads as sent | `player_rows_follow_seats_names_and_local_seat`, `host_rows_read_statuses_and_claim_names`, `roster_keeps_session_and_status_through_level_rejoin_and_expiry` | achievable as stated; passing (task 4) | pass |
| L3 single player one local row Player 1; leaver all rows absent, with and without backdrop (P14) | `single_player_counts_follow_install_tick_death_and_unload`, `client_counts_read_zero_once_its_level_unloads`, `frontend_backdrop_is_not_a_session_level` | achievable as stated; passing (task 4) | pass |
| L4 flapping status rate-limited; final and other-seat changes within one interval (P19) | `status_only_roster_updates_respect_rate_limit` | achievable as stated; passing (task 4) | pass |
| L5 rows by status equal counts on host each tick and clients within one interval, incl. undeclared death and respawn and joiner bind (P24) | `rows_by_status_match_counts_on_host_and_clients` (in-process roster and replication; the host's rows and counts come from one status pass each tick); P19 interval in `status_only_roster_updates_respect_rate_limit` | achievable as stated; passing | pass (in-process replication, no real transport) |
| L6 `forEach` rows, present-only, id suffix, same-row `labelledBy` / `focusNeighbors`, no rebuild | `for_each_unrolls_rows_with_suffixed_ids_and_matches_across_runtimes` (TS and Luau) | achievable as stated; passing (task 5) | pass |
| C1 script `setState` on counts, rows, family warns, unchanged | `engine_session_and_occupancy_slots_reject_script_writes` (log capture) | achievable as stated; passing (task 3) | pass |
| C2 `occupancy` / overlapping store rejected with and without level; `occupancyBoard` loads; engine insert succeeds (P5) | `occupancy_namespace_is_reserved_for_the_engine` | achievable as stated; passing (task 3) | pass |
| C3 tag with `.` or `:` warns once, no family; others get theirs | `unformable_tag_warns_once_and_gets_no_family`, `references_collect_from_crossings_reactions_ir_and_ui_binds` | achievable as stated; passing (task 6) | pass |
| C4 level reference to absent tag reads 0 and warns once; mod-scope reads 0 silently | `absent_tag_warns_for_level_reference_only` | achievable as stated; passing (task 6) | pass |
| C5 run-time tag from `getMapEntities` gets its family | `dev_map_declares_families_and_matches_across_runtimes` (the airlock tag is read off the map) | achievable as stated; passing (task 10) | pass |
| C6 level UI bind to a level-only tag installs without unknown-slot warning and reads | `dev_map_declares_families_and_matches_across_runtimes` (`coop.airlock` strip) | achievable as stated; passing (task 10) | pass |
| C7 after level change: mod-scope plus new level only (P2) | `level_change_replaces_level_families_and_zeroes_every_family` | achievable as stated; passing (task 6) | pass |
| C8 staged reload changing mod refs redeclares on host and clients; batches apply (P20) | `staged_reload_redeclares_mod_families_and_batches_apply` (in-process replication) | achievable as stated; passing | pass (in-process replication, no real transport) |
| C9 staged reload mid-level keeps level families and values; new mod ref binds; held crossing not refired (P22) | `staged_commit_mid_level_keeps_families_and_values` (families and values); the crossing-rebind half is the existing staged rebuild, not exercised end to end here | achievable as stated; partially proven | partial: families and values pass; the crossing rebind across a staged commit is not exercised end to end |
| C10 tag shared by mod and level survives unload; accumulator binds next level (P23) | `level_change_replaces_level_families_and_zeroes_every_family`; the hill accumulator binds and drains on install in `dev_map_declares_families_and_matches_across_runtimes` | achievable as stated; passing | pass |
| C11 every family and count reads 0 at install and from unload; threshold over held family crosses first frame; below over empty does not fire (P21) | `families_and_counts_read_zero_from_unload_through_install` (in-process replication) + `level_change_replaces_level_families_and_zeroes_every_family` | achievable as stated; passing | pass (in-process replication, no real transport) |
| C12 typedefs generated; drift guards cover them; module-scope `occupancy.tag` in a data script evaluates in build-time bridge and at install (P4) | `committed_sdk_types_match_current_registry`, `build_time_families_and_lists_match_engine_state_catalog`, `build_time_family_and_list_installers_match_runtime_bridge`; the dev scripts' module-scope `occupancy.tag` compiles through `prl-build` and installs | achievable as stated; passing | pass |
| N1 client counts and occupancy converge; host-mirrored crossing fires once; mover by replicated phase | `client_occupancy_converges_and_mirrored_crossing_fires_once` (in-process replication); the mover half rides existing replicated-phase coverage | achievable as stated; passing, mover half not re-proved here | pass (in-process replication); the mover half is not re-proved |
| N2 client joining mid-hold reads values from baseline; fires once; host does not refire (P10) | `mid_hold_joiner_fires_once_from_baseline` (in-process replication) | achievable as stated; passing | pass (in-process replication, no real transport) |
| N3 late joiner after other levels derives host's families; batches apply from first snapshot (P8) | `late_joiner_derives_host_family_set` (in-process replication) | achievable as stated; passing | pass (in-process replication, no real transport) |
| U1 surface example compiles in TS and Luau; TS and Luau puzzle and HUD fixtures behave identically | the dev scripts follow the example and compile through `prl-build` in both runtimes; `dev_map_declares_families_and_matches_across_runtimes` (identical lowered crossings), `for_each_unrolls_rows_with_suffixed_ids_and_matches_across_runtimes` (HUD rows) | achievable as stated; passing | pass |
| U2 `docs/scripting-reference.md` documents counts, list, `forEach`, family, all-players pattern | `scripting_reference_documents_session_players_and_occupancy` (grep gate) | achievable as stated; passing (task 11) | pass |
| U3 dev-tools Triggers tab shows effective beside raw; non-dev-tools build succeeds and names no column | `effective_occupancy_column_is_dev_tools_only` (grep gate) + `cargo check -p postretro` without `dev-tools` | achievable as stated; passing (task 11) | pass |
| M1 two-player session on the dev map | owner, in-engine | manual | outstanding: owner, in-engine |

## Tasks
Each task ends green and committed with this file. Hot path: the per-tick write reuses its per-seat and per-tag buffers across ticks; the per-tick allocations are the small lists `TickEvents` carries (the sorted captured and counted pawns, and each occupied volume's counted pawns), like its sibling event vectors. Writes are change-driven where a value is unchanged, and slot names are interned at declaration, never formatted per tick.

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | **Thin slice through the riskiest seams:** `MAX_CLIENTS` to `foundation`; static `SharedGlobal` readonly `session.playerCounts.*` catalog entries with typedefs and mirror; sim returns the tick's counted-player set on `TickEvents`; one host function derives seat status from `SeatTable`, registry live pawns and that set; the write in `frame_loop` before accumulators; session-level flag, zeroing at unload and leave. Proves S1–S5, S7 and the client half of S1 over two endpoints. | integrating executor | — | done: S3, S5, S7 pass; S1 and S4 partial as noted; full suites of postretro, netcode, sim, entities, script-compiler, foundation and scripting-core green |
| 2 | Split occupant tracking out of `trigger_system.rs`, behavior-preserving, own commit | integrating executor | — | done: `sim::trigger_occupants::TriggerOccupants`; sim lib (1990), netcode and app trigger filters green |
| 3 | Catalog `Family` and `List` nodes: both runtime installers, build-time twin, typedef emit, drift guards; value-preserving family slot replace; `occupancy` reservation; readonly proof (C1, C2, C12 typedef part) | integrating executor | 1 | done: C1, C2 pass; family/list installers, mirror and typedef drift guards green; full suites of postretro, netcode, sim, entities, script-compiler, scripting-core, foundation green |
| 4 | Roster names and statuses: `RosterEntry` fields, `WIRE_VERSION` 40, status dirty marking and 250 ms gate, client `entries` accessor; `session.players` rows on host, single player and clients; `Player N`; privacy tests and doc comments rewritten (L1–L5) | integrating executor | 1, 3 | done: L2, L3, L4 pass; WIRE_VERSION 40 gates and both privacy guards updated; full net, netcode and app suites green; L1 and L5 real-endpoint runs land with task 9 |
| 5 | `forEach` in TS and Luau, `present` gating, id and reference suffixing, export inventories; HUD reference list (L6) | integrating executor (no worktree build budget on this disk) | 3 | done: L6 passes in both runtimes; typedefs regenerated; dev HUD `hud.players` list (`dev_hud_player_list_unrolls_one_row_per_seat`); scripting-core, sim, script-compiler suites green |
| 6 | Referenced-tag collection over lowered descriptors; mod-scope families at first init and staged commit; level families at install, removed at unload unless mod-shared; absent and unformable tag warnings (C3–C11) | integrating executor | 3 | done: C3–C7, C10, C11 pass (C5, C6 and the P23 accumulator on the dev map); C8 and C11 on the replication harness; C9's crossing rebinding rides the staged-commit rebuild that already runs after the family commit |
| 7 | Sim aggregates per-tag `occupiedCount` / `occupants` from the capture onto `TickEvents`; the frame write publishes them with counts (O1–O5, S6) | integrating executor | 2, 6 | done: O1–O5 and S6 pass (O5, S6 on the task-8 harness); full postretro, sim, netcode, scripting-core, entities suites green |
| 8 | Shared headless harness (production sim tick, write, accumulators, crossings) and the puzzle rows P1–P9 | integrating executor | 7 | done: `coop_activation_tests.rs` drives the production sim tick, counts, families, accumulators and the frame's crossing pass; P1–P4 and P6–P9 pass (P5 is two-endpoint, task 9) |
| 9 | Two-endpoint harness on the sim tick; N1–N3, P5, C8, C11, S1 client half, L1, L5 | integrating executor | 4, 7 | done with a deviation: N1–N3, C8, C11, L1, L5 run on the in-process replication path (`HostStateReplication` / `ClientStateApply` and the host's roster messages, `coop_activation_replication_test.rs`), P5 on the headless host with the demotion lifecycle; no run drives real `NetServer` / `NetClient` transport |
| 10 | Dev map `coop-activation-test` and TS and Luau scripts (two-button, N plates, room gate, hill, HUD list); rewrite `coop-two-button-puzzles.ts`; surface example compile; TS/Luau parity (U1) | integrating executor | 5, 8 | done: `coop-activation-test.map` and its `-luau` twin (both compile clean), `coop-activation-test.ts` / `.luau` (the spike rewritten), `coop-hill.ts` in the dev mod; `dev_map_declares_families_and_matches_across_runtimes` passes |
| 11 | `docs/scripting-reference.md`; dev-tools Triggers effective column and gate (U2, U3) | integrating executor | 7, 10 | done: Session players and Occupancy by tag sections; Triggers tab Effective column; U2 and U3 gates pass; `cargo check -p postretro` without dev-tools is clean |

## Review loop
One panel (`/review-panel`): two correctness tracers, a contract verifier and an adversarial tester (opus), plus hygiene and drift (sonnet). No finding reached a Decision or Acceptance row. Fixed in `3541f4f9`:
- A hold or reclaim published a roster entry whose status contradicted its connection for up to one interval (two tracers and the tester): `seat_status` now lets the connection decide first.
- `occupants <= inPlay` could break for a pawn with no seat, such as an unmarked fallback pawn (tester): occupancy counts only seated pawns.
- A client that left showed the host's counts, rows and families until its queued unload ran (tracer): leaving zeroes them at once.
- The hill store needed a durable identity entry, and the dev-mod fixture and observability driver skipped the mod's occupancy commit that first init runs (surfaced by the post-fix suite).
- An empty tag was dropped without a warning; the Luau accessor error named the dot call; the `forEach` wrapper did not stretch; Luau row copies stopped at a `nil` hole; the open-seat log repeated on status-only rosters; the Triggers tab's counts allocated in non-dev-tools builds.
- Comment and doc drift: the unload table, roster "status-only" wording, two misplaced doc blocks, the Luau row suffix (0-based in both runtimes), previous-tick reads in trigger reactions, `inPlay` with no health component, and the per-tick allocation note above.

Left as they are:
- A client that installs before the host's next status publication shows the roster it already holds, for at most one interval. P24 allows it.
- A `localState` scope inside a `forEach` row is shared by every row. This is now documented; per-row scopes are outside the Decision.
- Rows past the list's capacity drop silently, as the non-goal says.
- `context/lib` still marks the shipped surface "decided, not yet built". The landing step updates it.

## Gates
- `cargo fmt --check`: pass.
- `cargo clippy --workspace --all-targets --features postretro/dev-tools -- -D warnings` and plain `cargo clippy -- -D warnings`: pass.
- `cargo check --release`: pass. Its only warnings are two unused dependencies that predate the branch: `combat-model` → `net`, and `scripting-core` → `notify-debouncer-full`.
- `crate-graph --check`: pass.
- `cargo test --workspace`: every suite passes except four `postretro-level-compiler` `prl-build` data-script tests, which cannot find the `scripts-build` sidecar. This container runs cargo 1.100, whose default build-dir puts test binaries under `target/<profile>/build/<crate>/<hash>/out/`. `find_scripts_build` looks only beside the executable and in the `deps/` layout. It predates this branch and is untouched by it, so it is a follow-up for that lookup. The sidecar itself builds, and both new dev maps compile through `prl-build`. The app suite passed in full (2656) on the final commit.
- Not run here: GPU-adapter tests skip without an adapter, as everywhere in this container.
- Post-merge preflight, after merging `main` at `830e1f87` (a clean merge with no overlapping files; `main` was still at `WIRE_VERSION` 39): fmt, both clippy forms, `cargo check --release` and `crate-graph --check` pass. `cargo test --workspace` passes 16034 tests; the same four `prl-build` data-script tests fail on the sidecar lookup and pass with `target/debug` on `PATH`, which the lookup's fallback searches.

