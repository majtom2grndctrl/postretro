# E16 - Ammo Resource

> **Status:** draft.
>
> **Epic:** 16 - Combat.
>
> **Milestone:** Weapon Systems.
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
- Tests: descriptor parse/validate both runtimes, SDK type generation, magazine
  seed + hot-reload preservation, reserve seed, fire consumption, empty-magazine
  block, atomic reload, unlimited-fire back-compat, HUD publish.

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
  type validates as a free-form ASCII identifier now (see open questions).

## Acceptance criteria

- [ ] Authors can set `components.weapon.resource` (an `ammo`-kind block) in
  TypeScript and Luau. An invalid block (unknown `kind`, empty/overlong/illegal
  `type`, non-finite or negative numeric) is rejected through the existing
  `WeaponDescriptor::validate()` path, identically in both runtimes. An absent
  `resource` parses to `None`.
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
  is consumed, no damage is applied, and the block is observable (a dry-fire
  event name reaches the caller's event drain / `ActivationOutcome::Empty`) — not
  a silent no-op. Cooldown-blocked shots stay silent as today.
- [ ] Reload transfers `min(capacity - magazine, reserve[type])` rounds from the
  pawn reserve pool into the magazine in one step. Reload with a full magazine or
  an empty reserve pool is a distinct blocked outcome, not a partial or silent
  transfer.
- [ ] Hot reload preserves the live `magazine` count (and cooldown) through
  `refresh_from_descriptor` while updating authored capacity/cost/type — an
  implementation that resets the magazine to full on reload fails this criterion.
- [ ] `player.ammo` reflects the active wieldable's live magazine and
  `player.ammoReserve` reflects its ammo type's reserve pool, republished each
  frame; both are readonly engine-owned slots the dev HUD reads through
  `getGameState().player`. Owner-private replication is schema-driven (no netcode
  change per M15 Phase 3.5).
- [ ] No ammo-grant, `onKill`, or resource-grant behavior runs as part of this
  plan (review/grep gate).
- [ ] No heat/cell variant, per-shell reload, pickup, or inventory is built
  (review/grep gate). No new `unsafe` (review/grep gate).

## Tasks

### Task 1: Resource tagged union on the descriptor + SDK types

Add `resource: Option<WeaponResource>` to `WeaponDescriptor` (`crates/foundation/src/data_descriptors/types/combat.rs`).
`WeaponResource` is a serde-tagged (`#[serde(tag = "kind", rename_all = "camelCase")]`)
enum with one variant, `Ammo(AmmoResource)`; the tag reserves `heat`/`cell` for
siblings. `AmmoResource` carries `ammo_type` (wire `type`), `magazine` (u32
capacity), `cost_per_shot` (u32, wire `costPerShot`, default 1), and `reserve`
(u32 starting pool). Validate in `WeaponDescriptor::validate()`: `type` an ASCII
identifier matching the existing `credit_source` charset (`[A-Za-z0-9_.:-]`, ≤64
bytes, non-empty), `magazine ≥ 1`, `cost_per_shot ≥ 1`, `reserve` any u32. The
POD references no `EntityId`, so it stays in `postretro-foundation` per the
descriptor partition rule (`scripting.md`).

Parsing is free once the field is serde — `entity_descriptor_from_js` /
`entity_descriptor_from_lua` deserialize `WeaponDescriptor` via `serde_json`.
Register SDK types in `scripting/primitives/mod.rs`: a `register_tagged_union("WeaponResource")`
with the `ammo` variant → `AmmoResource` type (mirroring the `ComponentValue`
tagged-union registration), and a `resource?` field on the `WeaponDescriptor`
registration. Regenerate committed SDK fixtures.

`WeaponDescriptor` is not `Default`, so adding `resource` requires updating every
`WeaponDescriptor` struct literal (production + tests); `Option` keeps existing
literals a one-field addition (`resource: None`).

### Task 2: Magazine state + effective() extension

Add `magazine: u32` live state to `WeaponComponent` (`crates/entities/src/components/weapon.rs`),
initialized to the descriptor's `magazine` capacity in `from_descriptor` (0 /
absent when `resource: None`). Surface the augmentable numbers through
`effective()` / `EffectiveStats`: capacity, per-shot cost, and the ammo type —
producers read the effective values, not raw component fields, exactly as
`credit_source` does today. Represent "no ammo resource" as an `Option` on the
effective stats so a resourceless weapon skips gating.

