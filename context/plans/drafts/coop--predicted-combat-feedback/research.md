# coop--predicted-combat-feedback — research

Read at 0d3b7dae5. Findings from the draft session; the executor re-verifies anything it builds on.

## Latency budget under the dev profile
45 ms + 0–60 ms jitter each way; one-way ≈ 45–105 ms. Estimates from the documented rules, not measured.

| Effect | Path | Delay after client contact |
|---|---|---|
| Self push | declaration up → splash hold → host push → snapshot down → reconcile replay | ~150–200 ms, then a ~1–1.5 m catch-up correction smoothed over ~10 ticks |
| Enemy push | as above, plus interpolation delay (~140–150 ms, about 9 ticks) | ~230–330 ms |
| Damage number | declaration up → hold → damage → Presentation down; shown on receipt | ~one round trip; appears before the enemy visibly reacts |

The interpolation delay is the larger of the time-sync delay (`rtt/2 + clamp(50 ms + 2×jitter + starvation, 50, 250)`) and the measured delay (lag peak + one snapshot interval). Under the profile, the measured delay wins at about 140 ms.

## Self push today
- Host: `ingest_hit_declaration` → `resolve_authorized_splash_projectile_impact` → `emit_splash_damage`. That computes `knockback_impulse(speed falloff, upward_bias, volume centre − blast centre)` and scales the shooter's push by `push.self_scale`. `apply_hit_knockback` → `PlayerMovementComponent::add_knockback` multiplies by the movement descriptor's `knockback.scale`, adds to `velocity` and `knockback_velocity`, and ungrounds on an upward impulse.
- Timing: `drain_ready` holds until `current_tick − fire_tick ≥ projectile_hold_ticks` (travel ceil + `SPLASH_HOLD_MARGIN_TICKS`). Ingest runs after `run_host_movement_tick`. At steady playout, the push lands after client tick ≈ T+1 is simulated and first moves the pawn at T+2. Agent inference, medium-high confidence.
- Client: `advance_predicted` emits impact effects, sound and the declaration, and marks the hitmarker. It applies no push, and no client `add_knockback` caller exists outside tests.
- Tuning parity: `TuningPayload` carries `movement.knockback` and each wieldable's `splash`, `knockback` and `projectile`.
- Reconcile: `WirePlayerMovementState` carries `knockback_velocity` and `push_velocity`. Merge, `prune_through_ack` and replay carry the protected velocity forward. Corrections classify as Ordinary ≤ 0.5 m, Oversized < 3 m, Teleport ≥ 3 m. A composed offset ≥ 3 m snaps.
- Precedents for client-predicted pushes: mover carry and launch, push-volume drift, teleporters. All are functions of replicated state recomputed by replay. None is a one-off event, so none needs dedupe.

## Enemy push today
- `apply_hit_knockback` routes to `AgentComponent::add_knockback` when the target has no player movement. `agent_steering::tick` decays it (`movement::knockback::decay`) and resolves it with `collide_and_slide`.
- Enemies replicate `Transform` and `MeshAnimationState` only. `TransformSample.intrinsic_velocity` is filled from player movement state only.
- Display copies carry no Agent, Brain or Health. They do carry `HitVolumeComponent`.
- Response tuning: `EntityTypeDescriptor.behavior.knockback`, copied to `AgentComponent.knockback` at spawn. The client resolves descriptors locally through `remote_entity_class`. Neither the tuning payload nor the mod digest carries them.

