# M15 Phase 2 — Replication, Time Sync, Interpolation, Lifecycle

> Milestone 15 (multiplayer co-op netcode), Phase 2. Design reference:
> `context/research/netcode/` (`index.md` Phase 2 + Wire format,
> `crate-pattern-research.md` snapshot interpolation / per-entity delta). Consumes the
> Phase 1 transport, wire, handshake, full-state Transform demo, and `postretro-net`
> boundary from `context/plans/ready/M15--p1-transport-wire-handshake/`.

> **Prerequisite/blocker:** implement Phase 1 first:
> `context/plans/ready/M15--p1-transport-wire-handshake/`. Current source does not yet
> have `postretro-net`, `crates/net`, `NetworkId`, `WireTransform`, or `crate::netcode`;
> those are Phase 1 outputs this draft assumes.

## Goal

Turn Phase 1's full-state, ugly-but-connected demo into the replication substrate later
gameplay phases can trust: per-entity delta state sync with acked baselines, server-clock
tracking, remote interpolation with a measured jitter buffer, join-in-progress convergence,
and disconnect cleanup. The observable bar is a dumb server-authoritative mover that stays
smooth at 150 ms RTT + 5% loss + jitter, including a mid-session join and a dropped client.

## Scope

### In scope
- Extend `postretro-net`'s Phase 1 snapshot wire with per-entity lifecycle records:
  spawn, delta update, full-baseline refresh, and despawn.
- Add separate client -> server ack and baseline-refresh request messages on the reserved
  input/timing channel.
- Server-side replication state: `NetworkId` registry, per-client acked baselines, dirty /
  resend tracking, snapshot sequence numbers, and monotonic server tick stamps.
- Dedicated wire-mirror payloads for Phase 2's minimum replicable component set:
  `Transform` and the mutable tick subset of `PlayerMovementComponent`. Descriptor params,
  render-only view-feel, and bound IR programs stay local data.
- Client-side `NetworkId -> EntityId` map with apply rules for spawn, update, despawn,
  duplicate packets, old packets, and unknown baselines.
- Time-sync over the reserved input/timing channel: client estimates server tick offset and
  smoothed RTT/jitter, then exposes a bounded server-time estimate to interpolation and
  later prediction code.
- Remote interpolation buffer for non-local entities using existing previous/current
  transform interpolation semantics. Interpolation delay is sized from measured jitter and
  clamped to a configured min/max.
- Join-in-progress: a newly accepted client receives a full baseline before deltas, then
  converges to ordinary delta flow.
- Player leave/disconnect lifecycle: clean disconnect and timeout both free the player slot
  and immediately despawn the remote pawn through game-logic-owned apply.
- A dumb AI-less server-authoritative mover fixture that proves replication without M10
  enemies. It may be a dev/test entity spawned by the host, not a new authored gameplay
  archetype.
- Latency harness coverage using Phase 1's conditioned in-memory relay, plus a manual
  `tc netem` loopback soak recipe if the Phase 1 promotion doc provides one.

### Out of scope
- Local-player prediction, rewind, replay, reconciliation smoothing, and command-frame
  ownership transfer. Phase 3 consumes the time-sync and snapshot substrate here.
- Co-op set-piece policy, respawn policy, trigger ownership, and real M10 enemy validation.
  Phase 4 consumes the lifecycle substrate here.
- Server-authoritative hitscan damage, projectile prediction, predicted-to-confirmed entity
  handoff, interest management, and 16-player bandwidth budgeting.
- Steam transport, NAT punchthrough, matchmaking, save/load of networked sessions.
- Replicating script execution or audio events. Clients derive presentation from replicated
  state and local confirmation events.

## Acceptance criteria

- [ ] A server sends per-client delta snapshots against each client's last acked per-entity
  baseline. Dropping one unreliable snapshot does not require a global full-state resend;
  only affected entities refresh or resend until acked. (Tasks 1–2)
