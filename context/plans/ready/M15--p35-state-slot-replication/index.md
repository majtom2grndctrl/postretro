# M15 Phase 3.5 - State Slot Replication

> **Status:** draft.
>
> **Fits between:** Phase 3 movement prediction/reconciliation and Phase 4 server-authored gameplay events.
>
> **Research:** see `research.md` beside this spec.

## Goal

Replicate authoritative state-store slots from the server to accepted clients.
The first visible fix is the connected-client HUD: `player.health` and
`player.maxHealth` must arrive through the same state binding path the UI
already uses in single-player and host mode.

This is not a HUD shortcut. Phase 3.5 creates the general slot projection path
for server-owned gameplay state: owner-private player values now, shared/global
values for objectives and set pieces next, and future engine or
descriptor-defined stats without one-off netcode fields.

## Design

The server remains authoritative. Clients may predict movement, but state-store
slots replicated by this phase are server-written values. A client applies them
through the existing slot table validation path, then the existing UI snapshot
reads the slot table once per frame.

`postretro-net` stays registry-blind and script-blind. It owns only wire-safe
slot identifiers, value payloads, per-client baselines, ack handling, and
validation of hostile wire bytes. `postretro` maps those payloads to
`SlotTable` names and applies them through the scripting/store layer.

State updates piggyback on the existing snapshot cadence and unreliable
snapshot channel. Acks and baseline-refresh requests use the existing reliable
input/control path. Do not add a renet channel for this phase.

Slot identity is schema-driven, not per-stat-field-driven. `postretro` builds a
deterministic replicated-slot schema from committed replicated slot projections:

- include only replicated slots; `none` and local-only slots do not receive a
  `StateSlotId` and do not affect the fingerprint;
- sort replicated slots by stable dotted slot name;
- assign a deterministic `StateSlotId(u16)`;
- include the name, slot type, enum values, replication scope, and validation
  shape in a canonical schema fingerprint;
- pass the opaque fingerprint and ids into `postretro-net`.

`postretro` computes the fingerprint with the workspace `blake3` dependency
over a canonical byte stream: version prefix, sorted replicated slots,
length-prefixed UTF-8 names, explicit type tags, enum values in declared order,
range finite/min/max flags, replication scope tag, and stable little-endian
numeric bytes. `postretro-net` stores and compares the fingerprint as opaque
bytes; it does not need to depend on `blake3`.

The schema describes destination slots and validation, not producer internals.
Allowed producers:

- engine catalog slots;
- mod store slots declared through `defineStore`;
- descriptor-defined gameplay values projected into named slots;
- engine systems that compute values from descriptors or behavior IR.

A descriptor value replicates only after it projects into a named slot with
type, scope, and validation metadata. The network path never sends descriptor
structs or component internals directly.

`postretro` lowers each replicated projection to a wire-safe schema descriptor
for `postretro-net`: slot id, value type, enum values, numeric range, scope,
payload caps, and schema fingerprint. `postretro-net` validates hostile bytes
against that descriptor. It never receives `SlotSchema`, `SlotValue`, scripting
types, or descriptor types.

Supported replication scopes:

- `none`: default for existing slots.
- `ownerPrivatePlayer`: server sends only to the owning accepted client. Phase
  3.5 uses this for engine-owned player slots such as `player.health` and
  `player.maxHealth`.
- `sharedGlobal`: server sends to every accepted client. Phase 3.5 supports
  mod-declared shared slots through `defineStore` metadata.

Value cardinality follows scope:

- `sharedGlobal`: server tracks one value per `StateSlotId`.
- `ownerPrivatePlayer`: server tracks one value per `(StateSlotId,
  owner_client_id)`. The recipient implies the owner on the wire. The client
  writes the value into its local dotted slot, such as `player.health`.

Mod-declared owner-private per-player slots are out of scope until the authoring
surface has a per-player state namespace or owner binding. This keeps Phase 3.5
honest: engine player slots get privacy now, shared/global mod slots get a real
path now, and the future per-player mod surface can reuse the same wire tracker.

## Boundary inventory