## Damage numbers today
- `apply_damage_with_context` → `ImpactDispatch` (amount after zone multiplier, health before/after, max, target, source, optional point) → `ImpactPolicyRuntime::evaluate_dispatch` → `apply_presentation_spawn`. The value comes from an authored expression. The anchor is the dispatch point, or the target transform for splash.
- `route_host_presentation_spawns` sends one unreliable copy per participating client. `presenter` is dropped, and no shot id travels.
- `ingest_client_presentation_messages` spawns on receipt at the host's world anchor.
- The client loads impact policies (`replace_global_events`, `replace_level_events`) but never evaluates them. They are outside `mod_compatibility_digest` by design.
- Readable inputs: `@impact.amount/healthBefore/healthAfter/maxHealth`, `@state.*`, and store and owner slots. Zone, point and source are evaluator-only. A recording `BindingScope` can list an expression's inputs at load.
- Client-side facts: `HealthDescriptor.max` and `zone_multipliers` (local descriptor); `ClientOverlayFactState` health and shield fractions for damaged enemies only.
- Not verified: whether display copies carry the tags that policy selection reads.

## Prior commitments touched
| Commitment | Source | Effect here |
|---|---|---|
| Knockback is host-authoritative; clients never submit impulse | `networking.md` §Game-logic-owned apply invariant | Kept; client predicts its own movement only |
| Predicted projectiles presentation-only until authoritative movement | same | Diverges for the firing client's own push |
| Splash never applied client-side | `plans/done/E16--aoe-splash-damage` | Diverges for prediction; host still applies |
| Enemies are remote entities, never predicted | `plans/done/E10--networked-enemy-authority-baseline` | Kept, apart from a cosmetic push offset |
| Client-predicted own damage numbers deferred | `plans/done/E16--combat-presentation-substrate` | Built here |
| Damage numbers host-routed, unreliable, no history | `plans/done/coop--networked-by-default` Decision 13 | Kept for everyone except the firing client's own shots |
| Host chooses the anchor | `ui.md` presentation layer | Diverges for the firing client only |
| No rewind / lag compensation | `context/lib/index.md` §4 | Kept; host applies only on unsimulated ticks |
| No splash or impact-policy code reads client hit volumes | `plans/done/coop--networked-by-default` Decision 4, AC 6 | Diverges: client reads its hit volume to aim cosmetic predictions only |
| Client enemy copies carry no tags | `plans/done/coop--networked-by-default` Decision 7 | Kept; the host replicates a per-entity number marker instead |
| Combat feel matches across roles | `plans/done/client-combat-feel-parity-contract` | Kept: the jolt plays where a hit is simulated or predicted, so the host matches the firing client |
| The author decides what a hit shows | `plans/done/E16--combat-presentation-substrate` | Diverges for the jolt, as the built-in impact burst does: an engine floor, author-tunable via `hitJolt`, zero off |

Stale pointer found: `E16--aoe-splash-damage` names a "Damage & Defenses knockback spec" as owner. Knockback shipped without one (45cdb3426).

## Direction review (validate-plan, Reshape) and owner answers
- The splash hold resolves after the contact tick is simulated in the steady case, so a self push keyed on the declaration usually lands late. Owner chose scheduling world-contact pushes at FIRE from frozen fire facts. Entity contacts ride the declaration.
- Stand-ins for host-only facts would have the engine define what health and state facts mean on the shooter. That conflicts with "the author decides what a hit means" (`E16--combat-presentation-substrate`), with "per-entity modder state is host-authoritative only" (`E16--impact-policy-substrate`) and with the `present()` SDK doc. Owner chose load-time input sorting. Shipped `present()` calls in `content/dev/scripts/combat-lifecycle.ts` read only the impact amount.
- Owner added a cosmetic flinch, an engine-owned procedural jolt, and moved host enemy animation to the render tick.
- Owner folded in `client-holding-projectile-cleanup`. Its defect: predicted projectiles survived Holding, producing effects and sounds until contact or expiry, with declarations suppressed by transport.

## Client-perceived delay audit (2026-10-21)
A connected client skips `sim::simulate_tick`. Locally it runs wieldable prediction, mover prediction, pawn movement and weapon prediction only.

