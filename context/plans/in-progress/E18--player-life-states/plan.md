# Player life states — plan of record

mode: resumable
status: approved
execution: stopped after bounded batch 04; awaiting next step
read at: 834a09c51

## Resume point

Branch `codex/player-life-states`, claimed from `origin/main` in a953f386f. Tasks
1–7 are implemented and committed. The whole-feature review panel is complete:
31 of 31 passes over 15 slices plus three cross-system seam tracers. No 🔴
findings remained after repairs.

- Rounds 1–4 (Codex, 2026-10-30 to 2026-11-01) found runtime defects C1–C6 and
  comment/context drift D1–D13. All are repaired with regression tests and were
  committed in 6b7560e6e.
- Round 5 (Claude Code, 2026-11-01) ran the last ten passes against 6b7560e6e.
  🟡 findings were comment drift, author-doc and SDK text errors, and test gaps:
  mid-burst/hold/charge capability loss, vacuous late-baseline and look
  assertions, and endpoint O36/O42 coverage. All are fixed in the following
  commit, along with 🟢 nits (HUD clock frame cap, respawn placements republished
  on hot reload).
- B03-F1 is resolved by the owner's O44 restatement (see Corrections). The new
  proof runs through the production host presenter.

Review reports and batch summaries stay on local disk, uncommitted, at the
owner's request. They are not linked from this plan.

Remaining before landing: merge `main`, full preflight, AC17/AC18 manual visual
proof per [manual-playtest.md](manual-playtest.md).

Every build checks for an active `rustc` or `prl-build` first (owner rule, below).

The legacy brief has no compact/resumable header. Use resumable coordination
because its seven tasks cross descriptor validation, prediction, replication,
simulation ordering, respawn, and seat lifecycle contracts. This changes no
Acceptance or Ordering requirement.

Source verification is complete, including two read-only audit workers. The
owner resolved the acceptance conflict on 2026-10-27 as recorded below. The
amended timer requirements are feasible; no acceptance row needs restatement.
The owner approved this plan on 2026-10-27. Task 1 is complete; later tasks
retain the dependency order below.

## Owner decisions

On 2026-10-27 the owner selected and then reaffirmed continuous advancement,
including through lag and loss. Preserve AC18's every-positive-frame and
no-backwards requirements. O24 and Task 4 now require rate correction instead
of a hold, and O25 identifies predicted elapsed plus alpha as the correction
target rather than requiring equality to the displayed value.

The presentation clock advances from game-frame duration at a bounded strictly
positive rate and converges toward the predicted elapsed target. Backwards
prediction corrections temporarily slow it; forward deficits temporarily speed
it up. Ordinary corrections never assign the displayed value directly. Real
life-state/pawn/teleport resets still reset the clock. Host tick time remains
the source for gameplay conditions and the bleed-out deadline.

The owner also requires a process check for running `rustc` or `prl-build`
before every build because another session is building on this laptop. All
session agents received this instruction. Defer builds while either is active
and recheck before starting; continue independent reads/edits in the meantime.

## Corrections

- **O44 restated by owner (2026-11-01, resolves B03-F1).** Original: "draws the pawn at the
  placement at every alpha of T+1." The production host presenter renders connected-client
  pawns on a delayed timeline, as teleporter arrivals already do (`networking.md`). A respawn
  bypass would show the arrival before the delayed death and fork the discontinuity rule.
  Owner chose the general invariant: a discontinuity is a cut, never a blend, on whatever
  timeline the viewer renders. No runtime change. Proof must run through
  `host_present_client_pawns`, shared with the teleport arrival case.
- The brief predates workflow mode headers and has no `read at` SHA. Resumable
  source verification must re-read the cited symbols; continuity is not proof.
- Task 5's Path excludes pawns with later respawn requests from overlap tests.
  AC13 instead says another live pawn blocks a placement and that earlier
  relocations affect later picks. Follow AC13: test all other live pawns at
  their current positions, including pending later requests. The Path wording
  has been corrected. This adjusts the approach without
  changing AC13 or O18.
