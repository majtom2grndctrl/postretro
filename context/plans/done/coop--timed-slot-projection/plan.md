# coop--timed-slot-projection — plan of record

mode: compact
status: landed
read at: 545c5c2f3

## Session constraint
- Owner requested usage monitoring. If either available usage window falls below 3% remaining, save a coherent checkpoint and pause at the next safe stopping point. Resume only on the owner's instruction. Latest resumed window began with 99% five-hour and 53% weekly remaining.

## Corrections
- Time-sync and snapshot/newest-gate consumers changed since d6f3471; re-opened current intake, estimator, reset and UI snapshot boundaries. `ClientTimeSync` now exposes an initialized-only estimate and RTT; use those directly.
- `StateApplyOutcome` still conflates an empty accepted batch and rejection. Add a local accepted-batch signal and per-slot refresh exclusions before collecting held-value samples. Wire and schema stay unchanged.
- Additional timed slot found: `player.weaponChargeProgress` is client-local activation presentation sampled from fixed ticks on each peer, outside the owner-private host timer stream. It is reported here and is not projected.

## Delegated answers
- Hold cap: 16 server ticks (266.67 ms), exceeding twice the longest four-tick stamp gap plus 60 ms jitter and one tick (210 ms). Check at 45 ms delay / 60 ms jitter / 5% loss and 144 Hz.
- Slope tolerance: 8%, covering a 1 ms carry over a shortest 16 ms timer interval (6.25%). Compare two positive signed slopes relative to the smaller.
- Timeline: initialized server estimate minus RTT/2, no snapshot-arrival re-anchor. Playback slews at 0.75–1.25× using frame delta and never rewinds. A reset clears its anchor, including while time sync remains live.

## AC-to-proof

