# coop--predicted-combat-feedback

Brief · resumable · reads: `context/lib/networking.md` §Combat authority, §Game-logic-owned apply invariant, §Presentation events vs. replicated state, §Host input command queue, §Version gates, §Testing the conditioned link · `context/lib/movement.md` §6 · `context/lib/ui.md` presentation layer · read at 0d3b7dae5

## Problem
Owner playtest under the dev lag profile (`networking.md` §Testing the conditioned link), after `coop--lossy-link-delivery`. Basis: observed defect. A connected client's rocket jump lifts it a split second after its own predicted explosion. Enemies its rocket hits are pushed noticeably after the client sees the contact, and damage numbers appear late too. Cause: the host alone applies splash push, enemy knockback, hit reactions and damage-number spawns, so each waits a round trip for the hit declaration to go up and the result to come back. Enemy push also waits out the interpolation delay. Meanwhile, host enemy animation states apply on receipt, so a pain or death clip leads the drawn body by that same delay. This is latency, not loss. When done, the firing client sees each hit enemy's push and jolt and its predictable damage numbers on the contact frame, and its own push within a tick of the explosion. Host-driven enemy animation lines up with the drawn pose. The jolt is new feel wherever a hit is simulated or predicted, single-player included. The host stays authoritative for movement, damage, health and AI. Clients still submit no impulse, and nothing rewinds.