- [ ] Snapshot decoding and apply are deterministic and panic-free for corrupt, duplicate,
  out-of-order, old, unknown-baseline, and missing-entity packets. Invalid packets are
  ignored or request a full-baseline refresh; they never mutate unrelated entities. Tests
  assert the registry mutation set, pending repair set, baseline table, and sequence
  tracking after the bad input. (Tasks 1–3)
- [ ] A client connecting after the host has already spawned and moved the dumb mover first
  receives a full baseline, then converges to delta updates with the correct
  `NetworkId -> EntityId` mapping. (Tasks 2–3)
- [ ] A cleanly disconnected client and a timed-out client both free their slot and run the
  immediate remote-pawn despawn path. Other connected clients receive the lifecycle update.
  Despawn tombstones resend until explicitly acked. (Task 4)
- [ ] Client clock sync tracks server tick within a stated bound under the Phase 1 latency
  harness at 150 ms RTT + 5% loss + jitter. Harness profile: deterministic seed `0x1502`,
  75 ms one-way base delay, +/- 30 ms one-way jitter, and 5% packet loss. Starting bound:
  within 2 sim ticks after 5 seconds of simulated time or 20 successful samples, whichever
  comes later. (Task 5)
- [ ] A remote server-authoritative mover renders smoothly at 150 ms RTT + 5% loss +
  jitter. Automated tests assert interpolation delay clamping, sample lookup by server
  tick, bounded extrapolation for at most 100 ms, then hold-last-pose. Manual loopback with
  the same profile is the observational smoothness gate. (Task 6)
- [ ] Snapshot application remains game-logic-owned: `postretro-net` stores wire and
  replication state but never imports or mutates `EntityRegistry`; engine glue converts and
  applies through `spawn`, `set_component_value`, and `despawn`. This is a module-structure
  review/grep gate: `postretro-net` has no `postretro`, `EntityRegistry`, or `EntityId`
  dependency. (Tasks 2–4)
- [ ] Malformed wire coverage includes corrupt bitcode and invalid explicit record /
  component kind values at the decode boundary, plus unknown baselines and duplicate/old
  packets at the semantic apply boundary. Tests assert ignored/request-refresh behavior
  without unrelated registry mutation. (Tasks 1–3)
- [ ] `cargo test -p postretro-net` and the focused `postretro` sim/netcode tests pass.
  Manual loopback with `--host` / `--connect` shows the mover, join-in-progress, and
  disconnect behavior. (All tasks)

## Tasks

### Task 1: Extend snapshot wire for delta + lifecycle
In `postretro-net`, evolve the Phase 1 full-state snapshot into a versioned snapshot
message with fields `version: u16`, `sequence: u32`, `server_tick: u32`, and
`records: Vec<EntityRecord>`. Records cover spawn/full baseline, delta update, despawn,
and full-baseline refresh response. The logical record variants carry these fields:
`FullBaseline { network_id: u32, baseline_id: u32, components: Vec<ComponentPayload> }`,
`Delta { network_id: u32, baseline_ref: u32, new_baseline_id: u32,
components: Vec<ComponentPayload> }`, and
`Despawn { network_id: u32, tombstone_id: u32, reason: u8 }`. A full-baseline refresh
response is encoded as a `FullBaseline` record. Keep `NetworkId` server-assigned
monotonic `u32` from Phase 1.
Add native `bitcode::Encode/Decode` mirror types for the Phase 2 component set:
`WireTransform`, `WireMovementState`, and a `WirePlayerMovementState`. `WireTransform`
uses `position: [f32; 3]`, `rotation: [f32; 4]`, `scale: [f32; 3]`.
`WireMovementState` variants are `Normal`, `Dash { elapsed_ms: f32, boost: [f32; 3] }`,
and `Crouching { eye_current: f32 }`. `WirePlayerMovementState` contains only mutable tick
state: `velocity: [f32; 3]`, `is_grounded: bool`, `air_jumps_remaining: u32`,
`air_dashes_remaining: u32`, `dash_cooldown_ms: f32`, `air_ticks: u32`,
`movement_state: WireMovementState`, `coyote_timer_ms: f32`,
`jump_buffer_timer_ms: f32`, `jump_spent: bool`, `capsule_half_height: f32`, and
`capsule_eye_height: f32`. Do not wire descriptor-immutable movement params, `view_feel`,
`standing_*`, `stuck_stop_*`, or `dash_programs`.
Use explicit numeric `record_kind: u16` and `component_kind: u16` fields at the encoded
boundary for record/component dispatch, then validate them into typed records/payloads.
Malformed decode tests cover corrupt bitcode bytes, invalid explicit record/component kind
values, and unknown component payload variants. Corrupt bitcode decodes to `Err`; invalid
kind values decode cleanly into the raw envelope but are rejected before registry apply.

