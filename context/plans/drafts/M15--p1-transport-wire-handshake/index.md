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
  `#[derive(bitcode::Encode, bitcode::Decode)]`), converted from the engine `ComponentValue`
  in `crate::netcode` (Resolved decisions). Phase 1 wire-binds the **Transform** payload
  only (enough for "a remote pawn appears and moves"); Phase 2 grows the replicable set.
- A hand-built **full-state snapshot envelope** (`NetworkId` + a count-prefixed list of
  per-entity component payloads) that round-trips byte-identical through the codec.
- A **protocol/version handshake**: the netcode `protocol_id` gates the connection, and an
  app-level version stamp is the first reliable message; a mismatch rejects the join with a
  **logged reason** and **no replicated state applied** (the client registry stays empty; the server sends none after rejecting).
- **Non-blocking transport integration** in the existing frame loop: poll renet via
  `update(dt)` + drain once per frame, before the catch-up tick loop. Honors
  `development_guide.md` §4.2 (never block the event loop).
- A **game-logic-owned apply step** in a new `netcode` module inside the `postretro` binary (`crate::netcode` — the engine has no lib target; declared `mod netcode;` in `main.rs`): the host serializes
  the replicable Transform set from the registry each tick and sends; the client receives,
  maps `NetworkId → EntityId`, and applies via `spawn` / `set_component_value`. The net crate
  emits typed values and **never mutates the registry** (`entity_model.md` §6).
- **Role selection at startup** via CLI: default single-player (net inert), `--host [port]`
  (listen server), `--connect <ip:port>` (client). Direct connect only.
- A **dev-tools-gated latency-sim harness**: an in-process conditioner on an in-memory packet
  relay (configurable delay/jitter/loss), exercised by an automated test, plus a documented
  `tc netem` soak recipe for the real-socket path. **Not** turmoil.
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
  `renet_netcode` + `bitcode` at exact versions; its dependency tree (via `cargo tree`) contains **no** wgpu,
  winit, tokio, or other async executor. `postretro` depends on `postretro-net`; `postretro-net` does not depend on
  `postretro`. (Task 1)
- [ ] Every wire envelope — snapshot, input-command, and handshake — encodes and decodes **byte-identical** through the codec, and a short/corrupted buffer decodes to `Err` (never a panic). **No serde-internally-tagged enum crosses the wire.** (Task 2)
- [ ] Two engine instances launched `--host` and `--connect <addr>` connect over loopback;
  a pawn that **moves on the host appears as a remote entity on the client and moves there**,
  full-state. Observable in two running processes. (Tasks 3–4)
- [ ] A version/protocol **mismatch is rejected**: the joining client is refused, the reason
  is **logged**, and **no replicated state is applied on the client** (asserted: empty client registry delta) while the server sends no entity state after rejecting — proven by a test that
  connects two instances with divergent version stamps and asserts the empty client delta + the logged rejection. (Tasks 2–3)
- [ ] The transport is **polled non-blocking** in the frame loop (`update(dt)` + drain, no
  spawned runtime); the window stays responsive while connected; the dependency tree adds no
  async executor. (Tasks 3–4)
- [ ] Snapshot application runs through the `crate::netcode` game-logic-owned apply step;
  `postretro-net` exposes typed snapshots and **calls no registry mutation**. Verifiable by
  module structure: no `EntityRegistry` reference in `postretro-net`. (Task 4)
- [ ] A dev-tools-gated latency harness conditions delay/jitter/loss on an in-memory packet relay; an automated test relays a handshake + a snapshot through it and asserts correct decode after conditioned delivery. (Task 5)
- [ ] `context/lib/networking.md` exists with an Agent Router entry in
  `context/lib/index.md`, including the `tc netem` soak recipe. (Task 6 — promotion gate)

## Tasks

