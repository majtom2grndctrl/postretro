# coop--in-app-host-and-join — plan of record

mode: resumable
status: approved
read at: 682a264a

Brief read at 7ffced62. Since then only `crates/netcode/src/command_queue.rs` and `crates/sim/src/sim/{host_movement,mod}.rs` changed, none cited. Every Decision and Path symbol was re-read at this commit. No Decision premise is false.

## Corrections
- Research: "the host stores parity declarations before admission" → `NetServer::process_control_messages` (crates/net/src/transport.rs) also accepts `JoinSeed` (pushed to `join_seeds`) and `Revealed` (into `revealed_declarations`) from a `Pending` slot without a state check. Today the mod-id verdict lands in the same batch as `Admission`, so nothing leaks. With a password the verdict waits for a proof. Planned around: a pending slot with an outstanding challenge buffers its `Parity`, `JoinSeed` and `Revealed` in transport. On admission it applies them in arrival order. On refusal it drops them (AC P6, "refused … applies no parity, join seed or reveal").
- Path: "`--connect` widens to match" → `parse_net_config` (crates/netcode/src/lib.rs) parses a full `SocketAddr`, so the port is required today. Planned around: one address parser in netcode (IPv4, optional port defaulting to `DEFAULT_HOST_PORT`, hostname and IPv6 refused), shared by `--connect` and `joinSession`.
- Path: "Settle how each field reaches its own slot" → `openTextEntry(onCommit?)` (sdk/lib/ui/reactions.ts) always pushes `core/ui/keyboard.json`, whose envelope fixes `text_entry_target: "ui.textEntry"`, and `resolve_text_entry_intents` edits only the active tree's target. Approved approach (owner approved the plan 2026-11-17 with this approach): `openTextEntry` gains an optional target slot that overrides the keyboard's `text_entry_target` for that push, so hardware keys and on-screen keys both write `session.joinAddress` or `session.password` directly. The password then never passes through `ui.textEntry`. This is an additive SDK parameter the boundary inventory does not list. The fallback needs no new surface: each field's `onCommit` copies `ui.textEntry` into its slot with `setState` and then `clearText`. The cost is that the password sits in `ui.textEntry` while it is typed.
- Path: "a new ledger operation that keeps seat 0's carry" → confirmed. `SeatTable`'s `session_id` and `next_seat` are private, and `apply_role` keeps any existing table (`get_or_insert_with`). Planned: `SeatTable::remint_for_hosting() -> Result<(), getrandom::Error>`. It draws the new id first, and only on success resets the id, the counter, the hold state and the roster bookkeeping. It keeps `carried[Seat(0)]` and seat 0's pawn binding. On failure the table is untouched.

- Test seam: E-row and S-row tests drive a real engine client against the relay host over loopback, as the spawn-view handoff brief did. Two `postretro-scripting-core` `test-support` seams stand in for mod init: committed mod identity and committed store identity. Dev builds relay both endpoints through lag proxies on their own threads, so these tests give each exchange 1 ms of wall time.
- Plan: tasks 4 and 5 landed together. The verbs need the slots, and the press publishes the facts.
- Path: "admission is first visible as the `Relevel` or `Holding` notice" → also a spawn-pose hint, roster or final character. The client treats any of them as admission. `waiting` and `playing` are read once per Control drain, from `NetClient::is_participating` after the batch. A hold then a re-promotion in one update therefore publishes both values, in order.
- Path: a client's leave clears `session_players.session_level()` before its queued unload runs. "A played level installed" therefore also counts a level a session end stopped (`level_stopped_by_session_end`), so a press after a leave waits for the unload.
- Decision clarification: "Cleared when hosting starts … kept on `hostFailed`" → the host press clears `session.password` only once the build succeeds.
- Refusal logging: entry refusals reuse the membership verbs' once-per-source warning (`report_refused_verb`). A second refusal from the same source logs at debug.

