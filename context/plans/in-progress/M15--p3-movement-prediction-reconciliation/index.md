# M15 Phase 3 — Movement Prediction and Reconciliation

## Goal

Make the connected client feel local while the host stays authoritative. The client predicts
its own `PlayerMovement` pawn from command frames, sends those commands to the host, and
reconciles against authoritative snapshots without visible rubber-banding under normal
latency. Phase 3 is movement-only: walk, jump, crouch, and dash.

## Scope

### In scope

- Extend the existing `postretro-net` input command wire so every `InputCommand` carries a
  monotonic client command tick.
- Add an authoritative movement-ack echo to snapshot records for movement pawns: the
  latest client command tick the host resolved before producing that pawn state.
- Add a recipient-local pawn marker to snapshot records so a client can identify which
  replicated pawn is its own prediction target.
- Add engine-side conversion between `SimCommand` / `MovementInput` / `FireButtonState`
  and `InputCommand`.
- Add engine-side conversion between the mutable tick subset of
  `PlayerMovementComponent` and `WirePlayerMovementState`.
- On clients, add prediction state in `crate::netcode`: command ring, predicted
  `Transform` + `PlayerMovementComponent` history, authoritative ack pruning, rewind,
  replay, and smoothing.
- On the host, decode `ClientMessage::Input` into per-client command queues and apply
  them to each accepted client's authoritative movement pawn in command-tick order.
- Replace Phase 2's Transform-only slot pawn for Phase 3 movement sessions with a
  descriptor-backed `PlayerMovement` pawn from the existing player descriptor/materialize
  path. Test harnesses may keep an explicit Transform-only fallback, but that fallback is
  not predicted.
- Add a side-effect-free movement replay helper for reconciliation. It advances only a
  cloned `Transform` + `PlayerMovementComponent` pair through `movement::tick`; it must
  not run AI, weapons, death sweep, reactions, or unrelated entity mutation.
- Keep non-local entities on the Phase 2 remote interpolation path.
- Add deterministic in-memory harness tests for input order, loss/duplication, command
  pruning, rewind/replay, dash correction, and teleport correction.

### Out of scope

- Co-op respawn policy, player-leave gameplay policy, trigger ownership, and set-piece
  progress. Phase 4 owns those policies.
- Server-authoritative hitscan, fire-intent validation, HP confirmation, and
  favor-the-shooter history. Phase 5 owns combat.
- Predicted projectiles and predicted-entity to confirmed-entity handoff. Phase 6 owns
  projectiles.
- Full rollback of arbitrary world state. Phase 3 replays only the local movement pawn
  through the movement-only replay seam.
- Authoritative weapon or fire effects. Phase 3 may preserve fire-button intent in
  `InputCommand`, but Phase 5 decides how the host consumes it.
- New movement mechanics or movement tuning changes.
- Moving platforms or kinematic carry behavior.

## Acceptance criteria

- [ ] A connected client sends one command frame per predicted fixed tick. Each command has
  a monotonic `client_tick`, round-trips through `ClientMessage::Input`, and survives
  duplicate / old / malformed input packets without host panic or unrelated entity mutation.
  Duplicate/old hardening is tested by injecting `ClientMessage::Input` directly into the
  host drain/queue seam, not by requiring reliable-ordered transport to produce duplicates.
- [ ] The host applies accepted client commands to that client's authoritative
  descriptor-backed `PlayerMovement` pawn in command-tick order. Missing command ticks hold
  the last known intent for `INPUT_HOLD_TICKS = 3`, then fall back to neutral input.
  Synthetic hold/neutral ticks advance the authoritative input cursor; later real commands
  at or below that cursor are dropped.
- [ ] Host snapshots for movement pawns include `Transform`, `PlayerMovementState`, the
  latest resolved `client_tick` for that pawn, and a recipient-local pawn marker. The
  payload stays registry-blind inside `postretro-net`; engine glue owns all registry reads
  and writes. Registry-blindness is a wire-test plus review/grep gate: `crates/net` must
  not reference `EntityRegistry`, `EntityId`, or movement descriptors.
