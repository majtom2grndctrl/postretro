# Weapon activations

Status: draft; public API migration requires owner sign-off.
Read at: `1527f5b26`.

## Goal

Give weapons primary and secondary actions with shared resource and ownership rules. Authors compose shot timing and charge scaling as data. Reference content demonstrates a charged plasma shot and a three-shot rifle alternate fire.

## Scope

- Primary and secondary input, authored shot/wait sequences, and charge on either action.
- Host-authorized resource use and shot statistics; responsive client prediction for co-op PvE.
- TypeScript and Luau authoring, validation, generated types, reference content, HUD feedback, and SDK documentation.
- Fixed-tick correctness, conditioned-network tests, performance measurements, and independent review.

Non-goals: tracked deployables and remote detonation; dual-wield; attachments; new resolution modes; PvP validation; rewind; arbitrary runtime script callbacks; a general gameplay scheduler.

## Decisions

### Authoring

Weapon defaults retain damage, range, projectile tuning, spread, placement, and one resource. Replace weapon-level `fireMode` and `fireRateMs` with required `primary.trigger` and `primary.recoveryMs`. Add optional `secondary`. Each block owns its trigger, recovery duration, optional charge tuning, shot/wait steps, and optional presentation overrides. Migrate all repository content and fixtures together; no legacy fallback parser.

Optional `sounds: { fire?, impact? }` overrides those weapon sound defaults for this action. Optional `emits: { activate?, impact? }` adds named reaction dispatch alongside the existing built-in event; omitted names add nothing. Each shot publishes activate once; impact retains existing per-shot/per-tick contact aggregation. Reject empty aliases and aliases equal to their built-in address to prevent duplicate dispatch. Carry action provenance with projectile/contact presentation, never infer it from the weapon's current execution. Shared reload/dry-fire/overheat sounds stay weapon-owned.

Triggers are `press` and `hold`. Press starts one execution per fresh edge. Hold starts another execution after the previous execution and recovery finish while input remains held. Charge requires `press`: its press starts charging and its explicit release starts execution. Holding at full charge does not auto-fire.

The SDK exports an `activation` namespace. `activation.shot()` and `activation.wait(ms)` build closed tagged data. There is no burst mode, burst count, burst opcode, or runtime repetition. Ordinary author-time language composition can assemble longer sequences.

```ts
// Proposed design: rifle secondary, three ordinary shots 80 ms apart.
secondary: {
  trigger: "press",
  recoveryMs: 300,
  steps: [
    activation.shot(), activation.wait(80),
    activation.shot(), activation.wait(80),
    activation.shot(),
  ],
}

// Proposed design: plasma secondary, 1–10× damage from 0–1 charge.
secondary: {
  trigger: "press",
  recoveryMs: 400,
  charge: { minMs: 200, fullMs: 1000 },
  steps: [activation.shot({
    scale: {
      damage: activation.charge.times(9).plus(1),
      projectileSize: activation.charge.plus(1),
      resourceCost: activation.charge.times(9).plus(1),
    },
  })],
}
```

Scale values accept finite literals or existing numeric IR. The activation scope exposes only normalized `charge`; uncharged actions read 1. Bind expressions when content installs. No state-store reads, writes, randomness, or retained closures. Evaluate once per resolved shot with no allocation. Supported scale keys are `damage`, `range`, `projectileSpeed`, `projectileRadius`, `projectileSize`, `knockbackSpeed`, and `resourceCost`. Unlisted values keep scale 1. Damage scale multiplies direct or splash damage once; visual size never changes collision or splash radius.

Limits: at most 64 steps and 16 shots; first and last steps must be shots; shots must have a positive wait between them. Durations must be finite, in (0, 60000] ms; recovery may be zero. Total quantized waits may not exceed 60 seconds. Charge permits minimum zero and requires minimum ≤ full duration. Waits round up to whole fixed ticks, with a minimum of one tick. Damage/knockback scales are in [0, 64]; other scales in (0, 64]. Invalid literals or structure reject content. Validate the final scaled values, including overflow and integer ammo conversion, before debit. Invalid evaluated values cancel execution and warn once per descriptor/field.

### Execution

One execution owns the weapon. Charging and executing join its central state machine. Primary and secondary share recovery and resource storage. Secondary wins simultaneous fresh starts; neither interrupts an existing execution. A blocked press is consumed, not queued. A hold action may start once idle. Each accepted shot sets recovery to that action's recovery duration; subsequent steps in the same execution bypass this start gate. Completion or cancellation preserves the remaining recovery from the last accepted shot. Every shot samples current aim and freezes resolved combat and presentation tuning.