- Path: the client answers a challenge inside `NetClient`'s Control drain, so the engine never sees one. Each endpoint keeps only the derived key: `NetServer::set_session_password` (empty means no challenge) and `NetClient::set_session_password` (empty still answers).
- Test seam: `postretro-test-log-capture` gains `OrphanWatch`, which counts uncaptured records containing one needle. The plain orphan count is process-wide and shared by parallel tests. P-k scans relay datagrams. An engine client's own UDP datagrams come from the same `NetClient` code.

- Approved approach → corrected (**owner review**). The on-screen keyboard's keys are mod reactions that append to an explicit slot: the dev mod's `kbAppend_*` reactions call `appendText(ui.textEntry, ch)`. A tree-level target override would therefore reach only the hardware path. The fallback cannot be expressed either: `updateState` does not project a String slot, so an `onCommit` cannot copy `ui.textEntry`. What was built keeps the approved signature, `openTextEntry(onCommit?, target?)`, and has the engine do the copy. Both input paths keep editing `ui.textEntry`. When the keyboard opens, the engine seeds `ui.textEntry` from the target. On commit it copies `ui.textEntry` into the target before `onCommit` fires, then empties it; on cancel it empties it and leaves the target as it was. Each field still reaches its own slot on both input paths. The cost is the one the fallback named: a password sits in `ui.textEntry` while it is typed, a client-local slot that is never replicated or persisted. The engine carries the target on `PushTree` as `commit_target`.
- `Text` `mask` applies wherever a Text's string resolves: its literal content at build, the retained binding driver and the fresh draw. Layout therefore measures the masked string too.

## Delegated answers
- Host-side bound on an unanswered challenge — **5 s** on the transport clock, measured from when the challenge was sent. One round trip is under 1 s even under the dev lag profile with loss, and 5 s sits well inside a seat hold. A proof drained in the poll where the bound elapses still admits (P22).
- Remembered address field and options mapping — `PlayerOptions::last_join_address: Option<String>` (TOML key `last_join_address`, default absent). It seeds `session.joinAddress` at session build, and the engine writes it on admission. It is not an options-bridge menu option, because no menu edits it except the Join field itself.

## AC-to-proof

Automated rows run as `cargo test` loopback tests over real transports, as the leave brief's membership tests do. Each row's test lives beside its owning module.

