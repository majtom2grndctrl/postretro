# Research — Per-Player Currency

Findings behind the spec's decisions, including why its shape changed twice.

## Code grounding

| Claim | Source |
|---|---|
| A slot record holds one global value; the mod-facing `ownerPrivate` declaration is actively rejected with a named diagnostic | `crates/entities/src/slot_table.rs`; `crates/scripting-core/src/store_bridge.rs` — `replication_scope_for` |
| The replication scope and its enum names shipped; only the mod-facing declaration was withheld, "until a per-player authoring namespace exists" | `context/plans/done/M15--p35-state-slot-replication/index.md` |
| The owner-private resolver dispatches per-owner projections before falling through to one global value served to every owner | `crates/postretro/src/netcode/state_slots.rs` — `owner_private_source_value` |
| Level unload despawns every entity and bumps generations, so an old entity id can never revalidate | `crates/entities/src/registry.rs` — `clear_for_level_unload`, called from `startup/lifecycle.rs` |
| The existing player identity is pawn-scoped on one arm and connection-scoped on the other, and is crate-private to the binary | `crates/postretro/src/trigger_system.rs` — `PlayerId::{Local, Remote}` |
| Every existing per-player consumer is within-level: trigger occupancy, use edges, alive players, canonical pawns | `crates/postretro/src/sim/mod.rs`, `crates/postretro/src/trigger_system.rs` |
| A connected client does not write the save file | `crates/postretro/src/main.rs` — `should_save_persisted_state(can_save, is_connected_client) = can_save && !is_connected_client` |
| `slot.add` rejects any target today and lowers to a self-referential add on a global slot | `crates/postretro/src/impact_policy.rs` — `bind_effect` |
| The HUD publisher republishes player slots each frame from local state, skipping rather than resetting when a source is absent | `crates/postretro/src/scripting/systems/ui_proxy.rs` |
| The activators-or-tag dual is shipped on a damage builder — the shape the reaction write path copies | `sdk/lib/data_script.ts` |

## Two reviews, two shape changes

**First shape — a slot as a view of a per-entity state field on the owning pawn.**
`/validate-plan`: under-scoped. Three consequences fell out of that one
placement: the value could not persist (components die with the level); it could
be earned only by dealing damage (per-entity state has one write site, inside an
impact policy); and fusing cardinality with backing into one declaration key
meant a non-pawn-backed per-player slot would need a second spelling for the same
replication scope. It also proposed overturning a `scripting.md` §11 invariant
that `E10--enemy-aggro-model` (in-progress) records in its own durable-decisions
table and is actively building on.

**Second shape — owner-instanced store slots keyed by the existing player id,
with device-local persistence and a join seed.** `/validate-plan`: under-scoped
again, on two verified counts.

- **Identity lifetime.** The existing player id is a pawn id or a connection id;
  level unload invalidates the first and a reconnect the second. But the store
  is explicitly never cleared on level unload (`scripting.md` §5). So per-owner
  values would have reset at every level change while global slots survived —
  failing in single-player, and failing the spec's own walkthrough criterion
  that "the only difference in the script is which slot each writes."
- **Persistence reversed a shipped rule silently.** `should_save_persisted_state`
  bars a connected client from writing the save file at all, and a
  client-to-host state write is a stated Phase 3.5 non-goal. The spec asserted a
  guest's values "persist to its own device on exit like any other player's" and
  named neither divergence — while citing Phase 3.5 approvingly for the
  declaration spelling in the same section.

**Current shape.** The seat closes the identity gap; persistence and the join
seed move to `E16--per-player-persistence`, where reversing the client-save rule
is a deliberate decision with its own Direction rather than an assumption inside
a currency spec.

## Why the seat is not its own spec

Every existing per-player consumer is within-level and rebuilt per level, so a
pawn id has always been sufficient — a value that outlives the level is the
first thing needing more. That leaves exactly one consumer requiring durability:
this spec. A standalone seat spec would ship a mechanism with no observable
outcome, and the epic's own precedent runs the other way — the impact substrate
bundled the per-entity-state keystone with the impact dispatch because the
keystone could not stand below it.

## Cardinality and replication are separate axes

Phase 3.5 reserved the `ownerPrivate` *spelling*; it did not decide that
cardinality and replication scope are one concept. Fusing them makes two things
inexpressible: a per-player value the HUD never shows (host-side bookkeeping),
and any future shared-but-privately-delivered value. Keeping them separate costs
one cross-check at load — `ownerPrivate` requires per-owner cardinality, since a
single global value fanned privately to each owner is meaningless.
