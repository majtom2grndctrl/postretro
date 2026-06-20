# M15 Phase 1 — Transport + wire + handshake

> Milestone 15 (multiplayer co-op netcode), Phase 1. Design reference:
> `context/research/netcode/` (`index.md` Phase 1 + Wire format, `research.md` §3/§6,
> `crate-pattern-research.md`). Grounded seam/API map: sibling `research.md`.
> Consumes the Phase 0 headless `sim::simulate_tick` seam.

## Goal

Stand up the netcode transport and wire foundation every later phase rides: a new
`postretro-net` sibling crate running `renet` 2.0 + `renet_netcode` (polled non-blocking in
the frame loop, no tokio), a `bitcode` wire codec that sidesteps the serde-tagged-enum trap,
and an application-level protocol/version handshake. The observable bar is **ugly-but-
connected**: two engine instances connect over loopback/LAN and a pawn moving on the host
appears and moves on the client, replicated full-state. Delta encoding, interpolation,
time-sync, and lifecycle are Phase 2; prediction is Phase 3.

## Scope

### In scope
- A new `crates/net/` (`postretro-net`) sibling crate owning transport, wire types, codec,
  and the latency-sim harness. Depends on `renet` 2.0, `renet_netcode`, `bitcode`; **no**
  wgpu/winit/tokio. `postretro` depends on it (never the reverse).
- A `bitcode` wire codec on dedicated **wire-mirror** types (native
  `#[derive(bitcode::Encode, bitcode::Decode)]`) — see Open questions for why mirror types,
  not derives on the engine `ComponentValue`. Phase 1 wire-binds the **Transform** payload
  only (enough for "a remote pawn appears and moves"); Phase 2 grows the replicable set.
- A hand-built **full-state snapshot envelope** (`NetworkId` + a count-prefixed list of
  per-entity component payloads) that round-trips byte-identical through the codec.
- A **protocol/version handshake**: the netcode `protocol_id` gates the connection, and an
  app-level version stamp is the first reliable message; a mismatch rejects the join with a
  **logged reason** and **no entity state applied** on either side.
- **Non-blocking transport integration** in the existing frame loop: poll renet via
  `update(dt)` + drain once per frame, before the catch-up tick loop. Honors
  `development_guide.md` §4.2 (never block the event loop).
- A **game-logic-owned apply step** in a new `postretro::netcode` module: the host serializes
  the replicable Transform set from the registry each tick and sends; the client receives,
  maps `NetworkId → EntityId`, and applies via `spawn` / `set_component_value`. The net crate
  emits typed values and **never mutates the registry** (`entity_model.md` §6).
- **Role selection at startup** via CLI: default single-player (net inert), `--host [port]`
  (listen server), `--connect <ip:port>` (client). Direct connect only.
- A **dev-tools-gated latency-sim harness**: an in-process conditioner wrapping the transport
  with configurable delay/jitter/loss, exercised by an automated loopback test, plus a
  documented `tc netem` soak recipe. **Not** turmoil.
- **At promotion:** a durable `context/lib/networking.md` doc + its Agent Router entry in
  `index.md`.

### Out of scope
- Per-entity delta encoding, per-entity acked baseline, eventual-consistency state sync —
  Phase 2. Phase 1 sends full-state every server tick.
- Remote-entity **interpolation**, jitter-sized interpolation delay, snapshot-send-rate
  decoupling from the 60 Hz tick — Phase 2.
- **Time-sync** (client tick clock ↔ server), **join-in-progress** baseline convergence,
  **player-leave/disconnect** lifecycle + timeout + slot-free — Phase 2.
- **Client-side movement prediction + reconciliation** — Phase 3. The client is a pure viewer
  of host-authoritative state in Phase 1; it runs no predicted tick.
- **Server-side application of remote client input** to a client-owned pawn (the upstream
  gameplay loop). Phase 1 defines and round-trips the input-command envelope but the moving
  pawn in the demo is **host-driven**; server-side input application lands with ownership +
  lifecycle in Phase 2.