| AC | Proof | Status |
|---|---|---|
| E1 SP at frontend hosts; slots publish; client waits; next level promotes it | `in_app_host_holds_a_joiner_at_its_frontend_and_promotes_it_into_its_next_level` | proven |
| E2 SP joins a host in a level, with and without a same-map backdrop; participates at hinted spawn | `in_app_join_from_the_frontend_participates_in_the_hosts_level_at_the_hinted_spawn` | proven |
| E3 Join during backdrop load cancels it; client-role install state and model prediction (P3) | `in_app_join_cancels_a_backdrop_load_in_flight_and_installs_the_hosts_level_as_a_client` | proven |
| E4 Host, end, join elsewhere, leave, host again in one process | `one_process_hosts_ends_joins_leaves_and_hosts_again` | proven |
| E5 Re-host mints new session id and seat counter; seat 0 keeps carry (P15) | `remint_for_hosting_starts_a_fresh_run_keeping_seat_zero_carry`, `hosting_again_mints_a_new_run_and_its_first_joiner_takes_seat_one` | proven |
| E6 Refusal matrix with no state change, each paired with a permitted case (P4) | `entry_off_the_frontend_or_outside_single_player_is_refused_without_change` | proven |
| E7 Press during pending unload or close publishes `connecting`, runs after; second press refused (P5) | `join_after_a_leave_waits_for_the_unload_then_joins`, `host_after_an_end_waits_for_the_close_then_hosts` | proven |
| E8 Failed mint at host entry: ledger untouched, SP, `hostFailed` | `failed_session_mint_at_host_entry_stays_single_player_with_its_ledger`, `failed_remint_leaves_the_ledger_untouched`; `hostFailed` on the shared failure branch: `host_presses_publish_offline_and_a_failed_build_publishes_host_failed` | proven |
| E9 `hostSession`+`joinSession` and `joinSession`×2 in one reaction: first wins, one save and one endpoint (P1) | `two_entry_verbs_in_one_reaction_take_the_first_and_refuse_the_second` | proven |
| E10 `leaveSession` then `joinSession` defers to after unload; other steps still drop at debug (P2) | `join_after_a_leave_waits_for_the_unload_then_joins`, existing `steps_after_leave_drop_at_debug` | proven |
| E11 Waiting press captures address and password; malformed never shows `connecting`; load or navigation cancels to `offline` (P16) | `a_waiting_press_runs_with_what_it_captured_and_a_load_or_navigation_cancels_it`, `a_waiting_join_answers_with_the_password_it_captured` | proven |
| E12 Boot `--host`/`--connect` role-bound state equals in-app entry | `boot_flags_and_in_app_entry_derive_the_same_role_bound_state` | proven |
| E13 One endpoint builder; no other non-test `NetEndpoint::from_role` call | `enter_role_is_the_only_production_endpoint_builder` | proven |
| S1 World and character slots on disk once join starts, before first connect packet | `join_saves_world_and_character_before_its_first_connect_packet` | proven |
| S2 Join then leave restores world-at-join; join seed carries character-at-join | `join_then_leave_restores_the_world_at_join_and_seeds_the_character_at_join` | proven |
| S3 Failed join save still rebases snapshot and warns | `failed_join_save_still_rebases_the_restore_snapshot_and_warns` | proven |
| S4 Join then leave: seat-0 carry equals a fresh SP ledger | `join_then_leave_leaves_seat_zero_with_a_fresh_ledgers_carry` | proven |
| P-a Right password admits | `right_password_admits_and_applies_what_was_declared_before_the_proof`; engine: `a_password_host_admits_the_right_password_and_refuses_a_wrong_or_missing_one` | proven |
| P-b Pre-proof parity, seed and reveal apply on admission; participates with join-seeded character (P6) | `right_password_admits_and_applies_what_was_declared_before_the_proof` (held join seed applies at admission); engine participation: `a_password_host_admits_the_right_password_and_refuses_a_wrong_or_missing_one` | proven |
| P-c Distinct nonces; replayed proof refuses; stray proof changes nothing (P7) | `nonces_differ_per_connection_replays_refuse_and_stray_proofs_change_nothing` | proven |
| P-d Wrong mod refused `refusedMod`, no challenge (P9) | `wrong_mod_is_refused_before_any_challenge` | proven |
| P-e Live write of `session.password` takes effect only at next hosting (P10) | `a_live_password_write_takes_effect_at_the_next_hosting` | proven |
| P-f Wrong or empty-password proof refuses `refusedPassword`; client lands at frontend | `wrong_or_missing_password_is_refused_and_applies_nothing`, `a_password_host_admits_the_right_password_and_refuses_a_wrong_or_missing_one` | proven |
| P-g Silent client refused within bound (P8) | `the_proof_bound_refuses_a_silent_client_and_admits_a_proof_on_its_last_poll` | proven |
| P-h Proof on the bound's poll admits; end during challenge sends `sessionEnded`; level change admits into new level (P22) | `the_proof_bound_refuses_a_silent_client_and_admits_a_proof_on_its_last_poll`, `an_outstanding_challenge_meets_the_session_end_and_a_level_change` | proven |
| P-i Password host: reclaim on deadline poll; fresh seat after hold expiry (P21) | `a_password_proof_on_the_hold_deadline_reclaims_and_one_after_it_does_not` | proven |
| P-j Passwordless host sends no challenge and admits clients with and without a password | `a_passwordless_host_admits_without_a_challenge` | proven |
| P-k Password never in datagrams, saves, slots or logs; slot cleared once consumed | `the_password_never_leaves_the_slot_that_held_it` (relay datagrams both ways, save and settings files, every slot on host and client, captured logs, `OrphanWatch` for uncaptured records) | proven |
| P-l Password-refused client applies no parity, seed or reveal | `wrong_or_missing_password_is_refused_and_applies_nothing` | proven |
| P-m `--host --password p` and `--connect --password p` | `parse_password_for_either_role`, `the_password_flag_serves_host_and_connect` | proven |
| P-n Wire round-trip and protocol-id tests; protocol id advanced | `password_control_variants_round_trip`, protocol gate tests at PRLF / 40 | proven |
| F1 Each end cause on its own path, incl. same-update refusal; `badAddress` cases open no socket; portless dials default; `Protocol` → `connectFailed` (P11) | `each_ending_publishes_its_own_end_cause`, `a_wrong_mod_join_publishes_refused_mod_over_the_transport`, `a_bad_address_opens_no_socket_writes_no_save_and_keeps_the_password`, `join_address_grammar_takes_ipv4_with_an_optional_port`, `a_password_host_admits_the_right_password_and_refuses_a_wrong_or_missing_one` | proven |
| F2 `badAddress` join: no save, stays `offline`, keeps password (P13) | `a_bad_address_opens_no_socket_writes_no_save_and_keeps_the_password` | proven |
| F3 Own `endSession` sets none; boot `--host` failure `hostFailed`; malformed `--connect` `badAddress` (P14) | `boot_and_own_endings_set_the_causes_entry_sets` | proven |
| F4 `session.connection` full sequence recorded per publish, incl. same-update hold+promote and admission+end (P23) | `connection_steps_through_waiting_and_playing_and_back_to_offline`, `a_refused_attempt_never_waits_and_an_admission_ended_in_one_update_waits_once` | proven |
| F5 Deferred host press `offline`→`connecting`→`offline`; failed build ends `hostFailed`; undeferred never leaves `offline` (P18) | `host_after_an_end_waits_for_the_close_then_hosts`, `host_presses_publish_offline_and_a_failed_build_publishes_host_failed` | proven |
| F6 `endCause` clears at next attempt; own leave sets none; host gone before admission → `connectFailed` (P17) | `each_ending_publishes_its_own_end_cause` | proven |
| F7 `waitReason` mapping from each `HoldingCause` (P19) | `wait_reason_names_each_hold`, and `host` at the host's frontend in `connection_steps_through_waiting_and_playing_and_back_to_offline` | proven |
| F8 `session.password` clear and keep rules (P20) | `the_password_clears_when_consumed_and_stays_for_a_retry`, `a_password_host_admits_the_right_password_and_refuses_a_wrong_or_missing_one` | proven |
| F9 Remembered address seeds after restart; updated only on admission (P12) | `the_join_address_is_remembered_on_admission_and_seeds_the_next_boot` | proven |
| U1 `mask: true` draws one `*` per grapheme; empty for empty | `mask_draws_one_star_per_grapheme_and_nothing_for_nothing`, `masked_text_never_draws_its_content` (fresh and retained paths) | proven |
| U2 Keyboard types `.` and `:` | `keyboard_has_dot_and_colon_keys_for_addresses`; the dev mod's `kbAppend_dot` and `kbAppend_colon` reactions | proven |
| U3 Scripting surface example builds in a `content/dev` script | both frontend fixtures carry the example and evaluate in QuickJS and Luau (`session_membership_fixture_lowers_to_the_engine_kinds_in_both_runtimes`); the dev frontend bundles in `the_dev_frontend_shows_a_notice_for_every_end_cause` | proven |
| U4 Late-joiner catch-up ignores host/join steps silently | `late_joiner_converge_skips_due_host_and_join_session`, `system_step_tests` classification table | proven |
| U5 Luau `UI.hostSession()`/`UI.joinSession()` produce the same steps | `session_membership_fixture_lowers_to_the_engine_kinds_in_both_runtimes` | proven |
| U6 No host or join `ui.*` action | `reserved_ui_actions_name_no_host_or_join_action` | proven |
| U7 `networking.md` wording corrected | `networking_contract_drops_the_stale_entry_and_handshake_claims` | proven |
| U8 Dev frontend shows a notice per end cause, list derived from the enum | `the_dev_frontend_shows_a_notice_for_every_end_cause` (derives the list from `EndCause::ALL`); fields: `a_menu_text_field_commits_into_its_own_slot` | proven |
| M1 Two-machine LAN host and join, gamepad and hardware keyboard | owner, two machines | manual |
| M2 Each notice on a real client, including different engine build | owner, two machines | manual |
| M3 Joining a host at its frontend shows waiting, then plays | owner, in-engine | manual |
| M4 SP progress survives join and leave | owner, in-engine | manual |