Charge measures bounded start-to-release input time, clamps to 1 at full duration, and freezes on release. Release before minimum cancels without firing or spending. Other releases fire the scaled shot. Switch, drop, death, input suspension, disconnect, level change, and descriptor replacement cancel charge and remaining steps. A fresh reload press cancels execution before the existing reload gate; an impossible reload leaves the weapon idle. Held reload does not repeatedly cancel. A normal sequence continues after button release.

Spend resource per shot, never for waits or cancelled charge. Shared gates enforce magazine availability, heat lockout, and cell affordability. Positive scaled ammo cost rounds up to an integer; heat and cell retain fractional cost. Insufficient ammo/cell ends execution with one dry-fire cue and applies action recovery, preserving held-empty cadence. Heat lockout ends it silently. Heat/cell passive updates retain existing authority and timing. The threshold-crossing heat shot fires, then cancels remaining steps. Cancellation never refunds previous shots or clears recovery already owed.

### Network

Host owns activation authorization, accepted charge, resource debit, and shot damage. Client predicts execution and feedback from the same bounded transition logic. Client hit declarations cannot choose activation statistics. Authorized shots and projectiles retain their resolved statistics through later switching or descriptor changes.

Separate activation identity from execution time. A shot key contains pawn network identity, initiating client tick, primary/secondary lane, and authored shot ordinal. Bind the accepted activation to the live weapon instance. Ordinals name authored attempts, including rejected attempts. Keep host fire tick separately. Widen the current shot key; never derive later shot identity from the current movement-command cursor.

Every execution, including a hold-trigger restart, has a client-named initiation request attached to a real input command. The request names that command's client tick and the action lane. The host accepts or rejects that request; it never independently invents remote hold restarts. Local and AI controllers generate the same request form from their own logical ticks. Client-held input may request another execution after predicted recovery; a refused request cancels only that execution and applies its correlated cooldown correction.

Both input channels carry explicit press, held, and release facts. Release and cancel name their initiating activation. Synthetic held/neutral commands contain no activation edges. Preserve release/cancel before stale-drop or backlog trimming, deduplicate them, and deliver them once. An edge received before its initiation is retained but has no effect until that start is admitted; otherwise it expires. Terminal-activation edges are inert. New starts require an admitted real command; a start discarded by existing playout does not fire later. Cancellation wins over release when both target the same execution in one command.

Focus loss, menu capture, and other gameplay-input suspension latch an explicit cancel, clear gameplay press/release latches, and transmit cancellation through the reliable input path even when no simulation tick runs in that render frame. Sending intent does not advance execution. The host applies it on the next simulation tick; it does not interpret suppression as a release or wait for the outage timeout.

Committed waits advance once per simulation tick, independently of movement-command cursor holds or jumps. An accepted sequence may finish across ordinary input gaps. Each shot still checks resources separately. Limit each execution to one shot per simulation tick. Cancel pending work after two seconds of host simulation time without an admitted real command; short held/neutral gaps alone do not cancel it. Independently, charge expires without firing 60 seconds after its full-charge duration.

Charge duration uses wrap-safe release tick minus accepted start tick, capped by host elapsed time since acceptance plus 150 ms, then by full duration. No client supplies a multiplier. Missing intermediate samples do not weaken an otherwise valid hold; late release arrival cannot increase duration beyond its authored input timestamps. A backlog trim that compresses host elapsed time beyond the tolerance clamps charge. Return resolved charge with acceptance so local presentation corrects that case. Tick spans outside the bounded activation lifetime reject rather than wrap into a valid duration.

Pending declarations track accepted activation ordinals until authorized, rejected, cancelled, or expired. An early declaration for a future shot waits; movement cursor progress is not evidence of rejection. Keep at most 64 pending declarations, 64 retained release/cancel edges, and 64 terminal activation records per client. Unknown initiations and terminal records expire after two seconds from first receipt/termination; duplicates never refresh expiry. Accepted future ordinals expire at their scheduled decision plus two seconds; projectile declarations retain existing shot travel/contact validation and expiry once authorized. Declaration overflow rejects the newest declaration. Edge overflow cancels the affected active execution and rejects the newest edge. Terminal overflow evicts the oldest record; a monotonic settled-initiation watermark and reliable terminal outcomes prevent replay after eviction. Watermarks may settle unknown starts, never live future ordinals. Cancellation rejects unissued ordinals; prior authorized shots keep their normal lifetime.