| Area | Finding | Owner |
|---|---|---|
| Weapon resources | Magazine, heat, cell, reload start, sounds and completion trail by ~RTT (+ 30 Hz stepping). The dry-fire boundary is wrong for ~RTT after the last round. Fire during an unseen reload presents. Ammo after a switch shows the old weapon's for ~RTT. | Owner-routed to its own draft session. It decides whether `coop--timed-slot-projection` still earns its build. |
| Hitmarker | Predicted state exists (`hitmarker_visible`), but nothing renders it on any role | Owner-routed to its own draft session |
| Recoil, viewmodel kick | Not implemented | `viewmodel-motion-layer` (draft) for kick; none for recoil |
| Enemy reactions | Pain and death names apply on receipt, ahead of the pose. Behavior-entry sounds are host-only. Gibs vanish on receipt. Bars run at RTT + ≤100 ms. | This brief: animation timing and the jolt. Others unowned. |
| Enemy projectiles | Drawn ~200 ms behind the host; a clean-looking dodge can still hit | Non-goal (no lag compensation) |
| Doors / use | Motion starts ~RTT late; the phase fast-forward pops the door partly open; door sounds never play on clients | Sounds: roadmap Audio step 4. Motion and pop: unowned. |
| Triggers | UI verbs never shown on clients; presentation commands ~RTT and lossy | Unowned |
| Pickups | Press prompt drawn nowhere; acquire ~RTT; weapon inventory waits for a Control tuning payload | Unowned |
| Damage taken | Health one-way + stepping; hurt flash only if authored; direction indicator decided, not built | Unowned |
| Respawn | Not built | Roadmap E18 R |

## Second direction review (validate-plan, Under-scoped) and owner answers
- Gap: every shipped `present()` sits in a tag-filtered policy (`dummy`, `enemy`, `combat-zombie` in `combat-lifecycle.ts`), but client copies are built tagless (`net_descriptor.rs`). Sorting by value and gate alone would predict nothing while the host suppressed the shooter's copy. Owner asked for the PvE-right call; chosen: a host-classified per-entity marker. It is exact under runtime tag changes. In a non-live-service engine with no installed base, a `SNAPSHOT_VERSION` bump costs nothing. Rivals: a per-class Control table (misses runtime tags) and map-placement tags (miss spawned enemies).
- Owner accepted the hit-volume departure as cosmetic-only.
- Owner made the jolt a feel feature on every role, with an authorable amplitude.