## Decisions
- **The firing client predicts its own splash push.** It applies the host's splash math to its predicted pawn before the push tick's movement, and replay re-applies the push until it reaches a terminal. Authority does not move: the client predicts its own movement, as it does for movers and push volumes. Promotion rewrites "Predicted projectiles remain presentation-only until authoritative movement arrives" and "without reapplying the hit" (`networking.md` §Game-logic-owned apply invariant), and "never applied client-side" (`plans/done/E16--aoe-splash-damage`).
- **A shot pushes its shooter at most once, and the damage replay never changes that.** A dry or silent pull forecasts a host refusal, so it schedules no push. Once applied, the push stands whatever the replay finds: a detonation on a live entity short of the wall, or none. Only the shooter is affected, and hits already run on trust with cheap validation (`networking.md` §Combat authority).
- **A world-contact push is scheduled at FIRE on both peers.** Each peer takes the shot's client fire tick plus travel ticks to its own static contact, rounded up and never earlier than the tick after FIRE. The host applies the push before that tick's movement, or its next unsimulated tick if it has passed, without waiting for the declaration, so loss cannot move a rocket jump. Launch origins differ slightly between peers, so a one-tick disagreement is accepted and heals on confirmation.
- **An entity-contact push rides the declaration.** A declaration whose first contact is an entity carries the push tick, by the same rule; a world-contact declaration carries none. The host applies it there when the declaration arrives in time, else on its next unsimulated tick. It supersedes the shot's world push, which stands again if the declaration is refused; a declaration arriving after the world push applied adds nothing.
- **Every predicted push reaches a terminal.** The pawn snapshot confirms each minted shot's self-push outcome beside its ack: the client tick it applied on, or none. Confirmation retires the ledger entry. A FIRE refusal, demotion, level reset, or a HIT-only refusal for a shot with no confirmed outcome drops it. Until then it survives every reconcile, history clears included.
- **Enemy push is a cosmetic guess on live display copies.** At contact the client applies the host's knockback math with its local descriptor's response, as a grounded displacement swept against the static world, on top of the interpolated pose. It hands over as the render tick passes the host tick the shot's first verdict names, so the push never doubles. A guess the host does not match eases back, and so does every guess of a shot whose declaration is refused, from the frame the refusal arrives. Enemy knockback stays off the wire, and enemies stay otherwise unpredicted (`plans/done/E10--networked-enemy-authority-baseline`).
- **The host marks what a client may predict on each display copy.** A per-entity marker carries a live flag, set while the entity is a damage target, and lists the predictable damage-number spawns its current policy selection yields. The host rewrites it whenever either changes. Client copies carry no tags and no health (`plans/done/coop--networked-by-default` Decision 7), so only the host knows either. Splash candidacy, push guesses and numbers start only on live-marked copies of non-player entities; a guess already running finishes its handover.
- **Client prediction aims with the host's damageable-volume rule:** the hit-zone bound for a zone-bearing model, else the hitbox, read from the display copy. The host never uses the client's volumes or guesses for damage, push or selection. This departs, cosmetically, from `coop--networked-by-default` Decision 4 and AC 6.
- **Hit jolt plays where the hit is simulated or predicted.** A hit on a non-player entity with a behavior graph plays a short procedural jolt along the hit direction on its drawn pose, never on the simulated transform. The host and single-player jolt every hit, which keeps parity with the firing client's jolt at its own contact (`plans/done/client-combat-feel-parity-contract`). Other clients learn of direct contacts only, so they show none. Authors tune or disable it on the behavior descriptor. Like the built-in impact burst, it is an engine floor beside "the author decides what a hit shows" (`plans/done/E16--combat-presentation-substrate`). The authored pain state (`drafts/E10--enemy-stagger`) stays host-decided.
- **Host enemy animation applies at the presented render tick.** Enemy animation-state names queue with their server tick and apply when presentation reaches it, as remote players' do (`networking.md` remote avatar state rule), so a pain or death clip starts with the pose it describes.
- **Damage numbers are predicted where the marker allows.** At load, each `present()` is predictable when its value and gate read only the impact amount. The firing client shows the marker's spawns at its own contact, and its declaration names, per target, the spawns it showed. The host withholds exactly those from the shooter and routes every other spawn to everyone. A refused hit's numbers stand. This builds the refinement `E16--combat-presentation-substrate` deferred. Promotion rewrites "offers each once to … every participating client" (`networking.md` §Presentation events vs. replicated state) and "the host chooses the anchor" (`ui.md`) for the firing client.
- **Different local content guesses differently, and that is accepted.** Policies and entity descriptors stay outside the mod digest. A differing client computes a different push or value, but never shows a missing or extra number.
- **Holding retires predicted projectiles.** A demoted client must not predict for a participation it left. Demotion removes every unmapped predicted flight before it can contact and drops every enemy guess.
- **Placement.** Push-tick derivation is one function the host's scheduling and the client's prediction both call. The ledger, guesses, the client jolt and number evaluation are client-side prediction and presentation. Scheduling, confirmation and the marker are host netcode. The wire vocabulary lives in `postretro-net`. Splash, knockback and policy math stay single-sourced.
- **Version constants move per `networking.md` §Version gates.** The declaration's new fields and the verdict's host tick move `WIRE_VERSION`. The marker and push confirmation move `SNAPSHOT_VERSION`. No message variant is added.
- **Non-goals.**
  - A guessed push on a teammate's pawn, or on the client's pawn from anyone else's shot. A teammate's pawn shows its owner's host-confirmed movement; a guess on another screen would show a push its owner never predicted.
  - Predicting the shooter's own health loss. Health is host-authoritative state no client simulates.
  - Combat feedback the delay audit found but this brief does not own: behavior-entry sounds, gib timing, health-bar latency (`research.md` §Client-perceived delay audit).
  - Weapon resource prediction and drawing the hitmarker: owner-routed to their own draft sessions.

### Scripting surface
```ts
// Proposed design
export const grunt = defineEntity({
  components: {
    behavior: {
      knockback: { scale: 1, groundDrag: 4, airDrag: 1, control: 0.2 },
      hitJolt: 0.08, // peak metres along the hit direction; 0 turns it off; omit for the engine default
      // …states, attacks
    },
  },
});
```

## Acceptance
The single-push envelope is the peak a clean-link run of the same single push reaches, plus the ordinary band. Pin ids refer to `research.md` §Pin table.