- Task 4's frame-final owner-slot read cannot witness a reset followed by
  another accepted snapshot in the same frame. Retain a local reset witness
  while applying accepted owner samples, and expose it to the display clock.
  A kept-name table remap changes no logical state identity and must preserve
  display continuity. No new wire field is needed.
- Task 3 named `switch_start_refused` as a switch-request gate. It actually
  gates activation starts during switch timing. Use the Declaration path in
  `host_switch_timing::apply` for `switchWeapon` and preserve its correlated
  refusal/active-slot resync; keep down-state firing independent. Consume raw
  reload levels while suppressing refused reload work and charge cancellation.
- Task 5's teleport arrival reuse needs canonical spawn capsule/eye and
  integration-state restoration before ground classification. Carrying the
  downed crouch capsule to a standing centre otherwise leaves the pawn airborne.
  Preserve the placement foot anchor and apply the target profile coherently,
  including the no-command remote path. The Path now calls this out.
- Joinable spawn eligibility permits differing descriptor capsules. Respawn
  retains the pawn archetype, so a fallback's precomputed standing centre may
  belong to another capsule; hot reload can also change capsule dimensions.
  Publish each placement's map foot anchor beside its resolved pose, derive the
  retained pawn's centre from the current target capsule at drain, and use that
  centre for overlap and ground settle. This preserves same-class spawn poses
  and AC13 without tightening join eligibility.
- Task 5's existing App command preparation runs before the sim prelude, and
  `host_resolve_remote_commands_with_switches` already judges switch requests.
  A respawn drained only inside the sim would let old-state admission strip an
  O33 activation, and judge T+1 switches against the old state. Preserve the
  outcome by factoring a two-phase tick-start seam: snapshot/mutations/physical
  respawn before resolution, finish arrival yaw against resolved facing, then
  prepare activation admission and run the started simulation tick. Keep the
  existing headless entry responsible for its own tick start. This is a Path
  correction; no Decision or Acceptance requirement changes.
- Existing surface harnesses are precedents, not complete AC20 proof:
  player-event `World` runs no movement/respawn, and the private teleporter
  `Level` harness binds no seats and runs no player-event pass. Build a focused
  two-seat production sim → player-event harness for the new consumer.

## Delegated answers and source findings

- Respawn with no resolvable `player_spawn` — logs a `[Respawn]` warning and leaves
  the pawn unchanged; no fallback. Owner (2026-11-01): a map without a spawn cannot
  boot, so this state is upstream of respawn and unreachable in practice.
- HUD feasibility — resolved by the owner's continuous-advancement decision.
  Use fixed-size high-precision presentation state and bounded positive rate
  correction at the store-to-UI seam; exact tick values remain separate.
- Descriptor placement — `EntityTypeDescriptor` has no `Default`; both
  `data_descriptors/{js,lua}/entity.rs` readers enumerate component fields.
  Add an optional descriptor field and a shared validator, updating every
  literal. The global name limit belongs in both manifest-reader paths.
- Registry lifetime — `DataRegistry::clear` clears level registrations and
  keeps entity descriptors; `replace_entity_types` deduplicates the committed
  snapshot; the session startup drain currently calls `upsert_entity_type` in
  a loop. Rebuild the name table once after the drain and on replacement.
- Reload — `commit_staged_manifest_result` applies the movement refresh before
  `replace_entity_types`; `plan_movement_replace` reconstructs the component
  and carries explicitly listed live fields. Carry life fields and stamps,
  then perform the separate authority-only remap after the table rebuild.
- Reaction mirror — `reaction_dispatch::is_trigger_consequential_primitive`
  still lacks `grantHealth`/`grantAmmo`. The Task 1 mirror correction remains
  necessary alongside the new verbs.
- Tuning encoding — `netcode/src/tuning_payload.rs::encode_tuning_payload`
  reconstructs the canonical payload through `TuningPayload::new`; the new
  fields must survive that reconstruction as well as ordinary construction.
