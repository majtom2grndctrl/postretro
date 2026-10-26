# coop--host-switch-timing — plan of record

mode: compact
status: test-ready
read at: a363ea550 (source identical to b88a31f91)

## Execution checkpoint

- Implementation and review fixes complete; root inspected the integrated diff. Focused gate47/42 passes. Final preflight passes: formatting, workspace lint and full tests12,927passed/0failed/68ignored across71 target result groups, including doc tests.
- Independent panel complete: four slices and seam pass. Two runtime defects, four proof gaps and one stale comment addressed; resolution ledger in review.md. No fresh full panel needed absent a new concrete finding.
- AC31 manual loopback remains external proof and blocks landing. Runbook: manual-test.md. No manual engine session, dev-map authoring bake, PR or landing request.
- Owner explicitly requested pushing the feature branch to origin for flexible testing before manual proof. This publication is authorized; status remains test-ready and AC31 still blocks landing.
- Owner requires pausing before a new step when either five-hour or weekly allowance remaining is5%or below. Query live usage at workflow boundaries. Never spend reset credits. Prior windows paused at cutoff; owner resumed each time.
- Branch codex/coop-host-switch-timing. Claim a363ea550 is on main; implementation cf96a7503, acceptance05dd08f39; subsequent checkpoint commits record review progress. Original checkout's unrelated work untouched.
- Isolated worktree target only, default dev profile and dev-tools, CARGO_INCREMENTAL=0. Last free space64GiB. No cache sharing or original-checkout branch switch.

## Corrections

- Requested slug `coop--switch-host-timing` resolves to promoted brief `coop--host-switch-timing`.
- Shared release prerequisite landed in `2efd7755b`. Re-read `release_wieldable`, `normalize_inventory_liveness` and `finish_lowering`: active release follows the recorded target; target-only removal returns to the origin; released lowering/raising instances become idle.
- `coop--lossy-link-delivery` landed in `3ccf0c3b7` and later review fixes. Declarations and outcomes now ride framed Input; the lane still orders application against retained commands. The harness must deliberately isolate/hold a declaration for the late-delivery and forced-bound scenarios; ordinary production Input remains reliable and ordered. Control-flow wording in the older brief describes the adversarial harness scenario, not a transport change.
- Current `ClientSwitchDeclaration` lives in `crates/net/src/wire.rs`, not `wire/control.rs`; the pending chain and `resolve_switch_outcome` moved from netcode `lib.rs` to `client_switch.rs`. Update these current owners and their callers.
- Current wire version is 34 (ghost-link changes); this field addition advances it to 35. Application protocol PRLC, snapshot version 22 and tuning epoch remain unchanged.
- The local lower still uses the shared state transition; expired Lowering stays Lowering until explicitly finished. Remote machine ticks cannot themselves commit a repoint. Preserve and comment this invariant.
- `WeaponSlotProjection::for_pawn` and observer `active_weapon_archetype` still derive selection from `Inventory.active_slot`; the begin/repoint split naturally keeps their outgoing selection until repoint. Seat carry harvests `active_slot` and ignores switch metadata, so retiring a lower at level teardown carries the outgoing slot, not an unfinished target.
- Acceptance 22's reload is the first tick after the client's raise completes, consistent with Decisions and P39; it is not a reload during the raise. No contract change.

## Source verification

Compared source since brief read `08eb5c98` with current main, then reopened affected cited symbols and boundary consumers: host switch intake/application; switch lane and command/press resolution; local/remote weapon commands, machine, reload timer and state transitions; release and inventory liveness; client declaration/ordered settlement and resync call sites; owner projection and seat harvest; conditioned fixture send/receive and inventory-resend seam. At the claim baseline the host still repointed/replied immediately and cancelled activation without abandoning a reload. The outgoing lower, deferred reply and client-tick raise gate remain necessary. No false Decision premise found.

## Delegated answers

- Build on landed lossy-link Input declarations/outcomes and wire34; this change advances wire35 while PRLC, snapshot22 and tuning gates stay unchanged.
- Integrating executor owns shared contracts, integration, plan, commits and final disposition. Bounded implementation workers and independent review lenses followed build-brief/model guidance. Edit workers ran no Cargo; one runner serialized focused checks and final preflight.

