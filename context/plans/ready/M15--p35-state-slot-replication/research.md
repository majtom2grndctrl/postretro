# Research Notes - M15 Phase 3.5 State Slot Replication

These notes record why the spec chooses schema-driven snapshot replication
instead of a HUD-specific patch.

## Local context

- `context/lib/networking.md`: `postretro-net` owns wire codec, transport
  envelopes, and latency harnesses. It must stay registry-blind. `postretro`
  owns the only replication path that touches engine registries and gameplay
  state.
- `context/lib/scripting.md`: scripts declare stores at load time; Rust owns
  runtime state. Store slots have stable dotted names, typed values, validation,
  engine-owned readonly slots, and existing engine-bypass writes through
  `write_store_slot`.
- `context/lib/ui.md`: UI reads a once-per-frame snapshot of authoritative slot
  values. The renderer never reads the live slot table and never writes back to
  authoritative state.
- `context/plans/done/mod-state-store/index.md`: the durable state store already
  defines the value model that Phase 3.5 should mirror on the wire:
  number, boolean, string, enum string, array, plus `Option<SlotValue>` for
  unset/current value.

## Code anchors

- `crates/postretro/src/scripting/slot_table.rs`: owns `SlotValue`,
  `SlotType`, `SlotSchema`, `SlotOwnership`, and `SlotTable`.
- `crates/postretro/src/scripting/engine_state_catalog.rs`: declares
  `player.health`, `player.maxHealth`, screen slots, input mode, and UI text
  entry.
- `crates/postretro/src/scripting/primitives/store.rs`: provides
  `write_store_slot`, the engine-side validation path that bypasses readonly but
  still enforces type, enum, range, and finite-number checks.
- `crates/postretro/src/main.rs`: `build_ui_slot_snapshot` clones value-bearing
  slot table entries into the UI read snapshot.
- `crates/postretro/src/scripting/systems/ui_proxy.rs`: today writes
  `player.health` and `player.maxHealth` on the host/local path.
- `crates/net/src/wire.rs`: existing snapshot envelope, ack messages,
  baseline-refresh messages, and bitcode version gate.
- `crates/net/src/replication.rs`: existing per-client entity baseline tracker.
  The state tracker should be a sibling because entity records are keyed by
  `NetworkId`, while state slots are keyed by schema ids.
- `crates/net/Cargo.toml`: `bitcode = "=0.6.9"` and comments already document
  that bitcode is live-wire only and version-gated.

## Web research

- Renet 2.0 documents unreliable and reliable-ordered channel send types
  (`https://docs.rs/renet/latest/renet/enum.SendType.html` and
  `https://docs.rs/renet/latest/renet/struct.ChannelConfig.html`).
  The existing channel split already fits this phase: state snapshots can use
  the unreliable snapshot channel, while acks and refresh requests use the
  reliable ordered client-message path.
- Bitcode 0.6.9 documents a compact Rust-native binary format
  (`https://docs.rs/bitcode/latest/bitcode/`) and explicitly does not promise a
  stable format across major versions or cross-language compatibility. This
  supports the existing rule: gate by protocol version and never persist bitcode
  bytes.
- Gaffer on Games, "State Synchronization"
  (`https://gafferongames.com/post/state_synchronization/`): state sync sends
  state as well as inputs, so it does not require perfect determinism. Delta
  compression with per-object baselines and bidirectional acks is powerful but
  complex. Phase 3.5 follows that shape with per-slot baselines and refresh
  repair.
- SnapNet, "Netcode Architectures Part 3: Snapshot Interpolation"
  (`https://www.snapnet.dev/blog/netcode-architectures-part-3-snapshot-interpolation/`): a
  client/server snapshot model sends authoritative gameplay state, clients ack
  snapshots, and servers can delta-encode changed data. This matches piggybacking
  slot records on the existing server snapshot cadence.
- Lightyear's book (`https://cbournhonesque.github.io/lightyear/book/`)
  describes a server-authoritative client/server replication model. The existing
  M15 research treats Lightyear as a design reference, not a dependency; Phase
  3.5 keeps that posture.

## Main design conclusions

- Do not add a HUD-specific message. Feed the existing state store so all UI and
  future game systems see one authoritative slot value.
- Do not add a new renet channel. The existing unreliable snapshot channel and
  reliable ordered client-message channel are enough.
- Do not put slot values into entity component payloads. Slots are global or
  owner-private state, not entity lifecycle records.
- Do not repeat dotted names in every per-tick delta. Use a deterministic schema
  id plus a schema fingerprint.
- Do not let mod-authored owner-private slots into this phase. That needs an
  owner-binding authoring surface. Shared/global mod slots are enough to prove
  the generic path.
