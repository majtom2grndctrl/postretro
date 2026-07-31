# Research — Seat, Session Identity, and Roster

Derivation notes for the spec. Decisions live in `index.md`; this file holds the investigation.

## Seat lifecycle

```mermaid
stateDiagram-v2
    [*] --> Unminted
    Unminted --> Bound: slot first reaches participating<br/>(claim recorded, or anonymous)
    Bound --> Bound: level change — demote then re-promote<br/>(harvest on unload, seed on install)
    Bound --> Held: slot closes<br/>(harvest, mark disconnected)
    Held --> Bound: new connection, claim matches exactly<br/>(rebind client id, restore state)
    Held --> Released: hold deadline passes<br/>(drop from roster; number never reissued)
    Released --> [*]
```

The `Bound → Bound` self-edge is the invariant the design turns on. A level change drives a slot out of `Participating` and back in, and `networking.md:98` clears every per-slot value on any such exit. The seat must sit outside that sweep.

## Level-transition carry

```mermaid
sequenceDiagram
    participant U as unload_level
    participant S as SeatTable
    participant R as EntityRegistry
    participant I as install_world_cpu

    Note over U: harvest window opens
    U->>S: harvest per bound seat
    S->>R: read health / reserve / magazines / inventory
    Note over U,R: clear_net_level_parity (:238) — SlotPawns destroyed
    Note over U,R: data_registry.clear() (:274) — descriptors gone
    Note over U,R: clear_for_level_unload (:280-285) — entities despawned
    I->>S: read carried loadout for seat
    S->>I: canonical names, magazines, active slot, reserve, health
    I->>R: compose inventory from names, then override
```

Three ordering facts drive the harvest placement, all confirmed in source:

- `clear_net_level_parity` is the **first** statement of `unload_level` (`startup/lifecycle.rs:238`) and replaces `SlotPawns` wholesale (`netcode/mod.rs:949`). After it, no client id resolves to a pawn.
- `data_registry.clear()` runs at `:274`, before entities die. A harvest needing descriptor lookups must precede it.
- Entities stay live until `clear_for_level_unload` at `:280-285`.

So the harvest window is *before* `:238`, not merely before `:280`.

## Observers

The same carried value is seen from three positions. The cross-product is not uniform, which is why the carry is not one hook.

| Vantage | Health | Ammo reserve | Magazines | Inventory + active slot |
|---|---|---|---|---|
| Single-player / host pawn | Component, harvested and seeded | Component on pawn | Component per weapon entity | `Inventory` on pawn |
| Host-owned remote slot pawn | Same as above | Same | Same | Same |
| Connected client's own pawn | **No component at all** — replicated slot only | Local shadow copy, never read | Local shadow copy, never read | Composed from tuning payload's canonical names |

The client column is why Task 8 exists. Composition follows the host for free because the tuning payload carries canonical names and the host builds it from its live inventory — but magazines and reserve have no wire representation, so they do not follow and must be added.

Warrant for the composition claim, stated because it eliminates work: `WieldableTuningPayload.canonical_name` (`tuning_payload.rs:22`) is built from the host pawn's live inventory, and the client composes through `compose_wieldable_inventory_from_slots` from exactly those names (`net_descriptor.rs:207-213`). Seeding the host pawn before tuning is sent therefore propagates composition. The same warrant does **not** extend to magazines or reserve: `WieldableTuningPayload` (`tuning_payload.rs:21-29`) carries neither, and `TuningPayload` (`:38-42`) carries no ammo at all.

## Why the carried set needs a structured record

`SlotType` is `Number | Boolean | String | Enum | Array` (`slot_table.rs:20-27`), and `SlotValue::Array` is **`Vec<f32>`** (`slot_table.rs:15`) — floats only, no string arrays and no maps.

| Value | Shape | Slot-expressible |
|---|---|---|
| Current health | `f32` | yes — `Number` |
| Active slot | index | yes — `Number` |
| Magazines | per-inventory-slot `u32`, 10 slots | yes — `Array` of `f32` |
| Ammo reserve | `HashMap<String, u32>` (`ammo_reserve.rs:10`) | no — string-keyed map |
| Inventory composition | 10 canonical name strings | no — `Array` holds floats |

Two of five resist, and composition is the load-bearing one: without it the seat cannot rebuild a loadout, which is the carry's purpose. A structured per-seat record is therefore required no matter how much else moves into slots.

The seat still serves as the shared key. A per-seat *scalar* axis and a per-seat *record* coexist on one key without duplicating a store, because they hold disjoint values.

## Where each identity type lives

`net` depends only on renet, renet_netcode, bitcode, and log — postretro-free by contract (`development_guide.md:28`). `entities` depends only on `foundation`, enforced by `layering_invariants_hold` (`crates/xtask/src/crate_graph.rs:496`). There is therefore **no crate from which both `net` and `entities` can name a shared type**.

| Type | Home | Why |
|---|---|---|
| `Seat` | `foundation` | Must be nameable from `entities` when per-seat storage reaches the floor slot table, and from the binary now. Wire carries a bare `u16` |
| `SessionId`, `PlayerClaimId` | `net` | Cross the wire; nothing below the binary names them |
| `SeatTable`, `SeatSessionState` | binary `netcode/` | Hold engine types; `entities` is a compile chokepoint with six dependents that domain logic stays out of (`development_guide.md:39-41`) |

