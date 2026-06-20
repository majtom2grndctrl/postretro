# M15 Phase 1 — Grounding notes

> Ephemeral grounding for the Phase 1 spec. Decisions live in `index.md`; the durable
> design reference is `context/research/netcode/`. Discard at ship.

## Seam Phase 0 left (confirmed against source)

- Headless tick entry: `sim::simulate_tick(...) -> TickEvents` (`crates/postretro/src/sim/mod.rs:42`).
  Per-tick command is `SimCommand { movement: MovementInput, fire_button: FireButtonState }`
  (`sim/mod.rs:24`); post-movement aim via a host closure returning `PostMovementCommand`.
- Frame-loop call site: `ApplicationHandler::window_event` → `WindowEvent::RedrawRequested`
  (`main.rs:1618`); the catch-up loop `for tick_index in 0..ticks` is at `main.rs:~2023`,
  calling `sim::simulate_tick` then `frame_timing.push_state(...)` **per tick**
  (`main.rs:~2113`); event drains and `dispatch_system_commands` run **post-loop**
  (`main.rs:~2154`, `~2199`). This loop is where a non-blocking net poll inserts (before the
  loop) and where host-side snapshot serialization / client-side apply hang off.

## Registry apply API (confirmed, `scripting/registry.rs`)

- `spawn(&mut self, transform: Transform) -> EntityId` (655). Allocates a slot, returns the id.
- `despawn(&mut self, id) -> Result<(), RegistryError>` (689).
- `set_component_value(&mut self, id, value: ComponentValue) -> Result<(), RegistryError>` (782)
  — direct `ComponentValue` apply, the path a snapshot-apply step uses.
- `set_component::<T>` (772), `get_component::<T>` (762), `iter_with_kind(kind) ->
  impl Iterator<Item=(EntityId, &ComponentValue)>` (616).
- `EntityId(u32)` = `index:u16 | generation:u16` (24–47); `to_raw`/`from_raw` exist. Server
  and client `EntityId`s do **not** coincide — replication needs a `NetworkId ↔ EntityId` map.
- **No mass-apply / batch-snapshot method exists.** Apply iterates received entities and calls
  `spawn` / `set_component_value` per entity.

## Component-on-the-wire constraint (confirmed)

- `ComponentValue` (`registry.rs:163`) is `#[serde(tag = "kind", rename_all = "snake_case")]`
  — internally-tagged; **cannot** deserialize on bitcode (`DeserializeAnyNotSupported`).
- `ComponentValue` and `Transform` are `pub(crate)` in `postretro`; their fields are glam
  `Vec3`/`Quat` (glam has `serde`/`bytemuck` features, **no** bitcode derives).
- Consequence for the crate split: a sibling `postretro-net` crate cannot name `ComponentValue`,
  and glam fields cannot derive `bitcode::Encode/Decode` directly. → wire-mirror types
  (`index.md` Open questions).

## Ownership invariant (durable, `entity_model.md` §6)

"A server snapshot is applied through a game-logic-owned apply step — the net subsystem
produces typed snapshots and never mutates the registry directly." Phase 1 honors this: the
apply step lives in `postretro`; `postretro-net` emits typed values only.

## Event-loop / async policy (durable, `development_guide.md` §4.2)

winit owns the loop; subsystems never block it. The engine adds no async runtime (only
`pollster` for wgpu init). renet 2.0 is synchronously frame-polled (`update(dt)`, blocking
`std::net::UdpSocket`) — honors §4.2 by design. **No tokio.**

## Workspace wiring (confirmed)

- Members in root `Cargo.toml` `[workspace].members`; sibling crates named `postretro-*`;
  shared fields via `version.workspace = true` etc.; sibling dep declared as
  `postretro-level-format = { workspace = true }` (root `[workspace.dependencies]` line 140,
  `crates/postretro/Cargo.toml:78`).
- No `renet`/`bitcode`/UDP anywhere in the repo today. `Cargo.lock` is checked in.

## renet 2.0 stack (from `crate-pattern-research.md`, verified mid-2026)

renet 2.0 + renet_netcode: Bevy-free, no tokio, sync `update(dt)` poll over blocking
`UdpSocket`. Reliable/unreliable channels cover seq/ack/fragmentation (no extra crate).
renet_netcode does the netcode.io encrypted connection handshake (carries a `protocol_id`).
bitcode 0.6.x for the wire — pin exactly, never persist its bytes, gate on the version
handshake.