| Boundary value | Rust | Wire | TypeScript/Luau | FGD |
| --- | --- | --- | --- | --- |
| Slot identity | `SlotTable` dotted names and deterministic `StateSlotId` map | `StateSlotId(u16)` plus schema fingerprint; names never appear in per-tick deltas | Existing state refs serialize dotted names; `defineStore` keeps dotted names; descriptors project into named slots | None |
| Slot value | `SlotValue` and `Option<SlotValue>` | `WireSlotValue` plus explicit `unset` state | `ReadonlyStateRef<T>` / `WritableStateRef<T>` refs and UI bind descriptors keep existing shapes; engine apply may use `StateValueForSlot` | None |
| Replication scope | New `network` metadata to add to `StoreSlotSchema`, `SlotSchemaInput`, `SlotSchema`, engine catalog, and store declarations | Scope is included in the schema fingerprint, not repeated in each record | `defineStore` accepts `network: "shared"` for mod slots; engine refs expose readonly/player values as before | None |
| Authority | Server-side slot producers, including descriptor-fed producers, and client-side apply path | Server snapshot records only; clients ack and request refresh | No new client write primitive | None |

## Wire format

Extend the existing snapshot envelope instead of adding a sibling message type:

- `RawSnapshotMessage` gains `state_schema_fingerprint: [u8; 32]` and
  `state_records: Vec<RawStateSlotRecord>`.
- Empty `state_records` is valid.
- Every message carrying state records must match the client's local
  fingerprint before any record is applied.
- Bump `SNAPSHOT_VERSION` and the app/wire protocol gate.

Add state records in a new net module rather than expanding
`crates/net/src/wire.rs` directly. `wire.rs` and `replication.rs` are already
over the split-before-extend threshold.

Record shape:

```text
RawStateSlotRecord {
  slot_id: u16,
  kind: fullBaseline | delta,
  baseline_ref: Option<u32>,
  baseline_id: u32,
  value: WireSlotValue,
}
```

`WireSlotValue` is pinned as:

- `unset`;
- `number(f32)`, finite only;
- `boolean(bool)`;
- `string(UTF-8)`, 256-byte cap;
- `enum(UTF-8)`, 256-byte cap and value must match the schema enum values;
- `array(Vec<f32>)`, 64-element cap, finite elements only.

Legal record combinations:

- `fullBaseline`: `baseline_ref` is `None`; `baseline_id` is always the new
  baseline id for this value.
- `delta`: `baseline_ref` is `Some`; `baseline_id` is always the new baseline id
  after applying the delta.

Reject invalid combinations before apply.

Validation rejects:

- unknown `slot_id`;
- schema fingerprint mismatch;
- unknown value kind;
- type mismatch against the local slot schema;
- non-finite numbers or arrays;
- strings over 256 bytes;
- arrays over 64 numbers;
- more than 1024 slot records in one snapshot.

`AckMessage` gains `slot_baselines: Vec<(u16, u32)>`. These acks travel as
`AckMessage` on `ClientMessage::Ack` over `Channel::Input`.

Append a new `ClientMessage` variant for state-slot baseline refresh requests.
The request carries `snapshot_sequence`, `slot_id`, `missing_baseline_ref`, and
reason, and travels as an appended `ClientMessage::StateBaselineRefresh` over
`Channel::Input`. Do not reuse the entity `BaselineRefreshRequest`; entity
baselines are keyed by `NetworkId`, while state baselines are keyed by
`StateSlotId`.

No slot tombstone is needed in Phase 3.5. Slot declarations are process-lived
after commit; clearing a value rides as `unset`.

Bitcode remains live-wire only. Do not persist bitcode payloads or write
replicated client state to `state.json`.

## Tasks

### Task 1: Schema and wire model

Add the replicated slot schema builder in `postretro` and the wire-facing state
types in `postretro-net`.

Work:

- Add replication metadata to store schemas and the engine state catalog. Touch
  `scripting/engine_state_catalog.rs`, `scripting/slot_table.rs`,
  `scripting/primitives/store.rs`, and the TypeScript/Luau typedef generation
  path.