`NetworkId` is the precedent: a wire type in `net` whose allocator lives in the binary. `Seat` inverts it because the *type* must reach the floor while the *table* must not.

## Identity inventory, before this spec

| Concept | Type | Lifetime | Assertable by the player |
|---|---|---|---|
| Connection | bare `u64`, client-minted from wall-clock nanos (`NetEndpoint::from_role`, `NetRole::Connect` arm, `netcode/mod.rs`) | one connection | no — `NetClient` does not even retain it |
| Pawn | `EntityId` (`registry.rs:37`) | one level | no |
| Network entity | `NetworkId` (`wire.rs:32`) | one level (allocator map reset, counter monotonic) | no |
| Within-level player | `PlayerId::{Local(EntityId), Remote(u64)}` (`trigger_system.rs:22`) | one level | no |
| Session | — | — | — |

Nothing durable, nothing assertable. The two arms of `PlayerId` do not even share a lifetime: the remote arm wraps a client id that survives a level change, the local arm wraps an entity id that does not.

## Why the connect token rather than a control message

Every existing feature carries data over `ClientControlMessage`. Choosing the transport field is a deliberate divergence.

- The claim is available on the `ClientConnected` edge, before any app-level message, so seat minting needs no deferred state.
- It is structurally immutable for the connection's life. A control message could be re-sent, so "fixed for this connection" would have to be enforced rather than inherited — and `networking.md:87` warns that putting a mutable value in the admission stage converts a recoverable difference into an unrecoverable disconnect.
- It costs nothing: the field is fixed-width 256 bytes and rides the connect token, not per-tick traffic.

Cost of the divergence: two edits instead of one (the client must populate `user_data`, the host must read it), and the host must read on the connect edge because renet_netcode drops the netcode entry at teardown.

Verified: `ClientAuthentication::Unsecure` carries `user_data: Option<[u8; NETCODE_USER_DATA_BYTES]>` with `NETCODE_USER_DATA_BYTES = 256`; `Unsecure` still transmits it (unsecure means a zero private key, not an absent token); `renet_netcode`'s server exposes `user_data(client_id)`, and discards the value when translating to `renet::ServerEvent::ClientConnected` — so it must be read off the transport, not the event. Confirmed against published crate sources for `renet_netcode` 2.0.0 / `renetcode` 2.0.0; no cargo registry exists in this environment, so Task 1 re-confirms the signature at the compiler.

Non-obvious consequence: renetcode fills an absent `user_data` with **random bytes**, not zeros (`token.rs:193-196`). Absence is therefore undetectable without a magic marker, which is why the envelope has one.

## Closed slots are two populations

`SlotState::Closed` is terminal via two independent mechanisms:

- an early return in `transition` for any slot already closed (`slots.rs:69-72`), which makes every public mutator a silent no-op;
- `entry().or_insert()` in `on_connect` (`slots.rs:59`), which refuses to reset a closed id to pending.

`SlotTable` has no `remove`, `clear`, or `retain` — the only writes are one `or_insert` and two `insert`s — so the map grows for the endpoint's lifetime.

The population split matters more than the terminality. Beyond genuine closes of slots that once participated, `transition` plants a permanent `Closed` tombstone for ids that were **never connected**, deliberately, so stale packets are refused (`slots.rs:79-82`). A reclaim path that treats all closed entries alike reopens that hole.

## Split-before-extend: production versus test lines

Raw line counts overstate the problem. Production halves:

| File | Total | Production | Extended here |
|---|---|---|---|
| `netcode/mod.rs` | 5,985 | 3,203 | yes — endpoint region |
| `startup/lifecycle.rs` | 4,407 | ~1,934 | yes — unload and install |
| `wire.rs` | 2,959 | 1,335 | yes — control region only |
| `data_archetype.rs` | 3,035 | **1,013** | yes — composition block only |
| `state_slots.rs` | 2,449 | ~1,000 | no |
| `transport.rs` | 1,801 | 1,021 | marginally — one arm |

`data_archetype.rs` carries 2,020 lines of tests on 1,013 of production. Splitting it wholesale would move tests, not reduce risk, so the spec extracts only the composition block. `state_slots.rs` and `transport.rs` are left alone: the spec's edits there are small and localized.

## Findings that did not drive decisions

- `collect_server_events` can call `transport.user_data(client_id)` inside its event loop without a borrow conflict. The body already calls `self.close_slot(...)`, which takes `&mut self`, proving the `while let` scrutinee holds no live borrow of `self.server`. No restructure needed.
- `SlotTable::is_accepted` and `accepted_clients` are `#[deprecated]` in favor of the participating spelling.
- On generation overflow, `despawn` retires a registry slot permanently rather than bumping it (`registry.rs:938-943`), so `EntityId` uniqueness holds either way. The conclusion that no entity id survives an unload is sound under both branches.
- `HealthComponent.pending_kill_credit` and `contributor_ledger` are already `#[serde(skip)]` and documented transient, which independently supports excluding them from the carry.
- `WeaponComponent` has no canonical-name field; durable weapon identity comes from `DescriptorProvenance.canonical_name` (`data_archetype.rs:705-713`), which the tuning payload already relies on.
- The shipped plan directories for this epic use two prefixes: `M15--p0…` through `M15--p35…`, then `E15--session-lifecycle`. This draft follows the newer `E15--` form.
