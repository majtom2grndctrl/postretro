# M15 Phase 2 — Replication, Time Sync, Interpolation, Lifecycle

> Milestone 15 (multiplayer co-op netcode), Phase 2. Design reference:
> `context/research/netcode/` (`index.md` Phase 2 + Wire format,
> `crate-pattern-research.md` snapshot interpolation / per-entity delta). Consumes the
> Phase 1 transport, wire, handshake, full-state Transform demo, and `postretro-net`
> boundary from `context/plans/ready/M15--p1-transport-wire-handshake/`.

## Goal

Turn Phase 1's full-state, ugly-but-connected demo into the replication substrate later
gameplay phases can trust: per-entity delta state sync with acked baselines, server-clock
tracking, remote interpolation with a measured jitter buffer, join-in-progress convergence,
and disconnect cleanup. The observable bar is a dumb server-authoritative mover that stays
smooth at 150 ms RTT + 5% loss + jitter, including a mid-session join and a dropped client.

## Scope

### In scope
- Extend `postretro-net`'s Phase 1 snapshot wire with per-entity lifecycle records:
  spawn, delta update, full-baseline refresh, despawn, and per-client ack.
- Server-side replication state: `NetworkId` registry, per-client acked baselines, dirty /
  resend tracking, snapshot sequence numbers, and monotonic server tick stamps.
- Dedicated wire-mirror payloads for Phase 2's minimum replicable component set:
  `Transform` and the mutable tick subset of `PlayerMovementComponent`. Descriptor params,
  render-only view-feel, and bound IR programs stay local data.
- Client-side `NetworkId -> EntityId` map with apply rules for spawn, update, despawn,
  duplicate packets, old packets, and unknown baselines.
- Time-sync over the reliable/control or input channel: client estimates server tick offset
  and smoothed RTT/jitter, then exposes a bounded server-time estimate to interpolation and
  later prediction code.
- Remote interpolation buffer for non-local entities using existing previous/current
  transform interpolation semantics. Interpolation delay is sized from measured jitter and
  clamped to a configured min/max.
- Join-in-progress: a newly accepted client receives a full baseline before deltas, then
  converges to ordinary delta flow.
- Player leave/disconnect lifecycle: clean disconnect and timeout both free the player slot
  and despawn or deactivate the remote pawn through game-logic-owned apply.
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
  ignored or request a full-baseline refresh; they never mutate unrelated entities. (Tasks
  1–3)
- [ ] A client connecting after the host has already spawned and moved the dumb mover first
  receives a full baseline, then converges to delta updates with the correct
  `NetworkId -> EntityId` mapping. (Tasks 2–3)
- [ ] A cleanly disconnected client and a timed-out client both free their slot and run the
  selected remote-pawn cleanup path. Other connected clients receive the lifecycle update.
  (Task 4)
- [ ] Client clock sync tracks server tick within a stated bound under the Phase 1 latency
  harness at 150 ms RTT + 5% loss + jitter. Starting bound: within 2 sim ticks after
  convergence. (Task 5)
- [ ] A remote server-authoritative mover renders smoothly at 150 ms RTT + 5% loss +
  jitter. Interpolation delay is derived from measured jitter and clamped; packet starvation
  uses bounded extrapolation only briefly, then holds the last known pose. (Task 6)
- [ ] Snapshot application remains game-logic-owned: `postretro-net` stores wire and
  replication state but never imports or mutates `EntityRegistry`; engine glue converts and
  applies through `spawn`, `set_component_value`, and `despawn`. (Tasks 2–4)
- [ ] `cargo test -p postretro-net` and the focused `postretro` sim/netcode tests pass.
  Manual loopback with `--host` / `--connect` shows the mover, join-in-progress, and
  disconnect behavior. (All tasks)

## Tasks

### Task 1: Extend snapshot wire for delta + lifecycle
In `postretro-net`, evolve the Phase 1 full-state snapshot into a versioned snapshot
message carrying a server tick, snapshot sequence, per-client ack metadata, and a list of
entity records. Records cover spawn/full baseline, delta update, despawn, and baseline
refresh request/response. Keep `NetworkId` server-assigned monotonic `u32` from Phase 1.
Add native `bitcode::Encode/Decode` mirror types for the Phase 2 component set:
`WireTransform` and a `WirePlayerMovementState` that contains only mutable tick state:
velocity, grounded/air counters, movement state, live ability charges, dash cooldown,
crouch/dash live state, and jump-forgiveness timers. Do not wire descriptor-immutable
movement params, `view_feel`, or `dash_programs`.

### Task 2: Server replication state and acked baselines
Add the server-side replication tracker in `postretro-net`: per-client connection state,
last received ack, per-entity acked baseline, pending dirty state, and resend/full-refresh
flags. The tracker accepts engine-produced component snapshots after each server tick and
emits one snapshot message per client at 20 Hz (every third 60 Hz sim tick) for Phase 2.
Delta is per entity: an unacked or lost packet only affects that entity's next encoding.
Define the equality / delta granularity at the wire-mirror level, not by serializing
`ComponentValue` and diffing bytes. Keep the owned post-tick snapshot buffer rule: engine
glue borrows the registry once, copies replicable state into owned wire mirrors, releases
the borrow, then the net crate encodes per client.