- [ ] After the local movement baseline is established, the client predicts its local pawn
  immediately from the same `SimCommand` it sends to the host. Under the deterministic
  Phase 2 latency profile, local walk, jump, crouch, and dash input have no added network
  wait before affecting the camera-followed pawn. The observable assertion is that the
  mapped local pawn / camera-follow pose changes on the same predicted fixed tick that
  emits `ClientMessage::Input`, before receiving an authoritative snapshot for that tick.
- [ ] On an authoritative movement snapshot, the client restores the acked
  `Transform` + mutable `PlayerMovementComponent` state, prunes commands through the echoed
  `client_tick`, replays remaining commands through the movement-only replay helper, and
  leaves unrelated replicated entities on the existing remote interpolation path. A
  `local_player` record with `None` ack after prediction has started resets prediction
  history, applies the baseline, and does not prune by command tick.
- [ ] Normal corrections are smoothed rather than snap-teleported. Automated tests assert
  correction classification and initial presentation-offset magnitude: ordinary corrections
  are `<= 0.5 m`, dash corrections are `<= 2.0 m`, and displacements `>= 3.0 m` classify
  as teleports. Teleports clear prediction history and presentation offset. Gameplay state
  always uses the reconciled registry transform; only local first-person presentation
  consumes a decaying correction offset. Visual smoothness is a manual QA gate, and any
  threshold change must update this AC with measured harness rationale.
- [ ] `crates/postretro/src/main.rs`, `crates/postretro/src/movement/mod.rs`, and
  `crates/postretro/src/netcode/client.rs` do not own core prediction state. Thin call-site
  edits are allowed, but long-lived prediction state, command history, smoothing state, and
  replay logic live in new focused netcode/sim modules or are split before extension. This
  is a source-layout review gate.
- [ ] `cargo test -p postretro-net` and focused `postretro` sim/netcode tests pass.
  Focused tests include the `postretro_net` wire/replication suite plus `postretro`
  `netcode`/`sim` prediction and reconciliation tests. Manual loopback with host/client
  verifies one `local_player` baseline, one camera-followed pawn, no second local-player
  marker after join/disconnect, immediate local input, remote interpolation still active,
  and no duplicate local pawn.

## Tasks

### Task 1: Command-frame wire and protocol bump

Extend `crates/net/src/wire.rs` so `InputCommand` carries `client_tick: u32` before
`movement` and `fire_button`. Extend the snapshot record model with movement authority
metadata for entity records. The raw entity record stores
`has_last_processed_client_tick: bool`, `last_processed_client_tick: u32`, and
`local_player: bool`; the typed record exposes that pair as
`last_processed_client_tick: Option<u32>` plus `local_player: bool`.
`last_processed_client_tick` is the latest client command tick the host resolved for that
pawn before snapshotting. It is `Some` on recipient-local movement pawn records once the
host has resolved at least one real or synthetic command tick; `None` is valid for
non-local movement pawns and for the first baseline before any command tick is resolved. If
a recipient-local movement record has `None` after prediction has started, the client
treats it as an authoritative reset: clear prediction history, apply the baseline, and do
not prune by command tick. `local_player` is true only in the per-recipient snapshot sent
to the pawn's owning client.

Bump `WIRE_VERSION` in `crates/net/src/transport.rs` and `SNAPSHOT_VERSION` in
`crates/net/src/wire.rs` because input and snapshot bitcode layouts change. Bump
`PROTOCOL_ID` in `crates/net/src/transport.rs` only if the app message vocabulary changes.
Keep bitcode as the only binary encoding owner; invalid record metadata must decode into
the raw envelope and fail validation before engine apply. Validation rejects
`last_processed_client_tick: Some(_)` or `local_player: true` on a record that does not
carry `PlayerMovementState`. Strict metadata validation rejects raw
`has_last_processed_client_tick = false` with a nonzero value, any ack/local-player
metadata on despawn or non-movement records, and non-finite `PlayerMovementState` before
typed apply. Add wire tests for command tick round-trip, snapshot metadata round-trip,
version rejection, metadata-on-non-movement rejection, malformed metadata, and invalid
`PlayerMovementState` payload rejection.

