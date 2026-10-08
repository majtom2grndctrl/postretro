# sh-streaming--reveal-gate-and-warm-horizon — plan of record

mode: resumable
status: proposed
read at: 4677eda27

Source is unchanged since the brief's `ef855f247`; only plan files moved. Every symbol cited by Decisions and Path was re-read at `4677eda27`. No Decision premise is false. The corrections below are Path or location drift, plus facts the Path omits that the build must handle.

## Corrections

**Boot and frame loop**
- C1. Brief: install is followed by `BootState::Running`. Source: `finish_level_payload` returns true, so the install redraw continues through the whole Running body (`frame_loop/mod.rs:92`), and the install frame is already the first Running frame. Plan: the install frame enters Settling and boot dispatch returns false, so it ticks nothing and presents no world.
- C2. Fact the brief omits: `frame_timing.begin_frame` runs above boot dispatch on every redraw, and its accumulator is clamped at 250 ms (`sim/src/sim/frame_timing.rs`). A Settling hold therefore ends with up to about 15 catch-up ticks. Plan: re-arm the fixed-step accumulator on the reveal edge, so the first Running frame runs today's install-frame tick count (P12).
- C3. Fact the brief omits: `poll_staged_manifest_results` runs only in the Running frame tail and the frontend UI path. Plan: Settling frames call it, so P13 holds. Clarification, same meaning: a committed reload in Running clears the scheduler when a level is installed, which drops `levelLoad` waits. Settling inherits that behavior ("as in Running").
- C4. Fact the brief omits: `loadLevel`, `restartLevel` and `returnToFrontend` are system commands. They become level requests only in `dispatch_system_commands`, which Settling holds. A script request queued during install therefore lands on the reveal frame. Requests that can arrive during Settling come from network relevel, the dev level cycle and frontend enqueues. Plan: the Settling load and unload ACs are proven through `enqueue_level_request`, the seam those paths share.
- C5. Brief: `drain_level_requests` unloads only from Running (`:193`, `:200`). Plan: Settling joins Running there, so a request unloads the Settling world first (P2).
- C6. Brief: `preload_batch` reads Mandatory blocks. Source: it reads every non-Band target. At install no drawn cells exist, so it reads mandatory plus pins. The preload is retired either way.
- C7. Source-scraping tests pin today's order: `loading_slots_reset_on_both_the_success_and_failure_routes` (`startup/loading_screen_tests.rs`), `install_range_errors_use_the_existing_boot_and_runtime_failure_routes` (`startup/lifecycle.rs`), and `test_app` (`boot_state: Running`). Plan: update them to the Settling order in the same change.

**Streaming**
- C8. Brief: SH can take id 51 the way lightmap does. Source: `prepare_streaming_drains` passes id 51 only through `LightmapLevelView::of`, which requires a lightmap stream manifest. An SH-only level never hands id 51 to `LevelStreaming`, although `LevelWorld.cell_residency_set` is loaded whenever the section is present. Plan: the cell-demand stage reads `LevelWorld.cell_residency_set` directly, independent of lightmap mode.
- C9. Brief: `take_async_drain_batch` → `departed_eviction_order`. Source: `take_async_drain_batch` is `cfg(any(test, feature = "capture"))`. Production eviction runs through `ShStreamingSession::begin_drain` (async) → `controller.begin_drain(true, …)` → `departed_eviction_order`. Eviction is unchanged; this note only aims the fixtures.
- C10. Location drift:

  | Symbol | Actual location |
  |---|---|
  | `HYSTERESIS_SECONDS` | `sh_streaming/controller.rs`, used in `targeting.rs` |
  | `DEFAULT_STREAMED_SH_POOL_FLOOR_BYTES` | renderer `render/sh_streaming/floor.rs` |
  | `PromotedBakedLightState` | renderer `render/renderer_types.rs` |
  | `all_targets_sampleable` | gated `cfg(any(test, feature = "capture"))`, not capture alone |