- Wire-binding the full `ComponentValue` set, enemy/`Agent`/`Brain` replication, weapon/health
  replication — Phase 2+ (Transform is the only wire-bound payload here).
- Steam transport (`steamworks-rs`), float quantization tuning, interest management,
  dedicated-server binary entry point — later phases.
- Any change to `sim::simulate_tick` behavior or signature.

## Acceptance criteria

- [ ] A `postretro-net` crate is a workspace member, builds, and pins `renet` 2.0 +
  `renet_netcode` + `bitcode` at exact versions; its dependency tree contains **no** wgpu,
  winit, or tokio. `postretro` depends on `postretro-net`; `postretro-net` does not depend on
  `postretro`. (Task 1)
- [ ] A hand-built snapshot envelope (a `NetworkId` plus a Transform-bearing payload) encodes
  and decodes **byte-identical** through the codec — a round-trip test is green. **No
  serde-internally-tagged enum crosses the wire.** (Task 2)
- [ ] Two engine instances launched `--host` and `--connect <addr>` connect over loopback;
  a pawn that **moves on the host appears as a remote entity on the client and moves there**,
  full-state. Observable in two running processes. (Tasks 3–4)
- [ ] A version/protocol **mismatch is rejected**: the joining client is refused, the reason
  is **logged**, and **no component state is applied** on either side — proven by a test that
  connects two instances with divergent version stamps and asserts an empty client registry
  delta + the logged rejection. (Tasks 2–3)
- [ ] The transport is **polled non-blocking** in the frame loop (`update(dt)` + drain, no
  spawned runtime); the window stays responsive while connected; the dependency tree adds no
  async executor. (Tasks 3–4)
- [ ] Snapshot application runs through the `postretro::netcode` game-logic-owned apply step;
  `postretro-net` exposes typed snapshots and **calls no registry mutation**. Verifiable by
  module structure: no `EntityRegistry` reference in `postretro-net`. (Task 4)
- [ ] A dev-tools-gated latency harness wraps the transport with delay/jitter/loss; an
  automated test drives the loopback handshake + a snapshot through it and asserts eventual
  delivery; a `tc netem` recipe is documented. (Task 5)
- [ ] `context/lib/networking.md` exists with an Agent Router entry in
  `context/lib/index.md`. (Task 6 — promotion gate)

## Tasks

### Task 1: Scaffold `crates/net` (`postretro-net`)
Add `crates/net` to the workspace `members`; name the package `postretro-net`, inheriting
`version`/`edition`/`rust-version`/`license` from `[workspace.package]`. Pin `renet = "2"`,
`renet_netcode`, and `bitcode = "0.6"` (exact patch pins, with a comment: bitcode format is
unstable across majors — never persist its bytes, gate every connection on the handshake).
Add `glam` (workspace) only if a wire-mirror needs vector math; prefer plain `[f32; N]` in
wire types to keep the codec glam-free. Declare the `postretro → postretro-net` path dep in
the root `[workspace.dependencies]` and `crates/postretro/Cargo.toml`. Stub the lib with the
module skeleton (`wire`, `transport`, `harness`) and a smoke test so CI builds the empty
crate. New crate — no split-before-extend.

### Task 2: Wire types + bitcode codec
In `postretro-net::wire`, define the wire surface with native `bitcode::Encode/Decode`:
`NetworkId(u32)`; a `WireTransform` (position `[f32; 3]`, rotation `[f32; 4]`); a tagged
per-entity component payload that carries an explicit `ComponentKind`-equivalent
discriminant (a `u16`, mirroring the engine `ComponentKind` numeric values) plus the
payload — replacing serde's internal `"kind"` tag, which is the thing that cannot round-trip
on bitcode. Phase 1 binds **only** the Transform variant; the discriminant + envelope shape
must extend to more variants without reshaping (Phase 2). Define the **full-state snapshot
envelope** (server tick stamp + a count-prefixed list of `(NetworkId, component payload)`),
the **input-command envelope** (mirrors `SimCommand`'s `MovementInput` fields + fire button —
defined and round-tripped now, gameplay-applied in Phase 2), and the **handshake message**
(a `ProtocolVersion` stamp: a `u32` protocol id + the wire-format version). Provide
`encode`/`decode` helpers over `bitcode`. Tests: every envelope round-trips byte-identical;
a deliberately-corrupted/short buffer decodes to a clean `Err`, never a panic.