Client catch-up resolves every due shot, including multiple logical shots within one rendered frame. Use that frame's rendered aim/target pose with each shot's immutable statistics and ordered spread/bloom state. Do not substitute empty declarations for scheduled shots. Projected resources choose feedback only: stale ammo/cell/heat cannot stop semantic attempts or declarations. A predicted projectile still simulates contact when its cosmetics are suppressed by that projection. Only authoritative denial terminates the resource-dependent predicted sequence. Render-only frames do not advance charge or consume sequence steps.

Host reports initiation accept/reject, execution accept with resolved charge at release, cancellation, and per-shot verdicts. Send execution acceptance before waiting for any hit declaration. Its payload names the activation and bound instance plus resolved charge and authoritative recovery. Clients use the matching installed tuning to update future shot snapshots and any still-live predicted projectile's size, speed, radius, and remaining travel budget; never respawn it, replay damage, rewind its transform, or resurrect an already-contacted projectile. An ordinary host/client launch/contact discrepancy remains the accepted no-rewind tradeoff. Correlate every correction with activation identity and the bound weapon instance; an older result cannot rewind a newer execution. Terminal cancellation stops future predicted shots. No full weapon rollback or predicted resources.

### Reference content and feedback

Preserve `reference_plasma_bolt` and `reference_rifle` canonical names. Plasma primary remains its existing automatic bolt. Secondary charges for 1000 ms, permits release after 200 ms, and reaches 100 damage, 50 cell cost, and twice the visual size at full charge. Collision, range, speed, lights, and splash stay unchanged. Cell regeneration retains its existing rule, including while charging. Charge level scales linearly from zero elapsed time.

Rifle primary keeps its automatic 110 ms cadence. Secondary fires three 9-damage shots, 80 ms apart after tick quantization, consuming one round per shot, with 300 ms recovery after the last shot. Holding secondary does not repeat it. Both weapons use the existing alternate-fire binding.

Expose local HUD facts for charging and normalized progress, attributed to the active weapon. Show a charge indicator in the dev HUD and clear it on release/cancellation/switch. Per-activation event routing preserves projectile provenance so delayed impacts use the originating action's event. Reuse existing sound/VFX assets; do not generate new assets.

## Acceptance criteria

1. Both reference primary actions keep their existing cadence, damage, resource cost, and presentation.
2. A full plasma secondary deals 100 damage before target modifiers, spends 50 cell units once, and emits one projectile with twice the primary's visual size. Partial charge follows the declared formula; early release cancels.
3. A rifle secondary produces exactly three timed shots and spends three rounds when available. Releasing does not interrupt it; holding does not repeat it. Each shot uses its current aim and normal spread/bloom.
4. Identical charge/sequence declarations work with ammo, heat, cell, or no resource. Exhaustion, overheat, reload, simultaneous inputs, interruption, and lifecycle cleanup obey Decisions without delayed ghost shots.
5. TypeScript and Luau compile equivalent descriptors. Malformed steps, limits, scopes, and non-finite values fail at the documented boundary. Generated types match Rust. Review confirms there is no gameplay burst verb or live script callback.
6. Host and connected-client tests prove charge/sequence timing and shot identities through clean links, 45–105 ms one-way delay, 5% loss, duplicate/stale input, frontier holds, backlog trimming, lost release, early declarations, and tick wrap. No repeated spending, synthesized release, or unauthorized damage. Ordinary loss of intermediate held samples preserves charge; severe trim follows the stated clamp. Cover hold restarts after rejection, release before start admission, press/release before a fixed tick, stale resource projections, and queue overflow/expiry. Discarded starts do not create deferred activations.
7. One-, zero-, and multi-tick rendered frames preserve due shots, charge progress, and ordered bloom. Shot damage and presentation remain tied to the originating activation after switching or descriptor replacement. Authoritative charge correction updates a still-live predicted projectile without a duplicate launch, retroactive contact, or change to a newer activation.
8. HUD charge feedback responds locally, reaches full, and clears on all cancellation paths. Local owner, host, and another client receive the intended fire/impact presentation without duplicate owner cues.
9. Activation advancement allocates nothing after installation and executes bounded work. Compare baseline and changed builds on the same release fixture, machine, tick count, cache mode, and player count. Record p50/p95 simulation time, serialized input bytes, and allocation evidence. Investigate regressions above 5% or measurement noise before acceptance; report GPU/frame measurements separately.
10. Focused tests, independent code review, final preflight, and a running-engine reference weapon check pass. Record manual visual/audio proof separately from headless tests; never infer a pass.