### Task 2: Server replication state and acked baselines
Add the server-side replication tracker in `postretro-net`: per-client connection state,
last received ack, per-entity acked baseline, per-client despawn tombstones, pending dirty
state, and resend/full-refresh flags. The tracker accepts engine-produced component
snapshots after each server tick and emits one snapshot message per client at 20 Hz (every
third 60 Hz sim tick) for Phase 2. Delta is per entity: an unacked or lost packet only
affects that entity's next encoding.
Define the equality / delta granularity at the wire-mirror level, not by serializing
`ComponentValue` and diffing bytes. Keep the owned post-tick snapshot buffer rule: engine
glue borrows the registry once, copies replicable state into owned wire mirrors, releases
the borrow, then the net crate encodes per client.
Engine glue owns `EntityId <-> NetworkId`: it holds a `postretro-net` allocator handle for
monotonic `NetworkId`s, then passes owned snapshots keyed only by `NetworkId` to
`postretro-net`. `postretro-net` never sees `EntityId`.
Ack messages are client -> server on the reserved input/timing channel, latest-wins, with
fields `latest_snapshot_sequence: u32`, `acked_server_tick: u32`,
`entity_baselines: Vec<(network_id: u32, baseline_id: u32)>`, and
`despawn_tombstones: Vec<(network_id: u32, tombstone_id: u32)>`. Baseline refresh requests
are client -> server on the same channel with fields `snapshot_sequence: u32`,
`network_id: u32`, `missing_baseline_ref: u32`, and `reason: u8`. Task 2 defines the wire
and server handling for refresh requests; Task 3 owns the client pending repair set and
resend cadence. Server -> client baseline refresh responses are `FullBaseline` snapshot
records on the unreliable snapshot channel. Despawn tombstones resend in snapshots until
the client ack names the tombstone.
After Phase 1 lands, replace this paragraph's channel/module references with the exact
module names from `context/lib/networking.md` if they differ from the Phase 1 plan.

### Task 3: Client apply, baseline repair, and join-in-progress
Extend `crate::netcode` engine glue from Phase 1. It owns `NetworkId -> EntityId`, local
spawn/despawn, component conversion, baseline repair decisions, and the client pending
repair set. On first sight of a spawn/full-baseline record for an unmapped `NetworkId`,
spawn an entity with `Transform`, apply all valid present component payloads, and record
the map. Phase 2's dumb mover is `Transform`-only; `WirePlayerMovementState` exists for
the replication substrate and later prediction work. Apply `PlayerMovementState` only to
an entity that already has a local descriptor-derived `PlayerMovementComponent`; do not
construct a full movement component from the mutable wire subset alone. For an unmapped
full baseline that contains `PlayerMovementState` but no local construction source, apply
the `Transform`, ignore the movement payload, and record a typed ignored-payload diagnostic.
When the `NetworkId` is already mapped, a full baseline replaces the stored baseline and
updates existing replicated components without respawning. If a mapped `EntityId` is stale
or missing, remove the stale mapping, add the `NetworkId` to the pending repair set, request
a full refresh, and leave unrelated registry state untouched. On delta, apply only when the
client has the referenced baseline; otherwise add the entity to the pending repair set,
request a full refresh, and leave current state untouched. Clients resend one
`BaselineRefreshRequest` per pending entity at 5 Hz until the matching full baseline
arrives. Old or duplicate snapshot sequence numbers are ignored. A joiner starts with no
baselines, receives full baseline records for relevant live entities, acks them, then enters
delta flow.
Tests cover unknown baselines, duplicate/old packets, missing mapped entities, unknown
component kinds, missing `Transform` on spawn/full-baseline, duplicate component payloads,
and deltas with empty component lists. An unmapped spawn/full-baseline without `Transform`
is invalid and does not spawn; duplicate component payloads in one record reject that
record; an empty delta is a no-op only if its baseline reference is known. Assertions prove
unrelated registry state is unchanged and check the registry mutation set, pending repair
set, baseline table, and sequence tracking.

