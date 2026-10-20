# movement--mover-pinned-rider — plan of record

mode: compact
status: active
read at: 411d3fb

## Corrections
- Brief read at c1cad7e → no mover, carry, displacement or blocking source changed since (only view-pose and fluid work landed); the brief's Decision reads stand.
- `kinematic_mover/blocking.rs` is ~700 non-test lines, already past the 600-line split line → the pin predicate and pin resolution land in a new `kinematic_mover/pin.rs` from the first commit; `blocking.rs` calls it. Its test module moves to `blocking_tests.rs` first, in its own behaviour-preserving commit.
- Path: "the pin test before the mover tick at the top of `simulate_tick`" → the call goes in `simulate_tick_with_presentation_aim` (the one body `simulate_tick` delegates to), after `snapshot_transforms` and before `run_kinematic_mover_tick`. It reads the side table, which still holds the last tick's published poses there, so it sees exactly what the pass saw.
- Driver: a `once` mover's backward arrival at waypoint 0 falls through `handle_arrival_at_waypoint`'s `_ => {}` and keeps travelling in the same tick if its direction is flipped → the restart fix also breaks `advance_mover`'s loop on the unstart, and resets direction to +1 and the target to none.
- The netcode loopback `host_simulate_tick` runs no triggers and no pass → the co-op row adds the pass there, not a production change.

## Delegated answers
- **Pre-tick test never releases.** It only adds holds (or reversals) and clears holds on `crush` movers. Release stays with the pass. Reason: nothing moves a pawn between ticks, so a pin the pass released stays released; a command that cleared `blocked` already released it.
- **Onset memory.** A host-only set of movers pinned at the last test (held or reversed for a pin). When a mover is pinned, its Blocked event fires only if it is not in the set; an unpinned stop-contact hold keeps today's onset (`!blocked` before the pass). Reason: the brief scopes "once per pin" to pins and keeps unpinned reactions as today.
- **Reverse eligibility.** Reverse for a pin only when every pinned body's push or carry direction has positive dot with the tick-end heading, and the reversed course, previewed, pins no body. Reactive (unpinned) reversals also preview their reversed course and hold when it pins (Q8 generalised), so no reversal commits an unpreviewed pin.
- **Pin blockers for a non-rider.** One combined cast of the capsule along the push normal for the push depth, against static geometry and every other mover at its current pose (the pinning mover excluded by a pose-source filter). Same skin semantics as `mover_push_is_blocked_by_static`, so the static half is unchanged.
- **Rider carry test.** Sweep the capsule from its position to `apply_mover_carry`'s end under the previewed pose, against static geometry and other movers. Rider identity is `carry_ground` (vault target's mover while vaulting, else ground), via one shared helper in movement/mod.rs.
- **Crush widening.** Crush keeps today's this-tick push test, now blocked by static or another mover, and adds the previewed rider-carry test plus a carried rider already embedded in static geometry (the sweep misses once it starts inside). A body with a running cadence also stays pinned while the previewed course still pins it, so displacement shuffling it between two movers does not restart the cadence. Crush never holds. In real tick order the old own-push test never caught a carried rider at all; the rider tests are what crush a ceiling rider now.
- **Held `displace` movers always enter the pass**, even with no pawn in reach, so a hold releases when its pawn despawns.
- **Dead pawns** count as bodies, as they do for every reaction today.
- **Other movers as blockers** are swept by the body's motion relative to each one's previewed next-tick translation (`pin::next_mover_travel`, movers within a tick of a pawn), so parts of one machine moving together never pin a rider between them. Last-tick travel was tried first and failed at every same-tick start and turnaround. A pre-tick yield changes that mover's travel, so the pre-tick test repeats until no mover newly yields. Another mover's spin is not part of the relative motion.
- **Rider sweep margin.** The rider carry sweep stops one skin width short, as the push test does. A contact-only sweep left a rider touching a ceiling mover, which then read that contact as a push when it slid (the hatch case). A rider left under one skin (2 cm) of headroom at a stop holds one tick short and warns.
- **Crusher with a running cadence** keeps pinning a rider embedded in static geometry while paused or completed.
- **Warning text** names the mover id and its TrenchBroom position (`(-z, -x, y) / MAP_UNIT_METRES`); movers carry no authored name in the component.

## AC-to-proof