### Task 2: Engine conversion and movement-state merge helpers

In `crate::netcode`, add conversions between `sim::SimCommand` and
`postretro_net::wire::InputCommand`, including `wish_dir`, `jump_pressed`,
`dash_pressed`, `running`, `crouch_intent`, `facing_yaw`, and `FireButtonState`. Add
conversions from
`PlayerMovementComponent` to `WirePlayerMovementState` and an inverse merge that writes
only the mutable tick subset onto an existing descriptor-derived component. The merge must
not construct a full `PlayerMovementComponent` from the wire payload and must not touch
descriptor-owned tuning, `view_feel`, standing dimensions, stuck-stop config, or
`dash_programs`. Put conversion helpers near the existing wire conversion seam in
`crates/postretro/src/netcode/mod.rs` or a new `netcode/wire_convert.rs`; put movement-state
merge helpers in a focused module shared by `ClientReplication` and host snapshot
production.

Phase 3 carries `FireButtonState` only because `InputCommand` mirrors `SimCommand`.
Host-side weapon/fire effects ignore it in this phase; Phase 5 consumes fire intent for
server-authoritative combat.

Validate inbound input before queueing it. Reject non-finite `wish_dir` or `facing_yaw`.
Clamp finite `wish_dir` components into `[-1.0, 1.0]`. Preserve finite `facing_yaw` as-is;
the current camera yaw is intentionally unconstrained, so Phase 3 does not introduce a new
wrapping policy. Boolean button fields are already typed by bitcode. Invalid commands do
not mutate queues or registry state. Export a `sanitize_input_command`-style helper; Task 4
must call it from `host_handle_client_messages` before inserting into the per-client queue.

### Task 3: Client prediction state

Add a focused prediction module under `crates/postretro/src/netcode/`. It owns the client
command ring and predicted state history for the local `NetworkId` / `EntityId` pawn. Each
predicted fixed tick records the `client_tick`, outbound `InputCommand`, resulting
`Transform`, resulting `PlayerMovementComponent`, and whether replay included dash. The
client does not apply local prediction until it has received and applied a full
`local_player: true` movement baseline and has a stable `NetworkId -> EntityId` mapping.
Before that baseline, the client may send input commands to the host, but it must not spawn
or drive a provisional local pawn. `ClientReplication::apply_snapshot` /
`apply_components_to` owns applying the `local_player` full baseline, marking the mapped
entity with `EntityRegistry::mark_local_player_pawn`, and notifying prediction state of the
armed `NetworkId` / `EntityId` baseline.

The `crates/postretro/src/main.rs` fixed-tick loop should delegate to this module instead
of storing prediction data on `App` beyond thin call-site plumbing. In the connected-client
role, the fixed-tick loop sends exactly one `ClientMessage::Input` per predicted tick,
routes the local pawn through the prediction module, and does not call full
`sim::simulate_tick` for local gameplay movement. Non-gameplay client drains may remain in
the main loop as thin plumbing. Live connected-client prediction advances only the local
pawn's movement stage; it must not call the full `sim::simulate_tick` path because that
would rerun AI, weapon fire, death sweep, and other registry-wide systems. Rewind replay
uses the same movement-only helper added here. Do not add networking branches inside
`movement::tick`.

The movement-only replay helper takes a cloned `Transform`, cloned
`PlayerMovementComponent`, `MovementInput`, `CollisionWorld`, gravity, and tick dt. It
calls `movement::tick`, returns the new pair plus `MovementEvents`, and never touches
`EntityRegistry`.

### Task 4: Host authoritative command queues