## AC-to-proof

Numbers follow index.md checkbox order. Conditioned profile is45ms one-way delay, up to60ms additive jitter and5%loss unless stated otherwise. Owner settlement and tuning installation use production paths; observer claims use actual second-peer snapshot receive. Focused proofs and final preflight pass. Manual proof remains external and outstanding.

| AC | Acceptance | Proof | Status | Result |
|---|---|---|---|---|
| 1 | A drop stamped on any tick from the begin through the tick before the repoint releases the outgoing weapon, and a reload stamped on those ticks starts nothing. A drop stamped on the repoint tick releases the incoming one. (P23) | Conditioned host/owner/observer harness: parameterized drop and reload at every lower tick, including begin and repoint. | conditioned every-lower-tick drop/reload including repoint passes | pass |
| 2 | Drop pressed during the lower: both peers then hold the switch target and agree on the active slot, with no switch in progress. The declaration settles accepted, and observers see one change. (P23) | First slice: conditioned mid-lower drop; production pending-chain settlement and inventory resend; one observer swap. | focused composed mid-lower drop passes | pass |
| 3 | The outgoing weapon, dropped mid-lower, lies in the world idle, not lowering. Whoever picks it up, the host player or a remote one, can fire it, and the host admits the start. (P23) | Conditioned release/reacquire scenarios for host and remote holders; idle world weapon and admitted start. | conditioned dropped-instance idle/reacquire/fire for both holders passes | pass |
| 4 | A re-target and a drop stamped on the same tick mid-lower: the first declaration is refused, the drop lands the re-target, and both peers hold it with no switch in progress. Observers see one change, never the first target. (P33) | Conditioned same-tick retarget/drop; ordered refusal then acceptance; replicated observer never sees superseded target. | focused same-tick retarget/drop passes | pass |
| 5 | Switching away from a non-blocking reload abandons it on the host: no completion, no reserve transfer after the begin, and switching back finds the weapon idle, not mid-reload. With `lowerMs` 0 too. | Conditioned non-blocking reload interruption at zero and long lower; magazine/reserve and idle-on-return assertions. | conditioned magazine-reload interruption at zero/long lower passes | pass |
| 6 | A reload that expires on the begin tick of a lower longer than one tick completes before the lower on the host, as in single-player, and the repoint still lands on the client's repoint tick. A reload pressed on a begin tick whose reload is not due starts nothing. (P25) | Conditioned due atomic reload on begin with multi-tick lower; completion delivery/transfer precede lower; new reload press refused. | clean and mandated-link due atomic reload completion/transfer at exact begin pass | pass |
| 7 | A reload that expires on the begin tick of a lower under one tick, or of `lowerMs` 0, is abandoned on the host: no completion and no reserve transfer. Switching back finds the weapon idle. (P25) | Conditioned due atomic reload on begin with zero and sub-tick lower; no completion or transfer; weapon idle on return. | clean and mandated-link zero/sub-tick due reload abandonment and idle return pass | pass |
| 8 | Switching away from a blocking reload is refused at begin. With a lower longer than the round trip, the client rolls back before its repoint and never shows the target. | Conditioned blocking-reload refusal with lower exceeding RTT; production rollback before client repoint. | conditioned blocking refusal and pre-repoint rollback pass | pass |
| 9 | With the trigger held through a switch, the client's first predicted shot after the raise is admitted on the host. A reload stamped on the tick the client's raise expires starts nothing, and one stamped on the next tick starts, as in single-player. This holds for a raise longer than one tick, one under one tick, and `raiseMs` 0, on a clean link and after a stall with catch-up trim. (P38, P39) | Client machine timing pin plus conditioned clean/trimmed runs for long, sub-tick and zero raises; admitted first predicted shot, refused expiry-tick reload, accepted following-tick reload. | clean and trimmed zero/sub-tick/long raise fire/reload boundaries pass | pass |
| 10 | The raise gate ends with the raise. After the raising weapon is dropped or removed, a reload on the weapon then held starts on the next tick. After a switch made during a long raise to a weapon with a short raise, a reload stamped after the new raise starts, even before the old raise would have ended. A resync starts no raise gate. | Conditioned raise retirement by expiry, drop, removal, replacement switch and resync; reload starts on surviving active weapon at its own boundary. | drop/removal/replacement short-raise and resync gate retirement pass | pass |
| 11 | With `lowerMs` 0 and `raiseMs` > 0, a reload stamped during the client's raise starts nothing on the host, and the first reload after it starts. | Conditioned zero lower/non-zero raise; stamped reload gate before and after expiry. | zero lower/nonzero raise stamped reload boundary passes | pass |
| 12 | A start bound to the outgoing weapon during the lower is refused on the host (regression guard: the client sends none). | Forged outgoing-weapon start through production host guard while lowering; explicit refusal. | conditioned forged outgoing start refused | pass |
| 13 | The host repoints on the client's repoint tick for a `lowerMs` shorter than one tick (begin and repoint share a tick), one ending exactly on a tick boundary, and one between boundaries. (P21) | Timer stepping pin against production client equip machine for zero/sub-tick/exact/between-boundary lower durations; conditioned tick trace. | conditioned actual owner/host lower-boundary comparison passes | pass |
| 14 | Observers see exactly one weapon change, on the repoint tick. Owner-private samples name the outgoing slot for the whole lower. | Second client snapshot apply and owner-private production state-slot receive; exactly one observer change at host repoint. | actual owner-private and observer snapshot source selections pass | pass |
| 15 | A→B then re-target to C mid-lower: one host repoint, to C, on the original repoint tick, and one observer change. The client settles both declarations holding C. With C refused at its begin, both declarations settle in order and both peers hold A, idle, with no switch in progress. (P22) | Conditioned accepted/refused retargets with production ordered outcome settlement; original repoint preserved or idle origin restored. | accepted and refused retarget chain settlement passes | pass |
| 16 | Three re-targets in one lower: the host refuses each superseded declaration in order and repoints once, to the last, on the original repoint tick. Re-selecting the origin or the target sends nothing. Under a catch-up trim that resolves past the repoint in one step, the same holds, with one observer change. (P29) | Conditioned three-retarget chains and no-send origin/target reselect; repeat with trim; one observer change. | origin/target no-send plus three retargets through actual trim pass | pass |
| 17 | A re-target made on the repoint tick repoints the host straight to it on that tick; the host never holds the first target. (P20) | Conditioned retarget exactly on repoint tick; no intermediate host or observer selection. | retarget on exact repoint tick passes | pass |
| 18 | A switch made during the raise starts a fresh lower on the host. | Conditioned switch during raise; fresh outgoing lower timed from new declaration. | switch during raise starts a fresh lower and new raise gate; focused pass | pass |
| 19 | Two switches inside one stall with no catch-up trim, the second made after the first's repoint: the host repoints to the first target on its tick, then begins the second. Both are accepted, and observers see two changes. Under a catch-up trim the host keeps that order, and observers may see only the second. (P19) | Conditioned two switches in one stall, with/without catch-up trim; ordered begins/repoints and accepted outcomes; observer trace. | two-switch stalled drain with and without actual catch-up trim passes | pass |
| 20 | A re-target that reaches the host after its repoint begins a fresh lower from the first target, repointing on the re-target's stamp plus the lower, at once if that tick has resolved. Both peers end on the re-target, with both declarations accepted. (P31) | Conditioned deliberately held declaration while Input advances; late retarget starts fresh lower from new active weapon. | exact130 for early delivery; overdue repoint equals actual lane-consumption resolution | pass |
| 21 | A target emptied before the repoint, by release or by removal, or released and refilled in one host tick, is refused. Both peers hold the origin, idle, with no switch in progress, and the client can fire it. (P24, P34) | Conditioned target release/despawn/retenant before repoint; production refusal/rollback and admitted origin fire. | actual target release/refill plus despawn/retenant refuse and origin fire passes | pass |
| 22 | After a stall with catch-up trim, the repoint still applies in client-tick order, and a reload stamped on the first tick after the client's raise starts on the host. | Conditioned catch-up trim ordering and first reload after raise completion; preserved press client tick. | long-lower trim orders repoint before preserved reload press tick132; focused pass | pass |
| 23 | A host-side lower that outlives its own timer by many host ticks (a stalled or silent client) stays lowering, refusing fire and reload, until the repoint. (P26) | Host lane plus conditioned silent-client lower beyond timer expiry; no host-time repoint; fire/reload refused until named ending. | 120 host-only ticks retain expired Lowering; delivered start/reload do not settle or reload; focused pass | pass |
| 24 | With a lower parked and Input stalled while Control flows, a later declaration the lane releases early, by its wait bound, its retained bound or a malformed lead, never gets its reply before the parked one's. The host first repoints to the parked target and settles that declaration accepted, then decides the released one; observers see the parked target first. Once Input resumes both peers agree with nothing pending. (P26, P32) | Lane tests for wait/retained/malformed forced release, plus conditioned isolated declaration delivery; parked acceptance precedes later reply and observer trace; resume converges. | wait, malformed and retained65/bound64 forced-release ordered outcomes and observer convergence pass | pass |
| 25 | Death mid-lower keeps the repoint on its tick, matching the client. (P30) | Conditioned owner death mid-lower; original client-tick repoint retained. | exact host selection change [(125,1)] despite death110 | pass |
| 26 | A level change, demotion or disconnect mid-lower: the host's active slot never moves at the old repoint tick, and after rejoin the client's first switch lowers and repoints on its own ticks with every declaration settled once. (P40) | Endpoint lifecycle scenarios for level reset, demotion and disconnect/rejoin; no retired repoint; fresh chain settles exactly once. | actual closure of slot7, fresh client9; all three boundaries lower271–305 and repoint exactly306, once | pass |
| 27 | A resync for a weapon with `lowerMs` > 0 repoints the host at once with no lower. No resync fires while a switch is in flight. (P27) | Production resync trigger plus conditioned non-zero-lower resync; immediate host repoint, no equip gate, no in-flight resend. | explicit immediate resync and real pending/Lowering/Raising detector no-send pass | pass |
| 28 | Both declaration kinds round-trip through the wire codec, and the wire version advances. | Codec round trips for both declaration kinds plus handshake version test; WIRE_VERSION 35 with unchanged PRLC/snapshot/tuning gates. | both kinds round-trip and wire-35 gates pass | pass |
| 29 | With `lowerMs` 0, the host repoints and replies in the declaration's own client-tick resolution, and observers see one change on that tick, under the conditioned link and through a stall with catch-up trim. | Conditioned zero lower with clean/stalled/trimmed stream; same-resolution reply, repoint and one observer swap. | zero-lower trimmed first resolution queues reply; transport-only drain proves no later resolution needed; focused pass | pass |
| 30 | A client that repointed before the host-side drop of its outgoing weapon reaches it keeps holding the incoming weapon. | Conditioned delayed production inventory resend after owner already repointed; released outgoing slot cannot change incoming active selection. | held production outgoing-drop tuning after owner repoint preserves incoming selection; focused pass | pass |
| 31 | Loopback recipe (`networking.md` §Manual loopback recipe) at 45/60/5, then 10% loss, with `lowerMs`/`raiseMs` authored on a dev weapon: switch, drop mid-lower, reload mid-lower and mid-raise, rapid re-targets. The observer sees one swap, at about the shooter's swap. | Owner manual loopback at 45/60/5 then 10% loss with authored non-zero equip durations and second observer; blocking external proof. | manual, outstanding; blocks landing | outstanding manual proof; blocks landing |