## Pin table
| Pin | Scenario | Ordering | Expected outcome |
|---|---|---|---|
| H1 | Harness reach | The netcode relay fixture runs host movement only. The app-crate conditioned harness (`host_activations/conditioned_tests.rs`, with `receive_alternate.rs` for snapshot receive) already drives the activation lane, host tick, predicted flights, declarations and verdicts on the shipped channels, but builds declarations by hand. | Extend the app-crate conditioned harness. Rows that assert the push tick, named spawns, enemy guesses or the jolt send the declaration through the production client builder and run the app's predicted-flight path, never hand-built records. Size this before the first slice. |
| P1 | Point-blank wall | Static contact lies within the first tick's travel, down to zero. Host FIRE mints after the fire tick's movement; the client's flight launches after it too. | Both peers push before the movement of fire tick + 1, never on the fire tick itself. |
| P2 | Late start | A start waits behind a live execution, the cadence half or a stall, and mints at a resolution past its client tick + travel. | The host pushes on its next unsimulated tick from its own frozen contact; the client settles on confirmation. Never on a simulated tick. |
| P3 | Freeze on push tick | The cursor freezes at the push tick; held movement re-runs before the real command resolves. | The push applies once, on the resolution that advances the cursor to its tick. |
| P4 | Trim skips push tick | Catch-up trim reseats the cursor past the push tick. | The push applies before the first resolved tick after the trim. |
| P5 | Burst ordinal | A later ordinal mints across a freeze or trim, so the host's resolved client tick at mint differs from the client's. | Both peers take the ordinal's fire tick as its start or release client tick plus the authored waits before it. |
| P6 | Confirmation vs ack | Confirmation rides the acking snapshot and always names a tick at or before that snapshot's ack. Case a: it names the client's own push tick. Case b: it names another tick, earlier or later than the client's, while the client's tick may still be unacked. Case c: the ack passes the client's push tick with no confirmation. | a and b: the push is in the authoritative state; the client's pending push for that shot leaves the ledger, so replay applies it at neither tick. c: the push stays pending, re-applied at the first replayed tick, until its outcome is confirmed or a refusal ends it. |
| P7 | Entity declaration after world push | Loss holds the entity declaration; the host advances past the world-push tick and applies it. | The world push stands; the entity declaration adds none. The client's entry settles on confirmation, accepting the size difference as an ordinary correction. |
| P8 | Entity push before verdict | The declaration arrives in time for the contact tick. The host's replay that decides detonation runs at least one tick later. | The push applies on the declared tick from the declaration alone and stands whatever the replay finds. |
| P9 | Late verdict, enemy handover | The render tick passes the host's resolution tick before the verdict arrives. | No frame jumps more than the interpolation step plus the guess's own step when the verdict lands. |
| P10 | Pain and death due together | A slew or hitch brings a pain and a death due on one frame. | Death applies; the pain is skipped, as for remote players. |
| P11 | Stale verdict | A verdict for a pre-demotion shot arrives after re-entry. | It resolves nothing and moves no push. |
| P12 | Marker flip in flight | A runtime tag change flips the marker between the client's contact and the host's evaluation. | The host withholds what the declaration names and routes the rest: each number shows once. |
| P13 | Splash sets differ | The host's splash and the client's predicted splash reach different marked targets. | Host-only target: the host routes its number to the shooter. Client-only target: the predicted number stands, and the host spawns none. |
| P14 | Push tick already predicted | The frame's fixed ticks are predicted before its shots resolve and its flights advance, so a world push on fire tick + 1 (multi-tick frame) or an entity push on its contact tick is scheduled after that tick was predicted. | The push lands on its derived tick. History holds commands only, so the client re-runs from a retained pose at or before that tick through the newest predicted tick. The re-run is a silent replay that emits no movement events. It moves the registry pose and the first-person eye on the same frame and adds nothing to the correction offset. The declared and derived ticks match the host's. |
| P15 | No replayed tick | A push is pending past its tick when the reconcile replays nothing: the ack reaches the newest predicted tick, a None-ack reset or plain teleport-floor correction clears history, or the ring evicted the push tick's entry. | The push applies once, before the next predicted tick's movement. The ledger is not the command ring; no history clear or eviction drops a pending push. |
| P16 | Push evaluation | The pawn moves between FIRE and the push tick, and a pending re-application moves the push again. | Each peer evaluates the impulse from the pawn's pose before the movement of the tick it applies on, with the splash's radius, falloff and static occlusion, on every replay too. A push that evaluates to zero still counts as the shot's one push. |
| P17 | Entity contact, then HIT-only refusal | The client's flight meets an entity before its scheduled wall; the declaration then takes a HIT-only refusal. | The world push scheduled at FIRE stays latent while the entity push is live. The refusal removes the entity push, and the world push stands on both peers, re-applied at the first replayed tick if the ack passed it. |
| P18 | Repeated and piled confirmations | The client's acks are lost during a burst, so the host repeats each confirmation in every snapshot and the list grows. | A repeat for a push the client already settled changes nothing. The list is bounded at 64 per pawn; the host drops the oldest past it and the net crate rejects a longer list. A shot whose outcome falls off the list reaches the client as an ordinary reconcile correction. |
| P19 | Several hits on one target | One shot carries several records on one target, such as pellets. | The declaration names the spawns shown for each record. The host withholds one spawn per named record and routes the rest. |
| P20 | Splash neighbours | The client's predicted splash reaches targets that carry no hit record. | The declaration names spawns for every target the client's splash reached, keyed by target network id apart from the records. A named target adds no damage, push or record; the host's splash set alone decides who is hit. |
| P21 | Two shots, one enemy | A second shot hits an enemy whose first guess has not handed over, and the verdicts name different host ticks. | Each shot keeps its own guess and hands over at its own verdict's tick; the offsets add. |
| P22 | Direct and radial shove | A splash weapon that authors a direct shove contacts an enemy directly. | The guess composes the direct shove and the radial shove, as the host does. The zero-damage direct dispatch predicts a number only if the marked spawn's gate admits it. |
| P23 | Demotion mid-guess | A shot contacted before demotion; its verdict would arrive after re-entry or never. | Demotion and level reset drop every enemy guess and client jolt, contacted shots included, on the same frame. |
| P24 | Queued states across a reset | Enemy states are still queued when a level reset, demotion or despawn lands. | The reset or demotion clears every enemy queue, and a despawn drops its entity's queue. Nothing queued before applies afterwards, including to an entity materialized after the reset. |
| P25 | Live flag across contact and resolution | (a) The client contacts a live-marked copy; the same shot kills it on the host, and the dead flag reaches the client before the guess hands over. (b) Another shot kills the target on the host after the client's contact and before this shot resolves. | (a) The host pushed on the killing hit, since liveness is read before lethal damage. The running guess continues to its handover; the live flag gates only new candidacy, guesses and numbers. (b) The host neither damages nor pushes the target. The predicted number stands, the host spawns none for it, and the guess eases back from the verdict's host tick. A copy already marked dead at contact is no splash candidate, takes no guess and shows no number. |
| P26 | Push tick already acked | A long render frame on a fast link: the client finds an entity contact whose derived push tick the host has already resolved, and this frame's reconcile pruned that tick before the post-loop schedules the push. | The client does not re-run from that tick. The push joins the ledger as pending past its tick and applies before the next predicted tick's movement (P15). The declaration still names the derived tick, the host applies it on its next unsimulated tick, and confirmation settles it. |
| P27 | Confirmation for an unknown push | A snapshot confirms a self push the ledger does not hold: demotion or level reset cleared it, or the client never scheduled it because its own contact lay beyond reach. | The confirmation adds nothing to replay and re-creates no entry. The host's push reaches the client as one reconcile correction, smoothed or snapped by its size. |
| P28 | HIT-only refusal with a live enemy guess | The client's contact made enemy guesses, then the declaration takes a HIT-only refusal. The refusal carries no host tick, and no verdict follows for the retired declaration. | The host never detonates the shot, so it pushes no enemy. Each of that shot's guesses starts easing back on the frame the refusal arrives, and reaches the host pose no later than the guess alone would have settled. |
| P29 | Verdict tick without a resolution | A FIRE refusal or an intake overflow sends a verdict for a shot that never resolved; a refusal whose declaration was already pending later drains and sends a second verdict. | A drained declaration's verdict names the tick it resolved on, and a refusal's verdict names the tick it was sent on. A shot's first verdict sets its handover; a later verdict for the same shot moves nothing. |
| M1 | Marker identity | Base and override policies share one qualified id; selection returns every matching policy, each group with its own gate. | A spawn identity carries the policy id, same-id variant ordinal, group index and effect index, or an equivalent the client resolves against its own load. |
| V1 | Client volume path | Host candidacy and `damageable_volume` need Health, which display copies and the client's pawn lack; rays already read `HitVolumeComponent`. | The client reads the hit-zone bound or `HitVolumeComponent` directly. Falloff and direction come from the entity descriptor or model zones, which sit outside tuning and the mod digest. |