Extend host netcode so `host_handle_client_messages` no longer drops
`ClientMessage::Input`. Netcode decodes finite commands into per-client queues keyed by
client id, drops old commands, and collapses exact duplicates. Game logic, not the net
crate, selects one command per client immediately before the Player movement stage, routes
it through the client-id -> pawn map, and snapshots only after the fixed tick settles.
This requires a new host-side multi-pawn movement seam in `crates/postretro/src/sim/`: it
takes `(EntityId, MovementInput)` pairs, snapshots transforms once at the start of the
fixed tick, runs `movement::tick` for those explicit pawns only, writes each resulting
`Transform` and `PlayerMovementComponent`, and aggregates per-pawn movement events into
`TickEvents::movement`. It must not use `local_movement_pawn()` or the single local-player
marker. In single-player, the existing `simulate_tick` path can call the helper with the
current local pawn. In host mode, every **remote client** movement pawn is included in the
explicit `(EntityId, MovementInput)` list and is driven by this seam, never by
`local_movement_pawn()`. The fixed-tick order is host movement commands first, then
AI/agent/weapon/death as today.

> **Phase 3 decision (2026-06-21):** The original AC also folded the *listen host's own
> player pawn* into the explicit seam so that host mode never touched `local_movement_pawn()`
> at all. As implemented, remote client pawns go through the seam (the Phase 3 deliverable),
> but the listen host's own pawn stays on `simulate_tick`'s local movement stage. Phase 3 is
> validated entirely through a **connected client** (Task 6 harness/loopback), so the host's
> own pawn is off the critical path and the gap has no observable Phase 3 cost. Retiring
> `local_movement_pawn()` from host mode is **deferred to a future host-as-loopback-client
> model** (host-player becomes a client to a same-process server via an in-memory transport),
> which dissolves the special case rather than patching it with a host self-queue. That work
> is its own small spec, tracked separately — not an amendment to this plan.

On accept, the host materializes one descriptor-backed `PlayerMovement` pawn through a new
net-slot spawn helper adjacent to `scripting::builtins::data_archetype`. Extend
`crates/postretro/src/netcode/lifecycle.rs` by adding a descriptor-backed variant to
`SlotPawnSource` and threading it through `on_slot_accepted`. Store a deterministic
slot-to-`player_spawn` placement assignment before spawning so a reused slot has an
auditable source. The helper reuses the descriptor materialization internals currently
behind `spawn_descriptor_instance`, but does not call `mark_local_player_pawn` and does not
assign a global `active_wieldable`. Spawn source is the accepted slot's assigned
`player_spawn` placement, using that placement's `entity_class` KVP and defaulting to the
`"player"` descriptor just like `spawn_from_player_starts`; tests may pass a synthetic
placement. Add a provenance path such as `DescriptorSpawnPath::NetworkSlot` if needed so
these pawns are distinguishable from map-start single-player spawns.

Use the accepted client's slot mapping as owner data, stamp the pawn with a
session-monotonic `NetworkId`, register it in `ReplicableSet`, and include
`owner_client_id` (engine-side metadata) in the owned snapshot passed to `postretro-net`.
This touches `crates/postretro/src/netcode/replication.rs::produce_owned_snapshots`,
`postretro_net::replication::EntitySnapshot` in `crates/net/src/replication.rs`, and the
server per-recipient encode path that derives `local_player`. `postretro-net` stays
registry-blind: it may compare `owner_client_id` with the recipient client id while
encoding per-client snapshots, but it never sees `EntityId`, `EntityRegistry`, or movement
descriptors. A Transform-only fixture remains allowed only in tests/dev paths and never
sets `local_player`.

Input gap policy is deterministic. The host expects the next command tick after the last
resolved tick. If the exact tick is queued, consume it. If it is absent, hold the previous
command for `INPUT_HOLD_TICKS = 3`; after that synthesize neutral input for the missing
tick. Real and synthetic commands both advance `last_processed_client_tick`. Any later
real command with `client_tick <= last_processed_client_tick` is stale and dropped. Tests
cover the gap, late-arrival, duplicate, and resumed-input cases.