- Forced posture — `physics/src/movement/intents/normal.rs` tests slide before
  crouch, and `crouching.rs` allows dash and crouch-jump. Forced crouch needs
  precedence through those consumers to satisfy AC5 when abilities remain true.
- Correction smoothing — `reconcile.rs` selects a correction class, but
  `prediction::seed_presentation_offset` independently snaps composed offsets
  at its threshold. Task 2 must preserve smoothing through both decisions.
- Current wire versions match the brief: tuning epoch 15, wire 35, snapshot 22.
  The fixed step is 16,667 microseconds; use integer tick-duration arithmetic,
  which still first reaches 15,000 ms on tick 900.
- Fire ordering — `sim/mod.rs` captures `local_fire_blocked` before Movement
  and consumes it in Weapons. Preserve that vault verdict while separately
  evaluating current life-state fire capability at Weapons for O16.
- Trigger ordering — the existing trigger capture is scoped to optional
  `trigger_context`, while pickups run afterward regardless. Capture life
  gates before dispatch and share them with pickups even without that context.
- Seat harvest — `SeatTable::harvest_pawn_inner` returns early when Health,
  Reserve and Inventory are all absent. A life-state-only pawn must also count
  as harvestable when the carried name is added.
- Health normalization — `set_health_absolute` rearms the death latch on
  positive health, but maps NaN/negative health to zero. Respawn must first
  warn-normalize invalid health to max, as AC13 requires.
- Arrival consumers — owner reconcile, observer generation cuts, host camera
  hold and `arrival_sound(NO_TELEPORTER)` support the proposed relocation/silence.
- Client lifecycle — payload install and snapshot materialization are separate
  rebuild callers. Both need table history; level reset/demotion must retire
  history and local timer reset evidence alongside existing tuning.
- Fire/charge — capability-aware liveness must not inherit the vault-only
  `charge.interruptedBy.vault: false` exemption. The independent death-sweep,
  host activation, client frame and charge-HUD sites remain required.
- Proof harnesses — the relay uses production prediction/reconcile but no App
  UI/eye construction. Set 75 ms explicitly each way (its default helper uses
  45 ms); use App-local deterministic eye/timer proofs plus manual feel checks.
- SDK regeneration — the existing generator command is
  `cargo run -p postretro-sim --bin gen-script-types`; committed-type and mirror
  tests already protect both runtime outputs. It counts as build work and
  requires the process precheck.

These findings are planning evidence, not implemented fixes. Current source
still supports the brief's mechanisms and ownership boundaries; no additional
false Decision premise was found.

## Continuous display-clock design

Use an `f64` millisecond integrator in a new App-side module. Let `D` be the
previous display and `P` the current predicted elapsed-plus-alpha target. For
a positive game-frame duration `dt`, advance by `dt * rate`, using a finite
rate bounded above and strictly above zero. A proportional correction against
`P - (D + dt)` gives ordinary frames rate 1, and absorbs target jumps without
assigning `D` or clamping it to its previous value. Reseed only for real resets.
Zero-duration frames return the previous value. Advance once per game frame;
loading/frontend snapshot reads do not advance the gameplay clock.

Planning experiment: with a 250 ms response time and rate bounds 0.25–1.25,
both ±50 ms target corrections at 30, 60, 144 and 240 Hz advanced on every frame,
including the UI's existing f32 conversion, and converged to under 0.016 ms
error after two seconds. This validates the proposed design, not engine code.
The exact response constants can be tuned during focused verification without
weakening continuous progression or moving gameplay deadlines.

The high-precision integrator avoids accumulated rounding; the UI's existing
`SlotValue::Number(f32)` still has its normal sampling precision. Include
rendered-value assertions at practical frame rates over the full 15-second
consumer interval, as well as internal clock/rate invariants. This change
does not widen the shared slot schema.

## AC-to-proof

Focused proof is recorded below and in `ordering-proof.md`, with executed
counts in the task checkpoints. Review and the final preflight remain pending.
No manual visual result is inferred.