- C11. Fact the brief omits: `record_visible_misses` runs inside `update_targets`, before `promote_composed_clusters` in the same async drain. Plan:
  - The settle check reads SH state after the frame's promotion.
  - The reveal frame draws from the settled pose, so its visible set is already Sampleable when it is counted (AC L10).
  - Settling frames skip miss recording but keep `prior_visible_misses` empty, so P9's fresh count on a timed-out reveal still holds.
- C12. Fact the brief omits: the SH clock is `script_time`, which is frozen through Settling, so hysteresis does not age during Settling. The settle pose is fixed, so nothing departs. Plan: keep `script_time` as the streaming clock.
- C13. Brief: L is set by the Streaming tab, read by capture, and set by the walk measurement. Source:
  - The tab's lead slider sits in the lightmap section, which shows "inactive" whenever `lightmap()` is `None`.
  - Capture only reads `lead_metres` from lightmap diagnostics.
  - The walk measurement (`#[cfg(test)]`, `#[ignore]`) sets L through the controller's levers.

  Plan: move the slider to a level-scope streaming section and point all three at the stage's L.
- C14. Clarification, same meaning: lightmap's `drawn_outside_baked_set` and `drawn_not_resident` miss buckets overlap. AC L10's "drawn block that is not resident" reads `drawn_not_resident`.

**Co-op**
- C15. Brief: "the in-memory harness". Source: `crates/net/src/harness.rs` is the latency conditioner. Participation is tested engine-free through the relay seam on `NetServer`/`NetClient` (`relay_pair`, `participate_relay_pair` in `transport.rs` tests). Plan: prove the net half there.
- C16. Fact the brief omits: the per-client `reevaluate_parity` after a Control drain fires only when `parity_moved` is set (Admission with a declaration, or Parity). Plan: the revealed declaration also sets it. Control is not drained while the host mod digest is unset. That gating is unchanged, and the stale comment at that site is fixed in passing.
- C17. Fact the brief omits: there are two log sites, not one. `client_drain_control` logs every Holding cause at warn, and the host logs `HandshakeOutcome::ParityHeld` at info as "held for content parity" (`main.rs`). Plan: match on the cause at both sites, so revealed holds log below warn and say "revealed", not "content parity".
- C18. Brief: the `JoinSeed` precedent bumps versions. Source: `JoinSeed` raised `WIRE_VERSION` 17→18 only, while `SessionRoster` raised `PROTOCOL_ID` PRL5→PRL6. The `HoldingCause` doc comment "variant order is also diagnostic precedence" is false (`parity_cause` check order decides). See Delegated answers, and fix the comment.
- C19. Fact the brief omits: many tests assume parity alone promotes. They need reveal setup through one fixture helper:
  - the proptest `participation_predicate_holds_after_generated_server_operations` (its `ParityModel` gains the term)
  - `relay_pair` and `participate_relay_pair`
  - `crates/net/src/harness.rs` `relay_pair`
  - `netcode/src/seat/roster_harness_test.rs`
  - `world_less_snapshot_drain_discards_current_epoch_bytes`
  - netcode `seat.rs`, `presentation.rs`, `weapon_cues.rs`, the e18 and `trigger_state` harnesses
  - `host_activations/conditioned_tests.rs`

  A version bump also trips three version tests: `enemy_projectile_authority_reuses_the_existing_wire_versions`, `activation_records_and_outcomes_reject_previous_layout_and_vocabulary`, and the ignored branch gate `wire_version_matches_main`.

## Delegated answers
- **Settling's per-drain budget.** Settling raises the shared per-drain decoded-byte budget from 8 MiB to 32 MiB as the starting value. No world frame is at stake, and the only frame cost is loading-tree cadence. The resource run (M3) reports hold duration at both values on both maps. 32 MiB stays only if it shortens the hold without a visible loading-tree stall. Otherwise Settling keeps 8 MiB. Play is unchanged either way.
- **Version bump.** Bump `PROTOCOL_ID` PRL9 → PRL10. `networking.md` §Two-gate handshake says a new control message is a vocabulary change that bumps the app id, and `SessionRoster` followed that rule. `JoinSeed`'s wire-only bump is the deviation, not the rule. `WIRE_VERSION` bumps too only if the new append-layout guards show an existing encoding changed. Either bump alone refuses a previous-build peer at gate 1, so AC C14 holds.

