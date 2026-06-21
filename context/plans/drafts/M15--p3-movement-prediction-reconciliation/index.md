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
  latest client command tick the host applied before producing that pawn state.
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
  descriptor-backed `PlayerMovement` pawn when the level/mod provides one. Test harnesses
  may keep an explicit Transform-only fallback, but that fallback is not predicted.
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
  through the existing `simulate_tick` / movement seam.
- New movement mechanics or movement tuning changes.
- Moving platforms or kinematic carry behavior.

## Acceptance criteria

- [ ] A connected client sends one command frame per predicted fixed tick. Each command has
  a monotonic `client_tick`, round-trips through `ClientMessage::Input`, and survives
  duplicate / old / malformed input packets without host panic or unrelated entity mutation.
- [ ] The host applies accepted client commands to that client's authoritative
  descriptor-backed `PlayerMovement` pawn in command-tick order. Missing command ticks hold
  the last known intent for at most a configured small window, then fall back to neutral
  input. Old commands are dropped.
- [ ] Host snapshots for movement pawns include `Transform`, `PlayerMovementState`, and the
  latest processed `client_tick` for that pawn. The payload stays registry-blind inside
  `postretro-net`; engine glue owns all registry reads and writes.
- [ ] The client predicts its local pawn immediately from the same `SimCommand` it sends to
  the host. At 100 ms RTT with jitter and 5% loss, local walk, jump, crouch, and dash input
  have no added network wait before affecting the camera-followed pawn.
- [ ] On an authoritative movement snapshot, the client restores the acked
  `Transform` + mutable `PlayerMovementComponent` state, prunes commands through the echoed
  `client_tick`, replays remaining commands, and leaves unrelated replicated entities on
  the existing remote interpolation path.
- [ ] Normal corrections are smoothed rather than snap-teleported. A mispredicted dash under
  the Phase 2 latency harness profile produces bounded visible correction; a respawn or
  large authoritative displacement is classified as a teleport and snaps by resetting the
  prediction history.
- [ ] `crates/postretro/src/main.rs`, `crates/postretro/src/movement/mod.rs`, and
  `crates/postretro/src/netcode/client.rs` are not grown with the core prediction state.
  New production prediction/reconciliation code lives in new focused netcode/sim modules
  or is split before extension.
- [ ] `cargo test -p postretro-net` and focused `postretro` sim/netcode tests pass.
  Manual loopback with host/client shows immediate local movement, smooth remote movement,
  and no duplicate local pawn under join/disconnect.

## Tasks

### Task 1: Command-frame wire and protocol bump

Extend `crates/net/src/wire.rs` so `InputCommand` carries `client_tick: u32`. Extend the
snapshot raw/typed record model with optional movement authority metadata for a movement
entity: `last_processed_client_tick: Option<u32>`. Bump `PROTOCOL_ID`, `WIRE_VERSION`, and
the snapshot version because both input and snapshot bitcode layouts change. Keep bitcode
as the only binary encoding owner; invalid record metadata must decode into the raw
envelope and fail validation before engine apply. Add wire tests for command tick
round-trip, snapshot metadata round-trip, version rejection, and malformed metadata.

### Task 2: Engine conversion and movement-state merge helpers

In `crate::netcode`, add conversions between `sim::SimCommand` and
`postretro_net::wire::InputCommand`, including `MovementInput.wish_dir`, jump, dash,
running, crouch intent, facing yaw, and `FireButtonState`. Add conversions from
`PlayerMovementComponent` to `WirePlayerMovementState` and an inverse merge that writes
only the mutable tick subset onto an existing descriptor-derived component. The merge must
not construct a full `PlayerMovementComponent` from the wire payload and must not touch
descriptor-owned tuning, `view_feel`, standing dimensions, stuck-stop config, or
`dash_programs`.

### Task 3: Client prediction state

Add a focused prediction module under `crates/postretro/src/netcode/`. It owns the client
command ring and predicted state history for the local `NetworkId` / `EntityId` pawn. Each
predicted fixed tick records the `client_tick`, outbound `InputCommand`, resulting
`Transform`, resulting `PlayerMovementComponent`, and whether replay included dash. The
main loop should delegate to this module instead of storing prediction data on `App` beyond
thin call-site plumbing. Reuse `sim::simulate_tick`; do not add networking branches inside
`movement::tick`.

### Task 4: Host authoritative command queues

Extend host netcode so `host_handle_client_messages` no longer drops
`ClientMessage::Input`. Decode finite commands into per-client queues keyed by client id,
drop old commands, collapse exact duplicates, and apply queued commands to the slot-owned
authoritative movement pawn before host replication snapshots the tick. If no command is
available for the next server tick, hold the previous command for a short configured window,
then use neutral movement input. Accepted clients with no descriptor-backed movement pawn
stay Transform-only and do not enter prediction.