| AC | Proof | Status | Result |
|---|---|---|---|
| 1 | Shared validator plus QuickJS/Luau descriptor and startup/staged-manifest parity cases for every rejection, valid defaults, rejoin, and the 255-name limit | automated | PASS (focused proof; final gate pending) |
| 2 | Undeclared zero-HP regressions across fire cancellation, vault/teleport, occupancy/damage eligibility, and ungated movement/look/switch/reload/use/touch/pickups | automated | PASS (focused proof; final gate pending) |
| 3 | Spawn/join/restart and level-change initial state plus elapsed-zero assertions before the first tick; O15 pins first condition reads | automated | PASS (focused proof; final gate pending) |
| 4 | Capability-by-capability refuse/permit pairs including zero HP; production host/local/client weapon paths; movement replay and owner look sampling; held/burst/charge and vault interruption | automated | PASS (focused proof; final gate pending) |
| 5 | Forced-crouch entry/hold/exit tests with abilities both enabled and disabled, including slide/dash/jump precedence and blocked stand clearance | automated | PASS (focused proof; final gate pending) |
| 6 | Trigger membership retained without spurious Exit, pickup and teleporter refusal/permit at zero HP, occupancy counts, O35/O40 | automated | PASS (focused proof; final gate pending) |
| 7 | In-tick command execution, same-state no-op, exact 900-tick threshold, and all O15/O39 first-read values | automated | PASS (focused proof; final gate pending) |
| 8 | Install and bind diagnostics naming reaction/state; both verbs; condition and crossing binding; runtime missing-state/no-state warnings | automated | PASS (focused proof; final gate pending) |
| 9 | Two-pawn player-event predicates and own-pawn local crossings in both runtimes | automated | PASS (focused proof; final gate pending) |
| 10 | Local String and owner-private Number publication/replication through the UI frozen map and visibleWhen resolver; undeclared empty name | automated | PASS (focused proof; final gate pending) |
| 11 | Production tick-start respawn drain: all velocity/ability resets, settled ground, yaw, health/latch, state elapsed, cancelled weapons/vault, retained inventory | automated | PASS (focused proof; final gate pending) |
| 12 | Owner teleport-class reconcile and observer cut with NO_TELEPORTER suppressing stale arrival sound, including O28 coalescing | automated | PASS (focused proof; final gate pending) |
| 13 | Seat/map-order overlap selection, sequential batches, no-free warning, malformed health clamp, non-player skip; AC13 governs later queued pawns | automated | PASS (focused proof; final gate pending) |
| 14 | Seat lifecycle scenarios for disconnect/hold expiry, suspend/demotion, reload removal and rejoin map, held seats and level-change clearing | automated | PASS (focused proof; final gate pending) |
| 15 | Late-join FullBaseline round trip carries teammate state/elapsed/revision/crouched capsule; host predicate stays true during join | automated | PASS (focused proof; final gate pending) |
| 16 | Authority reload remap for kept/removed/new/undeclared states and changed profiles, client no-remap, demotion/rejoin seam | automated | PASS (focused proof; final gate pending) |
| 17 | 75 ms each-way conditioned reconcile harness, oversized transition, eye-height envelope versus crouch input; two-client dev-mod visual playtest | automated plus manual-visual | automated PASS; manual visual proof outstanding |
| 18 | Deterministic frame-clock tests: strict positive-frame advance through trims, delay/loss and reordered snapshots, bounded-rate convergence, zero-duration frames and reset witnesses; owning-client playtest | automated plus manual-visual; owner wording resolved | automated PASS; manual visual proof outstanding |
| 19 | One named outcome test for every O1–O44 row, including amended O24/O25, linked in an ordering coverage ledger before final review | automated | PASS (focused proof; final gate pending) |
| 20 | TS/Luau consumer parity and a production two-pawn simulate-tick → player-event test through 900 ticks and next-tick respawn | automated | PASS (focused proof; final gate pending) |
| 21 | Review scripting-reference and teleporter verb tables/recipes against both SDKs and bundle command availability | manual-doc review plus SDK surface checks | PASS (focused proof; final gate pending) |
## Tasks