| AC | Contract | Named proof | Status | Result |
|---|---|---|---|---|
| 1 | Committed red reproduction and production proof setup | `conditioned_reload_target_advances_between_samples` — red `384f85491`, then green | achievable as stated | pass |
| 2 | First agreeing onset | `conditioned_all_timed_ramps_obey_frame_bounds_using_host_sample_slopes` (onset class); `partial_started_interval_projects_on_first_two_full_irregular_host_intervals`; `quantitative_oracle_rejects_projection_delayed_past_first_qualifying_sample` | achievable as stated | pass |
| 3 | Between-sample frame bounds | `conditioned_all_timed_ramps_obey_frame_bounds_using_host_sample_slopes` (between class and fed-line holds) | achievable as stated | pass |
| 4 | Sample/feed frame bounds | `conditioned_all_timed_ramps_obey_frame_bounds_using_host_sample_slopes`; `future_sample_feeds_on_timeline_and_break_discards_waiting_line`; `quantitative_oracle_rejects_a_projection_jump_to_capacity_without_a_bound_sample` | achievable as stated | pass |
| 5 | Slope-break retargeting | `a_lost_slope_change_retargets_when_the_next_real_timer_sample_breaks_agreement`; slowing-heat case in `actual_shots_delays_wraps_and_tuning_jumps_show_unagreed_samples_raw` | achievable as stated | pass |
| 6 | Host-derived slope | `conditioned_all_timed_ramps_obey_frame_bounds_using_host_sample_slopes` — installed reload duration/cool rate 2× host | achievable as stated | pass |
| 7 | Jumps remain raw until agreement | `actual_shots_delays_wraps_and_tuning_jumps_show_unagreed_samples_raw`; `real_cooldown_reaches_zero_then_a_new_shot_resets_it_raw` | achievable as stated | pass |
| 8 | Reload/cooldown/heat/cell bounds | `actual_overheat_cools_to_zero_in_step_with_the_raw_latch`; `actual_cell_regeneration_reaches_capacity_and_every_frame_stays_clamped`; `cell_requires_capacity_naming_its_host_slot_and_bounds_are_immediate`; `timed_slot_receive_cell_repair_applies_capacity_bounds_without_sampling_cell` | achievable as stated | pass |
| 9 | Cancel, slot switch and exit fade | `slot_round_trip_and_both_resource_absences_drop_waiting_history`; cancel case in `actual_shots_delays_wraps_and_tuning_jumps_show_unagreed_samples_raw`; `timed_slot_ui_exit_fade_captures_host_fall_after_completion_in_same_frame` | achievable as stated | pass |
| 10 | Reload start/completion timing | `conditioned_reload_endpoints_stay_in_step_and_lost_endpoints_do_not_break_onset`; `real_completed_and_fall_snapshots_in_one_frame_are_both_sampled_in_order`; `completed_snapshot_overtaken_by_the_real_fall_never_becomes_a_sample`; `timed_slot_ui_exit_fade_captures_host_fall_after_completion_in_same_frame` | achievable as stated | pass |
| 11 | Accepted/held/excluded/rejected sample intake | `held_empty_and_held_id_batches_sample_but_rejections_and_refreshes_do_not`; `long_real_cool_delay_samples_held_resends_omissions_and_empty_snapshots`; `timed_slot_receive_deferral_and_undecodable_snapshot_do_not_sample`; `timed_slot_receive_empty_fingerprint_mismatch_preserves_agreement_and_cap` | achievable as stated | pass |
| 12 | Ordered multi-snapshot and low-FPS frames | `duplicate_and_reordered_snapshots_are_not_samples_and_low_fps_projects_at_frame_time`; `real_completed_and_fall_snapshots_in_one_frame_are_both_sampled_in_order` | achievable as stated | pass |
| 13 | Outage cap and recovery | `outage_caps_each_real_timer_then_recovers_at_the_new_sample_projection`; `outage_holds_at_cap_and_first_late_agreeing_sample_retargets_its_projection` | achievable as stated | pass |
| 14 | Before sync and first initialized frame | `first_real_sync_mid_ramp_preserves_the_raw_floor_then_advances`; `real_clock_reseed_and_folds_slew_without_a_meter_retreat` | achievable as stated | pass |
| 15 | Lifecycle/identity/absence resets | `timed_slot_history_clears_at_level_manifest_and_demotion_boundaries`; `slot_round_trip_and_both_resource_absences_drop_waiting_history`; `fresh_rejoin_clock_and_apply_resume_lower_host_ticks_without_an_old_clamp` | achievable as stated | pass |
| 16 | Raw audio, pull, dry-fire and reconcile readers | `timed_slot_ui_full_reload_target_keeps_raw_pull_and_near_full_cancel_silent`; `timed_slot_ui_regen_target_does_not_supply_cell_charge_to_dry_fire`; `timed_slot_ui_cooling_target_keeps_overheat_cue_frames_on_raw_latch`; `timed_slot_ui_cooldown_target_does_not_enter_weapon_reconciliation` | achievable as stated | pass |
| 17 | Applied-sample crossings only | `timed_slot_ui_projection_does_not_fire_raw_state_crossings`; `timed_slot_receive_crossings_preserve_order_and_dispatch_after_local_writes`; `timed_slot_receive_crossings_skip_rejected_duplicate_older_deferred_and_refresh_slots`; `timed_slot_history_clears_at_level_manifest_and_demotion_boundaries` | achievable as stated | pass |
| 18 | Shared UI targets and tween/reduce-motion | `timed_slot_ui_snapshot_drives_binding_predicate_max_focus_and_accessibility`; `timed_slot_ui_tween_follows_moving_target_and_reduce_motion_snaps_this_frame` | achievable as stated | pass |
| 19 | Host/SP and unrelated client slots | `timed_slot_ui_snapshot_drives_binding_predicate_max_focus_and_accessibility` — host/SP equality; conditioned harness every-frame unrelated-slot equality | achievable as stated | pass |
| 20 | No projection writes to store/components | `accepted_host_samples_project_four_timers_without_mutating_raw_slots`; conditioned harness every-frame host/client component+store invariance; static write-API grep | achievable as stated | pass |
| 21 | Unchanged wire/schema/fingerprint | Static byte comparison against `545c5c2f3`: schema/fingerprint/wire-shape tags+host production unchanged; net wire/handshake/state record sources unchanged | achievable as stated | pass |
| 22 | Owner visual/audio loopback | Owner reported all manual tests pass on 2026-10-25, following the runbook below | manual-visual | pass (owner) |

