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

This is not a HUD shortcut. Phase 3.5 creates the general slot replication path
for server-owned gameplay state: owner-private player values now, shared/global
values for objectives and set pieces next, and future engine-declared stats
without one-off netcode fields.

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
deterministic replicated-slot schema from committed store declarations:

- sort by stable dotted slot name;
- assign a deterministic `StateSlotId(u16)`;
- include the name, slot type, enum values, replication scope, and validation
  shape in a canonical schema fingerprint;
- pass the opaque fingerprint and ids into `postretro-net`.

`postretro` computes the fingerprint with the workspace `blake3` dependency.
`postretro-net` stores and compares the fingerprint as opaque bytes; it does not
need to depend on `blake3`.

Supported replication scopes:

- `none`: default for existing slots.
- `ownerPrivatePlayer`: server sends only to the owning accepted client. Phase
  3.5 uses this for engine-owned player slots such as `player.health` and
  `player.maxHealth`.
- `sharedGlobal`: server sends to every accepted client. Phase 3.5 supports
  mod-declared shared slots through `defineStore` metadata.

Mod-declared owner-private per-player slots are out of scope until the authoring
surface has a per-player state namespace or owner binding. This keeps Phase 3.5
honest: engine player slots get privacy now, shared/global mod slots get a real
path now, and the future per-player mod surface can reuse the same wire tracker.

## Boundary inventory