## Tasks

1. **Network timing slice.** Establish minimal production activation/shot records and the shared pure transition kernel from Decisions. Prove a fixed fixture program through the real command queue and client catch-up path. Task 2 adds full authoring and Task 3 extends this same kernel; no throwaway or parallel scheduler. Own client-named initiation, explicit release/cancel capture, wire conversion, edge retention, terminal activation outcomes, and ordinal-aware pending declarations. Bound every retained record and expire from first receipt, never refreshed by duplicates. Migrate identity consumers in hit ingestion, predicted projectiles, presentation, and cleanup. AC 4, 6, 7.
2. **Descriptor and SDK contract.** Extract weapon descriptor validation before extension. Add activation data and a read-only numeric IR scope; integrate JS/Luau ingestion, generated types, host-resolved tuning, and content migration. Extract SDK builders into their own module. Bind and retain programs at install; descriptor replacement cancels live execution before replacing compiled data. AC 1–5, 9.
3. **Shared weapon execution.** Integrate charging/executing into the central machine and shared fire gate. Feed immutable resolved shot data into local, remote-authoritative, and predicted paths. Split client resolution and local/remote command consumers before extending. Preserve enemy primary fire through explicit activation input without actor-kind branches in weapon semantics. Test resources and every cancellation source. AC 1–4, 6, 7, 9.
4. **Prediction and presentation.** Extract binary client weapon orchestration before extension. Replace first-shot-only catch-up with bounded per-shot resolution. Carry action provenance into fire, projectile, and impact events; publish charge HUD facts through existing local state plumbing. Integrate reference content and document authoring. AC 1–3, 6–8.
5. **Verification.** Run semantic, cross-boundary, and conditioned-network tests; performance comparison; running-engine checks; review/fix loop; final preflight. Record evidence by acceptance row. AC 1–10.

## Sequencing

Task 1 first: settles the highest-risk cross-system behavior. Task 2 establishes shared data contracts. Task 3 consumes them. Task 4 consumes shared execution and shot records. Task 5 closes integration. Delegate independent SDK/content work only after shared contracts are stable; integrating executor owns Cargo runs and shared seams.

## Boundary inventory

All new names below are proposed. Tagged step encoding uses `kind: "shot" | "wait"`; wait duration is `durationMs`. Rust uses snake_case, serde/TS/Luau use camelCase. Activation keys are `primary` and `secondary`; charge fields are `minMs` and `fullMs`. No FGD tuning is added.

Proposed shot-key field order: pawn `u32`, initiating tick `u32`, lane `u8` (primary 0, secondary 1), ordinal `u8` (0–15). All identity-bearing bitcode records use that key. Input retains existing field order, extending with secondary button state, optional initiation, and activation-correlated release/cancel data. Reject unknown lane tags and out-of-program ordinals. Use existing bitcode encoding, not manual endian/length fields. No PRL section is added.

Binary input/shot-key changes bump the wire epoch. Terminal activation outcomes extend application vocabulary and bump its independent epoch. Tuning additions bump the JSON tuning epoch. Migrate every constructor, conversion, fixture, SDK type, and version rejection test in the same change.

## Invariants

| Invariant | Established by | Preserved at | Proof |
|---|---|---|---|
| One execution; each authored shot ordinal settles once | 1, 3 | Input gaps, client catch-up, simultaneous starts | 4, 6, 7 |
| No release inferred from missing input | 1 | Input latches, queue holds, neutralization | 6 |
| Every hit uses frozen authorized shot statistics | 2, 3 | Projectile contact, hit declaration, hot reload | 2, 6, 7 |
| Resource debit occurs once per accepted shot | 3 | Partial sequence, duplicate input, prediction | 3, 4, 6 |
| No gameplay VM or per-tick program allocation | 2, 3 | Prediction, install, descriptor replacement | 5, 9 |
| Cancellation leaves no deferred weapon work | 3, 4 | Reload, switch, death, disconnect, unload | 4, 8 |

## Owner decision

Approve the proposed public scripting migration and network contract before promotion. Gameplay timings and costs above are initial reference tuning; architecture and acceptance are the feature contract.