- Add an internal projection adapter for descriptor-defined values that should
  feed named replicated slots. Phase 3.5 adds no new descriptor authoring syntax:
  the first adapter reads `HealthComponent` current/max values from
  descriptor-spawned player pawns.
- Generate deterministic `StateSlotId` mappings from replicated projection
  schemas.
- Lower projection schemas into a `postretro-net` descriptor: slot id, value
  type, enum values, numeric range, replication scope, payload caps, and schema
  fingerprint.
- Compute the `[u8; 32]` schema fingerprint in `postretro` from: version prefix,
  sorted replicated slots, length-prefixed UTF-8 names, explicit type tags, enum
  values in declared order, range finite/min/max flags, scope tag, and stable
  little-endian numeric bytes.
- Add state-slot wire types in a new `crates/net/src/state_slots.rs` module.
  Keep `crates/net/src/wire.rs` as envelope glue only.
- Add `WireSlotValue`: `unset`, finite `number(f32)`, `boolean(bool)`,
  UTF-8 `string` capped at 256 bytes, UTF-8 `enum` capped at 256 bytes and
  schema-validated, and finite `array(Vec<f32>)` capped at 64 elements.
- Add `RawStateSlotRecord`: `slot_id`, `kind`, `baseline_ref`, `baseline_id`,
  and `WireSlotValue`.
- Pin record validation: `fullBaseline` requires `baseline_ref = None`; `delta`
  requires `baseline_ref = Some`; `baseline_id` is always the new baseline id.
  Reject bad combinations before apply.
- Extend `RawSnapshotMessage` with `state_schema_fingerprint` and
  `state_records`, extend `AckMessage` with `slot_baselines`, and append
  `ClientMessage::StateBaselineRefresh { snapshot_sequence, slot_id,
  missing_baseline_ref, reason }`.
- Reject unknown slot ids, schema mismatches, unknown value kinds, type
  mismatches, non-finite numbers/arrays, over-cap strings/arrays, and snapshots
  with more than 1024 state records.
- Add encode/decode and malformed-payload tests.
- Bump snapshot/protocol gates.

Keep `postretro-net` free of `crate::scripting::slot_table::SlotTable`,
`SlotValue`, `EntityRegistry`, `glam`, scripting types, and descriptor types.
Its own connection-lifecycle `slots::SlotTable` may remain internal.

### Task 2: Server-side state replication tracker

Add a sibling tracker to entity replication rather than folding slots into
`ServerReplication`.

Work:

- Track current `Option<WireSlotValue>` per `StateSlotId` for `sharedGlobal`.
- Track current `Option<WireSlotValue>` per `(StateSlotId, owner_client_id)` for
  `ownerPrivatePlayer`.
- Track per-client acked baselines and refresh requests.
- Emit full baselines for newly accepted clients.
- Emit deltas against the client's acked baseline when possible. A state delta
  carries the new complete `WireSlotValue`, the referenced baseline id, and the
  new baseline id. It is not a numeric or semantic diff.
- Fall back to full baselines when a referenced baseline is missing.
- Handle `ClientMessage::StateBaselineRefresh { snapshot_sequence, slot_id,
  missing_baseline_ref, reason }` and schedule a full baseline for that slot.
- Filter `ownerPrivatePlayer` slots to the owning client.
- Emit `sharedGlobal` slots to every accepted client.
- Register and remove per-client state with the existing accept/close lifecycle:
  the accepted-client path around `main.rs`'s accept handling and
  `netcode::dispatch_accepted_client`, plus the close path through
  `netcode::dispatch_closed_clients` / `on_slot_closed`.

The tracker should batch with the same sequence/tick used by entity snapshots so
one ack describes one server frame.

### Task 3: Engine production and client apply

Wire the tracker into `crate::netcode` without routing through entity
replication.

Work:

- Collect server-authoritative projected values after game logic has written
  them and before `host_replicate` sends the frame snapshot.
- Accept source values from store slots, engine systems, and descriptor-defined
  gameplay state.
- Store the local `StateSlotId -> dotted slot name/schema` map and fingerprint
  in a focused `crate::netcode::state_slots` module; pass it to production and
  client apply.