The integrating executor owns all shared contracts, `plan.md`, integration,
Cargo execution, commits, and final proof. Proposed workers receive the full
brief/research, Acceptance and Ordering rows, routed context, file ownership,
and upstream/downstream contracts. They do not commit or edit this plan.

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | Thin slice: shared descriptor validation → global table/profile stamps → spawn/state verb/predicate → tuning/wire history → production fire gates | integrating executor | plan approved | done: focused checks, conditioned fire and SDK proof pass |
| 2 | Movement value/ability/posture gates, teleport touch gate, owner look sampling, forced-smoothed reconcile and presented eye ease | movement worker; shared stamps/reconcile integration owned by executor | 1 | done: movement, relay ordering and eye/look proofs pass |
| 3 | Switch/reload/use/touch/occupancy/damage gates; single Triggers-stage capability capture and player_life helpers | simulation worker | 1 | done: 7 sim, 2 netcode and 2 app behavior tests pass |
| 4 | Number/String reads and SDK mirrors, continuously advancing HUD clock with rate correction, local predicates and authoritative reload remap | integrating executor | 1 | done: catalog, publication, clock, remap, replication and SDK proofs pass |
| 5 | Respawn SDK/binding/queue, published placements, shared teleport-arrival tail, tick-start seat-order relocation and yaw propagation | integrating executor | 2, 3, 4 | done: 20 library + 6 app respawn tests and regressions pass |
| 6 | Carried state names, disconnect/demotion/suspend harvest, rejoin restore and level-unload clearing | integrating executor | 5 | done: harvest, spawn, real App lifecycle and regressions pass |
| 7 | Dev down/bleed-out TS/Luau twins and HUD, baseline/lifecycle coverage, complete Ordering ledger, bundle-facing docs and manual runbook | integrating executor | 6 | done: implementation/focused proof; manual AC17/18 outstanding |

First proof is Task 1's real descriptor/verb/condition/wire/fire slice: it
tests the shared substrate every subsequent task consumes. Tasks 2–4 run
concurrently only after Task 1 passes; the executor coordinates their small
shared-file edits. Each completed resumable task commits its proof and plan
update together.

## Ordering proof ownership

### Task 1 proof checkpoint (2026-10-28)

- Focused `cargo check` for postretro, netcode and sim with dev-tools and all
  targets passes after repairing an import and two test endpoint literals.
- Cross-library `life_state` filter passed: combat-model 1, entities 4,
  foundation 1, net 1, netcode 6, scripting-core 11. The first sim pass exposed
  an elapsed-clock bug: the extra host pass inferred the locally stepped pawn
  from a fallback registry lookup. The movement result now explicitly carries
  the successfully stepped pawn. Focused sim retest passes all 8 cases.
- `cargo test -p postretro --features dev-tools --bin postretro life_state`
  passes 4 tests: install diagnostics; conditioned player event → fire refusal
  → owner state adoption at 150 ms RTT; permitted zero-HP held/burst actions;
  permitted zero-HP charge across repeated death sweeps and release.
- O15 trigger entry and O39 scheduled-tail drain/reset are covered by
  `life_state_o15_trigger_write_reads_zero_before_next_host_tick` and
  `life_state_o39_wait_tail_enters_at_drain_and_reload_drops_pending_tail`.
- SDK TS/Luau declarations and full-registry fixtures have been regenerated.
  Typedef consistency passes all 50 tests; O26 passes both history and production
  reconcile/replay tests; tuning codec passes all 17 tests; movement round-trip
  and SDK export inventory each pass their one focused test. The inventory was
  rerun after the usage-window interruption to recover its result.
- Formatting and whitespace checks pass. No full workspace suite has run.
  Tasks 2–4 may now run concurrently under the approved ownership boundaries.

### Tasks 2–4 integration checkpoint (2026-10-28)

- Implementation is integrated: movement and teleport gates, look sampling,
  forced-smoothed reconciliation and render-only eye easing; interaction/combat
  gates with immutable Triggers-stage capture; exact elapsed facts, local names,
  UI-only rate-corrected clock, authoritative reset witness and reload remap.
