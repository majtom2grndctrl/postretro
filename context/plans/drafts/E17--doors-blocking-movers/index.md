# Doors and Blocking Movers (E17-E)

## Goal

Make kinematic movers react to what's in their path. A mover push blocked by
static geometry is a no-op today (`mover_carry.rs`); movers are "unstoppable
kinematics" that displace but never yield (`movement.md` §6). E17-E turns that
seam into a real block policy — reverse, stop, or crush — authored per mover and
settable at runtime, applied to players and enemies, with an auto-close timer
and author-wired sounds. Doors become authoring on `kinematic_mover`, not a new
entity class.

## Scope

### In scope

- A per-mover **block policy** — `displace` (default, today's behavior),
  `reverse`, `stop`, `crush` — authored as a seeded KVP and settable at runtime
  via a `moverSetBlockPolicy` scripting command.
- Reaction on **swept contact** (the mover's leading face crossing an entity),
  not only terminal pinch. Block-reverse is gated to *approach* — it fires when
  the mover's tick-end heading is toward an entity, not while receding.
- A host-only **mover blocking/crush pass** that detects contact and dispatches
  the policy. `reverse` sets travel direction away from the contact; `stop` holds
  and resumes when clear (new replicated `blocked` flag); `crush` keeps moving
  and damages a pinned entity on a per-victim cadence through the E16 chokepoint,
  continuing **past death** so the existing beyond-lethal *overkill* impact fact
  keeps flowing to mod policies (gibbing is a mod-authored impact policy on that
  fact, deferred by assets — the engine adds no overkill/gib concept).
- **Players and enemies.** Enemy handling requires net-new mover-vs-enemy
  collision (movers collide only with the player today), against the enemy Agent
  capsule.
- **Auto-close / auto-return timer** — host-only. Engine default, overridable
  mod-wide via a new `ModManifest` field, overridable per mover via KVP. Homes
  interruption (a `stop` command cancels the pending return) and re-trigger
  (resets the timer).
- **Mover sound events** — a new `TickEvents` mover bucket emitting named
  transition events (open/close/blocked/crush) drained through the *executing*
  dispatch path, wired by authors to `playSound`. **Host-local** (see below).
- Wire: one new replicated phase field (`blocked`); `SNAPSHOT_VERSION` and
  `WIRE_VERSION` bump.

### Out of scope

- Portal/visibility interaction — E17-F owns whether a door affects what
  renders.
- Enemies **riding** movers (carry/displace under the `displace` default).
  Enemies are handled only under `reverse`/`stop`/`crush`; a `displace` mover
  ignores enemies exactly as today. Enemy navmesh does not account for moving
  movers.
- Client-predicted blocking. The block decision depends on entity positions, so
  it is host-authoritative and reconciled — never predicted (see Direction).
- **Networked peer mover audio.** Mover sounds are host-local: single-player
  hears everything; in co-op the host hears them and remote clients do not.
  Making a host-driven mover sound audible on a remote peer is deferred to a new
  roadmap item (Epic 12, *Networked peer audio*), gated on spatialization —
  `audio.md` plays all sounds dry, so a remote source at full volume regardless
  of distance would be worse than silence. No world sound is networked today.
- Spatialized crush/door audio (positional, reverb) — deferred with the above.
- **An engine overkill threshold or gib mechanism.** Overkill is the existing
  E16 impact fact (`health_after` beyond zero, `@impact.healthAfter`); gibbing is
  a mod-authored impact policy on it, asset-gated. E17-E only keeps crush damage
  flowing past death so that fact accumulates — it defines no death, overkill, or
  gib behavior (`roadmap.md` E16: "no engine concept of death").
- A typed `onMoverEvent` dispatch source (see Alternatives rejected).
- Client-side crush prediction; per-voice volume/looping on the sound path.

## Direction

**Problem.** A mover push blocked by static geometry returns the entity still
penetrated and logs `pinch/crush resolution is deferred` (`mover_carry.rs`, the
`unblocked_mover_displacement` blocked branch). The cause is E17-A deferring the
block decision, not a symptom of collision tuning. Observed in `movement.md` §6:
*"Movers are unstoppable kinematics… dev maps must avoid pinch points."* A door
cannot react to what it hits, so a closing mover soft-locks a pinned player and
neither crushers nor reversing doors are authorable.

**Placement.** Engine floor for the mechanism, author data for the taste
(`scripting.md` §1). The engine owns the *evaluator* — collision detection, the
deterministic driver, the host-authoritative block *decision*, the wire, the
determinism of how reversals compose. The author owns the *policy point* — which
of `displace/reverse/stop/crush` a mover uses — as seeded data plus a runtime
setter, the closed-vocabulary shape E17-C/D established for mover commands. Crush
damage amount and cadence are per-mover KVPs: this is level trap-authoring (a
specific crusher's lethality), not gameplay balance, so it is a mover KVP like
`speed`/`mode` and does not violate `entity_model.md` §4's "no map-overridable
gameplay tuning" rule, which targets weapon/movement balance. The rate is the
taste axis, exposed with an engine default; the per-victim cadence *mechanism*
under it is engine-owned correctness.

**Prior commitments.**
- `movement.md` §6 declares movers unstoppable and crush deferred — E17-E
  **overturns** this by design; the doc updates at promotion.
- E17-A's client prediction is a **pure function of {replicated phase, static
  path}** (`networking.md` §"Phase boundaries"). Blocking makes mover motion
  depend on entity positions, which the client does not simulate for remote
  entities — so the block decision **diverges** to host-authoritative and
  reconciled. This is the central architectural consequence, not a choice.
- `networking.md` "Hash only what cannot be replicated": `block_policy`, the
  auto-return timer, and per-victim crush countdowns are **neither** replicated
  **nor** a client-prediction input (the client never evaluates the policy or a
  timer, only observes their effects). So they stay host-only and off the level
  content digest — the same shape as `carry_yaw`, a mover component field held
  locally rather than replicated (seeded from PRL, excluded from the wire
  mirror). Only `blocked` — the stop-hold state the client must see to avoid
  snap-back — goes on the wire.
- `audio.md`: audio is host-local presentation with no replication path, and
  spatialization is deferred. Mover audio hooks are therefore host-local this
  slice; peer audibility is its own roadmap item.
- `roadmap.md` E16 (shipped): "the engine no longer treats 0 HP as death — kill,
  **overkill**, stagger… are authored as policy over impact facts, with no engine
  concept of death." Crush honors this: it keeps applying damage past the death
  latch (using `producer: InTick`, the arm whose impact policies evaluate — the
  app-drain producer is stubbed), so the beyond-lethal fact keeps reaching a
  future mod gib policy. Crush adds no engine death/overkill/gib behavior; a
  crushed enemy is removed by the mod's own death/gib despawn policy.
- `scripting.md` §12: a new dispatch source is justified only by ephemeral
  per-fire inputs a case blocks on. Mover sound events publish none an author
  needs, so E adds no typed source; named events reuse the dispatch-address model.

**Alternatives rejected.**
- **Client-predicted blocking.** Would need clients to simulate every entity's
  position against every mover — data they do not have for remote pawns/enemies,
  and a determinism liability on the hottest path. Host-authoritative +
  reconcile is the only model consistent with A.
- **All-peers mover audio in this slice.** The genre wants co-op crushers heard
  by all, but doing it *well* is gated on spatialization (deferred), and no world
  sound is networked today. Deferred to a roadmap item rather than shipping dry
  full-volume remote audio.
- **A typed `onMoverEvent` dispatch source.** Justified only by ephemeral
  dispatch inputs, which mover phase transitions do not have; deferred until a
  concrete case needs them.
- **Doors as a new entity class.** Settled: a door is authoring KVPs on
  `kinematic_mover`.

**Foreclosures / one-way doors.** The `blocked` wire field and the
`SNAPSHOT_VERSION`/`WIRE_VERSION` bump are a one-way door — removing them later
costs another version bump. Keeping `block_policy`/timers host-only (off the
digest) is **reversible**. Choosing host-authoritative reconciliation forecloses
a pure-function-predicted blocking model without a wire change; reversing costs a
wire bump plus reconcile rework. Nothing else material.

## Event edges

The four authored mover sound events map to phase transitions, defined here once;
Tasks 1/2/5/6 reference these rows.

| Event | KVP | Fires when | Emitted by |
|---|---|---|---|
| Opened | `open_event` | mover reaches its forward (open) terminus | host-only tick step |
| Closed | `close_event` | mover reaches its start (closed) terminus | host-only tick step |
| Blocked | `blocked_event` | a `stop`- or `reverse`-policy mover enters reactive contact (rising edge) | blocking pass |
| Crushed | `crush_event` | a `crush`-policy mover deals crush damage to a victim (each tick damage lands, per victim) | blocking pass |

"Open" is the forward-travel terminus (highest segment reached traveling `+1`);
"closed" is the start terminus (segment 0). A single-waypoint pure rotator has no
travel termini: no Opened/Closed edge, and it ignores `auto_close_ms` (warn at
seed). A cyclic waypoint chain is already a load error (`resolve_waypoint_chain`),
so no other terminus-free mover exists.
Reverse-on-contact fires `Blocked`, not a distinct event — the door reacting to
an obstruction is one authored sound whether the reaction is hold or reverse.

**All edge detection and emission is host-role-gated and lives *outside* the
shared mover driver** (`run_kinematic_mover_tick` / `advance_mover_phase_one_tick`
/ waypoint-arrival handling), which the client also runs for prediction and
reconcile replay. Opened/Closed are detected by a host-only step that diffs the
mover's pre/post phase around the order-1 tick; Blocked/Crushed are emitted inside
the host-only blocking pass. A connected client's mover bucket therefore stays
empty even through catch-up replay.

## Acceptance criteria

- [ ] **AC1 (stop, player).** A mover with `block_policy = stop` that contacts
  the player holds within one tick of contact and resumes within one tick of the
  player stepping clear. The client's predicted mover shows no snap-back at the
  hold edge or the resume edge.
- [ ] **AC2 (reverse, player).** `block_policy = reverse` sets the mover's travel
  direction away from a player in **swept contact** while the mover's tick-end
  heading is toward the player — a player standing in the doorway reopens it —
  and does not re-flip while the mover is already receding.
- [ ] **AC3 (crush, player).** `block_policy = crush` keeps moving and damages a
  player pinned between the mover and static geometry on the first pinned tick and
  every `crush_interval_ms` thereafter while pinned — **continuing past death**,
  each hit emitting the beyond-lethal overkill impact fact — and stops only when
  the player is no longer pinned. A player with room to be pushed clear takes
  **no** crush damage.
- [ ] **AC4 (default preserved).** No authored policy behaves exactly as today:
  the player is displaced out of overlap where possible, and the push no-ops
  (no damage, no mover reaction) when blocked.
- [ ] **AC5 (death paths reused).** A crush kills a player through the existing
  HP-latch → `playerDied` path and an enemy through the existing despawn +
  impact-animation path. No new death path is introduced, and no engine
  overkill/gib concept is added — overkill stays the existing `health_after` fact.
- [ ] **AC6 (enemies).** A map-placed enemy is subject to reverse/stop/crush the
  same as a player: a `reverse`/`stop` door reacts to an enemy in the doorway,
  and a `crush` mover kills an enemy it pins.
- [ ] **AC7 (co-op reconcile).** A connected client reconciles a host block
  reaction without divergence, and the block decision never executes on the
  client: the stop hold (via replicated `blocked`), the reverse direction change
  (via replicated `direction`), player crush HP loss (via the owner-private
  `player.health` projection), and enemy crush death (via the replicated
  despawn + impact-animation, since a remote enemy carries no client `Health`).
  A restart command (`Start`/`Reverse`/`GoToPathNode`) issued during a block
  clears the client's predicted `blocked` in lockstep, without a snapshot round-trip.
- [ ] **AC8 (runtime setter parity).** `moverSetBlockPolicy(policy)` changes a
  mover's policy at runtime; the reaction, sequence, trigger-KVP-bound, and
  KVP-seeded routes converge on the same effect (a KVP-seeded mover behaves
  identically to one set by command).
- [ ] **AC9 (auto behavior).** A mover with `auto_close_ms` reverses toward its
  closed terminus after the timer elapses; re-triggering during the hold resets
  the timer; a `stop` command during the hold cancels the auto-return.
- [ ] **AC10 (default cascade).** The engine auto-close default applies when
  neither the mover KVP nor the mod descriptor sets one; a `ModManifest`
  auto-close field overrides the engine default mod-wide; a per-mover KVP
  overrides both.
- [ ] **AC11 (sound executes, host-local).** On the host or in single-player,
  authoring a mover transition-event name and a matching
  `defineReaction(name, playSound(...))` plays the sound on that transition —
  the mover event reaches the executing dispatch path, not the non-executing
  drain. A connected client emits no mover sound.
- [ ] **AC12 (wire).** The drift-guard tests on both the net and engine sides
  pass with the new `blocked` phase field, and a pre-`blocked` peer is refused by
  the handshake.

## Tasks

### Task 1: Blocking substrate + player stop policy (thin slice)

Build the end-to-end vertical slice that falsifies the host-authoritative block
reconciliation model, using `stop` for the player only.

Add a `BlockPolicy` enum (`Displace`, `Reverse`, `Stop`, `Crush`;
`#[serde(rename_all = "snake_case")]`) and a **host-only** `block_policy` field on
`KinematicMoverComponent`, seeded from a new `block_policy` KVP.

**Seed all of E's per-mover KVPs (`block_policy`, `crush_damage`,
`crush_interval_ms`, `auto_close_ms`, `open_event`/`close_event`/`blocked_event`/
`crush_event`) through the full compiled-map chain in this task, exactly as
`carry_yaw`/`speed_mps` are** (the exemplar to follow end to end) — one format
bump; Tasks 2/5/6 read fields this task seeded and touch no format file. A map KVP
traverses: (1) the level-format record `KinematicMoverRecord`
(`crates/level-format/src/kinematic_geometry.rs`) — **bump
`KINEMATIC_GEOMETRY_VERSION` 2→3** and add the legacy-V2 read path plus its
round-trip test (the section is version-gated); (2) the KVP parse in
`crates/level-compiler/src/parse.rs`; (3) the `From<KinematicMoverRecord>` in
`crates/level-loader/src/prl.rs` onto `LoadedKinematicMover` (not onto
`KinematicGeometry`, which holds only `{movers, waypoints}`); (4) the real seed
into `KinematicMoverComponent` in `runtime_movers.rs` (`spawn_from_geometry`).
This PRL-section version bump is distinct from the SNAPSHOT/WIRE bumps below and
happens once. The `block_policy` field is held locally
and **not** added to the wire mirror `WireKinematicMoverState`, following
`carry_yaw`. `block_policy` is the first host-only field on this component; the
warrant is that per-client deltas are diffed from `WireKinematicMoverState`, not
from component `PartialEq`, so a host-only field drives no wire delta — the
executor confirms the delta path against the wire mirror.

Add a **replicated** `blocked: bool` phase field to `KinematicMoverComponent` and
to `WireKinematicMoverState`, appended beside `started`/`completed`, bumping
`SNAPSHOT_VERSION` 12→13 and `WIRE_VERSION` 15→16; extend the engine↔wire
conversion in `crate::netcode` (`netcode/replication.rs`) and both drift-guard
tests.

Add a new host-only module (e.g. `kinematic_mover/blocking.rs`) with
`run_mover_blocking_pass`, run from `sim/mod.rs` after agent steering
(entity_model §5 order 6) and before the death sweep (order 8). It obtains the
same collision inputs the movement stage assembles for `displace_from_movers` —
the mover collider set, the `MoverPoseSource`, and the static `CollisionWorld` —
plus the player capsule from the registry. It uses `deepest_mover_push_penetration`
(already swept-contact-aware — its regression is
`swept_mover_push_detects_thin_mover_crossing_capsule`; there is no separate
"swept" query, and `deepest_mover_push_penetration_excluding_swept` is the
opposite variant). Extract the "pinned/blocked" test currently inline in the
private `unblocked_mover_displacement` (a static-cast-blocks-the-push predicate)
into a shared predicate both `mover_carry.rs` and the pass consume. For `Stop`,
set `blocked = true` while the player is in swept contact and clear it when clear.

The deterministic driver (mover tick, order 1) must **honor `blocked`**: a blocked
mover advances neither `segment_elapsed_ms` nor spin phase (holds the whole mover,
so a stopped rotating gate stops rotating), and resumes from the held phase when
cleared. The hold is a zero-motion tick, not a skipped tick: the driver still
refreshes per-tick provenance (`spin_angle_before_tick_rad`,
`was_active_this_tick`, `current_linear_velocity`) so carry, replay, and the
replicated pose read zero motion — an early return above those writes leaves
last tick's rotation delta live and phantom-rotates riders. The `blocked` check
precedes the `completed` early-return so a stale flag cannot ride completion.
Split `blocked` mutation by concern so host and client
converge: the host-only pass **sets** `blocked` on contact and force-clears it on
no contact (host authority the client reconciles); the **completion-clear** lives
in the shared driver and the **restart-clear** (on `Start`/`Reverse`/
`GoToPathNode`) lives in the shared `apply_mover_command`, so a client applying a
replicated restart command clears its locally-predicted `blocked` in lockstep
rather than snapping on the next snapshot. The restart-clear runs before each
arm's idempotence early-return — a `Start` on a held mover is otherwise a
whole-arm no-op (`started` stays true through a hold) and AC7's during-block
clear would never fire; the host pass re-asserts `blocked` next tick if contact
persists.

**Producer→next-tick latency.** The mover moves at order 1; the pass decides at
order 6b; the driver honors it at the next tick's order 1. So a mover completes
one tick of motion into contact on the detection tick and resumes one tick after
clear. Make the swept-contact query conservative — inflate the mover's leading
face by at least one tick of travel — so the detection-tick over-penetration stays
sub-capsule and no visible dip results. The pass's position relative to the weapon
tick (order 7) is immaterial: crush and weapon damage both feed the order-8 death
sweep through the same latch.

The pass is host-only **by construction**: it lives inside `simulate_tick`, which
connected clients already skip (they run `run_kinematic_mover_tick` directly for
prediction) — there is no host-role flag to thread; single-player and the host run
it. `displace_from_movers` keeps today's behavior on all peers — the
pinned-predicate extraction it absorbs is a pure refactor. Define the
shared `MoverEventKind` enum and the mover `TickEvents` bucket; the bucket entry is
`(MoverEventKind, mover_id)` (Task 1 pushes edges — Task 6 maps kind → that mover's
authored `*_event` name → dispatch address). Add the host-only Opened/Closed
detection step (per Event edges — outside the shared driver) and emit
`Blocked`/`Opened` where this task produces those edges (drained by Task 6).
Reconcile: a client re-runs the driver from the replicated `blocked` phase with no
snap-back. Verifies AC1, AC4, AC7 (stop), AC12.

### Task 2: Player reverse + crush policies

Extend `run_mover_blocking_pass` with the remaining player policies.

`Reverse`: express the reversal as a **directional intent**, not a blind flip. On
swept contact, judge approach against the mover's **tick-end heading** — its
`direction_sign` and current-segment direction read live at the pass (post-driver,
post-command: an order-3 trigger command's mutation is deliberately visible) —
not the net per-tick `tick_delta`, which blends incoming/outgoing segments at a
corner and points the wrong way at a terminus the mover auto-reversed through this
tick. If the tick-end heading has a positive component toward the contact
(approaching), set the mover's travel direction *away* from the contact —
idempotent, a no-op if already receding (kills per-tick buzz on a slow reverse
door), and a no-op at a terminus where the only valid segment already leads away.
Clear any outstanding `target_segment` so a stale `go_to_path_node` destination
does not survive. The pass runs after the trigger tick (order 3), so a same-tick
block-reverse takes precedence over a trigger command. A `<2`-waypoint mover has
no path to reverse along; under `reverse` it degrades to `stop` (behaves as
`block_policy = stop`: `blocked` holds it, spin included, and the hold
replicates). Emit
`Blocked` on the reverse-contact **rising edge** (once on approach, suppressed
while receding) — this is pass-owned and decoupled from the `blocked` flag, which
stays `stop`-only.

`Crush`: the mover keeps moving; while the player is **pinned** (the shared pinned
predicate from Task 1 reports no room to push clear), apply damage through
`apply_damage_with_context` with `DamagePayload { amount }` and `DamageContext {
source_id: "mover.crush", attacker: None, weapon: None, zone: None, producer:
DamageProducer::InTick }` — `InTick` is the arm whose impact policies evaluate, so
a future gib policy is reachable. Evaluation is caller-pumped, not automatic: the
pass invokes the sim's impact hook (`simulate_tick`'s `on_impact` closure, which
runs `ImpactPolicyRuntime::evaluate_pending_in_registry`) immediately after each
hit, exactly as the AI and weapon producers do — an in-tick dispatch left for the
app-drain sink is dropped there with an error, never evaluated. Cadence is **per
victim**: each pinned entity
accrues its own countdown in a **host-only side-table** keyed by (mover, victim)
where the victim is the full `EntityId` including generation (a despawn bumps the
generation, so a reused slot cannot inherit a prior victim's countdown). Not a
component field — mutating per-tick state stays off the replicated component.
Damage lands on the first pinned tick and every `crush_interval_ms` of continuous
pinning (an interval of `0`, or below one tick, damages every pinned tick),
**continuing past death**: each post-death hit re-emits the beyond-lethal
overkill fact (`health_after`) at the E16 chokepoint, which a mod gib policy
accumulates (per-entity-state) and acts on — the engine defines no overkill
threshold or gib behavior. Retire a victim's entry only when it **leaves contact
or is despawned**, not at the death latch. A crushed player pawn is not despawned
(respawn model), so its overkill facts keep flowing while it stays pinned —
harmless (a mod gib policy fires once, idempotent via per-entity-state); the
cadence ends when the door reverses or the pawn is moved. `crush_damage` and
`crush_interval_ms` are seeded mover KVPs (seeded by Task 1's chain) with engine
defaults. A player with room to be pushed clear is not pinned and takes no
damage. The lethal path is unchanged — HP latches at zero and the existing sweep
fires `playerDied`; the firing client reconciles its HP via the owner-private
`player.health` slot. Emit `Crushed` per victim on each tick damage lands. Verifies
AC2, AC3, AC5 (player), AC7 (reverse, player crush).

### Task 3: Mover-vs-enemy collision + enemy policies

Net-new collision: extend `run_mover_blocking_pass` to sweep each active mover's
collider against the **enemy Agent capsule** (the Agent component carries a
collision capsule, so this reuses the capsule-based
`deepest_mover_push_penetration` — no new AABB-vs-mover query). Iterate the
Agent-bearing enemy set. Movers collide only with the player today, so this sweep
is the real cost of enemy scope.

Apply the full policy matrix to enemies: `reverse` and `stop` react to an enemy in
swept contact exactly as for the player (directional intent / `blocked` flag /
`Blocked` emission); `crush` damages an overlapped enemy through the same
`apply_damage_with_context` call (`source_id: "mover.crush"`) on the same
per-victim cadence, continuing past death and emitting `Crushed` for enemy victims
too. An enemy is "pinned" for crush purposes whenever the mover overlaps it —
enemies are not mover-pushed, so there is no displace-clear escape, unlike the
player (warrant: `displace_from_movers` is player-only). Retire the enemy's crush
entry on **un-pin or despawn**, not at the death latch: the enemy's lethal
transition flows through the existing death sweep → authored impact policy →
despawn/animation, and it is that mod policy (kill, or a future gib) that despawns
the body — which un-pins it and ends the cadence naturally (remote enemies
replicate as host despawn/animation; they carry no client `Health`). Until then,
crush keeps feeding the overkill fact, exactly as for the player. Under the
`displace` default, enemies are ignored (no push, no reaction). Consumes Task 2's
policy dispatch. Verifies AC5 (enemy), AC6, AC7 (enemy crush).

### Task 4: `moverSetBlockPolicy` scripting setter

Add a runtime setter across the mover-command wiring. Add
`MoverCommand::SetBlockPolicy(BlockPolicy)`
(`crates/entities/src/components/kinematic_mover.rs`, `#[serde(rename_all =
"snake_case")]` → `set_block_policy`) and its applier arm in `apply_mover_command`
(`commands.rs`), which sets the host-only `block_policy` field. **This arm is the
one exception to the applier's "phase-only, every-peer" contract** — it writes a
host-only, off-wire field; on a client the write is inert (the client never reads
`block_policy` and its replicated phase is byte-identical to one that never ran
it). Update the applier's doc comment to note this.

`block_policy` is **not** replicated phase, so `moverSetBlockPolicy` is **not**
consequential — do **not** add it to `is_trigger_consequential_primitive`
(`scripting-core/src/reaction_dispatch.rs`) or the `CONSEQUENTIAL_PRIMITIVES`
array (`postretro/src/trigger_bindings.rs`); reorder/double-run is harmless
because it touches no reconciliation state.

Register it everywhere `moverSetSpinRate` is registered — those two consequential
allowlists are the only sites to skip. Wire: the reaction registrar
(`register_mover_reaction_primitives`) **and** the sequenced registrar
(`register_sequenced_mover_primitives`), both with an args struct twin of
`MoverSetSpinRateArgs` (AC8 and the parity/closed-vocab tests require both); the
SDK type templates (`sdk_lib.luau` / `sdk_lib.d.ts`) and the regenerated
`expected.d.*` fixtures; the SDK handle method `setBlockPolicy(policy)` in
`sdk/lib/entities/movers.{ts,luau}`; and the closed-vocabulary test in
`scripting/reactions/registry.rs`. **Do not add a `trigger_bindings.rs`
`bind_command` arm** — `bind_command` runs only for primitives `classify()` marks
`Consequential`, so an arm for a non-consequential setter is unreachable dead code
(and "fixing" it by making the primitive consequential would break the
replication-safety rationale). The setter is still trigger-authorable: a trigger's
`on_fire` reaction that names `moverSetBlockPolicy` dispatches through the
tag-targeted reaction registry at app-drain — the one-tick app-drain latency is
harmless for a host-only field. AC8's "trigger-KVP-bound route" therefore means
that reaction path, not a `BoundTriggerCommand`. Add a parity test modeled on
`set_spin_rate_reaction_and_sequence_routes_match_shared_kvp_command_path` that
additionally asserts a **KVP-seeded** `block_policy` produces behavior identical to
one set by command (seed→command equivalence, beyond the spin-rate test's
reaction/sequence convergence). `commands.rs` is ~711 lines; extend in place.
Verifies AC8.

### Task 5: Auto-close timer + mod-descriptor default

Add a host-only auto-close/auto-return timer. The mover carries a seeded
`auto_close_ms` value (component field + KVP seeded by Task 1's chain, off-wire
like `block_policy`); when
a mover reaches its forward (open) terminus and `auto_close_ms > 0`, the host
counts a **host-only side-table** countdown down and on expiry issues the close —
a directional intent setting travel toward the closed terminus, mutating
replicated `direction` (the timer itself is never on the wire; clients reconcile
the reversal, same shape as the block decision). A re-trigger during the hold
resets the countdown; a `stop` command (interruption) during the hold cancels the
pending return. Neither is observable from phase — a `Start` re-fired at a
completed held door no-ops entirely in the shared applier — and the
dependency-free applier cannot reach a host-only table, so the reset/cancel
observation lives on the host's command-application path: the funnel
`apply_mover_command_to_targets` already threads level-scoped context
(`MoverCommandDiagnostics`) and shows the shape — an optional host-only observer
records `Start`/`GoToPathNode` (reset) and `Stop` (cancel) per mover; clients
thread none. Auto-close and a same-tick block reaction resolve in the host pass
with the block decision last, so a blocked door does not auto-close into the
obstruction. A `<2`-waypoint mover (pure rotator) has no closed terminus to return to —
it ignores `auto_close_ms` with a seed-time warning; cyclic waypoint chains are
already a load error, so no looping case exists.

The default value cascades: engine constant < `ModManifest` field < per-mover KVP
(most specific wins). The engine constant is `0` — auto-close disabled — so
unauthored movers keep today's behavior; the cascade exists for mods that want a
mod-wide close. Add the `ModManifest` field mirroring `render` — a new field
on `ModManifestResult` (`scripting-core/src/runtime/types.rs`), parsed in the
js/lua manifest parsers (`data_descriptors/js/manifest.rs`,
`.../lua/manifest.rs`) like `drain_render_profile_js`/`_lua`, drained at boot in
`run_deferred_mod_init` (`startup/splash_lifecycle.rs`) into the mover default, and
surfaced in the SDK `ModManifest` type (`sdk/types/postretro.d.ts`/`.d.luau`).
Emit `Closed`/`Opened` at the close/open edges (per Event edges, via the host-only
detection step). Timer state is transient — the side-table entry is cleared on
level unload. Verifies AC9, AC10.

### Task 6: Mover sound events through the executing drain (host-local)

Wire the mover `TickEvents` bucket (defined in Task 1) to author-wired
`playSound`, host-local. The mover carries seeded per-transition event-name KVPs
(`open_event`, `close_event`, `blocked_event`, `crush_event`; seeded by Task 1)
— each an optional string naming a reaction **dispatch
address**, mapped per the Event-edge table (`Opened → open_event`, `Closed →
close_event`, `Blocked → blocked_event`, `Crushed → crush_event`). The bucket entry
is `(MoverEventKind, mover_id)` (Task 1); Task 6 maps each entry's kind to that
mover's authored `*_event` name and pushes the resolved dispatch address.
Drain the bucket post-tick through the **executing**
`fire_named_event_with_sequences` (`reaction_dispatch.rs`, the death-event
precedent in `main.rs`) — **not** the non-executing `fire_named_event` the
weapon/movement drains use — so a `defineReaction("<name>", playSound(...))`
actually dispatches `SystemReactionCommand::PlaySound` into
`App::dispatch_system_commands` → `Audio::play`.

Sound is host-local this slice: emission and drain run host-side (single-player
included); a connected client's mover bucket stays empty (edge detection is
host-role-gated outside the shared driver, per Event edges) and it plays no mover
sound. Peer audibility is deferred (roadmap: Epic 12 *Networked peer audio*). Size
note: `main.rs` is far past the soft threshold, but the drain addition is a
localized insertion into the existing post-tick drain sequence — a `main.rs` split
is its own effort, out of scope; note the smell, do not split. Verifies AC11.

## Sequencing

**Phase 1 (sequential):** Task 1 — thin slice; falsifies host-authoritative
block reconciliation and the wire bump before any fan-out.
**Phase 2 (concurrent):** Task 2 (player reverse+crush — edits the pass module),
Task 4 (scripting setter — edits `commands.rs`, the `MoverCommand` enum, and the
SDK; no `trigger_bindings.rs` change), Task 5 (auto behavior + mod descriptor —
its own host-only timer step plus manifest/boot). Near-disjoint: Task 1 seeded
every KVP and component field, so the only overlap is one call line each for
Tasks 2/5 in `sim/mod.rs`.
**Phase 3 (sequential):** Task 3 — consumes Task 2's policy dispatch in the pass
module.
**Phase 4 (sequential):** Task 6 — emits at edges built by Tasks 1/2/5 and drains
through the executing path; touches the pass and driver after they settle.

## Rough sketch

- `BlockPolicy` enum and the static `block_policy` / `crush_damage` /
  `crush_interval_ms` / `auto_close_ms` / `*_event` fields live on
  `KinematicMoverComponent`, seeded via `LoadedKinematicMover`; all host-only
  (off the wire mirror), following `carry_yaw`.
- Per-tick host-only *mutating* state — per-victim crush countdowns and the
  auto-close countdown — lives in a host-only side-table keyed by mover (and
  victim `EntityId` for crush), cleared on unload. It never touches the
  replicated component, so it drives no wire delta.
- The blocking pass is a new host-only module under
  `crates/postretro/src/kinematic_mover/`; it reuses the movement stage's
  collision inputs and the extracted pinned predicate, and calls
  `apply_damage_with_context` directly, pumping the sim's in-tick impact hook
  after each hit. It runs after agent steering, feeding
  mover phase for the next mover tick — the producer→next-tick shape trigger
  commands use.
- `blocked` is the only new wire field, and it is `stop`-only. Reversals (block
  and auto-close) are idempotent directional intents, not blind sign flips.
  Edge detection is host-only and lives outside the shared driver.
- `MoverCommand::SetBlockPolicy` / `moverSetBlockPolicy` follow `SetSpinRate` /
  `moverSetSpinRate` wiring, minus the consequential allowlists.

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau | KVP |
|---|---|---|---|---|---|
| Block policy | `BlockPolicy` / `block_policy` field | host-only; command serde `set_block_policy` | `"displace"\|"reverse"\|"stop"\|"crush"` | same | `block_policy` on `kinematic_mover` |
| Blocked hold | `blocked: bool` phase | new `WireKinematicMoverState` field | n/a | n/a | n/a |
| Set-policy command | `MoverCommand::SetBlockPolicy` | `set_block_policy` | `setBlockPolicy(policy)` / `moverSetBlockPolicy` | same | via `on_fire` reaction (not a command KVP) |
| Auto-close | `auto_close_ms` field + `ModManifest` default | host-only | `movers.autoCloseMs` manifest field | same | `auto_close_ms` |
| Crush tuning | `crush_damage`, `crush_interval_ms` | host-only | n/a | n/a | `crush_damage`, `crush_interval_ms` |
| Sound events | `MoverEventKind` + `*_event` name fields | host-only | `defineReaction("<name>", …)` | same | `open_event`/`close_event`/`blocked_event`/`crush_event` |

## Wire format

One new field: `blocked: bool` appended to `WireKinematicMoverState`, beside
`started`/`completed`. bitcode owns endianness and bit-packing — no manual layout.
It is a deterministic input to client mover reconciliation, so it rides the
existing mover phase payload (added after the current phase fields, consistent
with `ComponentKind` numeric order for the payload). `SNAPSHOT_VERSION` 12→13 and
`WIRE_VERSION` 15→16; both drift-guard tests extend. Bumping `WIRE_VERSION`
alongside a snapshot field follows the replay-provenance precedent
(`SNAPSHOT_VERSION` 12 / `WIRE_VERSION` 13), and AC12 requires it: the
handshake gate compares only `WIRE_VERSION`, never `SNAPSHOT_VERSION`, so a
pre-`blocked` peer is refused only via the `WIRE_VERSION` bump.

Separately, the new authored per-mover KVPs (`block_policy`, `crush_damage`,
`crush_interval_ms`, `auto_close_ms`, `*_event`) live in the PRL kinematic-geometry
section, which is version-gated: bump `KINEMATIC_GEOMETRY_VERSION` 2→3 with a
legacy-V2 read path (this is a compiled-map format version, independent of the
SNAPSHOT/WIRE bumps). `block_policy`, the auto-return timer, per-victim crush
countdowns, and crush tuning are host-only and never serialized to the wire —
component-resident statics follow `carry_yaw`, mutating timers live in a host-only
side-table; neither reaches `WireKinematicMoverState`, so neither drives a wire
delta.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| The block decision is host-authoritative; no client path mutates mover phase on block | Task 1 (host-only pass) | Threatened if `displace_from_movers` or any client tick reacts to a block | AC7 |
| `blocked` is the only new replicated block state; `block_policy`, timers, crush tuning stay host-only | Task 1, Task 2, Task 3, Task 5 | Threatened if a client reads `block_policy`/a timer, or any lands on the wire | AC7, AC12 |
| `blocked` is `stop`-only, host-derived each tick; force-cleared on no-contact (host), completion (shared driver), and restart (shared applier) so the client converges | Task 1 | Threatened by a stale `blocked` riding completion/re-activation, or a client that cannot clear it in lockstep | AC1, AC7 |
| Crush damage flows only through `apply_damage_with_context` (no direct HP write); death paths unchanged; cadence continues past death and retires on un-pin/despawn; no engine overkill/gib concept | Task 2, Task 3 | Threatened by a bespoke HP mutation/death path, an engine overkill threshold, or retiring at the death latch (killing the overkill fact) | AC3, AC5, AC6 |
| Reversals (block, auto-close) are idempotent directional intents judged on tick-end heading, block-decision-last | Task 2, Task 5 | Threatened by a blind sign-flip, a net-`tick_delta` misclassification, or a cancel/buzz | AC2, AC9 |
| The `displace` default is byte-for-byte today's behavior on all peers | Task 1 | Threatened if the pass alters `displace_from_movers` | AC4 |
| Mover sound edges are host-role-gated outside the shared driver and route through the executing `fire_named_event_with_sequences` | Task 1, Task 6 | Threatened by emitting inside the shared driver (client double-emits) or copying the non-executing weapon/movement drain | AC11 |

## Orderings

Spec text; the test tasks cite these rows.

| Scenario | Ordering | Expected outcome |
|---|---|---|
| Two entities pinned by one crusher on the same tick | Both detected in one pass, each on its own per-victim countdown | Both take crush damage that tick (both on first-pinned tick); both swept the same tick if lethal |
| Second entity pins mid-interval (staggered) | Per-victim countdowns are independent | The newcomer takes damage on its own first pinned tick, not gated by the first victim's cadence |
| Victim un-pins and re-pins within one interval | Per-victim entry dropped on unpin, re-created on re-pin | Re-pin damages on its first pinned tick; wiggling cannot indefinitely evade damage |
| Crush keeps hitting a player pinned after death | Crush latches HP at 6b tick M; pawn persists (not despawned), still pinned tick M+1… | Damage and `Crushed` continue each cadence tick past death, each re-emitting the beyond-lethal overkill fact; a mod gib policy fires once (idempotent via per-entity-state); the engine adds no stop — the cadence ends only on un-pin |
| Crush kills an enemy whose mod death/gib policy despawns it | Lethal latch at 6b; the mod's death (or future gib) impact policy despawns the body | Crush continues past death until the despawn un-pins the enemy; the side-table entry retires on despawn/un-pin, never at the death latch |
| First tick of contact for a moving stop/reverse door | Mover moves at order 1; pass detects at 6b; driver honors next tick's order 1 | Mover completes one tick of motion into contact on the detection tick, then holds/reverses tick+1; swept-face inflation keeps over-penetration sub-capsule |
| Stop door: contact clears | Pass detects clear at 6b tick M; driver resumes at order 1 tick M+1 | Resume lags exactly one tick; no earlier resume, no double-advance |
| Reverse door whose one-tick motion cannot clear the capsule | Contact still present next tick while mover recedes (tick-end heading away) | Reverse fires once on approach; while separating, no re-flip and no `Blocked` re-emit — no per-tick buzz |
| Reverse-policy ping-pong mover reaches its forward terminus and swept-crosses an entity the same tick | Mover auto-flips `direction` at order 1; pass judges on tick-end heading at 6b, not net `tick_delta` | Classified on post-flip heading; "away" at the terminus is a no-op — the mover is not forced back into the contact and does not oscillate |
| Reverse-policy mover crosses an interior corner (travel direction changes within the tick), entity at the corner | Net `tick_delta` blends incoming+outgoing directions; pass uses tick-end heading | A mover that has passed the contact reads as receding → no spurious reverse |
| Reverse two entities in contact same tick | Both contacts detected before the directional set | Direction set away from contact once (idempotent), not double-flipped; one `Blocked` |
| Block-reverse vs same-tick trigger `goToPathNode` | Trigger at order 3, block pass at 6b | Block reverse overrides the command that tick and clears the outstanding `target_segment` |
| `auto_close` reverse and block reverse on the same mover, same tick | Auto-close and block resolved in the host pass, block last | Door ends moving away from the obstruction (block wins); direction changes once |
| `blocked=true` rides a mover into completion / re-activation | `blocked` force-cleared at completion (shared driver) and on restart (shared applier) before the next driver tick | Client never holds on a stale `blocked` |
| Rotating `stop`-policy mover contacts an entity | `blocked` freezes both linear and spin advance | A stopped rotating gate stops rotating; a `crush`-policy rotator keeps rotating and grinds (crush does not freeze) |
| Co-op client predicts a loaded mover across its open terminus | Client runs the shared driver (predict + reconcile catch-up); edge detection is a separate host-only step | Client emits no Opened/Closed; its mover bucket stays empty across N catch-up ticks |
| `reverse`/auto-close on a `<2`-waypoint (pure-rotator) mover | No path/terminus to reverse toward | Reverse degrades to `stop` (sets `blocked`, freezing spin); `auto_close_ms` is ignored with a seed-time warning — entity not left in permanent no-op contact |
| Auto-close timer expires the same tick a re-trigger arrives | Re-trigger observed before the expiry check | Timer resets; no close that tick |
| Auto-close hold crosses a level unload | Side-table entry cleared on unload | No dangling reverse |
| `stop` command during an auto-return hold | Interrupt observed during hold | Pending auto-return cancelled; mover stops |
| Crush policy, player has room to be pushed clear | Displace push resolves (not pinned) | Player pushed clear; no crush damage |
| Enemy under `displace` default | No push path for enemies | No push, no reaction — ignored as today |
| Connected client runs a `moverSetBlockPolicy` reaction on its own drain | Host-only `block_policy` written through the every-peer applier | Client's replicated mover phase is byte-identical to one that never ran it (inert host-only write) |

## Script syntax examples

Door authored in the map (KVPs on a `kinematic_mover`), reversing policy, 3s
auto-close, blocked and open sounds:

```
// kinematic_mover brush entity KVPs
block_policy   "reverse"
auto_close_ms  "3000"
blocked_event  "vaultDoorBlocked"
open_event     "vaultDoorOpen"
```

Author wires the sounds and flips a specific door to a crusher at runtime:

```typescript
import { defineReaction, playSound, world } from "postretro";

defineReaction("vaultDoorBlocked", playSound("door_blocked"));
defineReaction("vaultDoorOpen",    playSound("door_open"));

// Fired at runtime by a map trigger's on_fire KVP naming "armCrusher".
const [crusher] = world.query({ component: "kinematic_mover", tag: "trap_a" });
defineReaction("armCrusher", { sequence: crusher.setBlockPolicy("crush") });
```

Mod-wide auto-close default in the manifest, mirroring `render`:

```typescript
export default defineMod({
  name: "my-mod", id: "example.mymod", version: "1.0",
  movers: { autoCloseMs: 2500 }, // engine default unless a mover KVP overrides
});
```
