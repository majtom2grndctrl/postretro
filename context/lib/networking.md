# Networking

> **Read this when:** working on multiplayer, replication, the wire/codec format, the netcode transport, or the host/client role model.
> **Key invariant:** the net crate is registry-blind — it moves typed snapshots and never mutates entity state. `crate::netcode` (engine) is the sole replication path that touches the `EntityRegistry`.
> **Related:** [Architecture Index](./index.md) · [Entity Model](./entity_model.md) §6 · [Development Guide](./development_guide.md) §4.2 · [Scripting](./scripting.md) §11

---

This is **Milestone 15 Phase 1**: authoritative client-server co-op, "ugly-but-connected". General-purpose multiplayer is a non-goal (see `index.md` §4). The Phase 1 client is a pure viewer of host-authoritative full-state — no prediction, no client→server gameplay, no lifecycle reconciliation. See *Phase boundaries* below.

## Crate boundary and ownership

Netcode lives in a sibling crate, `postretro-net`, holding three concerns: the wire codec, the polled transport, and the dev-only latency harness. The dependency arrow points **one way** — `postretro → postretro-net`. The net crate never depends on the engine.

`postretro-net` is **glam-free and postretro-free by construction.** Wire types use plain `[f32; N]` / `f32` / `bool` — never glam or engine types. The crate is never handed an `EntityRegistry` and has no notion of entities, components, or game state. It moves opaque, typed messages.

The engine owns the other half of the contract in `crate::netcode`. That module is the *only* engine code that touches the registry on behalf of replication, and it owns everything that must know both sides: the role model, the `NetworkId↔EntityId` maps, and the wire↔engine type conversions (the glam-aware `Transform`↔`WireTransform` translation, the `ComponentKind→u16` mapping). The split is deliberate: the net crate stays a reusable, engine-agnostic transport, and all registry mutation stays in game logic.

## Transport contract — polled, non-blocking

The transport is **synchronous and frame-polled** — no async runtime, no tokio, no spawned threads. It builds on renet 2.0 + renet_netcode over a non-blocking `std::net::UdpSocket`. The crate pins `default-features = false` on both renet deps specifically to keep tokio/async-std/smol out of the dependency tree; a `cargo tree` gate enforces the no-async-runtime invariant.

The caller advances the transport once per frame with `update(dt)`: it drives renet, drains the socket to `WouldBlock`, processes events, and flushes outbound packets — then returns. It never blocks. This honors the event-loop ownership invariant (`development_guide.md` §4.2): winit owns the loop, and the netcode poll slots into the frame's Game-logic stage without stalling it.

renet 2.0 separates two layers, and the transport wraps both: a **connection layer** (owns channels, produces/consumes opaque packet payloads) and a **netcode transport** (encrypts payloads, moves them over UDP). The connection-layer packet I/O is also re-exposed directly so the in-memory harness can drive the same payloads without a socket.

## Channel model

Three channels, fixed layout, agreed by both peers (the layout is folded into the protocol gate, so it cannot drift between versions):

| Channel | Delivery | Carries |
|---------|----------|---------|
| Control | reliable-ordered | version handshake; later, spawn/despawn |
| Snapshot | unreliable | full-state snapshots, latest-wins (a dropped snapshot is superseded by the next) |
| Input | reliable-ordered (reserved) | client input-command stream — registered now, no traffic in Phase 1 |

The Input channel is registered in Phase 1 only so the channel layout — and thus the transport protocol gate — is stable before it carries traffic. Reliability is matched to the data: control state must arrive and stay ordered; snapshots are disposable because the next one obsoletes the last.

## Wire/codec invariants

The wire codec is **bitcode**, pinned to an exact version. bitcode owns endianness and bit-packing — wire types do no manual byte layout. Two hard rules follow from bitcode's unstable byte format across majors:

1. **Never persist bitcode bytes.** The format is not a storage format. It exists only between two live, version-matched peers.
2. **Every connection is gated on the handshake** before any bitcode payload is decoded (see *Handshake* below). A version-mismatched peer is refused before a single message is interpreted.

**No serde-internally-tagged enum crosses the wire.** The engine's `ComponentValue` is a `#[serde(tag = "kind")]` enum, which bitcode cannot round-trip (`DeserializeAnyNotSupported`). So replication does not send engine types: it sends dedicated **wire-mirror** types that derive bitcode's native `Encode`/`Decode`. The component payload carries an explicit `u16` discriminant **numeric-equal to the engine `ComponentKind`** (Transform = 0). The engine↔wire conversion lives in `crate::netcode`; the mirror types know nothing about glam component order or serde tags.

This discriminant equality is a load-bearing contract across the crate boundary: the net side and the engine side independently assert it (drift-guard tests on both sides), because a divergence silently mis-tags components on the wire. Phase 2 grows the payload by adding variants in the same numeric order — the envelope shape does not change.

**Snapshot envelope:** a server tick stamp (`u32`) plus a count-prefixed list of `(NetworkId, payload)` entries. bitcode length-prefixes the list — that *is* the count prefix; an empty snapshot encodes as count 0. Phase 1 sends **full state every snapshot — no delta.**

The codec surface is two functions (encode, decode) over these types. Decode of a short, corrupted, or over-long buffer is always a typed `Err`, never a panic — the transport must survive a hostile or truncated packet.

## Two-gate handshake

