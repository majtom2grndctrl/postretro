# E16--damage-types-and-defense-layers — research

Read at 44881f79. Findings that informed the brief but do not decide it.

## Consolidation

Three roadmap items were weighed together in the draft session:
- **Damage application order** (E16). This brief.
- **Per-entity state reads in pair expressions** (E18). Stays on the roadmap. It is a small additive change (`classify_pair_read`, `SideInput`, `seed_sides`, an SDK spelling). The per-owner store already covers its stated use case: see the healer recipe in `docs/scripting-reference.md`. With engine layers it reframes as pair-scope *layer* reads.
- **Local-slot crossing origin** (E15). Not combat work. The classifier exists: `crossing_machine_source` yields `MachineSource::OwnState` inside `FireOrigin::EveryMachine`. No defect is observed. Fold it into `coop--client-leave-session`. The roadmap's `drafts/reaction-body-composition` reference is stale; that plan is in `done/`.

The roadmap item "damage types / elemental" folds into this brief, because type-scaled drain needs a type on every hit.

## Layer home

The roadmap put layer values in per-entity `@state`. Source:
- `EntityStateComponent` is a schema-less f32 map. Every entity has one.
- Only impact-policy `setState` (on the target) and `adjustSentiment` write it. It has no per-tick writer, and reaction `setState` writes store slots, not entity state.
- It does not replicate. The only path to clients is the overlay's host-sampled `shield_fraction` / `has_shield` on `WireOverlayRoster`.
- `defineStore` rejects `perOwner` + `accumulate`, so recharge cannot live in the store per player either.
- Owner-slot `slot.set` in impact policies binds only to `@impact.source`, so a player's own shield cannot reach their HUD through the store.

## Damage chokepoint

- Every damage source calls `apply_damage_with_context`:
  - weapon impact (hitscan, projectile, splash, and host-validated client hits) via `apply_weapon_impact_damage_with_source`, which applies `zone_multipliers` first;
  - NPC contact via `AiHost::apply_damage`;
  - script `applyDamage` (`DamageProducer::AppDrain`);
  - mover crush (`kinematic_mover/blocking.rs`);
  - traps through the projectile path. Not traced end to end.
- Order inside the chokepoint: subtract, floor, `set_component`, contributor ledger, brain perception, `push_impact_dispatch`. Policies run in `evaluate_pending_in_tick_impacts` after `publish_health_from_registry`, `InTick` only.
- Damage also lands outside the tick: `host_flush_pending_hit_declarations` after the tick, and AppDrain at frame end. The last-damage stamp must therefore come from the chokepoint, not from a stage. The chokepoint has no tick counter today.

## Tick order

`simulate_started_tick_with_presentation_aim` runs these stages in order:
1. `begin_tick` (respawn drain)
2. `tick_deferred_effects`
3. Movers
4. Movement
5. Triggers
6. Projectiles
7. AI
8. Traps
9. Steering (crush)
10. Weapons
11. Death sweep
12. Assist

Recharge at tick start, after the respawn drain, puts every hit in tick N after that tick's recharge.

## Amount consumers

All of these read raw `payload.amount` today:
- the contributor ledger, kill credit and `player_contributor_ledger`;
- brain `record_attacker_damage` (threat and tolerance);
- brain recency (`time_since_damage_ms`, `damage_bearing`, `last_known_target_pos`);
- `ImpactDispatch` facts;
- overlay enrollment. Its `hide_at_full` checks health only.

Drafts that read the amount:
- `E10--enemy-stagger` accumulates `payload.amount` in the chokepoint.
- `coop--predicted-combat-feedback` relies on `@impact.amount` being client-predictable from `zone_multipliers`. Once `amount` means points dealt against host-only layers, it is no longer predictable; that draft moves to `@impact.raw` or to host-confirmed numbers. The owner chose the long-term vocabulary over keeping the drafts unchanged.

## Lifecycle seams

- Spawn: `HealthComponent::from_descriptor`. Hot reload: `refresh_from_descriptor`.
- Respawn: `respawn::drain` → `set_health_absolute`.
- Carry: `CarriedState.health_current`, written by `SeatTable::harvest_pawn_inner` through `CarriedField`, restored by `restore_carried_health` (local and net pawns).
- Same-map restart goes through the same unload path.

## Co-op

- Damage resolves only on the host, and clients predict no health.
- Engine owner-private slots form a fixed `PlayerSlot` enum plus catalog rows. A name-keyed family is a new mechanism.
- Slots are addressed by name on the wire. Adding one changes the state-schema fingerprint, not `WIRE_VERSION` (owner-private weapon-sample precedent, `networking.md` §Version gates).

## Commitments touched

- `entity_model.md` §1: shields are an engine noun, and the component vocabulary is engine-closed. Mods extend by declared data.
- `scripting.md` §11: "the engine ships the component and its per-tick system; the author ships the policy". Interned IR identity is decided, not built (`E16--persistent-firing-modes` proposes building it). The brief avoids that dependency with Bool-per-type facts.
- `research/weapon-model.md` §2–4: type grows inside the payload; augments may add a type. `WeaponComponent::effective()` exists as a passthrough.
- `research/combat-events.md` §4, §8: `damage` is post-mitigation, `rawDamage` is pre-mitigation, and `brokeShield` / `brokeArmor` / `element` are gated on this milestone. `@impact.absorbed` and `@impact.broke` are the substrate those facts reduce over.
- `plans/done/E16--combat-presentation-substrate`: the overlay `shield` reads `@state.shield`, with absorption deferred to Damage & Defenses.
- `plans/done/M10--skeletal-hit-zones`: zone multipliers apply before the chokepoint. The brief keeps that order: zone, then type and layers.

## Roadmap updates at promotion

- Mark "damage application order" and "damage types / elemental" as owned by this brief.
- Narrow "shields + shield types" to special effects.
- Reword the pair-scope item toward layer reads.
- Fold "Local-slot crossing origin" into `coop--client-leave-session` and fix its stale reference.