## Tasks

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | Extend loss harness with independent host/client clocks, production timers, newest gate and conditioned time sync; commit failing stepped-reload reproduction | integrating executor / bounded harness worker | — | done: `conditioned_reload_target_advances_between_samples` failed as intended (0 pass / 1 fail); all 248 eligible non-sample frames held. Setup assertions passed; 869-test target compiled. |
| 2 | Add accepted sample intake, four-slot agreed-slope projection and bounded slew timeline; resets and deterministic seam tests | integrating executor | 1 | done: covered by the final 36-test timer matrix, including first-sync correction, receive rejection, correlated bounds, lifecycle resets and four timer streams. |
| 3 | Integrate projection before every UI snapshot for connected clients; heat HUD and SDK documentation; UI/isolation proof | bounded UI worker / integrating executor | 2 contract | done: all 9 `timed_slot_ui_` CPU integration tests pass, including bindings/predicates/max, focus/accessibility, tween/reduce motion, exit fade and raw readers. |
| 4 | Extend conditioned harness across ramps, jumps, loss, clocks, caps, resets and raw readers; map every acceptance result | integrating executor / bounded harness worker | 2, 3 | done: 21 conditioned harness tests plus intake/lifecycle/receive seam tests pass; every automated AC has named proof. |
| 5 | Readiness checks, review-panel/fix loop, final preflight, manual runbook and result report | integrating executor | 4 | done: review fixes and focused retests pass; final fmt, Clippy and workspace tests pass. Manual AC22 passed by the owner's 2026-10-25 report; post-merge verification is recorded below. |

Hot-path constraints: four fixed histories; bounded pending samples; no allocations in frame projection; UI map substitution uses existing keys. Intake work is bounded by the existing validated batch and only four projected slots. Projection timing and fixed memory bounds are recorded below.

## Landing
Owner reported “All manual tests pass!” on 2026-10-25 and authorized merging latest main, preflight and landing. AC22 passes by that direct report. Merge `eb0dc2d36` incorporates `origin/main` at `d116329d7` without conflicts. Post-merge formatting, workspace Clippy and the full workspace suite pass. All 22 Acceptance rows now pass, including direct owner manual evidence. Brief archived in `done/` and roadmap marked complete for landing.

## External manual proof runbook

AC22 passed by the owner's report on 2026-10-25, after playtesting feature checkpoint `f730a92e5`. Retained reproduction recipe: use this branch's build on `coop-presentation-test`, with client presentation at 144 Hz and only the client conditioned to 45 ms delay, 60 ms jitter, 5% loss. The automated harness fixes the same cadence/profile; the owner supplies the visual and audio result.

From the checkout root, prepare binaries/content (tools compile nothing):

```sh
cargo build -p postretro -p postretro-level-compiler -p postretro-tool -p postretro-script-compiler --features postretro/dev-tools
target/debug/prl-build content/dev/maps/coop-presentation-test.map --baked-root baked --cache-dir .build-caches/prl-cache -o content/dev/maps/coop-presentation-test.prl
```

In two terminals:

```sh
RUST_LOG=info target/debug/postretro-tool run --install-root . maps/coop-presentation-test.prl --host 27015
POSTRETRO_NET_LAG=45,60,5 RUST_LOG=info target/debug/postretro-tool run --install-root . maps/coop-presentation-test.prl --connect 127.0.0.1:27015
```