### Task 3: Transport + handshake
In `postretro-net::transport`, wrap `renet` 2.0 + `renet_netcode` into a `NetServer` and
`NetClient` with a **synchronous, non-blocking** surface: `update(dt)` (drives renet +
transport), typed `send`/`drain` over named channels, and connection-state queries. Configure
the `ConnectionConfig` channels: **reliable-ordered** for control (the version handshake +,
later, spawn/despawn), **unreliable** for snapshots (latest-wins), a third channel reserved
for input commands. Implement the two-gate handshake: renet_netcode's `protocol_id` (derived
from the build's protocol/version) is the transport-level gate; on top, the client sends the
`ProtocolVersion` handshake message as its first reliable control message, and the server
**validates it before applying or sending any entity state** — a mismatch disconnects the
client, logs the reason (expected vs received), and leaves both registries untouched. No
async runtime; renet is polled by the caller. Unit-test the handshake accept and reject paths
in-process (two transports over loopback `UdpSocket`s).

### Task 4: Engine glue + roles (`postretro::netcode`)
Add a new `postretro::netcode` module (engine-side glue — **keep `main.rs` additions to thin
delegating calls**, `main.rs` is already ~5.6k lines). It owns: role selection parsed at
startup (default single-player, `--host`, `--connect`), an optional `NetServer`/`NetClient`
held by `App`, the `NetworkId ↔ EntityId` client-side map, and two game-logic-owned steps —
**serialize** (host: walk the replicable Transform set from the registry into a snapshot
envelope) and **apply** (client: for each `(NetworkId, payload)`, `spawn` on first sight
recording the map, else `set_component_value` on the mapped `EntityId`). Wire the per-frame
poll into the `RedrawRequested` handler **before** the catch-up tick loop (`main.rs:~2023`):
client applies received snapshots before render; host serializes + sends after the tick loop,
beside the existing post-loop drains. The net crate is never handed an `EntityRegistry`;
`postretro::netcode` is the only code that touches it. Phase 1 demo: host's local pawn,
driven by host input through the existing sim, replicated down; client renders the remote
pawn moving. No prediction, no client→server gameplay input.

### Task 5: Latency-sim harness
In `postretro-net::harness`, a dev-tools-gated (`#[cfg(any(test, feature = "dev-tools"))]`)
in-process conditioner that wraps the transport's packet send/receive with configurable
one-way delay, jitter, and loss (deterministic seed for tests). An automated test drives the
Task 3 loopback handshake + one snapshot through the conditioner at non-zero delay/loss and
asserts eventual delivery and correct apply. Document a `tc netem` recipe (Linux soak; delay,
jitter, loss flags) in `networking.md` (Task 6) for real-socket soak testing. **Not** turmoil
(it only conditions tokio sockets; this engine runs real blocking sockets).

### Task 6: `networking.md` context doc + Agent Router entry
At promotion, write `context/lib/networking.md`: the durable shape — `postretro-net` crate
boundary and ownership, the polled-non-blocking transport contract, the wire/codec invariants
(bitcode owns endianness; no serde-tagged enum on the wire; pin-and-never-persist; handshake
gates every connection), the channel model, the game-logic-owned apply invariant, the role
model, and the `tc netem` recipe. Add its Agent Router line to `context/lib/index.md`. Follow
`context_style_guide.md`: durable contracts, not function names.

## Sequencing

**Phase 1 (sequential):** Task 1 — scaffolds the crate every later task imports.
**Phase 2 (sequential):** Task 2 — wire types + codec; Tasks 3–4 send/apply these envelopes.
**Phase 3 (sequential):** Task 3 — transport + handshake; consumes Task 2's envelope + handshake types.
**Phase 4 (concurrent):** Task 4 (engine glue + demo) and Task 5 (latency harness) — both consume Task 3's `NetServer`/`NetClient`; independent files.
**Phase 5 (sequential):** Task 6 — `networking.md`, written against the landed shape (promotion deliverable).