### Task 4: Connection lifecycle and remote pawn cleanup
Model client slots explicitly. `postretro-net` tracks connection/client slot state and
closed/timeout transitions; `crate::netcode` owns the slot -> remote pawn
`EntityId`/`NetworkId` mapping and invokes `EntityRegistry::despawn` through the
game-logic-owned apply path. A clean disconnect and a timeout both transition the slot to
closed, stop accepting input/snapshot messages from that peer, and run one cleanup path for
that client's remote pawn. Phase 2 cleanup is immediate despawn. This is a mechanics
substrate, not the co-op respawn/player-leave policy; Phase 4 may replace the gameplay
policy while reusing the close/timeout machinery. Replicate the resulting despawn to
remaining clients. NetworkId allocation remains session-monotonic and never reuses ids,
even when a client slot is reused. Tests cover clean disconnect, timeout, stale packets
after close, and slot reuse without reusing stale `NetworkId`s.

### Task 5: Time-sync and jitter measurement
Add a lightweight time-sync exchange: client sends local send tick/time, server echoes with
server tick, client records receive time and computes RTT from client-local monotonic
send/receive times. Server tick is sampled at echo. The offset/server-tick estimate uses
the client receive midpoint and echoed server tick; it does not directly compare client and
server monotonic microseconds. Server echo microseconds are telemetry only unless a
same-process test asserts them. Expose a server-tick estimate and a jitter estimate to the
client interpolation code. Use an injectable monotonic clock source for estimator tests and
harness runs; production can wrap the engine's monotonic time source. Starting constants:
sample at 5 Hz; exponential smoothing weight `0.1` for offset and RTT, `0.2` for jitter.
Harness profile: deterministic seed `0x1502`, 75 ms one-way base delay, +/- 30 ms one-way
jitter, and 5% packet loss. Accepted post-convergence clock error is <= 2 sim ticks after
5 seconds of simulated time or 20 successful samples, whichever comes later. The command
stream for local prediction is still unused; this task provides the timing substrate Phase
3 will consume.
Time-sync uses the reserved input/timing channel for client requests and server echoes,
latest-wins, independent of snapshot messages. Empty snapshots may carry snapshot/ack
metadata, but are not the primary time-sync path.

### Task 6: Remote interpolation buffer + Phase 2 demo
Build a per-remote-entity interpolation buffer keyed by `NetworkId`. It stores received
transform samples by server tick and renders remote entities at
`estimated_server_tick - interpolation_delay`. Initial delay formula:
`clamp(100 ms + 2 * measured_jitter, 100 ms, 250 ms)`, rounded up to whole sim ticks. Use
existing `EntityRegistry` previous/current transform semantics when writing the visible
`Transform`; do not bypass render-stage interpolation. If samples run out, extrapolate for
at most 100 ms using last known movement velocity when available, then hold. The demo host
spawns a dumb AI-less mover that follows a deterministic path server-side. Under the
latency harness and manual loopback, a client sees smooth motion at 150 ms RTT + 5% loss +
jitter.
Client netcode samples interpolation buffers inside game logic after network receive/apply
for the frame and before render collectors read entities. For remote presentation
transforms, the game-logic-owned netcode apply stage updates registry presentation state
before the render stage. It writes through engine glue; the renderer remains read-only. Add
an `EntityRegistry` helper for remote presentation writes that updates the current visible
`Transform` while setting the previous transform to the last presented remote pose, so the
render-stage `interpolated_transform` path does not double-smooth or lose continuity. Use
that helper instead of directly overwriting only the current `Transform`.
The demo mover is a host-only Phase 2 net demo fixture owned by `crate::netcode` or sim
test support, activated only for the Phase 2 net demo/harness path. Do not add a new
authored gameplay archetype or script/FGD surface. Automated tests cover delay clamp,
sample interpolation by server tick, extrapolation cutoff at 100 ms, and hold-last-pose
after starvation; manual loopback remains the observational smoothness check.