- All-target `cargo check` for app, netcode, sim, physics, scripting-core and UI
  with dev-tools passes. Initial fixture import/API mistakes were repaired.
- First focused netcode run passed 15/16 cases, including both 150 ms relay
  transitions and the above-teleport-floor glide, O14 profile-only payload
  ordering and both authoritative reset-witness receive tests. The remaining
  switch fixture had no movement component; it now installs a genuine component
  before state entry. The same focused batch is rerunning. No failing behavior
  has been accepted as proof.
- A read-only source audit confirms finite ordinary state transitions on
  zero-tick receive frames leave frame-timing and eye raw endpoints paired;
  render correction remains continuous until the next predicted tick.
- The repaired library batch now passes: netcode 16, physics 8, scripting-core
  17, sim 19 and UI 1. App `life_state` tests are currently compiling (unified
  clock/display/eye/look/client weapon/conditioned paths); recover the active
  process or `/tmp/postretro-e18-app-tests.log` after any interruption.
- Task 2 exact O9/O13 assertions pass in both directions at 150 ms RTT: at
  most one correction exceeds the measured no-transition bound, k+1 and later
  shared-state records stay within baseline + 0.0001 m. Both 11 m/s and 80 m/s
  runs pass. O43's post-movement production verb ends the active vault before
  T serializes; T+1/replay remain non-vault and k+1 matches the paired baseline.
  The expanded netcode `life_state` filter passes all 17 tests.
- Owner-private catalog-driven pawn lookup/replication passes all three tests,
  including distinct elapsed values for two pawns and absence semantics.
- App `life_state` passes all 15 tests, including the three eye/look proofs,
  four continuous-clock proofs, two UI projection proofs and conditioned weapon
  paths. Task 3 is complete: its seven sim, two netcode and two app cases pass.
- SDK generation completed; typedef consistency first passed 49/50. The failed
  check assumed a contiguous old subset of player refs. It now checks the
  hierarchy while the existing source-derived catalog loop checks every leaf
  and capability. Focused retest passes all 50 tests.
- Catalog tests pass all 14 cases; formatting and whitespace checks pass.
  Task 4 is complete. The presentation value is tested separately from gameplay
  slots; AC17/AC18 external visual proof remains outstanding.
- Task 5 begins next, using its recorded two-phase tick-start and foot-anchor
  corrections. No full workspace suite or final preflight has run.

These are required scenario allocations. Replace each with concrete executed
test names/results as its task completes. Shared rows require proof at both
listed boundaries; no allocation is a passing result.

| Task | Required Ordering rows |
|---|---|
| 1 | O15 (set entry ticks), O26, O36, O39 (post-wait execution), O42 |
| 2 | O9, O13, O16 (movement/look), O38, O40, O43 |
| 3 | O11, O12, O16 (stage capture/fire), O35 |
| 4 | O8, O14, O15 (millisecond reads), O24, O25, O27, O34, O37, O39 (millisecond reads) |
| 5 | O3, O4, O10, O15 (respawn), O17, O18, O19, O20, O28, O29, O32, O33, O44 |
| 6 | O6, O7, O15 (rejoin), O21, O22, O23, O30, O31, O41 |
| 7 | O1, O2, O5; audit all O1–O44 names/results after integration |

## Task 5 implementation checkpoint (2026-10-28)

- Shared host-local requests/foot placements live in `entities::respawn` and on
  `ScriptCtx`. Requests coalesce by entity id and preserve optional state names.
  `Seat` now derives ordering for deterministic seat-keyed placement maps.
- The enqueue/runtime/sequenced/bound command paths, install/name checks and
  placement publication/unload cleanup are implemented. Queue survives reload
  and suspend; actual level unload clears it and the placement projection.
- `sim::begin_tick` snapshots transforms, applies queued teleporter changes and
  physically drains respawns before App remote switch/fire admission.
  `finish_tick_start` resolves yaw from resolved inputs or old transform yaw,
  runs the shared arrival tail and returns a consumed `StartedTick` token.
  App rotates raw inputs before activation preparation; retained start aim
  reads the updated accumulated yaw. The original sim API remains a wrapper
  that performs the same start for headless callers and rotates prepared aim.