## Tasks

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | Build production owner/host/observer fixture; pin client lower/raise boundary ticks and drop-mid-lower risk. | integrating executor | — | done; composed proof passes |
| 2 | Share canonical lower transition and exact bounded timer stepping; defer due atomic reload once before lower. | integrating executor | 1 | done; sim/timer focused tests pass |
| 3 | Add Switch/Resync kinds, update producers/consumers, bump wire34→35 and retain other version gates. | integrating executor | 1 | done; wire codec/gates pass |
| 4 | Add parked lower, fixed repoint and instance identity, ordered retarget/refusal/endings, lifecycle retirement and post-sim repair. | integrating executor | 2,3 | done; expanded conditioned proof passes |
| 5 | Carry reload press stamp; gate reload/fire in client ticks, retire raise gate and preserve raw-level latch without manufacturing presses. | integrating executor | 4 | done; raise/trim and held-reload regression pass |
| 6 | Complete acceptance matrix and strengthen exact death/late/rejoin timing and actual reconnect proof. | integrating executor | 4,5 | done; composed47/47 and netcode42/42 |
| 7 | Readiness, independent review→fix→focused retest, final preflight, results and manual runbook. | integrating executor | 6 | automated work complete; final preflight passes; manual proof outstanding |

## Runtime and resource constraints