## Sequencing

**Phase 1 (sequential):** Task 1 — wire record shape and component mirrors.
**Phase 2 (sequential):** Task 2 — server replication state consumes Task 1 records.
**Phase 3 (concurrent):** Task 3 and Task 5 — client apply/join flow and time-sync both
consume Task 1/2 messages but touch distinct logic.
**Phase 4 (sequential):** Task 4 — lifecycle consumes Task 3's mapping/apply behavior.
**Phase 5 (sequential):** Task 6 — interpolation and demo consume Tasks 3–5.

## Rough sketch

Phase 1 creates `postretro-net` and a `crate::netcode` engine glue module. Phase 2 should
keep that split: `postretro-net` knows `NetworkId`, wire mirrors, baselines, acks, channels,
and client/server replication state; `crate::netcode` knows `EntityRegistry`,
`ComponentKind`, `ComponentValue`, and the engine's spawn/apply/despawn rules.

`simulate_tick` already exists in `crates/postretro/src/sim/mod.rs` with
`SimCommand { movement, fire_button }` and a post-movement aim callback. Phase 2 does not
change its signature. The host runs the seam, snapshots owned replicable state after each
server tick, and feeds that data into `postretro-net`. Clients in Phase 2 are still pure
remote-state viewers for their local pawn; local prediction starts in Phase 3.

`EntityRegistry` iterates component columns in slot order. Use that deterministic order when
building owned snapshots, but do not send raw `EntityId`. Engine glue owns the
`EntityId <-> NetworkId` mapping and allocation flow: it holds a `postretro-net` allocator
handle for monotonic `NetworkId`s, then gives `postretro-net` owned snapshots keyed by
`NetworkId` only. Server `NetworkId`s are session monotonic and never recycled. Client
`EntityId`s are local handles only. `postretro-net` never imports or stores `EntityId`.

For `PlayerMovementComponent`, the wire mirror should be explicit rather than a copy of the
component struct. The component contains descriptor params, render-only view-feel, and
derived IR-bound dash programs that should never be authoritative wire state. Phase 2 only
needs enough mutable tick state for interpolation, later prediction reconciliation, and
disconnect cleanup: `velocity`, `is_grounded`, `air_jumps_remaining`,
`air_dashes_remaining`, `dash_cooldown_ms`, `air_ticks`, the active `movement_state`
(`WireMovementState::Normal`, `Dash { elapsed_ms, boost }`, or
`Crouching { eye_current }`), `coyote_timer_ms`,
`jump_buffer_timer_ms`, `jump_spent`, and live crouch capsule presentation
(`capsule.half_height`, `capsule.eye_height`). Descriptor-owned capsule values, movement
tuning params, `view_feel`, `standing_*`, `stuck_stop_*`, and `dash_programs` stay local.

## Boundary inventory

Netcode crosses **Rust ↔ wire** only. Scripts and FGD do not observe replication.