Physics tick-order rig (`kinematic_mover/pin_test_fixtures.rs`): pre-tick test → mover driver → player movement → auto-close → blocking pass, the sim order. Tests in `kinematic_mover/pin_tests.rs` unless named otherwise.

| AC | Proof | Status |
|---|---|---|
| R1 rider on default lift into ceiling, still embedded 60 ticks later | `default_lift_never_carries_its_rider_into_a_low_ceiling` (committed as expected failure in ab3caf5, flipped in 6e8a205); `a_pawn_carried_into_a_static_ceiling_rides_on_…` removed | achievable as stated |
| R2 jamb door and double door leave pawn inside a hull | `default_door_never_closes_into_a_pawn_at_its_jamb`, `default_double_door_never_traps_a_pawn_in_a_leaf` (same history) | achievable as stated |
| R3 crush: sideways rider and two-mover squeeze take no damage | `crusher_crushes_a_rider_carried_sideways_into_a_wall`, `crusher_crushes_a_pawn_squeezed_against_another_mover` (same history) | achievable as stated |
| R4 reversed `once` door ignores next start | `reversed_once_door_runs_again_on_its_next_start` (same history) | achievable as stated |
| 1 rider vs too-low ceiling, per policy | `rider_lifted_toward_low_ceiling_never_embeds_under_any_yielding_policy` | achievable as stated |
| 2 overhang under slab edge, resumes next tick | `overhanging_rider_holds_lift_then_resumes_the_tick_after_stepping_inward` | achievable as stated |
| 3 low arch platform, carousel under pillar, spin holds under reverse | `rider_under_low_arch_never_embeds`, `carousel_spin_holds_for_rider_under_pillar_under_every_policy` (a pathed `reverse` carousel included) | achievable as stated |
| 4 lift toward closed hatch mover | `lift_holds_under_closed_hatch_and_resumes_after_it_opens` | achievable as stated |
| 5 jamb, descending lift, rotating door, double door; resume and complete | `non_rider_pins_never_leave_more_than_one_tick_of_travel_and_resume` (jamb, descending lift, double door), `rotating_door_swinging_a_pawn_into_a_wall_holds_then_resumes` | achievable as stated |
| 6 airborne grace rider caught at ceiling | `airborne_rider_in_grace_caught_at_ceiling_counts_as_pinned` | achievable as stated |
| 7 vault onto rising mover under ceiling | `vault_onto_rising_mover_under_ceiling_never_embeds` | achievable as stated |
| 8 two pawns, one pinned, order-independent | `one_pinned_pawn_holds_the_mover_in_either_visit_order` | achievable as stated |
| 9 tick after first pin: hold moves nothing, reverse carries away (Q5) | `first_pinned_preview_holds_rider_in_place_or_reverses_it_away` | achievable as stated |
| 10 reverse lift pinned both ways holds, one event (Q8) | `reverse_lift_pinned_both_ways_holds_without_flipping_and_fires_once` | achievable as stated |
| 11 one warning per mover per level load | `rider_yield_warns_once_per_mover_per_level_load` (log capture) | achievable as stated |
| 12 once door resumes; goToPathNode target kept | `non_rider_pins_…` (the jamb door is `once` and completes), `held_go_to_path_node_keeps_target_and_arrives` | achievable as stated |
| 13 one event per onset; self-reactions don't loop | `blocked_event_fires_once_per_onset_even_under_self_reactions` | achievable as stated |
| 14 release by moverStop then restart refires | `stop_release_then_restart_into_pin_fires_again` | achievable as stated |
| 15 reversed once door ends unstarted, restarts | `reversed_once_door_runs_again_on_its_next_start`, `reversed_once_mover_ends_unstarted_at_its_start` (moverReverse) | achievable as stated |
| 16 auto-close into pin yields (Q1) | `auto_close_expiry_into_pin_yields` | achievable as stated |
| 17 trigger-stage start/reverse on held mover (Q2) | `trigger_stage_commands_on_held_mover_move_only_off_the_pin` | achievable as stated |
| 18 pinned pawn despawns releases (Q3) | `despawned_pinned_pawn_releases_hold_next_tick` | achievable as stated |
| 19 setBlockPolicy while held (Q4) | `block_policy_switch_mid_pin_takes_effect_next_tick` | achievable as stated |
| 20 frame-end drain commands on held mover (Q7) | rig `frame_end_commands_on_held_mover_never_embed_rider`, `blocked_event_reverse_backs_door_off_and_restart_closes_it`; through `simulate_tick`: sim `pre_tick_pin_test_runs_before_the_mover_tick` | achievable as stated |
| 21 player-event fire list in a two-tick frame (Q9) | sim `command_between_two_simulate_ticks_is_pin_tested_before_the_mover_tick` (player events apply between fixed ticks) | achievable as stated |
| 22 unpinned slider and headroom lift reproduce c1cad7e positions | `unpinned_slider_push_and_headroom_lift_match_baseline` (bit-exact positions captured at ab3caf5, before the change) | achievable as stated |
| 23 grounded rider in open air never stops/reverses; pawn in path still does | existing `grounded_player_does_not_block_its_stop_mover`, `grounded_player_does_not_reverse_its_mover`, `stop_policy_holds_on_swept_player_contact_…` | achievable as stated |
| 24 existing E17 tests unchanged except default pinned push | full `postretro-physics` suite; `a_pawn_pushed_out_of_a_mover_toward_a_static_wall_…` restated as a substrate-only test | achievable as stated |
| 25 agents unchanged | existing `displace_policy_ignores_enemy_contacts` and enemy stop/reverse/crush tests | achievable as stated |
| 26 crush ceiling rider still damaged, keeps moving, assertion silent | `crush_lift_still_damages_ceiling_rider_on_cadence_and_keeps_moving` | achievable as stated |
| 27 crush sideways rider and two-mover squeeze damaged on cadence | `crusher_damages_sideways_rider_and_two_mover_squeeze_on_cadence` | achievable as stated |
| 28 co-op loopback converges, no overlap after first hold reconcile; largest correction reported | netcode `predict_reconcile_harness::pinned_lift::client_riding_a_default_lift_into_a_low_ceiling_converges_on_the_host_hold` | achievable as stated |
| 29 diff gate: no wire, digest or FGD key/value | `git diff main -- crates/net crates/netcode/src/wire* … sdk/TrenchBroom` review + grep of FGD key lines | achievable as stated |
| 30 `docs/` gains no `cargo run -p xtask` | `grep -rn "cargo run -p xtask" docs/` | achievable as stated |
| M1 single player dev map | owner, in-engine, `content/dev/maps/mover-pins.map` | manual |
| M2 co-op client feel | owner, in-engine, lag preset 45/60/5 % | manual |
| M3 mapper predicts from FGD help + docs | owner, reading | manual |