| Boundary value | Rust | Wire | TypeScript/Luau | FGD |
| --- | --- | --- | --- | --- |
| Slot identity | `SlotTable` dotted names and deterministic `StateSlotId` map | `StateSlotId(u16)` plus schema fingerprint; names never appear in per-tick deltas | Existing state refs serialize dotted names; `defineStore` keeps dotted names | None |
| Slot value | `SlotValue` and `Option<SlotValue>` | `WireSlotValue` plus explicit `unset` state | `StateValue<T>` refs and UI bind descriptors keep existing shapes | None |
| Replication scope | New slot schema metadata in engine catalog and store declarations | Scope is included in the schema fingerprint, not repeated in each record | `defineStore` accepts `network: "shared"` for mod slots; engine refs expose readonly/player values as before | None |
| Authority | Server-side slot producer and client-side apply path | Server snapshot records only; clients ack and request refresh | No new client write primitive | None |

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
  value: RawStateSlotValueOrUnset,
}
```

Validation rejects:

- unknown `slot_id`;
- schema fingerprint mismatch;
- unknown value kind;
- type mismatch against the local slot schema;
- non-finite numbers or arrays;
- strings over 256 bytes;
- arrays over 64 numbers;
- more than 1024 slot records in one snapshot.

`AckMessage` gains `slot_baselines: Vec<(u16, u32)>`.

Append a new `ClientMessage` variant for state-slot baseline refresh requests.
The request carries `snapshot_sequence`, `slot_id`, `missing_baseline_ref`, and
reason. Do not reuse the entity `BaselineRefreshRequest`; entity baselines are
keyed by `NetworkId`, while state baselines are keyed by `StateSlotId`.

No slot tombstone is needed in Phase 3.5. Slot declarations are process-lived
after commit; clearing a value rides as `unset`.

Bitcode remains live-wire only. Do not persist bitcode payloads or write
replicated client state to `state.json`.

## Tasks

### Task 1: Schema and wire model

Add the replicated slot schema builder in `postretro` and the wire-facing state
types in `postretro-net`.

Work:

- Add replication metadata to store schemas and the engine state catalog.
- Generate deterministic `StateSlotId` mappings from committed slot schemas.
- Compute the `[u8; 32]` schema fingerprint in `postretro`.
- Add `WireSlotValue`, `RawStateSlotRecord`, validation, encode/decode tests,
  and the snapshot/ack/refresh envelope changes in `postretro-net`.
- Bump snapshot/protocol gates.

Keep `postretro-net` free of `SlotTable`, `SlotValue`, `EntityRegistry`,
`glam`, and scripting types.

### Task 2: Server-side state replication tracker

Add a sibling tracker to entity replication rather than folding slots into
`ServerReplication`.

Work:

- Track current `Option<WireSlotValue>` per `StateSlotId`.
- Track per-client acked baselines and refresh requests.
- Emit full baselines for newly accepted clients.
- Emit deltas against the client's acked baseline when possible.
- Fall back to full baselines when a referenced baseline is missing.
- Filter `ownerPrivatePlayer` slots to the owning client.
- Emit `sharedGlobal` slots to every accepted client.
- Register and remove per-client state with the existing accept/close lifecycle.

The tracker should batch with the same sequence/tick used by entity snapshots so
one ack describes one server frame.

### Task 3: Engine production and client apply

Wire the tracker into `crate::netcode` without routing through entity
replication.

Work:

- Collect server-authoritative slot values after game logic has written them and
  before `host_replicate` sends the frame snapshot.
- Feed state records into the same snapshot send path as entity records.
- On clients, validate the schema fingerprint, map `StateSlotId` to dotted slot
  names, and apply records through the engine-side store write path so type,
  range, enum, and finite-number checks still run.
- Apply state before `App::build_ui_read_snapshot`, so UI sees the authoritative
  value in the next frame snapshot.
- Do not call scripting VMs and do not expose a client-to-server state write
  primitive.

### Task 4: Player HUD state handoff

Move `player.health` and `player.maxHealth` from host-only visibility to
owner-private replication.

Work:

- Mark the engine catalog entries as `ownerPrivatePlayer`.
- Keep the server/host `UiStateProxy` or equivalent producer as the sole writer
  of those values.
- Prevent client-local proxy work from overwriting replicated player slots.
- Confirm the existing HUD binds still use `getGameState().player.health` and
  `player.maxHealth`; no UI descriptor change should be required.

### Task 5: Shared/global slot proof

Add one small shared slot fixture to prove the path is not hardcoded to health.

Work:

- Allow mod-authored `defineStore` entries to opt into `network: "shared"`.
- Reject `network: "ownerPrivate"` for mod stores until a per-player authoring
  namespace exists.
- Add a fixture slot such as objective progress or wave count and replicate it
  to all accepted clients.
- Preserve local/single-player persistence behavior, but exclude replicated
  server-authoritative client values from `state.json` in network sessions.

### Task 6: Tests and harness coverage

Add focused tests before broad manual testing.

Required tests:

- wire round-trip for every supported value kind and `unset`;
- malformed wire rejection for non-finite numbers, over-cap strings/arrays,
  unknown slot ids, wrong types, and schema fingerprint mismatch;
- server tracker full baseline, delta, ack, refresh, and late-join baseline;
- owner-private filtering: client B never receives client A's private slots;
- shared/global delivery to all accepted clients;
- client apply uses store validation and leaves the slot unchanged on rejection;
- networked client does not persist replicated slots to `state.json`;
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
- [ ] `player.health` and `player.maxHealth` reach the UI through the existing
  slot table -> `UiReadSnapshot` path, with no HUD-specific network field and no
  UI descriptor rewrite.
- [ ] Owner-private player slots are sent only to their owning accepted client.
  Another client neither receives nor applies those records.
- [ ] A `sharedGlobal` fixture slot replicates to every accepted client and to a
  late joiner through a full baseline.
- [ ] A schema fingerprint mismatch rejects state records before mutation; the
  client logs the mismatch and keeps existing slot values.
- [ ] Unknown slot ids, type mismatches, non-finite values, and over-cap payloads
  are rejected without panic or partial apply.
- [ ] Missing slot baselines trigger a state baseline refresh request and repair
  without requiring reconnect.
- [ ] Pending, rejected, and closed clients receive no state records; closing a
  client drops its per-client state baseline maps.
- [ ] Replicated state uses the existing snapshot cadence and channels. No new
  renet channel is added.
- [ ] `postretro-net` remains independent of `EntityRegistry`, `SlotTable`,
  scripting runtimes, and `glam`.
- [ ] Bitcode remains live-wire only. Replicated client state is not written to
  `state.json` during network sessions.
- [ ] Existing Phase 3 movement replication and reconciliation tests still pass.

## Non-goals

- Client-authored writes to server slots.
- Runtime scripting callbacks or a live VM path for replicated state.
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