| Name | Rust (`postretro-net`) | Wire (bitcode) | Engine side (`postretro`) |
|---|---|---|---|
| Network entity id | `NetworkId(u32)` | `u32`, never recycled in session | mapped to/from local `EntityId` |
| Snapshot sequence | snapshot `u32` | `u32`, monotonically increasing per server | drops old/duplicate packets |
| Server tick | `u32` tick stamp | `u32` | feeds interpolation/time-sync |
| Transform payload | `WireTransform` | position `[f32; 3]`, rotation `[f32; 4]`, scale `[f32; 3]` | `ComponentValue::Transform` |
| Movement payload | `WirePlayerMovementState` | explicit mutable tick fields | merged into `PlayerMovementComponent` |
| Lifecycle record | spawn/update/despawn/full-refresh typed records | explicit `record_kind: u16` plus native bitcode payload structs | game-logic-owned apply/despawn |
| Ack | last received snapshot + per-entity baseline refs + despawn tombstone refs | latest-wins message on the reserved input/timing channel | advances server per-client baselines and retires tombstones |
| Time-sync sample | ping/echo structs | local send stamp + server tick echo; server echo microseconds are telemetry | clock estimator, jitter estimator |

## Wire format

Binary surface remains `bitcode`-owned. Phase 2 adds these constraints:

- Server -> client snapshots travel on the unreliable snapshot channel. They are
  self-contained enough to ignore out-of-order delivery. Each carries server tick and
  sequence.
- Client -> server ack messages travel on the reserved input/timing channel, latest-wins.
- Client -> server baseline-refresh requests travel on the reserved input/timing channel,
  latest-wins per packet. Client owns a pending repair set and resends entries at 5 Hz until
  the matching full baseline arrives.
- Server -> client full baseline / refresh responses are snapshot records.
- Reliable control stays for connection and lifecycle control unless a later spec explicitly
  extends it.
- Delta records name the baseline they require. Missing baseline means request full refresh,
  not best-effort patch.
- Empty snapshot record list is valid for snapshot/ack metadata. Time-sync uses its own
  input/timing exchange and does not depend on empty snapshots.
- Despawn is idempotent. Server keeps per-client despawn tombstones and resends them in
  snapshots until `AckMessage` explicitly acks the tombstone. Repeated despawn for an
  unknown `NetworkId` is ignored.
- Full baseline is the join/repair format. It replaces any previous baseline for that
  entity on the receiving side. If the `NetworkId` is unmapped, it spawns; if mapped, it
  updates existing replicated components without respawning.
- No serde-tagged `ComponentValue` crosses the wire. Every payload is a native bitcode
  wire mirror with explicit component kind/discriminant.
- Record and component dispatch uses explicit numeric `record_kind: u16` and
  `component_kind: u16` fields at the encoded boundary so invalid kind values are testable
  without relying on `bitcode`'s internal enum representation.
- Snapshot cadence is 20 Hz in Phase 2. The 60 Hz sim still produces the source state; the
  net layer sends every third tick.
- Acks are latest-wins and sent on the reserved input/timing channel. Lost acks are
  harmless because the next ack supersedes them.

### Phase 2 wire schema