## Review loop
- Round 1 (`/review-panel` on the branch). Acted on: onset gated on the live hold (contact→pin, reactive reverse, pretest); no-motion reversal holds; `reverse` on a parked `once` mover opens it; auto-close on a parked unstarted mover is a no-op; relative-motion mover blockers; paused/completed crusher keeps an embedded rider pinned; one pose source; FGD help, `docs/level_design.md`, dev-map README; test strengthening. Declined: contact-only rider sweep (see Delegated answers).
- Accepted residuals: `blocking.rs` stays ~830 lines with the pin predicate and yields already split into `pin.rs`; the pass previews only movers within one tick of a pawn (`any_body_in_reach`), so the hot-path cost is bounded by movers near players.

## Tasks

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | Move `blocking.rs` tests to `blocking_tests.rs` (behaviour-preserving commit) | integrating executor | — | done c346ac6 |
| 2 | Tick-order rig; capture baseline for AC 22; reproduce-first tests as expected failures | integrating executor | 1 | done ab3caf5 |
| 3 | `pin.rs`: predicate, pass integration, pre-tick test, onset memory, warning, reversal preview; crush widening; once restart | integrating executor | 2 | done 6e8a205 |
| 4 | Sim wiring of the pre-tick test + sim tests (AC 20, 21) | integrating executor | 3 | done 5b4dc1c, 4b95760 |
| 5 | Automated AC matrix in `pin_tests.rs` | integrating executor | 3 | done 5b4dc1c, 55d7edb |
| 6 | Netcode loopback harness runs the pass; AC 28 | worker | 3 | done 55d7edb |
| 7 | FGD help, SDK `setBlockPolicy` doc + typedef fixtures, `docs/level_design.md` movers section | worker | 3 | done fa1f334, 5bd68dc |
| 8 | Dev map `content/dev/maps/mover-pins.map` holding every case | worker | — | done 5b313ec (compiles; in-engine untested) |
| 9 | Diff gates (AC 29, 30) | integrating executor | 6, 7, 8 | |