### Task 5: Local reconciliation and smoothing

When `ClientReplication` applies an authoritative record for the local predicted pawn, merge
the `PlayerMovementState` payload onto the existing component, restore the authoritative
`Transform`, prune command history through `last_processed_client_tick`, replay remaining
commands with the movement-only replay helper, and write the final predicted state back to
the registry.
Small corrections are smoothed over a bounded number of render frames by a local-pawn
presentation offset owned by prediction state. The registry transform and movement
component snap immediately to the reconciled predicted result for gameplay, collision, AI,
and future prediction. The render camera, view-feel, weapon-view presentation, and portal
visibility camera for the local player consume `registry_transform + correction_offset`
while that offset decays to zero, so first-person presentation is continuous without
lying to simulation. Use one local presentation pose accessor and route the current
`main.rs` seams through it: camera follow / post-movement aim construction, the
`frame_timing` state pushed after fixed ticks, `RenderCamera` construction, and portal
visibility / render-eye position. Large corrections are teleports: clear prediction
history, clear the presentation offset, snap the registry transform, and stamp
previous/current transform to the same pose by generalizing or renaming
`EntityRegistry::set_remote_presentation_transform` for local-pawn reset rather than
duplicating transform-history logic. Non-local records continue to feed
`RemoteInterpolationBuffer`.

### Task 6: Harness and loopback validation

Promote the useful scenario shape from `sim/predict_reconcile.rs` into
production-adjacent tests without wiring the prototype type itself into runtime state. The
prototype may inform fixtures and expected timelines only; new replay assertions must use
the movement-only helper and include a guard proving full `simulate_tick` side effects are
not invoked. Add in-memory tests for ordered input, missing input, duplicate input, stale
authoritative snapshots, unknown local mapping, dash correction, and teleport correction.
Add a manual loopback recipe using the Phase 2 latency harness / `tc netem` profile and
the campaign-test map or another map with a descriptor-backed player pawn.

Automated harness profile:
`LinkConfig { delay: 45 ms, jitter: 60 ms, loss_probability: 0.05, seed: 0x1502 }` in both
directions, matching the Phase 2 45..105 ms one-way range. Run for at least 5 seconds of
simulated time after time-sync convergence. Hard gates: final client/server pawn position
error after draining in-flight commands is <= 0.05 m; corrections below the teleport
threshold use smoothing; no stale/duplicate/malformed input mutates unrelated entities.
Drain is complete when the conditioner has no packets in flight, the host input queue is
empty or its resolved cursor is at least the final sent command tick, and the client has
processed a snapshot acking that cursor.

### Task 7: Client-side descriptor materialization of the local pawn

> **Phase 3 note (2026-06-21):** The local predicted pawn needs a real
> `PlayerMovementComponent` for the wire mutable subset to merge onto and for replay to run,
> but `apply_snapshot` spawns the local baseline Transform-only (descriptor-immutable tuning
> never crosses the wire). Task 7 carries the descriptor class on the wire so the client can
> materialize the matching component from its own (shared) content:
>
> - The snapshot entity record gains `entity_class`: `RawEntityRecord` stores
>   `has_entity_class: bool` + `entity_class: String`; the typed `EntityRecord::{FullBaseline,
>   Delta}` expose `entity_class: Option<String>`. `validate` rejects an `entity_class` on a
>   non-movement / despawn record and a `has_entity_class=false` paired with a non-empty
>   string. `SNAPSHOT_VERSION` is bumped to **4** (record bitcode layout changed).
> - The host stamps the class from the pawn's `DescriptorProvenance` (`canonical_name`, gated
>   to `DescriptorSpawnPath::NetworkSlot` movement pawns; default `"player"`) in
>   `produce_owned_snapshots`. `postretro-net` stays registry-blind — `entity_class` is a
>   plain `String` it never resolves.
> - On the `local_player` baseline the client materializes the descriptor-backed
>   `PlayerMovementComponent` via a focused helper next to the host net-slot spawn internals
>   (`scripting::builtins::data_archetype::materialize_net_local_movement_component`), keeping
>   `client.rs` thin; the glue then arms prediction and reconciles.
> - The Task 6 harness now exercises this **real** client materialization path (its prior
>   `materialize_local_pawn_movement` stub was removed); the headline latency gate still
>   passes deterministically under seed `0x1502` (measured post-drain error `0.00000 m`).