### Automated
**Self push** (app-crate conditioned harness per H1, dev profile, seeded)
- [ ] Reproduces first: the push-tick row's test, run on the base commit, fails because predicted vertical velocity rises no earlier than a round trip after contact. It is committed failing before the fix.
- [ ] A rocket at the client's feet raises its predicted vertical velocity on its push tick. On a fixture whose launches agree, the harness withholds only that shot's declaration from the Input stream, through a hook at the client's send, until the host has simulated the push tick. The host pushes on the same client tick, and the client records no correction above the ordinary band. On a clean link, none. Where the launches differ, the host's tick is within one of the client's, and the correction stays inside the ordinary band.
- [ ] At any frame rate, no frame shows the lift before the frame that shows the explosion. A dry or silent pull schedules no push on either peer.
- [ ] Under the dev profile across a seeded minute of rocket jumps: no correction above the ordinary band.
- [ ] A shot whose static contact lies inside the first tick's travel pushes on fire tick + 1 on both peers, never on the fire tick (P1).
- [ ] Refuse: a static contact beyond frozen range or lifetime schedules no push on either peer. Permit: one inside it does.
- [ ] A late-delivered start whose push tick the host already simulated pushes on the host's next unsimulated tick. The client settles on confirmation and records no correction above the ordinary band after it (P2).
- [ ] A frontier freeze on the push tick applies the push once, when the cursor advances to it. A trim that skips the push tick applies it on the first resolved tick after (P3, P4).
- [ ] A burst's later ordinals push on the same client tick on both peers across a frontier freeze and a trim (P5).
- [ ] A rocket fired in a frame that predicts several ticks, and an entity contact found after its push tick was predicted, each push on the derived tick, matching the host's, with no correction above the ordinary band on a clean link (P14).
- [ ] A push re-predicted onto an already-predicted tick lifts the first-person eye on the frame that schedules it. It adds nothing to the correction offset and replays no footstep, jump or landing event (P14).
- [ ] An entity contact whose derived push tick is at or below the ack when found pushes exactly once on both peers: the client before its next predicted tick, the host on its next unsimulated tick. The client records no correction above the ordinary band after confirmation (P26).
- [ ] A declaration whose push tick is at or before its shot's fire tick, or past the shot's frozen lifetime, schedules no entity push. The host keeps the shot's world push, and the declaration's hit records are judged as usual.
- [ ] An entity-contact push lands on its declared tick when the declaration arrives in time, otherwise on the host's next unsimulated tick. Peak height stays within the single-push envelope, and the correction is smoothed.
- [ ] An entity declaration arriving after the shot's world push applied adds no push, and the correction stays inside the ordinary band (P7). An entity-contact shot whose declaration takes a HIT-only refusal pushes exactly once on both peers, on the world tick (P17).
- [ ] A declared entity contact that detonates nothing on the host keeps its push on both peers, and so does a replay that detonates on a live entity short of the scheduled wall. Exactly one push applies per shot on both peers (P8).
- [ ] A pawn that leaves the blast radius between FIRE and its push tick takes no push on either peer, and a later entity declaration for that shot adds none (P16).
- [ ] A confirmation adds nothing on replay, whether it names the client's push tick or another. An ack past an unconfirmed push keeps it pending, never dropped and never doubled (P6). A confirmation past its snapshot's ack is rejected at validation.
- [ ] A push pending past its tick when a reconcile replays no tick, and one whose history entry the ring evicted, each apply exactly once on the next predicted tick (P15).
- [ ] A confirmation repeated across snapshots settles its push once. A snapshot whose confirmation list exceeds the bound is rejected at validation (P18).
- [ ] A confirmed outcome of none retires the predicted push, which the next reconcile removes as one smoothed correction. A start the host never mints pushes nothing on the host; its predicted push leaves the ledger at the HIT-only refusal, as one smoothed correction, never Teleport-class.
- [ ] A confirmation naming a push the ledger has dropped or never held adds nothing on replay and re-creates no entry. The host's push arrives as one correction (P27).
- [ ] Refuse: a FIRE refusal, by verdict or by initiation rejection, leaves no predicted push after the next reconcile. Permit: a confirmed push survives every reconcile.
- [ ] A weapon with zero self scale predicts no push. Two self pushes on one client tick each apply exactly once on both peers.
- [ ] A pending push is cleared on both peers at demotion and level reset and never applies after it. A verdict from before the demotion, arriving after re-entry, resolves nothing (P11).
- [ ] Regression guard: on a pinned fixture, the listen host's own rocket jump lifts on the same tick and reaches the same peak, within float tolerance, as on the base commit.