## AC-to-proof

Test names are planned, not yet written. "Engine-free" means a pure function, controller or session tests, `test_app`, or the net relay seam, all without a GPU.

| AC | Proof | Status |
|---|---|---|
| **S1** not settled while an SH settle target is below Sampleable, or a settle-set block is not installed | `settle_check_waits_on_each_sh_tier_and_each_lightmap_block` (settle chokepoint, controller fixtures) | achievable as stated |
| **S2** settles with band, seam-warm and lightmap-band targets cold | `settle_check_ignores_band_seam_warm_and_lightmap_band` | achievable as stated |
| **S3** drawn blocks outside the baked set, and on a non-portal path, block settling until installed | `settle_check_waits_on_drawn_blocks_on_every_visibility_path` | achievable as stated |
| **S4** settled immediately when nothing streams | `settle_check_answers_settled_when_no_resource_streams` | achievable as stated |
| **S5** a streamed level does not settle before its first demand update from the settle pose (P1) | `settle_check_refuses_before_first_settle_pose_update` | achievable as stated |
| **S6** streamed SH with no usable id 51 settles on Visible, Pinned and owners; no lead or band in play | `sh_without_id51_targets_no_lead_or_band_tier` | achievable as stated |
| **S7** budget below the settle set still settles; the set is admitted past budget | `settle_set_admits_past_budget_and_settles` (async controller, small floor) | achievable as stated |
| **S8** capture's SH preload waits on the chokepoint's SH answer and settles when the band exceeds budget | `capture_preload_settles_on_chokepoint_sh_answer` (`capture` feature) | achievable as stated |
| **L1** boot map, catalog, `restartLevel`, backdrop and relevel each pass install → Settling | `every_level_entry_installs_into_settling` (`test_app` with a level-less renderer stub; one case per entry route) | achievable as stated |
| **L2** tick count, game time and timed-reaction counter frozen; no system-command drain; first tick and drain after reveal (P12) | `settling_frames_hold_sim_scheduler_and_system_commands` + accumulator re-arm unit test | achievable as stated |
| **L3** no world present, no level sound, loading tree each redraw; queued level sounds dispatch on reveal | `settling_paints_loading_tree_and_defers_level_sounds` (Settling paint returns a world-less frame kind; system-command queue inspected) | achievable as stated |
| **L4** no Settling frame releases streaming sessions; active load not ended before reveal | `settling_keeps_install_streaming_sessions_through_reveal` (session generation identity) | achievable as stated |
| **L5** `loading.progress` never decreases; reads 1.0 on the last Settling frame, timeout included (P10) | `settle_progress_is_monotone_and_reaches_one_before_reveal` | achievable as stated |
| **L6** a UI input pressed in Settling activates nothing on the first Running frame; the clamp ages across Settling | `settling_drops_ui_input` + `limiter_frame_ages_across_settling` (extends the `main.rs` limiter test) | achievable as stated |
| **L7** cluster reaches Sampleable and block installs during Settling, without Running | `settling_streaming_step_promotes_cluster_and_installs_block` (session tests with the stub drain outcome capture fixtures already use) + M2 in engine | achievable as stated |
| **L8** no production path calls the spawn preload | grep gate test `no_production_path_calls_spawn_lightmap_preload` | achievable as stated |
| **L9** one presented pose for install, Settling and first Running frame; backdrop and menu-pushed relevel at the menu pose; else first `player_spawn` | `presented_pose_resolves_menu_then_spawn` (pure resolver) + `install_settling_and_reveal_read_one_presented_pose` | achievable as stated |
| **L10** a settled reveal records no SH visible miss and no drawn non-resident block, for pawn spawn and backdrop (P1, P9) | `settled_reveal_frame_counts_no_sh_or_lightmap_miss` | achievable as stated |
| **L11** timeout releases with exactly one warning; a settle on or before the deadline frame warns nothing (P3) | `settle_timeout_warns_once` + `settle_on_deadline_frame_releases_without_warning` (log capture) | achievable as stated |
| **L12** `restartLevel` after a timed-out entry gets a fresh timeout | `restart_after_timeout_starts_fresh_settle_timer` | achievable as stated |
| **L13** load request during Settling: no stale settle or timer; a request on a would-settle frame unloads first, no reveal edge (P2) | `level_request_during_settling_unloads_without_reveal_edge` | achievable as stated |
| **L14** suspend during Settling: no reveal edge, no settle state survives (P7) | `suspend_during_settling_leaves_no_settle_state` (`reset_boot_state_after_suspend`) | achievable as stated |
| **L15** Settling during prior-generation retirement issues no read until join, then settles (P8) | `settling_waits_for_retirement_then_settles` | achievable as stated |
| **L16** staged script commit during Settling recomposes reactions (P13) | `staged_commit_during_settling_recomposes_level_reactions` | achievable as stated |
| **L17** reveal-frame promotion at full weight; later promotion ramps over `PROMOTE_SECONDS` | `reveal_frame_promotion_starts_whole_later_promotion_ramps` (renderer light-slot data logic) | achievable as stated |
| **L18** regression guard: after Running, a non-resident visible cluster renders ambient floor that frame | `running_frame_never_waits_on_cold_cluster` (async controller: miss counted and sample word zero, no stall path) | achievable as stated |
| **C1** both peers publish parity at install; divergent client gets its cause during Settling | relay-seam test `parity_published_at_install_reaches_settling_client` | achievable as stated |
| **C2** parity-matched settling client held; pawn on the poll draining its reveal; one promotion when parity and reveal share a batch | `revealed_declaration_promotes_once_even_batched_with_parity` | achievable as stated |
| **C3** timed-out client still declares; timed-out host records reveal and promotes (P6) | net: `host_reveal_promotes_revealed_matched_client`; engine: `timeout_release_is_a_reveal_edge` | achievable as stated |
| **C4** client revealed while parity mismatched is promoted the moment parity matches | `revealed_before_parity_match_promotes_on_match` | achievable as stated |
| **C5** reveal never arrives: held with revealed cause, connected past timeout, no entity state | `unrevealed_client_stays_admitted_and_receives_no_entity_state` | achievable as stated |
| **C6** duplicate reveal: no second entry; reveal naming another level does not promote | `duplicate_or_foreign_reveal_does_not_repromote` | achievable as stated |
| **C7** client unload or suspend retracts; same-level re-install held until its own reveal, even when unload and install collapse into one batch (P7) | `retracted_reveal_holds_reinstall_until_new_reveal` | achievable as stated |
| **C8** host Settling: no promotion, client stays connected through the hold; promoted once on host reveal with pawn; also after host same-level `restartLevel` (P4, P5) | net: `host_unrevealed_holds_revealed_client`; engine: `host_reveal_recorded_after_settling_poll_spawns_pawn_in_world_poll` (frame-order test) | achievable as stated |
| **C9** host suspend clears own reveal; resumed same-level install promotes only at new reveal (P15) | `host_suspend_clears_own_reveal` (engine: `clear_net_level_parity` path) | achievable as stated |
| **C10** host restart: Running client re-promoted at host reveal without re-declaring; settling client promoted once after both reveals in either order; no client reload (P14) | `host_restart_repromotes_revealed_client_once_in_either_order` | achievable as stated |
| **C11** mod-digest demotion and recovery re-promotes without a new reveal | `mod_digest_recovery_repromotes_without_new_reveal` | achievable as stated |
| **C12** disconnect while held: no pawn, record gone, rejoin held until it reveals again | `disconnect_clears_revealed_record` | achievable as stated |
| **C13** revealed-only demotion sends a Holding diagnostic and retires the client epoch | `revealed_only_demotion_sends_holding_and_retires_epoch` | achievable as stated |
| **C14** declaration and both causes round-trip; previous wire constants refused at gate 1 | extend `control_envelopes_round_trip`; append-layout guard mirrors; `revealed_vocabulary_refuses_previous_protocol_id` | achievable as stated |
| **R1** SH mandatory = Visible, Pinned, owners, plus clusters of id-51 entries with lead ≤ L; a cluster with all cells past L is not mandatory | `sh_mandatory_tier_is_id51_reach_within_lead` | achievable as stated |
| **R2** one L drives both; L change moves both sets; SH-only level uses L; decline mid-level or mid-Settling leaves SH lead tier unchanged (P11) | `one_lead_drives_sh_and_lightmap_and_survives_decline` | achievable as stated |
| **R3** non-portal path in play: SH requests every drawn cluster Visible; lead and band from the stage's classes | `non_portal_path_keeps_sh_visible_and_stage_lead_classes` | achievable as stated |
| **R4** Streaming tab slider, capture `lead_metres` and walk measurement use level-scope L; slider present for SH-only and after decline | `streaming_levers_read_and_set_level_scope_lead` + dev-tools build check; walk measurement compiles against the stage | achievable as stated |
| **R5** over budget, mandatory SH admitted (pool grows); band yields farthest lead first | `band_yields_farthest_lead_first_mandatory_never_refused` | achievable as stated |
| **R6** within budget, a departed Sampleable cluster is still evicted after hysteresis | extend existing departure-eviction tests to the new classes | achievable as stated |
| **R7** no production path reads `WARM_SET_CLUSTERS` or the id-46 warm walk | grep gate test `no_production_path_reads_warm_set` (module deleted) | achievable as stated |
| **R8** re-read counter increments on a re-read after eviction, not on first read | `reread_counter_counts_only_post_eviction_reads` | achievable as stated |
| **R9** each visible SH miss in exactly one bucket, each bucket reached; Settling adds nothing; timed-out reveal counts once (P9) | `visible_miss_lands_in_exactly_one_bucket` (one fixture per bucket) + `settling_records_no_visible_miss` | achievable as stated |
| **R10** Line C's `first_level_frame` on the reveal frame; Settling hold its own mark | `line_c_marks_settle_hold_and_reveal_frame` (source-order and timing-name test) | achievable as stated |
| **M1** SH mandatory bytes per camera cell at default L on hallway-inspection and campaign-test, beside lightmap; worst and p95; stop if over the threshold | owner-visible report from `#[ignore]` harness (task 1) | manual |
| **M2** visual: no ambient-floor SH or SH-only lightmap in the first view, four entry routes, two maps | owner, in-engine | manual |
| **M3** resource run on largest stress map, before/after, pinned fixture and route | owner, in-engine (executor prepares the route and counters) | manual |
| **M4** route's in-play SH misses fall vs baseline; resident SH past budget is mandatory only | owner, from M3 data | manual |
| **M5** co-op loopback: both settle; neither sees the other's pawn move before its own reveal; host restart with a long settle | owner, two engines on loopback | manual |

