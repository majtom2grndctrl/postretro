# coop--client-inventory-awaits-tuning — plan of record

mode: compact
status: active
read at: 50ef6ea

## Corrections
- Brief read at fd8982c → source changed since in `netcode/src/lib.rs` (static-overlap report in `client_predict_tick_with_sender`) only; no cited symbol moved. Decision reads reused.
- Path names `spawn_slot_adoption_open`, `adopt_payload_spawn_active_slot` and the `selection_idle` parameter → adoption also owns `NetEndpoint::Client::selection_declared_pawn` (written only by `send_client_switch_declaration`, read only by the adoption gate) and `WieldableSelection::is_idle_on` (read only by main.rs's adoption call). Both go with adoption; nothing else reads them.
- `receive_alternate.rs` stores the payload in place of the install → it holds a client `NetEndpoint` (a `Connect` endpoint whose own transport stays unused) and routes Control through `install_tuning_payload`, then destructures the endpoint for `client_receive_and_apply` as main.rs does.

## Delegated answers
- Where the slot-key-in-window row runs — the conditioned harness, through the production selection reader (`local_wieldable_occupancy` → `WieldableSelection::advance_frame` / `take_pending_commit`), the client wieldable tick on `local_player_movement_pawn`, and the re-sync observer, because those are the gates that drop the press in production.

## AC-to-proof

| AC | Proof | Status |
|---|---|---|
| 1 never instantiates own loadout; first inventory is host layout | `conditioned_late_payload_join_composes_the_carried_slot_once_from_the_host_layout`; `client_arm_before_the_payload_composes_nothing_then_the_payload_composes_the_host_layout` | achievable as stated |
| 2 no inventory between baseline and payload | same two tests | achievable as stated |
| 3 late reserve and magazines equal payload-first, no own ammo type | `late_payload_seeds_reserve_and_magazines_as_the_payload_first_path` | achievable as stated |
| 4 demotion / unload before payload composes nothing | `client_demotion_or_unload_before_the_payload_composes_nothing_until_the_next` | achievable as stated |
| 5 late join lands on carried slot, no declaration | `conditioned_late_payload_join_composes_the_carried_slot_once_from_the_host_layout` | achievable as stated |
| 6 slot key in window | `conditioned_late_payload_join_drops_a_slot_key_pressed_in_the_window` | achievable as stated |
| 7 late payload, empty spawn slot → first populated | `late_tuning_install_with_an_empty_spawn_slot_composes_on_the_first_populated_slot` | achievable as stated |
| 8 late payload with no weapons, later fill | `late_empty_payload_composes_every_slot_empty_and_a_later_fill_activates_it` | achievable as stated |
| 9 payload before or same frame | `payload_before_the_arm_composes_once_on_the_spawn_slot`; `client_install_composes_the_host_carried_spawn_active_slot` | achievable as stated |
| 10 second payload keeps instances, does not move active slot | `second_payload_retunes_in_place_and_leaves_the_active_slot` | achievable as stated |
| 11 re-arm after a switch neither recomposes nor moves | `rearm_after_a_switch_neither_recomposes_nor_moves_the_active_slot` | achievable as stated |
| 12 merges from a payload-composed inventory | `tuning_merge_*` in `net_descriptor.rs`, re-seeded | achievable as stated |
| 13 epoch failure before / after composition | `tuning_failure_before_composition_composes_nothing_and_after_keeps_the_inventory` | achievable as stated |
| 14 re-sync converges after a switch | `conditioned_active_slot_resync_fires_after_an_accepted_switch` (unchanged) | achievable as stated |
| 15 grep gate | `rg` over crates for adoption symbols and `selection_idle` | achievable as stated |
| 16 diff gate | `git diff main` over host composition, carried restore, payload build | achievable as stated |
| 17 wire, epoch, snapshot versions | `cargo test -p xtask -- --ignored wire_version_matches_main`; `TUNING_PAYLOAD_EPOCH == 14`, `SNAPSHOT_VERSION == 21` unchanged in diff | achievable as stated |
| M1 conditioned-link co-op join | owner, in-engine | manual |

## Tasks

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | Extend `receive_alternate.rs` to install through the endpoint with distinct host/client descriptors and a held payload; write the late-join row against today's code (expect fail) | integrating executor | — | done — both late-join rows failed on the pre-change build ("the armed pawn composed before its payload"), pass after |
| 2 | Remove the no-payload composition branch; re-seed `net_descriptor.rs` merge tests; sim unit rows 1–3 | integrating executor | 1 | done — `net_descriptor` 34/34 |
| 3 | Remove adoption: gate, adopt fn, `selection_declared_pawn`, `selection_idle`, main.rs handling, `is_idle_on`, their tests; netcode endpoint rows 4, 7–11, 13 | integrating executor | 2 | done — `client_tuning_install_tests` 8/8, netcode endpoint/tuning filter 45/45 |
| 4 | Slot-key-in-window conditioned row; doc comments; `networking.md` "not yet built" sentence held for landing | integrating executor | 3 | done — conditioned/selection/client_weapon/environment-install filter 138/138 |
| 5 | Gates 15–17, preflight, review loop | integrating executor | 4 | |