- One parked lower and one raise gate per remote pawn. Existing bounded starts/press/declaration lanes retained; no host-time expiry or unbounded event history.
- Duration derivation uses shared timer stepping at switch begin, with32 bounded exact spans plus boundary work; no duration-proportional loop or ceil replacement. Passed tick delta determines boundaries.
- Keep new logic in sibling modules; no GPU or unsafe changes. Test-only instrumentation records actual lane consumption and permits a fresh shooter connection.
- After each task check free space. Below15GB clear only active/workflow-owned incremental caches, recheck, then downloaded crate archives if needed. Never bare cargo clean or unrelated target deletion.

## Verification and landing

- Focused post-fix gate: formatting; touched postretro/netcode/sim/net compilation with postretro/dev-tools; composed switch47passed; netcode switch42passed. Both runtime regressions and all four proof repairs pass.
- Final formatting: cargo fmt --all --check passes.
- Final lint: CARGO_INCREMENTAL=0 cargo clippy --workspace --all-targets --features postretro/dev-tools --target-dir target/preflight-clippy -- -D warnings passes.
- Final tests: CARGO_INCREMENTAL=0 cargo test --workspace --features postretro/dev-tools passes. Aggregated71 target result groups, including doc tests:12,927passed,0failed,68ignored. Main engine2110passed/8ignored; entities301passed; net313passed; netcode941passed/3ignored; sim1602passed. Ignored heavy suites were not requested.
- Concrete preflight failures justified repair reruns within this final gate: const assertions moved to const blocks; timing assertions use observed reload/switch ticks; stale current-wire34 test expectations advanced to35 with additional34 rejection cases; ammo fixtures seed through grant_ammo; instance-replacement fixture fills through acquisition while preserving lower metadata. Ownership guards and their whitelists stay unchanged.
- Targeted ownership guard, retentant-target scenario and encrypted/admission protocol checks pass after those repairs.
- Final logs: /private/tmp/coop-switch-preflight-test-final-complete.log and /private/tmp/coop-switch-preflight-clippy-simfix.log. Earlier failed attempts retained alongside them. Last free disk58GiB.