- Drain rebuilds canonical movement, derives the target capsule centre from
  the foot anchor, picks against every other live pawn in seat order, restores
  health/latch and initial/named profile, cancels activations, and preserves
  inventory. Undeclared respawn stamps the current table revision, avoiding an
  unresolvable revision after a descriptor loses its states.
- Ten sim drain tests, three shared-arrival tests, five enqueue tests, four
  App placement/unload tests and two conditioned network tests are complete.
  The unmarked local fallback is aligned with the actual movement consumer;
  dedicated tests cover that fallback with a remote input present and full-tick
  crouched host/command-free remote -> standing alive restoration.
- Formatting/whitespace and touched-crate all-target check pass. Three test
  fixture type/mutability mistakes were fixed. Focused `respawn_` passes:
  physics 3, scripting-core 1, sim 16, App 6. The App tests include actual
  conditioned command-100 admission/turned start aim/one movement step (O33),
  owner teleport cuts, and an odd-T teleporter/even-T respawn coalesced observer
  cut with no replayed cue (O10/O28). Sound proof observes production requests,
  not audio-device playback. Sim `life_state` regression passes 19; App passes
  17 including the new network cases. All filters matched tests.
- Task 5 is complete. Task 6 grounding confirms all harvest paths must receive
  the current name table, life-only movement must bypass the old component
  presence early return, held records need an explicit unload-only name clear,
  and remote restore runs before seat binding, so it cannot require membership.
- Tasks 6/7, review-panel/fix loop, final preflight and manual runbook are still
  outstanding. No full workspace suite has run.

## Task 6 implementation checkpoint (2026-10-29)

- Carried records store an authored name. All harvest routes receive current
  DataRegistry names, including world-less/running disconnect/demotion polls,
  host suspend and unload. Movement-only pawns harvest; absent movement keeps
  the prior value, while a present undeclared/unknown state clears it.
- Level unload clears names on live and held records. Suspend retains them.
  The native callback delegates its existing CPU sequence unchanged through
  `App::suspend_level_state`, enabling production lifecycle tests without a GPU.
- Both actual spawn routes restore the current descriptor's rejoin target or
  initial at elapsed zero, before net-slot seat binding. Spawn/respawn share
  posture shaping so a forced crouch begins at the correct foot anchor even
  before the first remote command.
- All-target check passes; four must-use test warnings were repaired. Focused
  harvest (4), real host demotion (1), spawn/first-tick `rejoin_` (8, including
  one SDK surface case), exact O7 next-level materialization (1), and actual App
  lifecycle (6) all pass. O23/O41 first used a forbidden faction-roster edit;
  the fixture now changes a supported hashed trigger-pool declaration and both
  actual staged-commit/demotion tests pass. Rejections surface diagnostics.
- Shared spawn-posture/suspend changes regress cleanly: sim `respawn_` 16,
  App `respawn_` 7. The new geometry starts crouched before a first command and
  respawn still restores standing/ground coherently. Every filter matched.
- Task 6 is complete. Source audit of the Ordering ledger identified remaining
  Task 7 proof strengthening: O5 actual continuation tick; O14 profile-only
  prediction correction counts; O24 real backlog trim -> replay -> UI clock;
  O38 look sampling through actual receipt -> host facing agreement; O39 actual
  reload while a wait is parked. O1/O2, AC3/AC15/AC20 and docs remain Task 7.
  No full workspace suite or final preflight has run.

## Task 7 verified checkpoint (2026-10-30)

- Owner resumed after two pauses and reports this is the only active agent on
  the laptop. The per-build rustc/prl-build guard remains in force. All worker
  edits are integrated; workers ran no builds.
- Dev player declares `alive` and the requested `down` profile with no rejoin
  override. TS/Luau death-policy twins register named down/respawn decisions,
  local vignette/clear crossings and the down-only elapsed HUD bar. A zero-
  strength 1 ms vignette validly clears the effect.