1. Fire then reload an ammunition weapon; observe a smooth client reload ramp and its start/completion/cancel cues.
2. Pick up `coop_heat_test` at editor coordinates (128,384) with use. Hold fire: heat steps upward per shot, then cools smoothly. Overheat, wait for zero, and confirm the cooled cue matches the latch.
3. Select `reference_plasma_bolt` from the default loadout. Charge steps down per shot and refills smoothly up to capacity.
4. Compare with the host, then switch the client's Network tab to Off and repeat. Record build SHA, platform, actual frame rate, profile, and pass/fail for reload/heat/cell, plus any visible snap or cue mismatch. Do not infer a manual pass from the CPU suite.

## Verification checkpoints
- Red checkpoint `384f85491`: intended 1-test failure, all 248 eligible frames held between host samples.
- Focused netcode matrix after first-sync correction: `CARGO_INCREMENTAL=0 cargo test -p postretro-netcode --features dev-tools --lib timed_slot` — 27 passed, 0 failed, 875 filtered; includes 16 conditioned-timer tests.
- Static isolation gate: schema/fingerprint/wire-shape tags and host production byte-identical to `545c5c2f3`; net wire, handshake and state record sources unchanged (`SNAPSHOT_VERSION=21`, `WIRE_VERSION=33`, application protocol PRLC). Projection modules have no store or weapon-component write APIs.
- App UI integration: `CARGO_INCREMENTAL=0 cargo test -p postretro --features dev-tools timed_slot_ui_` — 9 passed, 0 failed, 1897 filtered. An unrelated integration target matched 0 tests (3 filtered); the intended unit target matched all nine.
- Quantitative budgets use accepted host overheat/capacity samples and the cooldown ramp's first sample. The 27-test matrix passes after this tightening.
- First-sync mid-ramp initialization now primes the agreed line on the first initialized frame. The real time-sync harness caught and verifies the correction; later sample frames satisfy the normal bound.

## Review checkpoint
- Reviewed immutable range `545c5c2f3..84e02bf3b` in four slices plus a crossing seam pass. Three correctness findings require repair: refresh-excluded cell retains stale capacity; empty state batches bypass fingerprint validation; multiple applied raw samples in one frame can hide an intermediate threshold crossing (AC17).
- AC17 remains achievable within the Decisions: retain ordered raw observations for threshold watchers on the four enumerated slots, then dispatch at the existing frame crossing stage. Predicate watchers, other slots and host/single-player retain their frame cadence. No projected value is observed and no store value is restored or rewritten.
- Harness repairs: echo time-sync probes before host catch-up ticks; derive onset from accepted samples independently; enforce frame limits when projection reaches a bound without a bound sample. Record projection timing and fixed memory bounds after repairs.
- Mechanical comment repairs: raw/UI boundary descriptions and the already accepted zero-delay aggregate-shot exception in `networking.md`.
- No Decision or Acceptance restatement is needed. Manual AC22 remains outstanding.

## Focused review-fix gate
- Compile and format checks passed after repairs. Initial matrix exposed two fixture assumptions: a purported held-id resend was encoded as FullBaseline; a Completed and fall accepted in one frame were incorrectly required to leave a full final frame. Corrected the delta fixture and tested ordered intake against P25. Added an outage case that requires projected completion by the P6 deadline before the fall arrives.
- Final focused matrix: 36 passed, 0 failed, 1 ignored (875 filtered). Broader state-slot module: 69 passed, 0 failed, 1 ignored. Detector: 18 passed. Simulation crossing integration: 16 passed. App UI: 9 passed. Ignored timing observation explicitly run and passed.
- Review fixes cover all three correctness findings, three harness proof repairs, performance proof and three comment/doc repairs. No Decision or Acceptance changed. Final workspace preflight passed; manual AC22 remains outstanding.