## Review round 1 (review-brief) and resolutions
The owner's standing instruction is to apply fixes that help PostRetro solve the problem or set up the builder. Owner items were resolved toward the PvE, client-feel-first stance and reported for veto:
- R4 (blocker): an entity push lands before the host knows it detonates. Resolved: the self push belongs to the shot, and the damage replay never moves, removes or adds one (P8).
- R5 (blocker): an ack and a report on different channels left confirmation ambiguous. Resolved: the pawn snapshot names applied self pushes, so confirmation rides the ack (P6). The host resolution tick for the enemy handover later moved onto `ShotVerdict` (round 2).
- R3 / premise F1: the peers launch from different origins. Resolved: a one-tick disagreement is accepted and healed by confirmation.
- Premise F2: the host replay can detonate on a live entity short of the wall. Resolved by R4's rule.
- R10: resolved as P7.
- R11: clients see no enemy death. Resolved: guesses drop on despawn only, and corpses jolt.
- R12 and R14: resolved: the declaration names the numbers the client showed. The host withholds exactly those, so a host-only splash target still reaches the shooter (P12, P13).
- R13 / premise F5: resolved: the marker lists predictable spawns, with identities that tell an override from its base.
- Premise F3: the client cannot run the agent knockback model. Resolved: a grounded guess, swept against the static world.
- Premise F4: resolved: the client uses the host's volume rule (hit-zone bound, else hitbox).
- Premise F6 / R16: other clients hold no per-victim hit fact. Resolved: the jolt plays where a hit is simulated or predicted.
- Premise F8: the rewrite list gains the routing and anchor lines.
- Subtract findings applied: handover formula, jolt duration and seam moved to Path; version wording follows §Version gates; bare counts dropped; non-goals trimmed, teammate warrant added, audit leftovers named; Non-goals moved above the Scripting surface.