## Boundary inventory

Netcode crosses **Rust ↔ wire** only — never JS/Lua/FGD. Wire encoding is `bitcode`.

| Name | Rust (`postretro-net`) | Wire (bitcode) | Engine side (`postretro`) |
|---|---|---|---|
| Network entity id | `NetworkId(u32)` | `u32` | mapped to/from `EntityId` in `postretro::netcode` |
| Local entity id | — | not sent raw | `EntityId(u32)` (`scripting/registry.rs`) |
| Component payload | `WireTransform` + `u16` kind tag | native `Encode/Decode`, explicit `u16` discriminant | converted from/to `ComponentValue::Transform` |
| Snapshot envelope | `Snapshot { tick, entries }` | tick `u32` + count-prefixed entries | built in serialize step, consumed in apply step |
| Input command | mirrors `SimCommand` (`MovementInput` + fire) | bitcode struct | built host-side (Phase 2 gameplay use) |
| Handshake | `ProtocolVersion { protocol_id: u32, wire_version: u32 }` | bitcode struct, first reliable msg | compared before any state applied |

Scripts never observe the wire; FGD never configures replication.

## Wire format

Binary surface, **bitcode-owned** endianness/bit-packing. Per-field byte layout is the
implementer's call (`context_style_guide.md` — constraints, not offsets); the invariants:

- **No serde-internally-tagged enum on the wire.** The engine `ComponentValue`
  (`#[serde(tag = "kind")]`) fails `deserialize_any` on bitcode. The wire carries an explicit
  `u16` kind discriminant (numeric-equal to engine `ComponentKind`) + the inner payload, on
  dedicated `bitcode::Encode/Decode` wire-mirror types.
- **bitcode version pinned exactly; bytes never persisted.** Co-op host+client ship together;
  every connection is gated on the version handshake.
- **Snapshot envelope:** a server tick stamp (`u32`) + a count-prefixed list of
  `(NetworkId, component payload)`. Empty list = count 0. Full-state in Phase 1 (no delta).
- **Channel model (renet):** reliable-ordered = control (handshake; later spawn/despawn);
  unreliable = snapshots (latest-wins); a third reserved for the input-command stream.
- **Handshake:** `ProtocolVersion` is the first reliable control message; the server rejects a
  mismatch (logged reason, no state) — mirrors the `BakedIr` persist version-stamp discipline.

## Open questions

- **Wire-component representation — wire-mirror types vs. dual-derive on the engine structs.**
  The epic's boundary inventory names `ComponentValue` carrying "native `bitcode::Encode/Decode`"
  ("one type, two representations"). But: (1) `ComponentValue`/`Transform` are `pub(crate)` in
  `postretro`, unnameable from a sibling crate that must stay `postretro`-free; (2) their fields
  are glam `Vec3`/`Quat`, which have no bitcode derives. This spec therefore plans **dedicated
  wire-mirror types in `postretro-net`** (plain `[f32; N]` fields, native bitcode derives) with
  conversion in `postretro::netcode`, rather than dual-deriving the engine structs. This keeps
  the net crate glam-free and dependency-clean and still honors "serde stays for persistence."
  **Decision needed before promotion:** confirm the wire-mirror approach (recommended), or
  require dual-derive (which forces either a glam-bitcode shim or relocating the component
  types into a shared lower crate — larger blast radius). *Recommend wire-mirror.*
- **`NetworkId` allocation** — server-assigned monotonic vs. recycled. Phase 1 only needs the
  host to stamp its own entities; the allocation policy + predicted-spawn reconciliation is
  pinned in Phase 2/6. Phase 1 uses a simple server-monotonic `u32`.
- **Snapshot send rate vs. 60 Hz tick** — Phase 1 sends full-state **every server tick**
  (simplest for the demo). The decoupled send-rate + interpolation-delay decision is Phase 2's
  (the epic's "timing parameters up front" applies at the Phase 2 spec, where interpolation
  consumes it).