**Enemy push, jolt and animation**
- [ ] A direct or splash hit on a live-marked enemy with a knockback response displaces its drawn pose on the contact frame, whether or not its marker lists any spawn. A copy the marker shows dead takes no guess.
- [ ] When the enemy's own shot kills it, the guess keeps converging to the host's displacement after the dead flag arrives mid-guess, with no frame jump beyond the handover bound. When another shot killed the enemy before this shot resolved, the firing client keeps its predicted number, draws no host spawn for it, and eases the guess back from the verdict's host tick. A splash over a dead-marked copy predicts no number for it and names none (P25).
- [ ] An entity-contact shot whose declaration takes a HIT-only refusal eases every enemy guess back from the frame the refusal arrives, and no enemy stays drawn off its host pose (P28).
- [ ] The firing client's splash over a teammate's pawn guesses no push on it. The teammate's drawn pose follows only the host's movement for it.
- [ ] As the render tick passes the verdict's host tick, the drawn displacement converges to the host's. Its peak stays within the single-push envelope, and no frame jumps more than the interpolation step plus the guess's own step. A verdict arriving after the render tick passed its host tick hands over within the same bound (P9).
- [ ] Two rockets on one enemy, resolved at different host ticks, converge to the host's drawn displacement. The peak stays within twice the single-push envelope, and no frame jumps more than the interpolation step plus the guesses' own steps (P21).
- [ ] A direct contact by a splash weapon with an authored direct shove guesses both shoves and converges within the single-push envelope of their sum (P22).
- [ ] A guess on an enemy the host does not push starts easing back when the render tick passes the verdict's host tick. The drawn pose reaches the host pose within float tolerance no later than the guess alone would have settled. A despawn mid-guess drops it on the same frame. No guess shows an enemy inside static world geometry.
- [ ] A demotion while a contacted shot's guess awaits its verdict leaves the enemy drawn at the host pose on the next frame (P23).
- [ ] On a clean link, a splash on a zone-bearing enemy and one on a hitbox-only enemy each give the firing client the host's number and push direction, within float tolerance. The target is stationary through the shot.
- [ ] A hit on a non-player entity with a behavior graph jolts it on the host and in single-player on the hit tick, and on the firing client on its contact frame. A second client shows no jolt for a remote shot's hits. An entity with player movement never jolts. Refuse: `hitJolt: 0` plays none on any role. Permit: an omitted `hitJolt` plays the engine default.
- [ ] Two jolts on one enemy compose and peak at no more than twice the authored amplitude.
- [ ] The jolt and the enemy guess never write the simulated transform: the host's replicated Transform and the client's interpolated sample are unchanged by them.
- [ ] The Scripting surface example, set on the dev mod's reference enemy, loads and jolts at its authored amplitude.
- [ ] A host enemy animation state applies when the presented render tick reaches its server tick, not on receipt. A pain and a death that come due on one frame apply only the death (P10). Before time sync names a render tick, a state applies on arrival. A state queued before a level reset, demotion or despawn never applies after it (P24).