## Tasks

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | **Thin slice: runtime join.** Extract the role and endpoint build out of `Session::build` into one function (`session/net_entry.rs`): parse or accept a role, `from_role`, the lag seed, the seat ledger, `SessionRole`, the published address and `apply_role`. Boot calls it. Add `App::join_from_frontend(addr)` beside `startup/session_membership.rs`. It cancels a backdrop load, calls the builder, then `install_network_mod_content`. Loopback against a CLI-style host, with no password, saves or slots. Proves E2, E3, E13, and the boot half of E12. | integrating executor | — | done |
| 2 | **Runtime host.** `SeatTable::remint_for_hosting`; `App::host_from_frontend()` through the same builder, `hostFailed` on mint, bind or transport failure. Proves E1, E4, E5, E8, rest of E12. | integrating executor | 1 | done |
| 3 | **Join save seam.** Extract the clean-exit save in `main.rs` into one entry point. Join calls it before building the endpoint, then rebases `session.persisted_state` (and so the join seed) on what it wrote, warning on failure. Proves S1–S4. | integrating executor | 1 | done |
| 4 | **Verbs, refusals, deferral.** `SystemReactionKind::{HostSession, JoinSession}`, machine-local classification and catch-up ignore, the drain exemption after `leaveSession`, the one-press rule, and the pending-press state that captures address and password and is cancelled by load or navigation. SDK `hostSession()`/`joinSession()` in TS and Luau plus generated declarations. Proves E6, E7, E9–E11, U4, U5. | integrating executor | 2, 3 | done |
| 5 | **Session facts.** Catalog `session.joinAddress`, `password`, `connection`, `waitReason`, `endCause`. Shared address parser and `--connect` widening. End-cause publication on every path, boot included. Connection phase from holding notices and participation. `waitReason` from `HoldingCause`. `last_join_address` option. Proves F1–F9 except the password rows. | integrating executor | 4 | done |
| 6 | **Password gate.** `crates/net` gains `blake3`. Wire variants appended, `PROTOCOL_ID` advanced. Per-pending-slot nonce, proof check in constant time, the 5 s bound, buffered pre-admission declarations, and interactions with reclaim and level change. `--password`. Client answers from the captured password, and the slot clears per P20. Leak-scan test. Proves P-a–P-n and the password half of F1 and F8. | integrating executor | 5 | done |
| 7 | **UI.** `Text` `mask`, keyboard `.` and `:` keys, the text-field target approach (see Corrections), and dev frontend Host and Join submenus with phase, wait reason and per-end-cause notices. Proves U1–U3, U6, U8. The `crates/ui` mask and keyboard part may run as a worktree worker beside task 6. | integrating executor (mask and keyboard: optional worker) | 5 | done |
| 8 | **Docs and gates.** `networking.md` §Two-gate, §Role model and §Session-state ledger corrections, plus the mask screen-reader rule in `ui.md` §4.2. Proves U7. The `scripting.md` §10.4 and `player_options.md` updates wait for landing. | integrating executor | 6, 7 | done |