| Gate | Result | Landing |
|---|---|---|
| Automated AC1–30, review fixes and final preflight | pass | clear |
| Manual AC31 at45/60/5 and45/60/10 | outstanding | blocks landing |

Manual runbook: [manual-test.md](manual-test.md). Retain this brief in in-progress with status test-ready. Wait for owner's manual result and “land the plane” before durable context updates, done move, cleanup or landing.

### Implementation corrections

- Applying a begin after command resolution would discover its repoint gate after starts/presses were already drained. The host now uses synchronous switch callbacks inside queue resolution, with per-pawn timing beside the lane, then post-simulation release/liveness settlement. This preserves Decisions and Path meaning; older simple queue test seams remain available and require migration/hygiene review before finalizing.
- Use the passed tick delta for derivation: production `TICK_DURATION` is 16,667 microseconds while the conditioned fixture uses `1/60` seconds. Hard-coding either would miss some large-duration boundaries on the other path.
- `lowerMs`/`raiseMs` are unbounded u32. Literal duration stepping could do 257 million passes at switch begin. Shared countdown derives exact binary spans for ordinary fixed ticks, skips only safe pre-expiry spans, then uses the canonical final step. Stack-only, bounded to 32 spans plus boundary work; tests compare literal stepping. No author duration cap or duration-division/ceil replacement.


### Existing-test clarification

The former same-switch-tick forged-start ordering test explicitly bypassed honest client prediction and expected a host shot on the repoint tick. The approved raise Decision now refuses that start even at raiseMs 0, because fire opens on the following client machine pass. Updated its expected verdict while retaining same-resolution switch application and loss ordering assertions. The adjacent legitimate next-tick start still passes. No owner Decision/Acceptance changed.