## Tasks

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | **Measure the reach first (riskiest premise).** An `#[ignore]` harness beside the lightmap walk measurement: for every camera cell at default L, SH mandatory bytes = clusters of Visible (lead-0 entries) ∪ pinned ∪ owner closure ∪ id-51 entries with lead ≤ L, priced from the id-50 index, plus fixed metadata. Report lightmap mandatory bytes beside it, worst cell and p95, on `stress-warren-hallway-inspection` and `campaign-test`. First confirm both PRLs carry ids 49–51; if not, the owner rebakes (long bake). Stop for the owner if the worst cell exceeds the 256 MiB floor. | integrating executor | — | |
| 2 | **Split streaming from the Running frame.** One call, from a given pose: visibility → `prepare_streaming_drains` → lightmap drain → SH drain, compose and submit with no world present. The renderer gains a submit-only path beside `submit_frame_without_readback`. `present_world_less_frame` gains a variant that does not clear level streaming. Running calls the same seam, so behavior is unchanged. | integrating executor | — | |
| 3 | **Presented-pose chokepoint.** One resolver for position, yaw and pitch: the menu pose whenever the frontend menu is present, else the first `player_spawn` pose. Install, the streaming step and the first Running frame read it. Replaces `spawn_eye_position`. | integrating executor | — | |
| 4 | **Settle chokepoint and predicates.** SH: a class-filtered predicate (Visible, Pinned, owner closure, lead tier). Lightmap: mandatory plus drawn blocks, with drawn blocks requested on any visibility path while settling. Each answers "not yet asked" until demand updates from the settle pose (P1); a non-streaming resource answers settled. Capture's SH preload switches to the SH answer (S8). | integrating executor | 2 | |
| 5 | **Settling boot state.** Covers the following; L1–L18 and R10 pass with the lead tier stubbed to empty until task 7.<br>• Install → Settling on every entry. Dispatch returns false, so there is no sim tick, scheduler counter, system-command drain, gameplay input or world present.<br>• UI input dropped; loading tree painted; limiter aged; progress monotone to 1.0 (P10).<br>• Staged-manifest poll. `has_installed_level` and request draining count Settling as installed (P2, C5).<br>• Fresh 10 s timer with a one-shot warning (P3). Suspend reset (P7) and retirement wait (P8).<br>• Reveal edge: accumulator re-arm (C2), end of loading screen, promotions start whole, Line C marks.<br>• `install_spawn_lightmap` and its warning retired. Settling drain budget per Delegated answers. | integrating executor | 2, 3, 4 | |
| 6 | **Cell-demand stage.** Level scope, owning L, reading `LevelWorld.cell_residency_set` directly (C8). It turns id 51 plus the frame's path into per-cell classes: lead ≤ L, band, drawn, held. Lightmap's `BlockDemand` maps those cells to blocks, and `LightmapLevers` drops L. The Streaming tab slider moves to a level-scope section; capture and the walk measurement read and set the stage (C13). | integrating executor | 1 | |
| 7 | **SH reach.** Retire `warm_set.rs`, `WARM_SET_CLUSTERS`, `warm_rank` and the `warm_clusters` diagnostics. Add a lead class (mandatory, `DrainClass::Lead`, not pressure-eligible) and a band class (optional, trimmed farthest lead first in `compare_pressure_keys`). Both come from the stage via `cell_to_cluster`, with cluster lead = min over its cells. Visible stays drawn clusters on every path. With no usable id 51 there is no lead or band tier. The settle set includes the lead tier. | integrating executor | 4, 6 | |
| 8 | **Diagnostics.** Re-read counter. Miss buckets: outside reach, trimmed by pressure, read in flight, held by the drain budget, awaiting compose, failed. Settling records no misses, with the `prior_visible_misses` handling from C11. Wire into `ShStreamingLiveDiagnostics`, the log line's `cumulative_counters`, the Streaming tab and capture JSON. | integrating executor | 5, 7 | |
| 9 | **Net revealed term (crates/net, plus netcode test fixtures).**<br>• Appended `ClientControlMessage::Revealed(Option<String>)` and two `HoldingCause` variants, with Display arms.<br>• Client record and sent flag, re-sent on change.<br>• Host per-slot record, cleared only by `close_slot`; host self-reveal setter.<br>• The predicate term after every parity check in `reevaluate_parity`, with `parity_moved` set by the declaration (C16).<br>• `NetEndpoint` dual-role reveal setter.<br>• `PROTOCOL_ID` bump, append-layout guards, stale comment fixes, and one reveal fixture helper for the tests in C19.<br>• Proves C1–C14 on the relay seam. | delegated worker (concurrent with 2–5; files disjoint from the engine crate) | — | |
| 10 | **Engine co-op wiring.**<br>• Publish the host and client reveal on the Settling→Running edge, after the frame's Settling poll (P4); a timed-out reveal counts (P6).<br>• Clear both at unload and suspend through `clear_net_level_parity` (P5, P7, P15).<br>• Client: revealed holds log below warn. Host: the `ParityHeld` label matches on cause (C17).<br>• Relevel during client Settling does not reload (P14). | integrating executor | 5, 9 | |
| 11 | **Measurement prep for owner runs.** Rebuild stale stress PRLs if needed. Pin the route, machine, cache mode and baseline (`main` at `4677eda27`). Hand the owner M2–M5 checklists with the counters to record. | integrating executor | 8, 10 | |

Hot paths touched: the Running frame's streaming call (task 2: same work, new seam, no per-frame allocation), SH targeting (task 7: the warm walk is replaced by a cached per-camera-cell id-51 lookup recomputed only on cell or L change, as lightmap does today), and diagnostics counters (task 8: fixed counters, no allocation).
