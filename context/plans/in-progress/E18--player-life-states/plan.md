# Player life states — plan of record

mode: resumable
status: proposed
read at: a953f386f

## Resume point

The brief was claimed from current `origin/main` (4b59b3b93), moved to
`in-progress/`, and pushed to `main` in a953f386f. Feature branch:
`codex/player-life-states`. No implementation has started and no tests have run.

The legacy brief has no compact/resumable header. Use resumable coordination
because its seven tasks cross descriptor validation, prediction, replication,
simulation ordering, respawn, and seat lifecycle contracts. This changes no
Acceptance or Ordering requirement.

Source verification is continuing with two read-only audit workers. The owner
resolved the acceptance conflict on 2026-10-27 as recorded below. The amended
timer requirements are feasible; no acceptance row currently needs restatement.
Finish the cited-source audit and present this resumable plan for owner review
before implementation. Proposed is not implementation approval.

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

- The brief predates workflow mode headers and has no `read at` SHA. Resumable
  source verification must re-read the cited symbols; continuity is not proof.
- Task 5's Path excludes pawns with later respawn requests from overlap tests.
  AC13 instead says another live pawn blocks a placement and that earlier
  relocations affect later picks. Follow AC13: test all other live pawns at
  their current positions, including pending later requests, and update the
  Path wording with the final plan. This adjusts the approach without
  changing AC13 or O18.
- Task 4's frame-final owner-slot read cannot witness a reset followed by
  another accepted snapshot in the same frame. Retain a local reset witness
  while applying accepted owner samples, and expose it to the display clock.
  A kept-name table remap changes no logical state identity and must preserve
  display continuity. No new wire field is needed.

## Delegated answers and source findings

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

These findings are planning evidence, not implemented fixes. Complete the
remaining cited-source audit before the owner reviews the plan.

## AC-to-proof

Test descriptions below identify intended scenarios, not tests already added
or executed. All rows remain unproved. No manual result is inferred.

| AC | Proof | Status |
|---|---|---|
| 1 | Shared validator plus QuickJS/Luau descriptor and startup/staged-manifest parity cases for every rejection, valid defaults, rejoin, and the 255-name limit | automated, planned |
| 2 | Undeclared zero-HP regressions across fire cancellation, vault/teleport, occupancy/damage eligibility, and ungated movement/look/switch/reload/use/touch/pickups | automated, planned |
| 3 | Spawn/join/restart and level-change initial state plus elapsed-zero assertions before the first tick; O15 pins first condition reads | automated, planned |
| 4 | Capability-by-capability refuse/permit pairs including zero HP; production host/local/client weapon paths; movement replay and owner look sampling; held/burst/charge and vault interruption | automated, planned |
| 5 | Forced-crouch entry/hold/exit tests with abilities both enabled and disabled, including slide/dash/jump precedence and blocked stand clearance | automated, planned |
| 6 | Trigger membership retained without spurious Exit, pickup and teleporter refusal/permit at zero HP, occupancy counts, O35/O40 | automated, planned |
| 7 | In-tick command execution, same-state no-op, exact 900-tick threshold, and all O15/O39 first-read values | automated, planned |
| 8 | Install and bind diagnostics naming reaction/state; both verbs; condition and crossing binding; runtime missing-state/no-state warnings | automated, planned |
| 9 | Two-pawn player-event predicates and own-pawn local crossings in both runtimes | automated, planned |
| 10 | Local String and owner-private Number publication/replication through the UI frozen map and visibleWhen resolver; undeclared empty name | automated, planned |
| 11 | Production tick-start respawn drain: all velocity/ability resets, settled ground, yaw, health/latch, state elapsed, cancelled weapons/vault, retained inventory | automated, planned |
| 12 | Owner teleport-class reconcile and observer cut with NO_TELEPORTER suppressing stale arrival sound, including O28 coalescing | automated, planned |
| 13 | Seat/map-order overlap selection, sequential batches, no-free warning, malformed health clamp, non-player skip; AC13 governs later queued pawns | automated, planned |
| 14 | Seat lifecycle scenarios for disconnect/hold expiry, suspend/demotion, reload removal and rejoin map, held seats and level-change clearing | automated, planned |
| 15 | Late-join FullBaseline round trip carries teammate state/elapsed/revision/crouched capsule; host predicate stays true during join | automated, planned |
| 16 | Authority reload remap for kept/removed/new/undeclared states and changed profiles, client no-remap, demotion/rejoin seam | automated, planned |
| 17 | 75 ms each-way conditioned reconcile harness, oversized transition, eye-height envelope versus crouch input; two-client dev-mod visual playtest | automated plus manual-visual, planned |
| 18 | Deterministic frame-clock tests: strict positive-frame advance through trims, delay/loss and reordered snapshots, bounded-rate convergence, zero-duration frames and reset witnesses; owning-client playtest | automated plus manual-visual, planned; owner wording resolved |
| 19 | One named outcome test for every O1–O44 row, including amended O24/O25, linked in an ordering coverage ledger before final review | automated, planned |
| 20 | TS/Luau consumer parity and a production two-pawn simulate-tick → player-event test through 900 ticks and next-tick respawn | automated, planned |
| 21 | Review scripting-reference and teleporter verb tables/recipes against both SDKs and bundle command availability | manual-doc review plus SDK surface checks, planned |

## Tasks

The integrating executor owns all shared contracts, `plan.md`, integration,
Cargo execution, commits, and final proof. Proposed workers receive the full
brief/research, Acceptance and Ordering rows, routed context, file ownership,
and upstream/downstream contracts. They do not commit or edit this plan.

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | Thin slice: shared descriptor validation → global table/profile stamps → spawn/state verb/predicate → tuning/wire history → production fire gates | integrating executor | source audit, plan approval | not started |
| 2 | Movement value/ability/posture gates, teleport touch gate, owner look sampling, forced-smoothed reconcile and presented eye ease | movement worker; shared stamps/reconcile integration owned by executor | 1 | not started |
| 3 | Switch/reload/use/touch/occupancy/damage gates; single Triggers-stage capability capture and player_life helpers | simulation worker | 1 | not started |
| 4 | Number/String reads and SDK mirrors, continuously advancing HUD clock with rate correction, local predicates and authoritative reload remap | integrating executor | 1 | not started |
| 5 | Respawn SDK/binding/queue, published placements, shared teleport-arrival tail, tick-start seat-order relocation and yaw propagation | integrating executor | 2, 3, 4 | not started |
| 6 | Carried state names, disconnect/demotion/suspend harvest, rejoin restore and level-unload clearing | integrating executor | 5 | not started |
| 7 | Dev down/bleed-out TS/Luau twins and HUD, baseline/lifecycle coverage, complete Ordering ledger, bundle-facing docs and manual runbook | integrating executor | 6 | not started |

First proof is Task 1's real descriptor/verb/condition/wire/fire slice: it
tests the shared substrate every subsequent task consumes. Tasks 2–4 run
concurrently only after Task 1 passes; the executor coordinates their small
shared-file edits. Each completed resumable task commits its proof and plan
update together.

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
  about 44 GiB; check again before any build and after each numbered step/task.

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