**Damage numbers**
- [ ] Load-time sorting marks a spawn whose value and gate read only the impact amount as predictable. One whose value or gate reads health before, health after, max health, entity state or any slot is host-routed. Both cases are pinned.
- [ ] A tagged enemy whose selection yields predictable spawns carries a marker listing them on the client. A runtime tag change that empties the selection clears the list within one snapshot, and one that fills it lists the spawns within one snapshot; neither needs a full baseline. A death clears the live flag within one snapshot. Every shipped damage-number policy marks its tagged targets.
- [ ] The firing client spawns each marked number on the contact frame, for direct and splash targets. On a clean link with matching content, each value equals the host's. The target is stationary through the shot.
- [ ] An enemy whose selection holds a predictable and a host-routed spawn shows the firing client each number once.
- [ ] Refuse: the host withholds from the firing client exactly the spawns its declaration names. Permit: every other spawn from its own shot still reaches it, and the same hit's numbers reach a second client and the listen host.
- [ ] A multi-pellet shot with several records on one marked target shows the firing client each predicted number once per record, and delivers none of them again (P19).
- [ ] A splash shot whose predicted splash reaches two marked neighbours names both, and the host withholds those spawns from the shooter. A target named only in the spawn list takes no damage on the host (P20).
- [ ] A marker flip between the client's contact and the host's evaluation shows the firing client each spawn at most once: the spawns its declaration named as predicted, and every other spawn the host's evaluation yields as routed (P12).
- [ ] A marked target only the host's splash reached reaches the firing client as a host-routed number. One only the client's splash reached keeps its predicted number and draws no host spawn (P13).
- [ ] Regression guard: a client whose local policy differs from the host's shows exactly one number per hit target and spawn.
- [ ] A refused hit's predicted numbers stand.

**Holding**
- [ ] A demotion through the real control path with a live predicted projectile leaves no flight, effect, sound, declaration, push or number after it. Repeated demotion is harmless. A later participation predicts new shots without restoring old ones.

**Version and grep gates**
- [ ] Every verdict names a host tick and reaches only its owner. A drained declaration's verdict names the tick it resolved on, and a refusal's verdict names the tick it was sent on. A shot's first verdict sets its handover, and a later verdict for that shot moves nothing (P29).
- [ ] A peer on the previous wire version is refused at the handshake; the app-protocol id does not move. A snapshot envelope at the previous snapshot version is rejected at validation. Permit: a peer on the new set is admitted, and its snapshots validate.
- [ ] Grep gates: no declaration or snapshot wire field names an impulse or enemy knockback. Outside their home crates, no `fn knockback_impulse`, `fn splash_damage_amount` or knockback `fn decay` is defined, and client push, guess and number code calls those. One push-tick derivation function exists, called by host scheduling and client prediction.

### Manual
- [ ] Loopback recipe under the dev profile: a rocket jump lifts on the explosion.
- [ ] Same link: a rocket into a group pushes and jolts enemies and shows numbers on contact, with no visible double push. Pain and death clips start with the drawn body.
- [ ] A second client watching sees host-routed numbers and pushes, as before.
- [ ] The owner picks the jolt's engine default on the dev map: the jolt reads at typical engagement range and never hides an authored pain clip.