| Message / record | Direction | Channel | Fields |
|---|---|---|---|
| `SnapshotMessage` | server -> client | unreliable snapshot | `version: u16`, `sequence: u32`, `server_tick: u32`, `records: Vec<EntityRecord>` |
| `EntityRecord::FullBaseline` | server -> client | unreliable snapshot | `network_id: u32`, `baseline_id: u32`, `components: Vec<ComponentPayload>` |
| `EntityRecord::Delta` | server -> client | unreliable snapshot | `network_id: u32`, `baseline_ref: u32`, `new_baseline_id: u32`, `components: Vec<ComponentPayload>` |
| `EntityRecord::Despawn` | server -> client | unreliable snapshot | `network_id: u32`, `tombstone_id: u32`, `reason: u8` |
| `ComponentPayload::Transform` | server -> client | snapshot record payload | `position: [f32; 3]`, `rotation: [f32; 4]`, `scale: [f32; 3]` |
| `ComponentPayload::PlayerMovementState` | server -> client | snapshot record payload | `velocity: [f32; 3]`, `is_grounded: bool`, `air_jumps_remaining: u32`, `air_dashes_remaining: u32`, `dash_cooldown_ms: f32`, `air_ticks: u32`, `movement_state: WireMovementState`, `coyote_timer_ms: f32`, `jump_buffer_timer_ms: f32`, `jump_spent: bool`, `capsule_half_height: f32`, `capsule_eye_height: f32` |
| `WireMovementState` | server -> client | movement payload field | `Normal`, `Dash { elapsed_ms: f32, boost: [f32; 3] }`, `Crouching { eye_current: f32 }` |
| `AckMessage` | client -> server | reserved input/timing | `latest_snapshot_sequence: u32`, `acked_server_tick: u32`, `entity_baselines: Vec<(network_id: u32, baseline_id: u32)>`, `despawn_tombstones: Vec<(network_id: u32, tombstone_id: u32)>` |
| `BaselineRefreshRequest` | client -> server | reserved input/timing | `snapshot_sequence: u32`, `network_id: u32`, `missing_baseline_ref: u32`, `reason: u8` |
| `TimeSyncRequest` | client -> server | reserved input/timing | `sample_id: u32`, `client_send_tick: u32`, `client_send_time_us: u64` |
| `TimeSyncEcho` | server -> client | reserved input/timing | `sample_id: u32`, `client_send_tick: u32`, `client_send_time_us: u64`, `server_tick: u32`, `server_echo_time_us: u64` telemetry |

## Resolved decisions

- **Snapshot cadence:** 20 Hz for Phase 2. This gives one snapshot every three 60 Hz sim
  ticks, enough to validate interpolation without starting the milestone at the Phase 7
  bandwidth budget.
- **Initial interpolation delay:** `clamp(100 ms + 2 * measured_jitter, 100 ms, 250 ms)`,
  rounded up to whole sim ticks.
- **Clock-sync constants:** 5 Hz sample cadence; smoothing weight `0.1` for offset/RTT and
  `0.2` for jitter; target post-convergence error <= 2 sim ticks under deterministic seed
  `0x1502`, 75 ms one-way base delay, +/- 30 ms one-way jitter, and 5% packet loss, measured
  after 5 seconds of simulated time or 20 successful samples, whichever comes later.
- **NetworkId allocation:** engine glue holds a `postretro-net` allocator handle for
  monotonic server-assigned ids. The net crate never sees `EntityId`.
- **Ack channel:** latest-wins ack messages use the reserved input/timing channel even
  before gameplay input is applied. They also ack despawn tombstones. Reliable control stays
  for lifecycle/control messages.
- **Message families:** snapshots are server -> client on the unreliable snapshot channel;
  acks, refresh requests, and time-sync requests/echoes use the reserved input/timing
  channel; refresh responses are full-baseline snapshot records.
- **Refresh repair:** client owns a per-client pending repair set and resends one
  `BaselineRefreshRequest` per pending entity at 5 Hz until the matching full baseline
  arrives. Requests are additive on the server; a later request does not clear other pending
  repairs.
- **Disconnect cleanup:** Phase 2 immediately despawns the disconnected client's remote
  pawn. Co-op respawn or spectate policy belongs to Phase 4.
- **Despawn reliability:** server retains per-client despawn tombstones and resends them in
  snapshots until `AckMessage.despawn_tombstones` acknowledges the matching `tombstone_id`.
- **Movement wire payload:** explicit mutable tick state only:
  `velocity`, `is_grounded`, `air_jumps_remaining`, `air_dashes_remaining`,
  `dash_cooldown_ms`, `air_ticks`, `WireMovementState`, `coyote_timer_ms`,
  `jump_buffer_timer_ms`, `jump_spent`, `capsule.half_height`, and `capsule.eye_height`.
  Phase 2 mirrors source field types: ability counters and `air_ticks` are `u32`; live
  timers are `f32`; no quantization yet.
- **Time-sync domain:** client RTT uses client-local monotonic send/receive times. Server
  tick estimate uses receive midpoint plus echoed server tick. Server echo microseconds are
  telemetry only outside same-process tests.

## Open questions

- None for the draft. Re-review may still reshape details before promotion.
