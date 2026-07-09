# E16 - Ammo Resource

> **Status:** draft.
>
> **Epic:** 16 - Combat.
>
> **Milestone:** Weapon Systems.
>
> **Depends on:** **E16 — Client-Authoritative Combat** — this spec's
> co-op-correct behavior needs that seam, the one that lets the host apply a
> remote client's per-pawn combat inputs (fire and reload) beyond today's
> movement-only `host_resolve_movement_inputs`
> (`crates/postretro/src/netcode/command_queue.rs:365`).
>
> **Fits first:** front-loads the hard-to-reverse resource tagged-union
> discriminant and the magazine/reserve/reload contract. Later `heat` / `cell`
> land as sibling variants and the discriminant is expensive to change
> afterward. `onKill` then consumes the clean resource-grant seam this leaves
> open (grant-ammo-on-kill), but this ships no grant policy.

## Goal

Add the first variant of the weapon `resource` tagged union: per-weapon-instance
ammo. A weapon draws from a **magazine** (currently-loaded rounds, live
per-instance state), backed by a **reserve pooled by ammo type** (the union
discriminant keys the pool). Firing consumes the magazine at the activation
chokepoint; an empty magazine blocks the shot and surfaces the block rather than
silently no-opping. **Reload** is an atomic transfer from the reserve into the
magazine. The live magazine and reserve feed a real `player.ammo` HUD readout.

This ships no grant/reward policy and no world pickups. It leaves the
resource-grant chokepoint (inverse of `applyDamage`) as a clean seam a later
`onKill` spec fills.

## Scope

### In scope

- A `resource` field on `WeaponDescriptor`, a serde-tagged union keyed by `kind`
  with a single `ammo` variant now; absent = the current unlimited-fire weapon.
  Parsed in both descriptor runtimes and emitted in generated SDK types.
- The ammo variant carries the ammo `type` (reserve pool key), magazine
  `magazine` (capacity), `costPerShot`, and a starting `reserve`.
- Magazine capacity and per-shot cost flow through the `WeaponComponent::effective()`
  / `EffectiveStats` seam (so later augments/attachments modify them), extended
  the same way `credit_source` was added in the ledger spec.
- A live `magazine` count on `WeaponComponent`, preserved across hot reload like
  `cooldown_remaining_ms`; capacity/cost/type refresh from the descriptor.
- A pawn-owned reserve component pooling rounds by ammo type. Seeded at
  equip-at-spawn from the weapon descriptor's starting reserve; the magazine
  materializes full.
- Firing consumes `costPerShot` from the magazine at `weapon::tick_resolved`. An
  empty magazine blocks the activation and surfaces it (a dry-fire event +
  `ActivationOutcome::Empty`), consuming no ammo and dealing no damage.
- Reload (`Action::Reload`, already bound) as an atomic transfer of
  `min(capacity - magazine, available(type))` from the pawn reserve pool into the
  magazine; blocked when the magazine is full or the reserve is empty, surfaced
  distinctly.