### Task 1: Scaffold `crates/net` (`postretro-net`)
Add `crates/net` to the workspace `members`; name the package `postretro-net`. Pin `renet = "2"`,
`renet_netcode`, and `bitcode = "0.6"` (exact patch pins, with a comment: bitcode format is
unstable across majors — never persist its bytes, gate every connection on the handshake).
Add `glam` (workspace) only if a wire-mirror needs vector math; prefer plain `[f32; N]` in
wire types to keep the codec glam-free. In the new crate's `[package]`, inherit shared fields via `version.workspace = true` / `edition.workspace = true` / `rust-version.workspace = true` / `license.workspace = true`. Add `postretro-net = { path = "crates/net" }` to the root `[workspace.dependencies]` (path-only, mirroring `postretro-level-format`), and reference it from `crates/postretro/Cargo.toml [dependencies]` as `postretro-net = { workspace = true }`. Stub the lib with the
module skeleton (`wire`, `transport`, `harness`) and a smoke test so CI builds the empty
crate. Define a `dev-tools` feature in `crates/net/Cargo.toml` for the harness gate (Cargo features are per-crate; postretro's `dev-tools` is egui-bound and does not propagate). Forward it from the engine when desired: postretro's `dev-tools = [..., "postretro-net/dev-tools"]`. New crate — no split-before-extend.

### Task 2: Wire types + bitcode codec
In `postretro-net::wire`, define the wire surface with native `bitcode::Encode/Decode`:
`NetworkId(u32)`; a `WireTransform` (position `[f32; 3]`, rotation `[f32; 4]` = glam `Quat` in `[x, y, z, w]` order, a field copy from `ComponentValue::Transform`); a tagged
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
from the build's protocol/version) is the transport-level gate. Pin `PROTOCOL_ID` and `WIRE_VERSION` as explicit hand-bumped `const`s in `postretro-net`; the reject test forces divergence by overriding the client's `ProtocolVersion` stamp. On top, the client sends the
`ProtocolVersion` handshake message as its first reliable control message, and the server
**validates it before applying or sending any entity state** — a mismatch disconnects the
client, logs the reason (expected vs received), and leaves both registries untouched. No
async runtime; renet is polled by the caller. Unit-test the handshake accept and reject paths
in-process (two transports over loopback `UdpSocket`s).

### Task 4: Engine glue + roles (`crate::netcode`)
Add a new `crate::netcode` module (engine-side glue — **keep `main.rs` additions to thin
delegating calls**, `main.rs` is already ~5.9k lines). It owns: role selection parsed at
startup (default single-player, `--host`, `--connect`), an optional `NetServer`/`NetClient`
held by `App`, the `NetworkId ↔ EntityId` client-side map, a host-side `EntityId → NetworkId` allocator (a monotonic `u32` counter + map; the serialize step stamps each replicable entity through it), and two game-logic-owned steps —
**serialize** (host: walk the replicable set — entities carrying `ComponentValue::Transform`, the host pawn in Phase 1 — from the shared registry handle (`script_ctx.registry`) into a snapshot
envelope) and **apply** (client: for each `(NetworkId, payload)`, `spawn` on first sight
recording the map, else `set_component_value` on the mapped `EntityId`). Wire the per-frame
poll into the `RedrawRequested` handler **before** the catch-up tick loop (`main.rs:~2023`):
client applies received snapshots before render; host serializes + sends after the tick loop,
beside the existing post-loop drains. The net crate is never handed an `EntityRegistry`;
`crate::netcode` is the only code that touches it. Phase 1 demo: host's local pawn,
driven by host input through the existing sim, replicated down; client renders the remote
pawn moving. No prediction, no client→server gameplay input.

### Task 5: Latency-sim harness
In `postretro-net::harness`, a dev-tools-gated (`#[cfg(any(test, feature = "dev-tools"))]`) in-process conditioner that sits on an **in-memory packet relay** between an already-connected `NetServer`/`NetClient` — driving renet's packet-level API (`get_packets_to_send` / `process_packet`), bypassing the renet_netcode UDP transport — and applies configurable one-way delay, jitter, and loss to the relayed packet buffers (deterministic seed for tests). This is why it is **not** turmoil (which conditions only tokio sockets) and needs no socket interception. An automated test relays a handshake + one snapshot through the conditioner at non-zero delay/loss and asserts the snapshot **decodes correctly after conditioned delivery** (codec/transport level, in-crate — the engine-side apply lives in `crate::netcode`, a different crate, and is not exercised here). The real-socket soak path runs renet_netcode over loopback shaped by `tc netem` (recipe documented in Task 6).

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
| Network entity id | `NetworkId(u32)` | `u32` | mapped to/from `EntityId` in `crate::netcode` |
| Local entity id | — | not sent raw | `EntityId(u32)` (`scripting/registry.rs`) |
| Component payload | `WireTransform` + `u16` kind tag | native `Encode/Decode`, explicit `u16` discriminant | converted from/to `ComponentValue::Transform` |
| Snapshot envelope | `Snapshot { tick, entries }` | tick `u32` + count-prefixed entries | built in serialize step, consumed in apply step |
| Input command | mirrors `SimCommand` (`MovementInput` + fire) | bitcode struct | built host-side (Phase 2 gameplay use) |
| Handshake | `ProtocolVersion { protocol_id: u32, wire_version: u32 }` | bitcode struct, first reliable msg | compared before any state applied, distinct from the renet_netcode transport `protocol_id` (`u64`) |

Scripts never observe the wire; FGD never configures replication.

## Wire format

Binary surface, **bitcode-owned** endianness/bit-packing. Per-field byte layout is the
implementer's call (`context_style_guide.md` — constraints, not offsets); the invariants:

- **No serde-internally-tagged enum on the wire.** The engine `ComponentValue`
  (`#[serde(tag = "kind")]`) fails `deserialize_any` on bitcode. The wire carries an explicit
  `u16` kind discriminant (numeric-equal to engine `ComponentKind`) + the inner payload, on
  dedicated `bitcode::Encode/Decode` wire-mirror types. Map the discriminant via an explicit `ComponentKind → u16` conversion in `crate::netcode` (`ComponentKind` is `#[repr(u16)]`, `Transform = 0`), not a reliance on enum layout.
- **bitcode version pinned exactly; bytes never persisted.** Co-op host+client ship together;
  every connection is gated on the version handshake.
- **Snapshot envelope:** a server tick stamp (`u32`) + a count-prefixed list of
  `(NetworkId, component payload)`. Empty list = count 0. Full-state in Phase 1 (no delta).
- **Channel model (renet):** reliable-ordered = control (handshake; later spawn/despawn);
  unreliable = snapshots (latest-wins); a third for the input-command stream, registered in `ConnectionConfig` now but carrying no traffic until Phase 2.
- **Handshake:** The renet_netcode transport `protocol_id` (a `u64` derived from the build) is
  the transport gate; the app-level `ProtocolVersion` is a separate, first reliable control
  message — the server rejects a mismatch (logged reason, no state), mirroring the `BakedIr`
  exact-match version-epoch discipline (`version: u32` validated against `CURRENT_IR_VERSION` at
  load; `scripting.md` §11).

## Resolved decisions

- **Wire-component representation → wire-mirror types in `postretro-net`.** The wire-bound
  component set is dedicated mirror types — plain `[f32; N]` fields, native
  `bitcode::Encode/Decode` — converted to/from the engine `ComponentValue` in
  `crate::netcode`. This keeps `ComponentValue` `pub(crate)` and the net crate both glam-
  and `postretro`-free, the boundary the dedicated-server split (Phase 7) rides; serde stays on
  the engine structs for JSON/persistence, bitcode lives only on the wire. The per-component
  conversion cost is bounded by the engine-closed component vocabulary and guarded by the
  round-trip tests. (Supersedes the epic boundary-inventory shorthand of native bitcode *on*
  `ComponentValue`; reconcile that wording at promotion.)
- **`NetworkId` allocation → server-assigned monotonic `u32`, never recycled within a
  session.** No free-list or generation field: a never-reused id lets a delayed packet naming a
  despawned entity be dropped unambiguously — the stale-handle safety the engine's generational
  `EntityId` provides, free at `u32` width. Bounded co-op sessions never approach overflow (the
  slot-exhaustion risk in the epic's ledger is the local `u16` `EntityId`, not this). Phase 6
  layers predicted-spawn → confirmed reconciliation on top without changing allocation.
- **Snapshot cadence → per-tick full-state.** The host serializes the full replicable set every
  server tick. The send-rate and interpolation-delay decoupling that governs bandwidth and
  remote-motion smoothness is Phase 2's, alongside the client interpolation buffer that consumes
  it.