## Review

Round 1: `/review-panel` with seven read-only reviewers over two slices. Five depth lenses on opus: join-flow tracer, host-and-saves tracer, password tracer, contract verifier and adversarial tester. Two hygiene/drift passes on sonnet. Fixed directly rather than through `/fix-review-findings`: the fixes cross `session_entry.rs`, `ui_actions.rs` and `main.rs`, and per-file agents would have collided.

Fixed:
- The JS and Luau widget bridges dropped `Text.mask`, so mod password fields drew in clear text. Both now read it, with a bridge test in both runtimes.
- `resolve_map_path` took `--password`'s value as the boot map. It now steps over it under the same predicate `parse_net_config` uses, which no longer swallows a following flag.
- A waiting join remembered the address in the slot when it ran, not the one it dialed. The press now captures the typed text.
- The on-screen keyboard's `kbAppend_*` reactions lived in one level script. They are now mod-wide in the dev frontend, so the keyboard types at the frontend.
- With persistence off, a join skipped its rebase. `StateStoreLifecycle::can_collect` now gates the snapshot, and only the write needs `can_save`.
- A text-entry field now tracks the `ModalInstance` of its keyboard. It is dropped, with `ui.textEntry`, once that push closes any other way. A consumed password abandons an open field for it.
- A level unload keeps queued `hostSession` and `joinSession`, so a press whose frame brought the host's end survives.
- A suspend cancels a waiting press, and resume runs only in states that drain level requests.
- A known relevel clears a stale hold and the missing-level flag, as does `HostLevelAbsent`. A level mismatch while following a host-named load reads `host`.
- `session.waitReason` and `session.endCause` are enums with an empty value. A drift test holds the end causes equal to the catalog's.
- Boot attributes a parse failure to the flag that failed (`NetArgError::failed_role_flag`) and shares the entry vocabulary.
- The password paths in `crates/net` refuse instead of panicking. An unproven peer holds at most the latest declaration of each kind.
- `enter_net_role` with no session reports a failure.
- Doc drift: `scripting.md` §10.4 and the slot table, `ui.md` §3, `player_options.md` §2, the `join_facts`, text-entry, `parse_net_config`, `OrphanWatch` and `HOLD_WINDOW` docs, and the gate-test comment. Text edits are redacted in the post-leave drop log.