- Live `player.ammo` (magazine) and `player.ammoReserve` (active weapon's pool)
  engine-owned HUD slots, a publisher mirroring `PlayerHudStatePublisher`, and a
  restored ammo readout in the dev HUD.
- A per-owner ammo projection (`player.ammo` / `player.ammoReserve`) for co-op
  remote-client pawns, mirroring the health projection. Single-player and the
  host's own pawn already get correct values from the publisher alone.
- Tests: descriptor parse/validate both runtimes, SDK type generation, magazine
  seed + hot-reload preservation, reserve seed, fire consumption, empty-magazine
  block, atomic reload, unlimited-fire back-compat, HUD publish, net-slot pawn
  arming + host-side reserve seeding, per-owner ammo projection.

### Out of scope

Each is its own later roadmap bullet; the shape only accommodates them.

- `heat` and `cell` resource variants (sibling union variants — later Weapon
  Systems spec) and the per-tick resource update they need.
- Per-shell / incremental cancellable reload and the `reloadStyle` classifier —
  atomic magazine reload is the only style here (per-shell reload spec).
- Weapon switching and inventory. The reserve lives on the pawn as an
  inventory precursor; the switching+inventory spec relocates it and repoints
  `active_wieldable`.
- World ammo-pickup entities (pickup spec). Descriptor-seeded reserve is the
  only ammo source here — see the seeding rationale below.
- Dual-wield, augments/attachments, charge-on-activation, secondary/alt
  activation, viewmodel.
- The resource-grant chokepoint itself and any `onKill` / `onImpact` / `onDamage`
  dispatch. Named as consumers of the grant seam; not built. No ammo is granted
  by any event here.
- A generated `AmmoType` union backed by a declared-ammo-type registry — the
  type validates as a free-form ASCII identifier now (see design decisions below).
- Host-side application of a remote client's fire/reload commands to their own
  pawn — that command-application seam belongs to the prerequisite spec **E16
  — Client-Authoritative Combat**. This spec seeds the reserve (Task 3)
  and projects it per-owner (Task 7); it does not apply the remote command.

## Acceptance criteria

- [ ] Authors can set `components.weapon.resource` (an `ammo`-kind block) in
  TypeScript and Luau. An absent `resource` parses to `None`. Rejection happens
  at two layers: (a) serde-deserialize rejections — unknown `kind`, or a
  wrong-type or negative value for any `u32` field — fail before
  `WeaponDescriptor::validate()` runs; (b) `validate()` rejections —
  empty/overlong/illegal `type` (charset), `magazine < 1`, `cost_per_shot < 1`.
  Both layers reject identically across the QuickJS and Luau runtimes, which
  are behavioral twins for descriptor parsing (`scripting.md` §1).
- [ ] Generated TypeScript and Luau SDK types include the `resource` tagged union
  on `WeaponDescriptor` with an `ammo` variant carrying `type`, `magazine`,
  `costPerShot`, `reserve`, all camelCase, identical in both runtimes.
- [ ] A weapon with `resource: None` fires with no magazine gating — the current
  single-weapon fire tests pass unchanged (back-compat).
- [ ] At equip-at-spawn a weapon with an ammo resource materializes with a full
  magazine (`= magazine` capacity), and the pawn's reserve pool for that ammo
  type is credited the descriptor's starting `reserve`.
- [ ] Firing consumes `costPerShot` from the magazine; the shot resolves exactly
  as today (hit-zone multiplier, ledger attribution, impact FX unchanged). The
  reserve is untouched by firing.
- [ ] With magazine `< costPerShot` the trigger blocks: no shot resolves, no ammo
  is consumed, no damage is applied, and the block is observable — both a
  dry-fire event name reaching the caller's event drain AND
  `ActivationOutcome::Empty`, not a silent no-op. Cooldown-blocked shots stay
  silent as today.
- [ ] Reload transfers `min(capacity - magazine, available(type))` rounds from the
  pawn reserve pool into the magazine in one step. Reload with a full magazine or
  an empty reserve pool is a distinct blocked outcome, not a partial or silent
  transfer.
- [ ] Hot reload preserves the live `magazine` count (and cooldown) through
  `refresh_from_descriptor` while updating authored capacity/cost/type — an
  implementation that resets the magazine to full on reload fails this criterion.
- [ ] `player.ammo` reflects the active wieldable's live magazine and
  `player.ammoReserve` reflects its ammo type's reserve pool, republished each
  frame; both are readonly engine-owned slots the dev HUD reads through
  `getGameState().player`. This is correct for single-player and for the
  host's own pawn via the publisher. Correct per-owner values for a co-op
  remote client's pawn need the separate per-pawn projection (Task 7) — an
  `OwnerPrivatePlayer` slot declaration alone does not make per-owner values
  correct (see Task 7's own AC). With the active weapon's `resource: None`
  (effective ammo `None`) or no active wieldable (no pawn / fly-camera), the
  publisher skips the write, matching how the health publisher handles the
  no-pawn case — the slots keep their last value rather than publishing a
  stale 0.
- [ ] No ammo-grant, `onKill`, or resource-grant behavior runs as part of this
  plan (review/grep gate).
- [ ] No heat/cell variant, per-shell reload, pickup, or inventory is built
  (review/grep gate). No new `unsafe` (review/grep gate).
- [ ] A net-slot pawn materializes host-side with its descriptor-seeded reserve
  and a full magazine (Task 3 seeding). Ammo's deliverable for that pawn is the
  seeded reserve plus the per-owner projection (Task 7) that surfaces it to
  its owner; once **E16 — Client-Authoritative Combat** applies a remote
  client's reload to their pawn, that transfer draws against this same
  host-side reserve. Applying the remote reload command itself is the
  prerequisite spec's job, not built here.
- [ ] A co-op remote client observes their own pawn's `player.ammo` /
  `player.ammoReserve`, not the host's, via the per-pawn projection (Task 7).

## Tasks

### Task 1: Resource tagged union on the descriptor + SDK types

Add `resource: Option<WeaponResource>` to `WeaponDescriptor` (`crates/foundation/src/data_descriptors/types/combat.rs`).
`WeaponResource` is a serde-tagged (`#[serde(tag = "kind", rename_all = "camelCase")]`)
enum with one variant, `Ammo(AmmoResource)`; the tag reserves `heat`/`cell` for
siblings. `AmmoResource` carries `ammo_type` (wire `type`), `magazine` (u32
capacity), `cost_per_shot` (u32, wire `costPerShot`, `#[serde(default =
"default_cost_per_shot")]` — a `fn default_cost_per_shot() -> u32 { 1 }`
defined in `combat.rs`, matching the `default_credit_source` convention
(`weapon.rs`)), and `reserve` (u32 starting pool). Validate in
`WeaponDescriptor::validate()`: `type` an ASCII
identifier matching the existing `credit_source` charset (`[A-Za-z0-9_.:-]`, ≤64
bytes, non-empty), `magazine ≥ 1`, `cost_per_shot ≥ 1`, `reserve` any u32. The
POD references no `EntityId`, so it stays in `postretro-foundation` per the
descriptor partition rule (`scripting.md`).

Parsing is free once the field is serde — `entity_descriptor_from_js` /
`entity_descriptor_from_lua` deserialize `WeaponDescriptor` via `serde_json`.
Register SDK types in `scripting/primitives/mod.rs`, in order: first
`register_type("AmmoResource")` with its `type`/`magazine`/`costPerShot`/
`reserve` fields, then `register_tagged_union("WeaponResource")` with the
`ammo` variant pointing at that now-registered `AmmoResource` type (mirroring
the `ComponentValue` tagged-union registration), and a `resource?` field on the
`WeaponDescriptor` registration. Regenerate committed SDK fixtures.

`WeaponDescriptor` is not `Default`, so adding `resource` requires updating every
`WeaponDescriptor` struct literal (production + tests); `Option` keeps existing
literals a one-field addition (`resource: None`).

### Task 2: Magazine state + effective() extension

`effective(&self)` takes no descriptor
(`crates/entities/src/components/weapon.rs:63`), so — exactly as `damage` /
`range` / `credit_source` are stored — `WeaponComponent` must also store the
raw ammo tuning to return it from `effective()`, not just the live magazine.
Add to `WeaponComponent`: a stored ammo tuning `Option<{ ammo_type, capacity,
cost_per_shot }>` (absent when `resource: None`) plus the live `magazine: u32`.
`from_descriptor` initializes both from the descriptor's `AmmoResource` (0 /
absent for either when `resource: None`), with the magazine materializing at
full capacity. Surface the augmentable numbers through `effective()` /
`EffectiveStats`: capacity, per-shot cost, and the ammo type — producers read
the effective values, not raw component fields, exactly as `credit_source`
does today. Represent "no ammo resource" as an `Option` on the effective stats
so a resourceless weapon skips gating.

`refresh_from_descriptor` overwrites the stored tuning (type/capacity/cost)
from the descriptor but must retain the live `magazine` count — it is
per-instance state like `cooldown_remaining_ms`, not authored tuning. Adding
the tuning `Option` and the `magazine` field to `WeaponComponent`, and the
mirrored fields to `EffectiveStats`, requires updating their struct literals
across the crate.

### Task 3: Pawn reserve pool + spawn seeding

Add an engine-owned reserve component (`AmmoReserve`, entities crate) pooling
rounds by ammo type (`type → u32`). It references no `EntityId`. The pawn and
its active wieldable are separate entities: inside `attach_descriptor_components`
only the single spawned entity's `id` is in scope, and for the weapon spawn
that `id` is the weapon entity, not the pawn — a pawn-owned `AmmoReserve`
cannot be seeded there. Seed where pawn `id`, weapon `id`, and the weapon
descriptor all coexist: in `spawn_from_player_starts`
(`crates/postretro/src/scripting/builtins/data_archetype.rs:654`), at the
default-weapon spawn that resolves `weapon_id` (~:744) — attach/credit the
pawn's `AmmoReserve` for the weapon's ammo type by the descriptor's starting
`reserve`, and let the weapon materialize a full magazine (Task 2). The
net-slot path seeds at the analogous site in `net_descriptor.rs`'s
`spawn_net_slot_pawn` (~:104), where the same three values coexist for the
host-authoritative remote pawn. A net-slot pawn needs its reserve seeded
host-side so its owner has a pool to draw from once reload reaches it —
applying that owner's reload command host-side is not built here; see Task 7
and **E16 — Client-Authoritative Combat**. The net path's only distinct
concern remains spawning the sibling weapon instance but never promoting it
to the host's active wieldable.

`AmmoReserve` exposes a small interface with its backing map private: a query
`available(type) -> u32` (rounds on hand for a type) and an atomic consume
`take(type, n) -> u32` (removes up to `n`, returns the amount actually removed).
All reserve access — the reload transfer (Task 5) and the HUD publisher (Task 6)
— goes through this interface, never direct map indexing. That makes the later
switching+inventory relocation and any inventory-backed storage a single-seam
change: swap the backing store, callers unchanged.

**Seeding rationale (the judgment call).** World pickups are out of scope, so a
test map has no other ammo source. Descriptor-seeded starting reserve at spawn is
grounded in the one path that already reads the weapon descriptor *and* knows the
pawn; it needs no new entity kind and no dev-only grant. A dev grant primitive is
rejected — it would foreshadow the resource-grant chokepoint this spec keeps out
of scope.

### Task 4: Fire-tick consumption + empty-magazine block

In `weapon::tick_resolved` (`crates/postretro/src/weapon/mod.rs`), after the
cooldown gate passes and the weapon wants to fire: if the effective stats carry
an ammo resource and `magazine < cost_per_shot`, block — resolve no shot,
consume no ammo, and surface the block as `ActivationOutcome::Empty` (a new
variant beside `Hit`/`Effect`/`Spawned`). `ActivationOutcome` alone does not
reach the caller: it rides inside `WeaponImpact`, which requires a
`point`/`normal` a dry fire has neither, and `WeaponFireEvents`
(`activate`/`impact`, reported by `event_names()`) has no field for it. Add a
carrier `event_names()` reports — a `dry_fire: bool` field, or broadening to
`outcome: Option<ActivationOutcome>` — so the dry-fire block actually reaches
the caller's event drain and audio/HUD can react. Otherwise decrement
`magazine` by `cost_per_shot` (mirror how the cooldown is set on fire) and
resolve exactly as today. A fired round is spent on a miss or overkill. A
resourceless weapon skips the gate entirely. Reserve is not touched here.

### Task 5: Reload as atomic transfer

The prerequisite **E16 — Client-Authoritative Combat** introduces the
`reload` field on `SimCommand` — sampled from `Action::Reload` (already bound in
`input/defaults.rs`) as a rising edge (`ButtonState::Pressed`, per the dash
precedent in `input.md`, so a held R does not re-attempt every tick), carried on
the wire `InputCommand` with the single `WIRE_VERSION` bump, and defaulted in
`neutral_sim_command`. This spec does **not** re-add that field or bump the wire.

This spec consumes it. In `sim/mod.rs` beside `run_weapon_fire_tick`, where
`local_movement_pawn` resolves the local pawn (which owns `AmmoReserve`), read
the resolved `SimCommand.reload`: when set, query `available(type)` then
atomically `take(type, min(capacity - magazine, available))` and add the
returned rounds to the magazine — never index the pool directly. The `take` is
the atomic step. A full magazine or empty pool is a distinct blocked outcome (a
dedicated event name), not a partial/silent transfer. Reload does not interrupt
cooldown and is not a per-shell state machine (out of scope). Reload skips the
`weapon_fire_command` → `WeaponFireCommand` hop entirely — that command is
aim/fire only.

This wires the transfer for the local/host player's own pawn. Applying a remote
client's reload to their pawn host-side is the prerequisite's named per-pawn
delivery seam `deliver_reload_to_weapon`, which routes the reload intent to the
mapped weapon and calls this spec's transfer.

### Task 6: HUD slots, publisher, readout, docs, tests

Add `player.ammo` and `player.ammoReserve` to `BUILTIN_ENGINE_STATE`
(`crates/entities/src/engine_state_catalog.rs`): readonly Number,
`OwnerPrivatePlayer` network scope, matching `player.health`'s slot
declaration. Single-player / host-pawn correctness follows from the publisher
below; co-op per-owner correctness needs the projection in Task 7. Adding
these two `OwnerPrivatePlayer` slots breaks two existing tests in
`engine_state_catalog.rs`'s test module — update both:
`built_in_catalog_preserves_wire_names_and_capabilities` (a hard-coded
wire-name vector) and `player_health_slots_are_owner_private_replicated`
(asserts every non-health slot is `None`). Extend/mirror `PlayerHudStatePublisher`
(`scripting/systems/ui_proxy.rs`) to read the active wieldable's live magazine →
`player.ammo` and the pawn reserve's `available(type)` for that weapon's ammo
type → `player.ammoReserve`; the publisher needs the active-wieldable id, which the
`main.rs` call site (`self.active_wieldable`, near the `player_hud_state.tick_for_role`
call) supplies. With `resource: None` or no active wieldable (no pawn /
fly-camera), skip the write, matching the health publisher's no-pawn handling.
Restore an ammo readout in `content/dev/scripts/hud.ts` bound to
`getGameState().player.ammo` / `player.ammoReserve`. Seed the reference pistol
(`content/dev/scripts/reference-pistol.ts`) with an `ammo` resource so the loop
is demoable. Update the committed SDK snapshot tests that currently assert ammo's
absence (`scripting/typedef/tests/committed.rs` — `readonly ammo:` /
`ammo: ReadonlyStateRef<number>`). Extend the `## components.weapon` section of
`docs/scripting-reference.md` with the resource block. Add the Rust tests listed
in Scope.

### Task 7: Per-owner ammo projection (co-op)

Single-player and the host's own pawn get correct ammo from the Task 6
publisher alone. A co-op remote client does not: `player.ammo` /
`player.ammoReserve` are `OwnerPrivatePlayer` slots, but without a per-pawn
source they fall back to the slot table's single global value
(`owner_private_source_value`,
`crates/postretro/src/netcode/state_slots.rs:441-452`) — every owner would see
the host's ammo. Add an ammo-specific per-pawn projection alongside
`descriptor_health_for_pawn`
(`crates/postretro/src/netcode/state_slots.rs:460`): for `player.ammo` /
`player.ammoReserve`, read the given pawn's active-weapon live magazine and
that pawn's `AmmoReserve` for its ammo type, mirroring how
`descriptor_health_for_pawn` reads `HealthComponent` per-owner. This is
ammo's own projection over state already seeded by Task 3 — it is not the
host-apply seam that lets a remote client's fire/reload commands run against
their own pawn; that seam is **E16 — Client-Authoritative Combat**.

## Sequencing

**Phase 1 (sequential):** Task 1 → Task 2 → Task 3 — descriptor union, component
magazine/effective, and the reserve component + spawn seed all edit shared
struct-literal call sites and the spawn path, so they run in sequence.
**Phase 2 (sequential):** Task 4 — consumes the magazine owner at the fire
chokepoint.
**Phase 3 (sequential):** Task 5 — adds the reload transfer over the same tick
and the pawn reserve.
**Phase 4 (sequential):** Task 6 — publishes the HUD slots and documents/tests
the completed surface.
**Phase 5 (sequential):** Task 7 — adds the co-op per-pawn ammo projection
over the reserve seeded in Task 3.

## Rough sketch

Grounded identifiers: `WeaponDescriptor` and its `validate()` in
`crates/foundation/src/data_descriptors/types/combat.rs` (re-exported as
`postretro_foundation::WeaponDescriptor`); `WeaponComponent` /
`EffectiveStats` / `effective()` / `from_descriptor_with_canonical` /
`refresh_from_descriptor` in `crates/entities/src/components/weapon.rs`;
`ActivationOutcome::{Hit, Effect, Spawned}`, `WeaponImpact`, `WeaponFireCommand`,
`WeaponFireEvents`, and `tick_resolved` in `crates/postretro/src/weapon/mod.rs`;
descriptor parsers `entity_descriptor_from_js` / `entity_descriptor_from_lua` in
`crates/scripting-core/src/data_descriptors/{js,lua}/entity.rs`; SDK registry
(`register_tagged_union`, `register_enum`, `WeaponDescriptor` fields) in
`crates/postretro/src/scripting/primitives/mod.rs`; equip-at-spawn in
`crates/postretro/src/scripting/builtins/data_archetype.rs`
(`spawn_from_player_starts`) and `.../net_descriptor.rs`
(`spawn_net_slot_pawn`); the fire command build in
`crates/postretro/src/main.rs` (`build_sim_command`, `fire_button`); `reload`
arrives on `SimCommand` from the prerequisite spec and is consumed in
`crates/postretro/src/sim/mod.rs` (`weapon_fire_command`, `run_weapon_fire_tick`,
`local_movement_pawn`, `SimCommand.reload`);
the wire `SimCommand` literal sites: `neutral_sim_command` and
`host_resolve_movement_inputs` in `crates/postretro/src/netcode/command_queue.rs`,
and `input_command_to_sim` / `sim_command_to_input` in
`crates/postretro/src/netcode/wire_convert.rs`; the HUD catalog in
`crates/entities/src/engine_state_catalog.rs` (`BUILTIN_ENGINE_STATE`,
`player.health` precedent, `ReplicationScope::OwnerPrivatePlayer`) and slot table
in `crates/entities/src/slot_table.rs`; the HUD publisher
`PlayerHudStatePublisher` in `crates/postretro/src/scripting/systems/ui_proxy.rs`
and its call site near `main.rs` `player_hud_state.tick_for_role`; the dev HUD
`content/dev/scripts/hud.ts` reading `getGameState().player`; the per-owner
replication seam `owner_private_source_value` / `descriptor_health_for_pawn` in
`crates/postretro/src/netcode/state_slots.rs`.

Proposed shape:

```rust
// Proposed design (foundation crate — no EntityId, stays in postretro-foundation).
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum WeaponResource {
    Ammo(AmmoResource),   // first variant; `heat` / `cell` are sibling variants later
}

#[serde(rename_all = "camelCase")]
pub struct AmmoResource {
    #[serde(rename = "type")]
    pub ammo_type: String,   // reserve-pool key; validated as the credit_source charset
    pub magazine: u32,       // full-magazine capacity (an augmentable stat via effective())
    #[serde(default = "default_cost_per_shot")]
    pub cost_per_shot: u32,  // rounds per activation (augmentable via effective())
    pub reserve: u32,        // starting reserve pooled by ammo type, seeded at spawn
}
```

`WeaponComponent` gains a stored ammo tuning `Option<{ ammo_type, capacity,
cost_per_shot }>` (authored, refreshed on hot reload) plus `magazine: u32`
(live per-instance state, preserved on hot reload). `EffectiveStats` gains the
ammo capacity/cost/type behind the `effective()` accessor — an `Option`,
absent = unlimited-fire. The reserve is a separate pawn-owned component:

```rust
// Proposed design (entities crate — pawn-owned inventory precursor).
pub struct AmmoReserve {
    pools: HashMap<String, u32>,  // private; ammo type -> rounds
}

impl AmmoReserve {
    // The relocation seam: an inventory-backed store implements these two
    // unchanged, so switching+inventory swaps the backing store, callers as-is.
    pub fn available(&self, ammo_type: &str) -> u32 { /* rounds on hand */ }
    pub fn take(&mut self, ammo_type: &str, n: u32) -> u32 { /* remove ≤ n, return taken */ }
}
```

Reload queries `available(type)`, then atomically `take`s
`min(capacity - magazine, available)` into the magazine. Firing decrements
`magazine` by the effective `cost_per_shot`; an empty magazine yields
`ActivationOutcome::Empty` and a dry-fire event, never a silent no-op.

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
| --- | --- | --- | --- | --- | --- |
| Resource discriminant | `WeaponResource` tag | `"kind"` (`"ammo"`) | `components.weapon.resource.kind` | same | n/a |
| Ammo type | `AmmoResource::ammo_type`, `EffectiveStats` ammo type | `"type"` | `resource.type` | same | n/a |
| Magazine capacity | `AmmoResource::magazine`, `EffectiveStats` capacity | `"magazine"` | `resource.magazine` | same | n/a |
| Cost per shot | `AmmoResource::cost_per_shot`, `EffectiveStats` cost | `"costPerShot"` | `resource.costPerShot` | same | n/a |
| Starting reserve | `AmmoResource::reserve` | `"reserve"` | `resource.reserve` | same | n/a |
| Stored ammo tuning (component) | `WeaponComponent` ammo tuning `Option` (`ammo_type`/`capacity`/`cost_per_shot`), refreshed by `refresh_from_descriptor` | n/a | n/a | n/a | n/a |
| Live magazine (HUD) | `player.ammo` slot ← `WeaponComponent::magazine` | `player.ammo` | `getGameState().player.ammo` | same | n/a |
| Reserve pool (HUD) | `player.ammoReserve` slot ← `AmmoReserve` | `player.ammoReserve` | `getGameState().player.ammoReserve` | same | n/a |

## Design decisions & rationale

- **`player.ammo` is added fresh, not retired from a static proxy.** The Epic 13
  game-state SDK cleanup deleted the fake ammo HUD entirely — no live
  `player.ammo` slot survives, and the committed SDK snapshot + demo tests assert
  its absence. This spec *introduces* the engine-owned `player.ammo` /
  `player.ammoReserve` slots, a publisher, and a restored readout — it does not
  swap a static value. The decision (feed a real ammo slot) is unchanged; only
  the framing is corrected.
- **Ammo `type` stays a free-form charset-validated identifier.** It matches the
  shipped `creditSource` precedent — same `[A-Za-z0-9_.:-]` charset, same
  modder-owned-key role; two sibling contracts in one milestone should not
  disagree on how a modder names a category. A generated `AmmoType` union is not
  an ammo/inventory concern: it belongs to a future cross-cutting
  "declare-your-categoricals → codegen" spec spanning ammo type, credit source,
  damage type, and status effects — decided once, there. String→union is a
  compatible tightening — the union is generated from the declared values — so
  the shape is not hard to reverse and deferral is safe.
- **Reserve lives on the pawn as an inventory precursor.** Pooling by ammo type
  is honored; the durable home is the inventory (`weapon-model.md` §6), out of
  scope here. The `available`/`take` interface (Task 3) makes pawn ownership
  safe — it localizes the later switching+inventory relocation to one seam and
  keeps the immersive-sim inventory-backed-storage use case open. Nothing
  downstream indexes the pool directly, so nothing assumes the pawn is its
  permanent owner.
- **`reloadStyle` is omitted, not defaulted.** Atomic magazine reload is the only
  style; the per-shell reload spec introduces the classifier as a resource-block
  field. Adding it now would imply a state machine this spec does not build.
- **The reserve keeps two use cases open.**
  - The `u32` count is a near-term stand-in for inventory-backed storage. An
    inventory tracking ammo as space-occupying items backs the same
    `available`/`take` interface without touching callers. Per-round unique state
    (per-bullet durability, mixed-ammo magazines) is the only thing the count
    forecloses — additive later by replacing the count with a stack at
    relocation time; no near-term case needs it.
  - "Takes up space" (weight/volume capacity) is a write-side concern enforced
    when ammo *enters* the reserve — the deferred grant/pickup chokepoint — not
    here. This spec ships no grant and no cap, so it prejudges no capacity.
  - Borderlands-style pooled-by-type ammo is directly expressible today via the
    `type` string plus `costPerShot`; a per-type carry cap is a future additive
    field, not a shape change.
- **One resource kind per weapon.** The single `WeaponResource` tagged union
  means a weapon that consumes ammo *and* builds heat (or ammo + cell)
  simultaneously is not expressible — the one genuinely hard-to-reverse bet. It
  traces to the `weapon-model.md` "resource is one-of" decision, not this spec,
  and is orthogonal to the pooled-ammo and inventory use cases; both are
  single-resource.