`refresh_from_descriptor` refreshes capacity/cost/type (authored tuning) but must
retain the live `magazine` count — it is per-instance state like
`cooldown_remaining_ms`, not authored tuning. Adding a field to `WeaponComponent`
and `EffectiveStats` requires updating their struct literals across the crate.

### Task 3: Pawn reserve pool + spawn seeding

Add an engine-owned reserve component (`AmmoReserve`, entities crate) pooling
rounds by ammo type (`type → u32`). It references no `EntityId`. Seed at
equip-at-spawn: the `player_spawn` path in `scripting/builtins/data_archetype.rs`
(`spawn_from_player_starts`) already resolves the pawn id and the weapon
descriptor and calls `WeaponComponent::from_descriptor_with_canonical`; there,
attach/credit the pawn's `AmmoReserve` for the weapon's ammo type by the
descriptor's starting `reserve`, and let the weapon materialize a full magazine
(Task 2). Mirror the same in the net-slot pawn path
(`scripting/builtins/net_descriptor.rs`) so a remote pawn is armed consistently,
without promoting its weapon to the host's active wieldable.

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
an ammo resource and `magazine < cost_per_shot`, block — resolve no shot, consume
no ammo, and surface the block as a distinct dry-fire event in `WeaponFireEvents`
plus an `ActivationOutcome::Empty` (a new variant beside `Hit`/`Effect`/`Spawned`),
so audio/HUD can react. Otherwise decrement `magazine` by `cost_per_shot` (mirror
how the cooldown is set on fire) and resolve exactly as today. A fired round is
spent on a miss or overkill. A resourceless weapon skips the gate entirely.
Reserve is not touched here.

### Task 5: Reload as atomic transfer

Thread a reload intent from `Action::Reload` (already bound in `input/defaults.rs`)
through `build_sim_command` → `SimCommand` → `weapon_fire_command` →
`WeaponFireCommand` (`crates/postretro/src/{main.rs,sim/mod.rs,weapon/mod.rs}`),
beside the existing fire button. Reload needs both the weapon and the pawn
reserve, so run it in `sim/mod.rs` beside `run_weapon_fire_tick`, where
`local_movement_pawn` already resolves the pawn (which owns `AmmoReserve`).
Query `available(type)`, then atomically `take(type, min(capacity - magazine,
available))` and add the returned rounds to the magazine — never index the pool
directly. The `take` is the atomic step. A full magazine or empty pool is a
distinct blocked outcome (a dedicated event name), not a partial/silent transfer. Reload does not
interrupt cooldown and is not a per-shell state machine (out of scope).

Reload intent rides the command frame like fire, so it replicates through the
existing M15 command-frame path; hit/authority networking stays a Resolution
Modes concern.

### Task 6: HUD slots, publisher, readout, docs, tests