Version compatibility is enforced **twice**, because the two gates catch different failures at different layers. Both gates derive from the same two build constants — an app protocol id and a wire-format version. The app id bumps when the message *vocabulary* changes (a new control message, a changed channel layout); the wire version bumps when any wire type's bitcode byte layout changes (added field, reordered enum, bumped bitcode major).

**Gate 1 — transport `protocol_id` (u64).** Both constants are packed into the netcode `protocol_id`. A peer whose `(protocol_id, wire_version)` pair differs fails the *encrypted netcode handshake itself* — the connection never establishes. This catches wire-incompatible peers before any app code runs.

**Gate 2 — app-level `ProtocolVersion`.** The client's *first reliable control message* carries the same two values. The server validates it against its own build before accepting the client. A mismatch is a **typed reject reason** (carrying expected vs received), logged with a `[Net]` tag, and the client is disconnected. **No entity state is sent or applied to a rejected client** — the snapshot send path refuses any client that has not passed this gate. A connected-but-not-yet-validated client is held in a pending state that receives nothing.

The two gates are not redundant. Gate 1 stops wire-incompatible peers cheaply at the encryption layer. Gate 2 is the app-level, unit-assertable contract: it produces a typed, logged reason and proves the "no state to a rejected peer" invariant independent of sockets. This mirrors the `BakedIr` exact-match version-epoch discipline (`scripting.md` §11): an exact-match epoch validated up front, mismatch refused rather than migrated.

## Game-logic-owned apply invariant

The net crate emits typed snapshots and **never mutates the registry.** All registry-touching replication lives in `crate::netcode`, which owns the two halves of the data path:

- **Host serialize:** walk the replicable set (Phase 1: every entity carrying a `Transform`), stamp each `EntityId` to its stable `NetworkId`, convert to wire mirrors, build the tick-stamped snapshot. Borrows the registry **immutably**.
- **Client apply:** for each entry, on first sight of a `NetworkId` spawn and record the mapping; otherwise mutate the mapped `EntityId`. The *only* registry-mutating path, and only on the client.

`NetworkId` is the network-stable identity assigned by the host; the host owns an `EntityId→NetworkId` allocator (monotonic, never recycled, stable for an entity's lifetime) and the client owns the inverse `NetworkId→EntityId` map. Stable ids keep the client's mapping coherent across snapshots. This is the network projection of the entity-model ownership rule (`entity_model.md` §6): game logic owns entities; replication is just another reader (host) and a controlled writer (client).

## Role model

Role is selected once at startup from CLI flags; default is **single-player with net fully inert** — no endpoint is constructed, serialize/apply never run.

| Flag | Role |
|------|------|
| *(none)* | Single-player. Net inert. |
| `--host [port]` | Listen server. Bare `--host` uses the default port. |
| `--connect <ip:port>` | Client connecting to an explicit address. |

`--host` and `--connect` are mutually exclusive. **Direct connect only** — no discovery, no matchmaking, no relay. Endpoint construction can fail (socket bind, transport init); a failure is logged and **degrades to single-player** rather than blocking boot — a netcode setup error never stops the engine from running. The Phase 1 client is a pure viewer of host-authoritative full-state.

## Testing the conditioned link

Two complementary paths exercise the netcode under loss and latency:

**In-memory harness (deterministic, unit-test path).** A dev-only packet conditioner (gated on `dev-tools`, always built under `test`) sits between an already-connected server/client pair, conditioning the *connection-level packet buffers* — bypassing the UDP transport entirely. It applies one-way delay, bounded jitter, and loss on a **virtual clock the caller advances** (it never reads wall-clock time), driven by a seeded PRNG. Same seed ⇒ same drops and arrival times, every run, every platform. This is deliberately not turmoil: turmoil conditions tokio sockets, and this path has no socket and no async runtime. It is the deterministic, reproducible unit-test path.

**`tc netem` (manual, real-socket soak path).** To shape the *real* renet_netcode UDP loopback path — the in-memory harness's real-socket complement — use Linux `tc netem` on the loopback device. Run the host and client locally over `lo`, then apply impairment:

```sh
# 80ms one-way delay, ±20ms jitter, 2% packet loss on loopback
sudo tc qdisc add dev lo root netem delay 80ms 20ms loss 2%

# Inspect the active qdisc
tc qdisc show dev lo

# Tear down — restores normal loopback
sudo tc qdisc del dev lo root netem
```

`tc netem` shapes every packet over `lo`, so it affects all local loopback traffic for the duration — apply it only for a soak session and always tear it down afterward. The in-memory harness is the deterministic automated gate; `tc netem` is the manual end-to-end soak over the real encrypted UDP path.

## Phase boundaries

Phase 1 ships the durable shape above. The following are **deferred** and must not be read into the Phase 1 contracts:

- **Phase 2:** delta encoding, snapshot interpolation, time-sync, entity lifecycle (spawn/despawn over the control channel, remove-missing reconciliation), and the client→server input stream over the reserved Input channel.
- **Phase 3:** client-side prediction and reconciliation.

Phase 1 explicitly **never despawns:** a `NetworkId` absent from a later snapshot is left untouched; remove-missing is Phase 2's job. The component payload binds **only `Transform`** in Phase 1; other components join in the same `ComponentKind` numeric order without changing the envelope shape.

## Non-goals

- Deterministic lockstep / rollback, competitive PvP, matchmaking, anti-cheat, peer-to-peer, full server-rewind lag compensation (see `index.md` §4).
- bitcode as a persistence format — wire-only, gated on the handshake, never stored.
- An async runtime in the net path — the transport is polled and synchronous by contract.