### Task 8: Adaptive remote interpolation delay clamp

> **Phase 3 amendment (2026-06-22):** The 30 Hz snapshot cadence is a good co-op
> default only if clean links keep low latency and bad links get enough buffer. The
> remote interpolation delay is now adaptive. Time-sync jitter remains the feed-forward
> input. Held-newest interpolation starvation is the feedback input.
>
> Production law:
> `clamp(50 ms + 2 * jitter + starvation_margin, 50 ms, 250 ms)`, rounded up to whole
> 60 Hz sim ticks. `starvation_margin` rises when a sampled remote has to hold its
> newest pose, and decays back to zero after clean sampled frames. The margin caps at
> 100 ms, so a clean link still presents near the 50 ms floor, while repeated starvation
> can move the effective floor toward 150 ms before jitter applies. The global 250 ms
> ceiling still wins.
>
> This changes only remote presentation. The local pawn remains prediction/reconcile
> driven. Do not add network interpolation delay to the local player. Review should
> verify tests for: unchanged clean-link floor, delay rise after starvation, decay after
> clean sampled frames, and the unchanged global ceiling.

## Sequencing

**Phase 1 (sequential):** Task 1 — wire identity and authority metadata block all replay.

**Phase 2 (concurrent):** Task 2 and Task 3 — conversion helpers and client-side state can
land in parallel once the wire shape is pinned.

**Phase 3 (sequential):** Task 4 — host command application consumes Task 1 and Task 2.

**Phase 4 (sequential):** Task 5 — reconciliation consumes client history, host authority,
and movement-state merge helpers.

**Phase 5 (sequential):** Task 6 — validates the integrated prediction path.

**Phase 6 (calibration):** Task 8 — adaptive remote interpolation delay clamp. It depends
on the Task 6 starvation signal and does not change wire format or local prediction.

## Rough sketch

Current source already has the right foundation:

- `postretro_net::wire::InputCommand`, `WireMovementInput`, and `WireFireButtonState`
  exist, but `InputCommand` has no command tick.
- `host_handle_client_messages` decodes `ClientMessage::Input` and drops it.
- `produce_owned_snapshots` currently emits only `Transform`; Phase 3 extends it to append
  `PlayerMovementState` for registered movement pawns.
- `ClientReplication::apply_components_to` currently does not apply
  `PlayerMovementState`. It records `IgnoredPayload::MovementWithoutLocalComponent` only
  when the entity lacks `ComponentKind::PlayerMovement`. Phase 3 adds the local merge path
  for the predicted pawn.
- `simulate_tick` is the current single-command fixed-tick seam, and its movement stage only
  drives `local_movement_pawn()`. Phase 3 adds an explicit multi-pawn movement helper for
  host-owned client pawns and a movement-only local prediction helper; connected-client
  prediction and reconciliation must not call full `simulate_tick`.
- `EntityRegistry::mark_local_player_pawn` and `local_player_pawn` identify the pawn the
  camera follows and movement tick drives. Phase 3 should set the marker when applying a
  `local_player: true` full baseline and clear/reset it on local-pawn despawn or teleport
  reset.

Split-first flags:

- `main.rs` is over 6k lines. Add only thin calls there.
- `netcode/client.rs` is over 1.4k lines. Put prediction/reconciliation in a new module.
- `movement/mod.rs` is over 4k lines. Reuse it; do not extend it for networking.
- `crates/net/src/wire.rs` and `crates/net/src/replication.rs` are each over 1k lines. If
  Task 1 or server metadata work grows them substantially, split message-family helpers or
  tests first.

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| Command tick | `client_tick: u32` on command/history entries | `InputCommand.client_tick` bitcode field | n/a | n/a | n/a |
| Movement input | `MovementInput` | `WireMovementInput` inside `InputCommand` | n/a | n/a | n/a |
| Fire button | `FireButtonState` | `WireFireButtonState` inside `InputCommand` | n/a | n/a | n/a |
| Movement authority ack | latest resolved client command tick for a movement pawn | optional `last_processed_client_tick` on snapshot entity record | n/a | n/a | n/a |
| Local pawn marker | recipient-local prediction target flag | `local_player: bool` on snapshot entity record | n/a | n/a | n/a |
| Movement state payload | mutable subset of `PlayerMovementComponent` | `ComponentPayload::PlayerMovementState` / `WirePlayerMovementState` | n/a | n/a | n/a |

## Wire format

Bitcode remains the encoding owner. No manual byte layout is introduced.

`InputCommand` field order is `client_tick: u32`, `movement: WireMovementInput`, then
`fire_button: WireFireButtonState`. A client sends one
`ClientMessage::Input(InputCommand)` per predicted fixed tick on the existing
reliable-ordered `Channel::Input`. Phase 3 does not change the channel layout. The host gap
policy handles temporarily absent command ticks caused by transport delay, retransmit
head-of-line blocking, or commands not yet drained this frame; it is not treating
reliable-ordered input as permanently lossy at the command layer.

Snapshot records gain movement-authority metadata. Use an explicit optional shape in the
raw envelope rather than overloading `server_tick`: `has_last_processed_client_tick: bool`
plus `last_processed_client_tick: u32`. The typed apply model exposes that pair as
`Option<u32>`. Records also gain `local_player: bool`. Validation rejects
`last_processed_client_tick: Some(_)` and `local_player: true` unless the same record
carries `PlayerMovementState`.

`WirePlayerMovementState` mirrors exactly this mutable tick subset:

| Field | Wire type | Unit / rule |
|---|---|---|
| `velocity` | `[f32; 3]` | metres per second, finite |
| `is_grounded` | `bool` | live grounded flag |
| `air_jumps_remaining` | `u32` | live ability counter |
| `air_dashes_remaining` | `u32` | live ability counter |
| `dash_cooldown_ms` | `f32` | milliseconds, finite |
| `air_ticks` | `u32` | consecutive airborne ticks |
| `movement_state` | `WireMovementState` | `Normal`, `Dash`, or `Crouching` |
| `coyote_timer_ms` | `f32` | milliseconds, finite |
| `jump_buffer_timer_ms` | `f32` | milliseconds, finite |
| `jump_spent` | `bool` | coyote/jump-budget gate |
| `capsule_half_height` | `f32` | metres, finite |
| `capsule_eye_height` | `f32` | metres, finite |

`WireMovementState::Dash` carries finite `elapsed_ms: f32` and finite `boost: [f32; 3]`.
`WireMovementState::Crouching` carries finite `eye_current: f32`. Drift guards must derive
from `PlayerMovementComponent` and `MovementState`; do not hand-maintain a second source of
truth for variant coverage. Snapshot validation rejects non-finite movement-state payloads
before engine apply/merge, including velocity, timers, dash boost, crouch eye value, and
capsule dimensions. Add malformed movement-state payload tests alongside malformed
metadata tests.

Empty command queues and no-metadata records encode as empty lists / `None`. `NetworkId`,
baseline ids, tombstone ids, and command ticks stay `u32`.

## Calibration

- Correction smoothing constants may need tuning after the harness exists. Starting caps are
  pinned above so implementation can proceed. Treat changes as measured calibration, not a
  blocking design question.
