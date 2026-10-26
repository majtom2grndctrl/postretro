# Independent review panel — host switch timing

**Target:** entire feature diff `a363ea550..05dd08f39`, 46 files, 4,858 changed lines. Subsequent HEAD9b505b532 changes checkpoint documents only. Source held unchanged throughout review.
**Verdict:** request changes. Required independent panel coverage is complete; fixes and regression verification remain.
**Panel:** 4 correctness tracers, 2 contract verifiers, 3 adversarial testers (Sol xhigh); 4 hygiene/drift reviewers (Sol medium). Four slices plus cross-slice seam pass. Coordinator triaged and aggregated only.

## Code findings

### 🔴 Must fix

1. **Same-slot resync cancels a valid active charge/burst** — `crates/netcode/src/host_switch_timing.rs:293–301` (correctness tracer + contract verifier + adversarial, 3x).

   Host and client already hold A, but owner-private samples still name B for the resync window. Charging/bursting does not block the detector; it only excludes pending declarations and lower/raise. A legitimate Resync(A) enters the same-slot branch and calls cancel_activation at301. An unreleased charge has no blocking ordering horizon, so this can erase its live cursor and return the weapon to Idle; charge release/remaining burst work cannot execute. The preserved contract says resync accept “is a no-op when the projection merely lagged” (networking.md:733), and client_weapon/mod.rs:69 promises “an idempotent accept”. Previous same-slot handling returned accepted before cancellation. Restore same-slot no-op acceptance before cancellation and add active-charge/burst coverage.

2. **A held refused reload becomes a fresh reload on the incoming weapon after trim** — `crates/postretro/src/host_activations.rs:150–155`, especially missing-stamp fallback154 (B adversarial + cross-slice correctness tracer, 2x).

   Honest sequence: switch A→B at100, lower50ms→repoint102 and raise50ms→fire105; press reload101 and hold through120. Stall the drain until commands100..120 queue, so trim retains119/120. Resolution119 applies begin100 but parks repoint102 behind preserved press101. That press bindsA and is refused/consumed on A. Resolution120 repointsB. No pending press remains, so press_edges.rs:194–209 preserves raw reload=true with reload_press_tick=None. Guard judges120 (after raise), and B latch remainsfalse, so machine.rs:177–184 starts a reload without a new press. Preserve press identity/consumption across binding change instead of interpreting missing delivered stamp as fresh current-tick intent. Add sustained-hold regression through actual trim skipping incoming raise. Support: host_switch_timing.rs:196–209 does not seed B latch.

### 🟡 Should fix — acceptance proofs

3. **Death scenario does not prove the repoint was not early** — `crates/postretro/src/host_activations/conditioned_tests/switch_timing.rs:861–875` (C tracer + hygiene, 2x).

   Death is introduced at110; assertion only checks slot1 when125 resolves. Repointing immediately at death110 still passes final owner/observer selection and that assertion. AC25 requires unchanged repoint tick. Assert host_selection_changes()==[(125,1)] for dies variant.

4. **Fresh switch after teardown/re-entry lacks exact timing proof** — `crates/postretro/src/host_activations/conditioned_tests/switch_timing_teardown.rs:297–301` (D hygiene, 1x).

   Current assertions establish eventual slot1, accepted outcome and existence of resolved306; they never connect selection to306 or show lowering before it. Immediate271 or delayed repoint passes. AC26 requires first rejoined switch to lower/repoint on its own ticks. Capture fresh pawn selection trace: slot0 through305, slot1 at306, for all three boundaries.

5. **Late-retarget test allows an extra host-time lower** — `crates/postretro/src/host_activations/conditioned_tests/switch_timing_ordering.rs:218` (D hygiene, 1x).

   changes[1].0>=130 permits arbitrary delay. With extra_delay20, declared repoint130 already resolved before delivery, so erroneously lowering for a full duration on receipt still passes eventual convergence. AC20 requires immediate catch-up. Record delivery/application resolution and assert second repoint there when overdue; assert130 exactly for earlier delivery branch.