Add `player.ammo` and `player.ammoReserve` to `BUILTIN_ENGINE_STATE`
(`crates/entities/src/engine_state_catalog.rs`): readonly Number,
`OwnerPrivatePlayer` network scope (mirroring `player.health`, so co-op
replication is free per M15 Phase 3.5). Extend/mirror `PlayerHudStatePublisher`
(`scripting/systems/ui_proxy.rs`) to read the active wieldable's live magazine →
`player.ammo` and the pawn reserve's `available(type)` for that weapon's ammo
type → `player.ammoReserve`; the publisher needs the active-wieldable id, which the
`main.rs` call site (`self.active_wieldable`, near the `player_hud_state.tick_for_role`
call) supplies. Restore an ammo readout in `content/dev/scripts/hud.ts` bound to
`getGameState().player.ammo` / `player.ammoReserve`. Seed the reference pistol
(`content/dev/scripts/reference-pistol.ts`) with an `ammo` resource so the loop
is demoable. Update the committed SDK snapshot tests that currently assert ammo's
absence (`scripting/typedef/tests/committed.rs` — `readonly ammo:` /
`ammo: ReadonlyStateRef<number>`). Extend the `## components.weapon` section of
`docs/scripting-reference.md` with the resource block. Add the Rust tests listed
in Scope.

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
`crates/postretro/src/scripting/builtins/data_archetype.rs` (`spawn_from_player_starts`,
`attach_weapon`) and `.../net_descriptor.rs`; the fire/reload command build in
`crates/postretro/src/main.rs` (`build_sim_command`, `fire_button`) and
`crates/postretro/src/sim/mod.rs` (`weapon_fire_command`, `run_weapon_fire_tick`,
`local_movement_pawn`); `Action::Reload` in `crates/postretro/src/input/{types.rs,defaults.rs}`;
the HUD catalog in `crates/entities/src/engine_state_catalog.rs` (`BUILTIN_ENGINE_STATE`,
`player.health` precedent, `ReplicationScope::OwnerPrivatePlayer`) and slot table
in `crates/entities/src/slot_table.rs`; the HUD publisher
`PlayerHudStatePublisher` in `crates/postretro/src/scripting/systems/ui_proxy.rs`
and its call site near `main.rs` `player_hud_state.tick_for_role`; the dev HUD
`content/dev/scripts/hud.ts` reading `getGameState().player`.

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
    #[serde(default = "one")]
    pub cost_per_shot: u32,  // rounds per activation (augmentable via effective())
    pub reserve: u32,        // starting reserve pooled by ammo type, seeded at spawn
}
```

`WeaponComponent` gains `magazine: u32` (live per-instance state, preserved on
hot reload). `EffectiveStats` gains the ammo capacity/cost/type behind the
`effective()` accessor — an `Option`, absent = unlimited-fire. The reserve is a
separate pawn-owned component:

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
| Live magazine (HUD) | `player.ammo` slot ← `WeaponComponent::magazine` | `player.ammo` | `getGameState().player.ammo` | same | n/a |
| Reserve pool (HUD) | `player.ammoReserve` slot ← `AmmoReserve` | `player.ammoReserve` | `getGameState().player.ammoReserve` | same | n/a |

## Open questions

- **`player.ammo` is added fresh, not "retired from a static proxy."** The
  Epic 13 game-state SDK cleanup already deleted the fake ammo HUD entirely —
  there is no live `player.ammo` slot, and the committed SDK snapshot + demo
  tests actively assert its absence. This spec therefore *introduces* the
  engine-owned `player.ammo` / `player.ammoReserve` slots, a publisher, and a
  restored HUD readout, rather than swapping a static value. The intent
  (settled decision: feed a real ammo slot) is unchanged; only the framing
  differs.
- **Ammo `type` stays a free-form charset-validated identifier.** It matches the
  already-shipped `creditSource` precedent — same `[A-Za-z0-9_.:-]` charset, same
  modder-owned-key role; two sibling contracts in one milestone should not
  disagree on how a modder names a category. The generated `AmmoType` union is
  *not* an ammo/inventory concern: it belongs to a future cross-cutting
  "declare-your-categoricals → codegen" capability spanning ammo type, credit
  source, damage type, and status effects — decide it once, there, not here.
  String→union is a compatible tightening (the union is generated from the
  declared values), so it is not a hard-to-reverse shape; deferring is correct.
- **Reserve lives on the pawn as an inventory precursor.** Pooling by ammo type
  is honored, but the durable home is the inventory (`weapon-model.md` §6), out
  of scope here. The `available`/`take` interface (Task 3) is what makes pawn
  ownership safe: it keeps the switching+inventory relocation localized to one
  seam *and* keeps the immersive-sim inventory-backed-storage use case open.
  Nothing downstream indexes the pool directly, so nothing assumes the pawn is
  its permanent owner.
- **`reloadStyle` is omitted, not defaulted.** Atomic magazine reload is the only
  style; the per-shell reload spec introduces the classifier as a resource-block
  field. Adding it now would imply a state machine this spec does not build.
- **Forward compatibility — the reserve's two open use cases.**
  - The `u32` count is a near-term stand-in for inventory-backed storage. An
    inventory that tracks ammo as space-occupying items backs the same
    `available`/`take` interface without touching callers. Per-round unique state
    (per-bullet durability, mixed-ammo magazines) is the only thing the count
    forecloses — additive later by replacing the count with a stack at
    inventory-relocation time; no near-term case needs it.
  - "Takes up space" (weight/volume capacity) is a write-side concern enforced
    when ammo *enters* the reserve — the deferred grant/pickup chokepoint — not
    here. This spec ships no grant and no cap, so it prejudges no capacity.
  - Borderlands-style pooled-by-type ammo is directly expressible today via the
    `type` string plus `costPerShot`; a per-type carry cap is a future additive
    field, not a shape change.
- **`WeaponResource` is one resource kind per weapon.** The single tagged union
  means a weapon that consumes ammo *and* builds heat (or ammo + cell)
  simultaneously is not expressible — the genuinely hard-to-reverse bet, and it
  traces to the `weapon-model.md` "resource is one-of" tagged-union decision, not
  this spec. It is orthogonal to the pooled-ammo and inventory use cases; both
  are single-resource.