## Projection cost observation
- Explicit ignored observation `timed_slot_projection_frame_cost_observation`: 100,000 frame advances at the standard optimized dev profile, four agreed histories including capped steady state. Observed 30.3 ns/frame on this host; no wall-clock pass threshold. This measures projection advance, excluding the existing UI snapshot copy and snapshot intake.
- Four fixed histories, each with 32 pending lines; at most 128 waiting-line entries can be visited in a frame. Instrumented history container occupies 4,688 bytes, including four test-only sample counters; production is no larger. Frame advance allocates nothing.
- Raw crossing observations retain four tagged optional numbers per accepted batch, bounded by the transport drain, then drain at the same frame's crossing stage. Lifecycle resets clear retained observations. Reactions use the existing allocation/dispatch path only when an edge fires.

## Pre-integration final preflight
- `CARGO_INCREMENTAL=0 cargo fmt --check` — pass. Workspace formatter corrected one import order after the mechanical lint repair.
- `CARGO_INCREMENTAL=0 cargo clippy --target-dir target/preflight-clippy --workspace --all-targets --features postretro/dev-tools -- -D warnings` — pass. Initial lint found two unnecessary unwraps in the harness oracle; explicit matching preserves the calculation. Warm rerun passed.
- `CARGO_INCREMENTAL=0 cargo test --workspace --features postretro/dev-tools` — pass: 12,332 passed, 0 failed, 60 ignored across 71 result groups, including doc tests. No ignored test is counted as a pass. The performance observation was run explicitly in the focused gate.
- All 21 automated Acceptance rows pass. AC22 is outstanding manual proof; status is `test-ready`, brief remains in progress, feature branch remains local pending owner result and “land the plane.”
- Last resource check: 38 GiB free; usage 34% five-hour / 43% weekly remaining, above the owner's 3% pause threshold.

## Owner acceptance and main integration
- 2026-10-25: owner reported all manual tests pass and explicitly authorized “land the plane.” Manual proof is direct owner evidence; no frame-rate/platform measurements beyond that report are invented.
- Fetched `origin/main` at `d116329d7`, merged cleanly as `eb0dc2d36`. Latest main brings unrelated ghost replication wire versions (`SNAPSHOT_VERSION=22`, `WIRE_VERSION=34`); the timed-slot branch adds no wire/schema change relative to that main. The original wire gate against `545c5c2f3` remains the pre-integration historical proof.
- Post-merge formatting and workspace Clippy pass. Updated the receive test's snapshot/component initializers with main's new `ghost_audio: None` and `ghost_link: None` fields; no production behavior changed. Full workspace tests passed.
- Wire/schema/host-producer static comparison against fetched `origin/main` passes after integration.

## Landing verification (2026-10-25)
- Latest main integrated: `d116329d7`, merge commit `eb0dc2d36`; no merge conflicts.
- `CARGO_INCREMENTAL=0 cargo fmt --check` — pass.
- `CARGO_INCREMENTAL=0 cargo clippy --target-dir target/preflight-clippy --workspace --all-targets --features postretro/dev-tools -- -D warnings` — pass after the two test-only ghost field initializers were updated for main.
- `CARGO_INCREMENTAL=0 cargo test --workspace --features postretro/dev-tools` — exit 0; 12,877 passed, 0 failed, 68 ignored across 71 result groups. Luna monitored the existing process; integrating executor confirmed exit status and totals. Ignored tests are not counted as passes.
- AC22 passed by owner's direct 2026-10-25 report. No remaining acceptance gaps or owner decisions.
- Durable sample-intake, correlation/reset and crossing contracts updated in `networking.md` and `ui.md`; SDK documentation was already updated with the implementation.
- Landing cleanup targets only session temporary files and high-churn packages in this checkout's targets. No auxiliary worktree was created by this session; other chats' worktrees and the active checkout are retained.
- Resource check before cleanup: 27 GiB free, 79% five-hour and 97% weekly usage remaining, above the requested 3% pause threshold.