## Path
- First slice: extend the app-crate conditioned harness (H1), then the world-contact push scheduled at FIRE, proved by the reproduce-first row.
- Push tick: the shot's client tick is its start or release tick plus authored waits, never `AuthorizedShot.fire_tick`, which is a host tick (P5). Fire facts: `projectile_static_contact_distance`. Derive the client tick from fire facts, not from the render frame where the flight met the wall: predicted projectiles advance per render frame (`advance_predicted`). A push tick already predicted re-runs silently from a retained pose (P14); one already acked joins the ledger as pending (P26).
- Host seam: `host_prepare_remote_pawn_commands`, with `stamp_start_fire_gate` and `ActivationEdges` `RetainedStart`/`due_start` as precedent. Key "simulated" on the cursor advancing, not on movement re-running under Held.
- Host push today: `ingest_hit_declaration` → `emit_splash_damage` → `apply_authorized_splash_impact_damage` → `apply_hit_knockback` → `add_knockback`. The splash hold is `PendingHitDeclarations::drain_ready`.
- Ledger: `PredictedTick` stores commands only; `reconcile_local_pawn_with_mover_history` merges, then `prune_through_ack`, then replays. The ledger lives apart from the command ring (P15). Confirmations ride the movement record's authority metadata beside `last_processed_client_tick`. FIRE refusal arrives as a verdict or `InitiationRejected`; both clear the push. A start dropped at intake mints nothing and ends only in `HitRefused`.
- Volumes: host falloff and candidacy need Health (`is_damage_target_eligible`, `damageable_volume` in `hit_zones.rs`), which display copies and the client's own pawn lack. The client needs a volume path that reads the hit-zone bound or `HitVolumeComponent` directly.
- Enemy guess: grounded response scale and ground drag, swept with the client's `CollisionWorld`. Handover offset: the guess at time since contact, minus the same model at time since the render tick passed the verdict's host tick. `ClientReplication::sample_into_registry` writes the interpolated pose directly. The jolt decays in about 150 ms.
- Animation timing: enemies call `apply_mesh_animation_state` from `apply_delta` on receipt. Remote players queue through `queue_correction` and `apply_due_remote_player_correction`.
- Numbers: `evaluate_dispatch` and `apply_presentation_spawn` are private. Load-time sorting walks bound inputs with a recording `BindingScope`; `OverlayStateScope` is the precedent. Selection in `impact_policy.rs` selects every matching policy, and a same-id variant replaces its base under one qualified id, so a spawn identity needs more than the policy id (research: marker identity). Deltas cannot remove a component, so the marker persists and empties. Client copies are built tagless in `net_descriptor.rs`. Routing: `route_host_presentation_spawns`; shooter-exclusion precedent: `send_observer_weapon_cue_to_server`. The `present()` doc in `sdk/lib/data_script.ts` gains the shooter exclusion.
- Jolt: `hitJolt` sits beside `knockback` on `BehaviorGraphDescriptor`; `retaliation` is the precedent for engine-owned scalar feel tuning.
- Strongest rival for the world push: key it on the declared contact tick. That is exact only when the declaration beats the tick, so loss moves every rocket jump. Rejected.
- `ready/coop--host-switch-timing` also bumps versions and works the same per-client-tick seam; whichever lands second rebases.
- Files past ~800 lines this extends: netcode `lib.rs` and `client.rs`, `prediction.rs`, `reconcile.rs`, `projectile_stage.rs`, `wire.rs`, `main.rs`. Split first where the change lands inside one.

## Wire format
Bitcode wire mirrors; no manual layout (`networking.md` §Wire/codec invariants).

| Surface | Change | Semantics |
|---|---|---|
| `HitDeclaration` (client → host, Input) | appended: push tick `Option<u32>`; shown spawns, keyed by target network id | Push tick: present only when the first contact is an entity; world contacts and hitscan carry none. After the fire tick and within the shot's frozen lifetime, else ignored. Shown spawns: every target the client showed a number for, records and splash neighbours alike, one entry per record. Naming a target adds no damage. An empty list withholds nothing. |
| `ShotVerdict` (host → owner, Input) | appended: host resolution tick `u32` | The host tick the shot resolved on; the enemy handover keys on it. |
| Pawn movement record authority metadata (snapshot) | self-push outcomes: shot id with an applied client tick or none, bounded at 64 | Outcomes for minted shots not yet covered by a confirmation the client acked. A repeat changes nothing. A tick past the record's ack, or a longer list, fails validation. |
| New entity component, the marker (snapshot) | live flag; predictable spawns, in identities that distinguish an override from its base | Persists once present; empties rather than disappearing. Delta-replicated on change. |

## Boundary inventory
| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| Hit jolt amplitude | `BehaviorGraphDescriptor.hit_jolt` | `"hitJolt"` | `hitJolt` | `hitJolt` | n/a |

Both TypeScript and Luau ship it, through the generated `sdk/types/postretro.d.ts` and `postretro.d.luau`.

## Open questions
- None.