### Task 5: Local reconciliation and smoothing

When `ClientReplication` applies an authoritative record for the local predicted pawn, merge
the `PlayerMovementState` payload onto the existing component, restore the authoritative
`Transform`, prune command history through `last_processed_client_tick`, replay remaining
commands with `simulate_tick`, and write the final predicted state back to the registry.
Small corrections are smoothed over a bounded number of render frames by an engine-side
visual offset or equivalent presentation layer; large corrections are classified as
teleports and snap by clearing prediction history and calling the existing hold/reset path
for previous/current state. Non-local records continue to feed `RemoteInterpolationBuffer`.

### Task 6: Harness and loopback validation

Promote the useful shape from `sim/predict_reconcile.rs` into production-adjacent tests
without wiring the prototype type itself into runtime state. Add in-memory tests for
ordered input, missing input, duplicate input, stale authoritative snapshots, unknown local
mapping, dash correction, and teleport correction. Add a manual loopback recipe using the
Phase 2 latency harness / `tc netem` profile and the campaign-test map or another map with
a descriptor-backed player pawn.

## Sequencing

**Phase 1 (sequential):** Task 1 — wire identity and authority metadata block all replay.

**Phase 2 (concurrent):** Task 2 and Task 3 — conversion helpers and client-side state can
land in parallel once the wire shape is pinned.

**Phase 3 (sequential):** Task 4 — host command application consumes Task 1 and Task 2.

**Phase 4 (sequential):** Task 5 — reconciliation consumes client history, host authority,
and movement-state merge helpers.

**Phase 5 (sequential):** Task 6 — validates the integrated prediction path.

## Rough sketch

Current source already has the right foundation:

- `postretro_net::wire::InputCommand`, `WireMovementInput`, and `WireFireButtonState`
  exist, but `InputCommand` has no command tick.
- `host_handle_client_messages` decodes `ClientMessage::Input` and drops it.
- `produce_owned_snapshots` currently emits only `Transform`; Phase 3 extends it to append
  `PlayerMovementState` for registered movement pawns.
- `ClientReplication::apply_components_to` currently ignores `PlayerMovementState` unless
  a local component exists. Phase 3 replaces that local case with the reconciliation path
  for the predicted pawn while preserving the ignore diagnostic for non-local entities
  without a descriptor-derived component.
- `simulate_tick` is the shared tick seam. Prediction and replay should call it with the
  same `SimCommand` shape the main loop already builds.
- `EntityRegistry::mark_local_player_pawn` and `local_player_pawn` identify the pawn the
  camera follows and movement tick drives. Phase 3 should make the predicted pawn use that
  marker on the client.

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
| Movement authority ack | latest processed client command tick for a movement pawn | optional `last_processed_client_tick` on snapshot entity record | n/a | n/a | n/a |
| Movement state payload | mutable subset of `PlayerMovementComponent` | `ComponentPayload::PlayerMovementState` / `WirePlayerMovementState` | n/a | n/a | n/a |

## Wire format

Bitcode remains the encoding owner. No manual byte layout is introduced.

`InputCommand` gains `client_tick: u32` before or after its existing payload fields; the
exact field order is pinned by tests because bitcode layout changes. A client sends one
`ClientMessage::Input(InputCommand)` per predicted fixed tick on `Channel::Input`.

Snapshot records gain movement-authority metadata. Use an explicit optional shape in the
raw envelope rather than overloading `server_tick`: either `has_last_processed_client_tick:
bool` plus `last_processed_client_tick: u32`, or an equivalent bitcode-supported option
field. Validation rejects metadata on records that do not carry `PlayerMovementState`
unless the implementation deliberately permits it as harmless. The typed apply model
exposes it as `Option<u32>`.

Empty command queues and no-metadata records encode as empty lists / `None`. `NetworkId`,
baseline ids, tombstone ids, and command ticks stay `u32`.

## Open questions

- Descriptor-backed pawn source: production should prefer the existing level/mod player
  descriptor. The implementation must choose the exact accept path that maps an accepted
  client to that descriptor without making Transform-only fixtures look predicted.
- Correction smoothing threshold: start from Phase 0's measured tolerance and the
  prototype's `RECONCILE_EPSILON`, then tune with the dash harness. The spec pins behavior
  classes, not final numeric thresholds.
- Host missing-input policy: the plan chooses short hold-last then neutral. The exact hold
  tick count should be a named constant covered by tests.