- The co-op room has red subject-only down and teammate-operated blue group
  recovery pads, plus three placements for host and two clients. Its TS/Luau
  setup twins and fixture counts include all five pad bindings. Author docs
  cover the zero-HP rule, capabilities, reads, verbs, rejoin and three recipes.
- Focused PASS: sim `life_policy_` 3; App `player_death_` 4; App `life_state`
  27; netcode `life_state` 23; strengthened O16/O35 local/remote movement and
  stage capture 1; actual co-op setup twin CPU installation 1; real compiled
  map three-placement/one-pawn spawn 1. Every filter matched. Fixture-only
  compile mistakes were repaired before these passes. O16 now proves T moves
  under the old profile and T+1 follows speedScale 0, paired against scale 1.
- New proof closes O1/O2/O5/O14/O24/O38/O39 and AC3/AC15/AC20. The actual dev
  consumer runs simulate-tick -> PlayerEventTable for 900 ticks, then drains
  respawn next tick, preserving the other seat. The elapsed UI test drives real
  host trim, jumped ack pruning and replay into 240 Hz projection. A read-only
  pending-command getter is gated by `test-support`.
- `ordering-proof.md` names all O1–O44 outcomes and records executed results;
  the AC table records focused passes and explicit manual gaps. O26 represents
  lost/resend Control by deferred installation, with real replay/state gates.
- Format/whitespace and the touched-crate all-target readiness check pass
  after the last fixture updates. No review or full workspace suite/
  preflight has run. `review-scopes.md` is metadata only, awaiting readiness.
- Runnable dev-tools engine, `scripts-build`, `prl-build` and `postretro-tool`
  are built. Dev start JS and `coop-presentation-test.prl` are freshly compiled.
  `manual-playtest.md` gives the owner/observer runbook. No visual pass is
  inferred; AC17/AC18 still block landing. No feature push/PR/landing occurred.
- Logs: `/tmp/postretro-e18-task7-{fmt,check,policy,consumer,app-life,net-life,
  o16,coop-twins,map-spawn}.log`; playtest tool/map logs use
  `/tmp/postretro-e18-playtest-{tools,map}.log`. Task 7 was committed in 3c2b915b1.
  Most recent free space about 15 GiB; check before the final gate and follow
  the cache-only cleanup rule if it falls below 15 GB.

## Performance and resource constraints

- Per-tick gates use component stamps through one helper; no VM, descriptor
  lookup, string allocation or table rebuild per capability query.
- Elapsed increments once per simulated host tick, including remote pawns
  without commands; replay must not count a command twice.
- Name-table rebuild/remap is load/reload work; client table history retains
  four recent revisions plus its protected pawn revision, per the brief.
- UI projection uses fixed-size clock/witness state at the existing UI snapshot
  boundary; it writes no presentation values into the gameplay store.
- Respawn work is request-driven and bounded by live seats, eligible map
  placements and pawn overlap checks. Requests coalesce per pawn; level unload
  clears them. Pick against earlier relocations as AC13 requires.
- Builds use this worktree's own `target/`, `CARGO_INCREMENTAL=0`, the default
  dev profile and dev-tools where relevant. Free space at this checkpoint is
  about 15 GiB at Task 7; check again before any build and after each numbered step/task.

## Verification and landing

Focused tests accompany each task; confirm filters run nonzero tests. Before
review, run fmt, checks of touched crates and focused seam tests. Then run the
review-panel → fix-review-findings → focused-retest loop, followed by preflight
once as the final workspace gate. Do not run the full suite during planning or
before integration/review.

AC17/AC18 include external visual proof. Preserve those as outstanding until
observed; the brief does not explicitly permit landing first. Prepare a
150 ms RTT two-client runbook on `coop-presentation-test` for mid-sprint down,
crawl/fire, bleed-out/respawn, eye easing and timer progression. After automated
proof, report any manual gaps as test-ready and await the result and the
owner's separate "land the plane" instruction.