### Task 3: Client apply, baseline repair, and join-in-progress
Extend `crate::netcode` engine glue from Phase 1. It owns `NetworkId -> EntityId`, local
spawn/despawn, component conversion, and baseline repair decisions. On first sight of a
spawn/full-baseline record, spawn an entity with `Transform`, apply all present component
payloads, and record the map. On delta, apply only when the client has the referenced
baseline; otherwise request a full refresh and leave current state untouched. Old or
duplicate snapshot sequence numbers are ignored. A joiner starts with no baselines, receives
full baseline records for relevant live entities, acks them, then enters delta flow.

### Task 4: Connection lifecycle and remote pawn cleanup
Model client slots explicitly. A clean disconnect and a timeout both transition the slot to
closed, stop accepting input/snapshot messages from that peer, and run one game-logic-owned
cleanup path for that client's remote pawn. Phase 2 cleanup is immediate despawn. This is a
mechanics substrate, not the co-op respawn/player-leave policy; Phase 4 may replace the
gameplay policy while reusing the close/timeout machinery. Replicate the resulting despawn
to remaining clients. Tests cover clean disconnect, timeout, stale packets after close, and
slot reuse without reusing stale `NetworkId`s.

### Task 5: Time-sync and jitter measurement
Add a lightweight time-sync exchange: client sends local send tick/time, server echoes with
server tick, client records receive time and computes offset, RTT, and jitter using smoothed
estimators. Expose a server-tick estimate and a jitter estimate to the client interpolation
code. Starting constants: sample at 5 Hz; exponential smoothing weight `0.1` for offset and
RTT, `0.2` for jitter; accepted post-convergence clock error <= 2 sim ticks under the Phase
1 harness profile. The command stream for local prediction is still unused; this task
provides the timing substrate Phase 3 will consume.

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
building owned snapshots, but do not send raw `EntityId`. Server `NetworkId`s are session
monotonic and never recycled. Client `EntityId`s are local handles only.

For `PlayerMovementComponent`, the wire mirror should be explicit rather than a copy of the
component struct. The component contains descriptor params, render-only view-feel, and
derived IR-bound dash programs that should never be authoritative wire state. Phase 2 only
needs enough mutable tick state for interpolation, later prediction reconciliation, and
disconnect cleanup: `velocity`, `is_grounded`, `air_jumps_remaining`,
`air_dashes_remaining`, `dash_cooldown_ms`, `air_ticks`, the active `movement_state`
(`Normal`, `Dash { elapsed_ms, boost }`, or `Crouching { eye_current }`), `coyote_timer_ms`,
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
| Lifecycle record | spawn/update/despawn/full-refresh variants | native `Encode/Decode` enum | game-logic-owned apply/despawn |
| Ack | last received snapshot + per-entity baseline refs | unreliable latest-wins ack message on the Phase 1 input-command channel | advances server per-client baselines |
| Time-sync sample | ping/echo structs | local send stamp + server tick echo | clock estimator, jitter estimator |

## Wire format

Binary surface remains `bitcode`-owned. Phase 2 adds these constraints:

- Snapshot messages are self-contained enough to ignore out-of-order delivery. Each carries
  server tick and sequence.
- Delta records name the baseline they require. Missing baseline means request full refresh,
  not best-effort patch.
- Empty record list is valid: it can carry tick/ack/time-sync data.
- Despawn is idempotent. Repeated despawn for an unknown `NetworkId` is ignored.
- Full baseline is the join/repair format. It replaces any previous baseline for that
  entity on the receiving side.
- No serde-tagged `ComponentValue` crosses the wire. Every payload is a native bitcode
  wire mirror with explicit component kind/discriminant.
- Snapshot cadence is 20 Hz in Phase 2. The 60 Hz sim still produces the source state; the
  net layer sends every third tick.
- Acks are latest-wins and sent on the reserved input-command channel. Lost acks are
  harmless because the next ack supersedes them.

## Resolved decisions

- **Snapshot cadence:** 20 Hz for Phase 2. This gives one snapshot every three 60 Hz sim
  ticks, enough to validate interpolation without starting the milestone at the Phase 7
  bandwidth budget.
- **Initial interpolation delay:** `clamp(100 ms + 2 * measured_jitter, 100 ms, 250 ms)`,
  rounded up to whole sim ticks.
- **Clock-sync constants:** 5 Hz sample cadence; smoothing weight `0.1` for offset/RTT and
  `0.2` for jitter; target post-convergence error <= 2 sim ticks under the harness profile.
- **Ack channel:** latest-wins ack messages use the Phase 1 input-command channel even
  before gameplay input is applied. Reliable control stays for lifecycle/control messages.
- **Disconnect cleanup:** Phase 2 immediately despawns the disconnected client's remote
  pawn. Co-op respawn or spectate policy belongs to Phase 4.
- **Movement wire payload:** explicit mutable tick state only:
  `velocity`, `is_grounded`, `air_jumps_remaining`, `air_dashes_remaining`,
  `dash_cooldown_ms`, `air_ticks`, `movement_state`, `coyote_timer_ms`,
  `jump_buffer_timer_ms`, `jump_spent`, `capsule.half_height`, and `capsule.eye_height`.

## Open questions

- None for the draft. Re-review may still reshape details before promotion.