6. **Disconnect/rejoin proof reuses the old transport/client** — `crates/postretro/src/host_activations/conditioned_tests/switch_timing_teardown.rs:185`, parity re-entry218–235 (D adversarial, 1x).

   Disconnect case directly calls authority cleanup and demote_client_state, then re-promotes same fixture.client/CLIENT. It never closes original transport or admits replacement. This bypasses NetServer::close_slot retirement of parity/reveal records, connection claims and participation epochs (net/transport.rs:595–608), plus new endpoint client id assignment (netcode/endpoint.rs:557). AC26 requires actual disconnect/rejoin. Close original, admit fresh connection and run first switch there. This is separate from finding4's timing assertion weakness.

## Comment drift

### 🔴 Misleading production resolver pointer

`crates/netcode/src/command_queue.rs:988–990` (A hygiene, 1x) says:

> Test seam only: production resolves through [`host_resolve_remote_commands_in_registry`]

Production instead invokes host_resolve_remote_commands_with_switches at postretro/main.rs:5154. The named old exported resolver skips timed switch callbacks, so the pointer directs future integration toward a path omitting this feature. Point to timed resolver and describe legacy resolver's remaining purpose. Live text and call were verified; not a stale historical finding.

## Triage and completed coverage

Partitions chosen from stat before diff scan, then triaged:

| Slice | Assigned paths / flow | Completed lenses |
|---|---|---|
| A (~1.2k) | All changed net/** and netcode/** plus plan; declaration intake→retention/frontier→lower/retarget/repoint→ordered outcomes. Public declaration kind/codec/version and resolver API. Exits to sim helpers and production resolution. | tracer, contract, adversarial, hygiene |
| B (~1.1k) | All changed sim/**; production postretro main/host_activations/client_weapon/frame_loop; old conditioned_tests/receive_alternate/switch_order changes; switch_timing_fixture.rs and remaining small constructor adapters. Resolution/equip flags→remote machine/shared countdown→outcomes. | tracer, contract, adversarial, hygiene |
| C (~1.13k) | switch_timing.rs + switch_timing_boundaries.rs; actual local equip/declarations→conditioned transport→host resolution→pending-chain/tuning/snapshot receives and boundary assertions. | tracer, hygiene |
| D (~1.38k) | switch_timing_lifecycle.rs + switch_timing_ordering.rs + switch_timing_teardown.rs; trim/stall/forced-bound order, release/refill/raise retirement, disconnect/demotion/reset→re-entry outcomes. | adversarial, hygiene |
| Seam | Full Input reload/start stamp→A queue/timing/frontier→B host guard/sim machine→C/D received owner/observer outcomes, plus release/liveness/retirement. | correctness tracer |

Every reviewer read required router/guidance, full brief index/research/plan and relevant routed context; outputs were bounded on resumed reads to close prior truncated contract read. Completed A tracer/hygiene from initial window reused. A contract resumed and completed. Reviewers inspected adjacent producer/consumer code as their lenses required, without general review by coordinator.

B tracer and helper-contract reviewer confirmed callback-before-drain ordering, flags before guard early return, single deferred atomic reload completion then lower, immediate reload abandonment, refused-latch consumption on same weapon, canonical local transition preservation, passed-dt boundaries, expired Lowering named endings, post-touch ownership checks and post-sim-before-snapshot settlement. Additional read-only arithmetic experiment:40,036 equip-duration cases matched independent exact-rational countdown, including16667µs ticks and fullu32 duration.

C traced production declaration-before-Input, actual outcome chain/tuning installation and source-tick snapshot assertions, actual owner timing, first predicted shot, distinct reload/fire expiry boundaries, verified trim cursor, reload completion tick/exact transfer/balance/idle return. D checked all1379 added lines and forced wait/overflow order with raw outcomes and real observer receipt. No extra findings beyond those listed. B/C/D hygiene found no live comment drift.

## What is done well

- Shared canonical lower transition preserves local/host semantics; fixed-tick duration derivation avoids duration-proportional work.
- Fixture settles through actual pending chains, installs production tuning, and applies observer snapshots rather than substituting expected state.
- Trim scenarios demonstrate actual cursor jumps; forced-bound scenarios assert raw outcome ordering and observer receipt.

## Resource and completion checkpoint

Initial window paused at owner cutoff (5%, then4%primary remaining); resumed2026-10-26 in fresh window. Usage checked before every worker dispatch; no reset credits consumed. Last seam check13%primary/86%weekly remaining. No Cargo, source edits or commits by coordinator/workers. Only this report written. Disk last checked63GiB available; final disk check reported separately.

All mandatory review lenses and seam pass now complete. Findings remain unfixed, and no approval implied. AC31 manual loopback remains external outstanding proof and blocks landing; passing focused tests do not substitute for it. Root owns plan, source fixes, tests/preflight and later disposition.

Suggested fix ownership: netcode host_switch_timing.rs for finding1; coordinated queue/host guard/remote machine repair for finding2 (cross-file semantics); switch_timing.rs for3; one teardown-file worker for4+6; switch_timing_ordering.rs for5; command_queue.rs documentation for drift. Keep test-proof repairs distinct from confirmed runtime regressions.

## Integrating executor resolution — focused gate verified

All findings addressed after the independent panel, with no Decision or Acceptance change. Original request-changes verdict above records the reviewed baseline; this ledger records the subsequent fixes.

| Finding | Repair | Focused verification |
|---|---|---|
| 1 | Same-slot acceptance returns before cancellation, preserving active cursor and existing raise gate. | same_slot_resync_preserves_active_charge_and_burst passes both charge/burst cells |
| 2 | Host reload entry requires a delivered press stamp; raw continued levels still update the latch. | refused_reload_held_through_trim_reloads_incoming_weapon_only_after_release_and_repress passes perfect/conditioned cells; actual trim119/120, no held reload onB, fresh142 starts once |
| 3 | Death test asserts sole host change125. | conditioned_outgoing_start_during_lower_is_refused_and_death_keeps_repoint passes |
| 4 | Fresh lower trace covers271–305 with origin/target/Lowering, then sole repoint306. | all three lifecycle boundaries pass |
| 5 | Trace retained declaration consumption around real timed resolution. Early branch exactly130; overdue branch repoints in application resolution. | conditioned_late_retarget_begins_a_fresh_lower_after_the_first_repoint passes both delays |
| 6 | Close relay slot7, process real disconnect/lifecycle, replace transport and endpoint with fresh client9, clear retired conditioners, then real parity/reveal admission. | closed oldslot, no retired reply/repoint, fresh exactly-once outcome and observer swap pass |
| Comment drift | Sampled resolver comment names timed production resolver. | live diff inspected; touched-crate check passes |

Serialized post-fix gate: formatting and touched-crate check pass; composed47/47 and netcode42/42 pass. Final preflight passes: workspace lint and full tests12,927passed/0failed/68ignored. ManualAC31 remains outstanding and blocks landing.

## Final preflight closure

The final gate surfaced additional test/fixture integration defects after the review fixes. All were corrected and verified without changing owner Decisions or Acceptance:

- Version-floor assertions use const blocks; timing comparisons read actual recorded reload/switch ticks.
- Both encrypted handshake and Control admission tests now use matching wire35 and reject stale wire34, preserving PRLB/wire32/33 refusal checks.
- Fixture reserve seeding uses canonical grant_ammo; no source-ownership whitelist was widened.
- Adversarial target replacement fills through canonical acquisition, retaining lower metadata so refusal still requires instance identity. Inventory ownership guard unchanged.

Clean final gate: formatting, workspace all-target lint with dev-tools, and full workspace tests with dev-tools. Independent log aggregation:71 result groups,12,927passed,0failed,68ignored. No further source edits after this successful gate; subsequent changes record results only. ManualAC31 remains outstanding.