Left, with reasons:
- Tests cite acceptance IDs ("AC E2"). That is the repository's existing convention (for example `client_tests.rs` "AC 6"), and the IDs stay in this plan once it moves to `done/`.
- A promotion followed by a hold in one update publishes only `waiting`. The participation markers are filtered inside `NetClient::drain_control`, and P23 pins the reverse order, which works.
- A host RNG failure refuses with `ClosingCause::Password`. No cause fits better without a new wire variant, and the failure needs a broken OS RNG.
- A failed join build leaves the frontend without its backdrop. It only follows an ephemeral-port bind failure.
- A boot `--connect` does not remember its address: `--connect` is a developer path.
- `enqueue_dev_level_cycle_target` queues a played level under the menu. It is dev-tools only.
- The fixture reuses `frontend.host` as a reaction name, as the brief's Scripting surface example does.

Round 2: one opus re-review of the round-1 fixes. It confirmed them and found three more, fixed with a test each:
- A host level change cleared a `ModDigest` hold, and the host never resends an unchanged cause, so `modDiffers` was lost. A level change now clears only level-scoped holds (`wait_reason_names_each_hold`).
- A password field abandoned at admission stopped being tracked, so typing after it stayed in `ui.textEntry`. The field now stays tracked with no target, and its commit or cancel discards the entry (`an_open_password_field_cannot_restore_a_consumed_password`).
- An unload kept a queued press even when it led to a played level, where the press is refused. The press now survives only while the frontend menu is up (`an_unload_away_from_the_frontend_drops_a_queued_press`).

## Trial notes

- Preflight deviation: clippy ran in the main target directory rather than `target/preflight-clippy`, to stay inside the session's disk allowance.
- Clippy's one warning is the pre-existing unused `postretro-net` dependency in `combat-model`, outside this brief.
- Loopback tests pump lag-proxied endpoints, so the exchange helpers sleep 1 ms per round; closing relay clients need `step_closing`, bounded.
- The text-entry approach changed from the brief's path: the engine copies `ui.textEntry` into the field's slot on commit (see Corrections). It needs owner sign-off.