- Feed state records into the same snapshot send path as entity records.
- On clients, validate the schema fingerprint, map `StateSlotId` to dotted slot
  names, and apply records through the engine-side store write path so type,
  range, enum, and finite-number checks still run.
- Add a slot-table batch apply helper in `scripting/slot_table.rs` or
  `scripting/primitives/store.rs`: prevalidate all mapped values, then commit
  all or none. `write_store_slot` mutates one slot at a time and is not enough
  for atomic apply.
- Validate every state record in a snapshot before mutating slots. Any invalid
  record rejects the whole state batch and leaves prior values unchanged.
- Use `HealthComponent` current/max values from descriptor-spawned player pawns
  as the first descriptor-fed source values. They project to `player.health` and
  `player.maxHealth`; no general descriptor-struct replication is added.
- Apply state before `App::build_ui_read_snapshot`, so UI sees the authoritative
  value in the next frame snapshot.
- Do not call scripting VMs and do not expose a client-to-server state write
  primitive.

### Task 4: Player HUD state handoff

Move `player.health` and `player.maxHealth` from host-only visibility to
owner-private replication.

Work:

- Mark the engine catalog entries with the Task 1 replication-scope field:
  `player.health = ownerPrivatePlayer` and
  `player.maxHealth = ownerPrivatePlayer`.
- Keep `PlayerHudStatePublisher` as the host/single-player slot writer only.
- On the server, extract per-owner health from slot-owned player pawns via the
  owner mappings and `HealthComponent`. Project each owner's current/max health
  to `(StateSlotId, owner_client_id)`.
- Gate `PlayerHudStatePublisher::tick` off for `NetEndpoint::Client`, or make
  the publisher role-aware at the `main.rs` call site so a client cannot
  overwrite replicated player slots.
- Confirm the existing HUD binds still use `getGameState().player.health` and
  `player.maxHealth`; no UI descriptor change should be required.

### Task 5: Shared/global slot proof

Add one small shared slot fixture to prove the path is not hardcoded to health.

Work:

- Allow mod-authored `defineStore` entries to opt into public
  `network: "shared"`. Reject public `network: "ownerPrivate"` for mod stores
  until a per-player authoring namespace exists. Keep internal enum names
  `SharedGlobal` and `OwnerPrivatePlayer`.
- Update the TypeScript and Luau SDK surface plus author-facing validation for
  `network: "shared"` and rejected owner-private mod stores.
- Add an integration fixture store, e.g. `defineStore("netFixture", {
  objectiveProgress: { type: "number", default: 0, network: "shared" } })`,
  and replicate `netFixture.objectiveProgress` to all accepted clients.
- Add one descriptor-fed projection fixture through descriptor parse/materialize
  paths: a descriptor-spawned health component projects current/max health to
  `player.health` / `player.maxHealth`.
- Skip `collect_persisted_state` / `save_persisted_state` for
  `NetEndpoint::Client` in the clean-exit save path. Connected clients do not
  save replicated slot writes to `state.json`; single-player and host-local
  persistence stay unchanged.

### Task 6: Tests and harness coverage

Add focused tests before broad manual testing.

Required tests:

- wire round-trip for every supported value kind and `unset`;
- malformed wire rejection for non-finite numbers, over-cap strings/arrays,
  unknown slot ids, wrong types, and schema fingerprint mismatch;
- server tracker full baseline, delta, ack, refresh, and late-join baseline;
- owner-private filtering: client B never receives client A's private slots;
- shared/global delivery to all accepted clients;
- descriptor-defined source value projects into a named slot and replicates
  through the same wire schema;
- client apply validates all state records before mutation, uses store
  validation, and leaves every slot in the batch unchanged on rejection;
- networked client skips `state.json` save on the client role while single-player
  persistence still saves;
- schema mismatch logging is captured with the existing log-capture helper or a
  net-local equivalent; assert stable substrings, not full log lines;
- extend the existing in-memory prediction/reconciliation harness or add a
  sibling state-slot harness to cover loss, refresh, and repair;
- existing movement snapshot, prediction, reconciliation, and lifecycle tests
  remain green.