## Review round 2 (review-brief) and resolutions
Blockers: the push tick could name the fire tick (subtract S2); a pending push had no terminal (rows F1, premise P-1, subtract S3); P6 case b was unreachable and doubled the push (rows F2). Resolutions:
- The push tick is never earlier than the tick after FIRE; hitscan carries none.
- A pending push expires once the ack passes its tick by the longest the host could still apply it. Expiry joins confirmation and FIRE refusal as terminals; FIRE refusal covers `InitiationRejected` too.
- P6 rewritten: confirmation always sits at or before its ack and retires the entry whatever tick it names.
- A push tick already predicted re-predicts from that tick (P14), never shifting it.
- The resolution report merged into `ShotVerdict` as an appended host tick (premise P-7, subtract S1). No message variant is added, so the app-protocol id stays.
- Other clients learn of direct contacts only (premise P-2); they show no jolt, stated as a choice.
- Corpses: host splash never reaches them and no hit pushes them (premise P-3). The marker gains a live flag, and splash candidacy, guesses and numbers predict only on live-marked copies.
- Dry or silent shots push: the push follows the shot (rows F16).
- "Report" for the declaration renamed "declaration" (rows F22).
- The future-detonation sentence moved to the promotion write-up: any later source of host-only detonation states how it treats a scheduled push.
- Rows and pins P14–P24, M1 and V1 added; subtract trims applied.

## Review round 3 (review-brief) and resolutions
Blockers: the expiry correction could not stay in the ordinary band (rows F1); an enemy guess had no end after a HIT-only refusal (rows F2). Resolutions:
- Expiry removed. The snapshot confirms each minted shot's self-push outcome, including none (premise F2). A start never minted ends at its HIT-only refusal, as one smoothed correction.
- Enemy guesses of a refused declaration ease back from the frame the refusal arrives (P28).
- Dry or silent pulls schedule no push (premise F1), reversing round 2: such a pull forecasts a host refusal, so a predicted lift would be false.
- Push tick only on entity-contact declarations (rows F9, subtract S2); range-checked against fire tick and frozen lifetime.
- A shot's first verdict sets its handover (P29, premise F4).
- Number sorting reads the impact amount only; "slots the client holds" dropped (subtract S1, rows F13).
- Push-tick derivation is one shared function (subtract S3). Live-marked copies of non-player entities only, with teammates pinned (rows F12).
- Pins P25–P29 added, with P14 amended; the Problem names the single-player jolt and the push's within-a-tick timing; subtract trims applied.