Manual or loopback harness checks:

- two clients connect; each sees its own health and max health through the
  existing HUD binding;
- damaging player A updates only player A's owner-private HUD state;
- a shared objective slot updates on both clients;
- a late joiner receives a full state baseline without waiting for a value
  change;
- under 150 ms RTT, jitter, and 5 percent packet loss, missing baselines repair
  and UI state converges.

## Acceptance criteria

- [ ] A connected client no longer renders `player.health` or
  `player.maxHealth` as missing after its first accepted state baseline.
  Runnable metric: after applying the first full state baseline,
  `UiReadSnapshot.slot_values` contains both slots.
- [ ] `player.health` and `player.maxHealth` reach the UI through the existing
  slot table -> `UiReadSnapshot` path, with no HUD-specific network field and no
  UI descriptor rewrite. The no-field/no-rewrite checks are review gates.
- [ ] Server encode never includes player A's owner-private records in player
  B's snapshot. Clients apply only records present in their validated snapshot.
- [ ] A `sharedGlobal` fixture slot replicates to every accepted client and to a
  late joiner through a full baseline. Use the `netFixture.objectiveProgress`
  fixture or an equivalent authoring-path fixture.
- [ ] A descriptor-defined source value projects into a named replicated slot
  and uses a `StateSlotId` from the same deterministic schema, plus the same
  wire value, baseline, and apply path as store slots. The proof fixture is
  descriptor-spawned health projecting to `player.health` / `player.maxHealth`.
- [ ] A schema fingerprint mismatch rejects state records before mutation; the
  client logs a stable mismatch diagnostic and keeps existing slot values.
- [ ] Unknown slot ids, type mismatches, non-finite values, and over-cap payloads
  reject the whole state batch without panic or partial apply.
- [ ] Missing slot baselines trigger `ClientMessage::StateBaselineRefresh`
  keyed by `StateSlotId` and repair without requiring reconnect.
- [ ] Pending, rejected, and closed clients receive no state records; closing a
  client drops its per-client state baseline maps.
- [ ] Replicated state uses the existing snapshot cadence and channels. No new
  renet channel is added. The channel-count check is runnable; the absence of a
  new channel is also a review gate.
- [ ] `postretro-net` remains independent of `EntityRegistry`,
  `crate::scripting::slot_table::SlotTable`, scripting runtimes, and `glam`.
  Its own connection-lifecycle `slots::SlotTable` may remain internal. This is a
  grep/cargo-tree review gate.
- [ ] Bitcode remains live-wire only. Replicated client state is not written to
  `state.json` by connected clients. Single-player and host-local persistence
  still save.
- [ ] Existing Phase 3 movement replication and reconciliation tests still pass.

## Non-goals

- Client-authored writes to server slots.
- Runtime scripting callbacks or a live VM path for replicated state.
- Replicating arbitrary descriptor structs or component internals. Descriptors
  feed named slot projections instead.
- Mod-declared owner-private per-player slots before a per-player authoring
  namespace exists.
- Interest management or priority accumulation for large slot sets.
- Combat/damage mechanics beyond using existing health values as a replication
  fixture.
- Save-game synchronization for network sessions.

## Sequencing

1. Task 1 first. It defines the schema, version gate, and validation boundary.
2. Task 2 next. It builds the per-client server tracker.
3. Task 3 then wires production/apply into the frame loop.
4. Task 4 proves the immediate HUD fix.
5. Task 5 proves the general shared/global path.
6. Task 6 runs continuously, with the loss/jitter harness after Tasks 2-5 land.

## Split-before-extend decisions

- `crates/net/src/wire.rs` is already over the threshold. Add state wire types in
  a new module and keep `wire.rs` as envelope glue.
- `crates/net/src/replication.rs` is already over the threshold. Add a sibling
  state tracker module.
- `crates/postretro/src/netcode/client.rs` is already over the threshold. Keep
  client state apply and refresh handling in a focused netcode state module.
- `crates/postretro/src/scripting/primitives/store.rs` is already over the
  threshold. Reuse its public helpers; add small slot-table helpers elsewhere
  only if the apply path needs schema lookup.
