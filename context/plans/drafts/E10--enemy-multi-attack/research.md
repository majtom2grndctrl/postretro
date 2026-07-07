# Research notes — Enemy Multi-Attack

Grounding verified against source 2026-07-07. Line numbers from that tree; treat as hints, not contracts.

## Weapon system

- `WeaponDescriptor { damage, range, cooldown_ms ("fireRateMs"), fire_mode, resolution }` — `crates/foundation/src/data_descriptors/types/combat.rs:28-35`. `ResolutionMode { Hitscan }` single-variant at :19-21 — the extension point for `Contact` (and later `Projectile`). `validate()` :38-64.
- Rides on `EntityTypeDescriptor` as `weapon: Option<WeaponDescriptor>` + `default_weapon: Option<String>` — `crates/entities/src/data_descriptors/types/entity.rs:176-186`. The `default_weapon` canonical-name reference is the precedent for `AttackDescriptor::weapon`.
- Fire tick is pawn-agnostic: `tick_resolved(registry, active_wieldable, &WeaponFireCommand, collision_world, hit_zone_store, anim_time, tick_dt)` — `crates/postretro/src/weapon/mod.rs:133-184`. Player coupling is all call-site: single `App.active_wieldable` slot (`main.rs:513-514`), camera-aimed `WeaponFireCommand` (`sim/mod.rs:160-184`), damage + zone scaling applied in `run_weapon_fire_tick` (`sim/mod.rs:198-241`), not in the weapon.
- Ray primitives: `collision::cast_ray` (`collision/mod.rs:172-182`, parry3d TriMesh) + `hit_zones::nearest_entity_hit` (`hit_zones.rs:540-547`) + nearest-of resolution (`weapon/mod.rs:234-247`). **No shooter-exclusion parameter exists** — a ray from inside the firer's hitbox can self-hit. Zero-HP targets already skipped (`hit_zones.rs:574`).
- Map-placed archetypes spawn with `attach_weapon: false` (`data_archetype.rs:607`) — enemies do not get `WeaponComponent` today, and one entity cannot carry two (dense per-kind columns). Hence spawn-time stat resolution into brain tuning instead of companion entities.

## AI FSM

- `LogicalState { Idle, Alert, Attack, Death }`, engine-closed — `crates/entities/src/components/brain.rs:28-39`.
- `AiTuning::from_descriptor` 1:1 copy — brain.rs:108-125; `BrainComponent.attack_cooldown_remaining_ms` — brain.rs:135-137.
- `evaluate_transition(player_pos, agent_pos, tuning, current, evaluate_acquisition)` — `ai.rs:216-222`; attack-range edges every tick (not strided) at :246-252, :266-273.
- Attack fire: `Attack && cooldown <= 0 && selected_target_alive` (:632-638) → apply pass `apply_damage(... tuning.attack_damage)` (:757-765) + `ENEMY_ATTACK_EVENT` (:767) + `restart_animation_clip` in-state replay (:782-789). Damage is cooldown-gated, not animation-synced (comment :775-776).
- `EnemyOutcome` (:409-424) carries the compute→apply handoff — selection rides here.
- Spawn validation of animation mappings: `validate_brain_animation_states` — brain.rs:212-244.

## Descriptor conventions

- Wire camelCase ↔ Rust snake_case via `#[serde(rename_all = "camelCase")]` (combat.rs:172); `FireMode`/`ResolutionMode` enum values are camelCase (combat.rs:11, :18).
- Both runtimes funnel through one serde parse + shared `validate()` — `js/entity.rs:103-112`, `lua/entity.rs:136-145`; twins cannot diverge, only SDK typing files need manual updates (`sdk/types/postretro.d.ts:233-250`, `.d.luau:232-249`) plus committed typedef fixtures under `scripting/typedef/tests/fixtures/`.
- No `Vec<Struct>` field exists in the entity-descriptor tree today; nearest precedents: `ReactionDescriptor::Sequence(Vec<SequenceStep>)` (`entities/.../reactions.rs:15-24`, twinned parsers) and the name-keyed `MeshDescriptor.animations` map with pathed per-entry errors (`entity.rs:33-148`). Ordered `Vec` chosen because declaration order is the deterministic selection priority.

## Coordination

- `E10--enemy-combat-positioning` (ready): single `engagement_radius: f32` on `CombatQuery`; composes onto the chase-destination write at `ai.rs:545`. Multi-attack defines what feeds it (selected attack's band).
- `E10--enemy-facing-slew` (ready): pins `FACING_TURN_RATE` beside `MOVE_SPEED_EPSILON`; defers an attack-windup facing lock — windups are out of scope here too, so the lock stays unowned.
- Replication: only `Transform` + mesh animation state name cross the wire (`net/src/wire.rs:181-183`); per-attack states replicate for free, in-state restarts do not.

## File sizes (split-first triggers)

`ai.rs` 839 + `ai_tests.rs` 2130 (`#[path]`-included) — split is Task 1. Narrow-point edits land in already-oversized `main.rs` (6859), `data_archetype.rs` (2328), `hit_zones.rs` (2994), `registry.rs` (1455) — touched surgically, not split here. `combat.rs` 227, `brain.rs` 471, `weapon/mod.rs` 998 (~720 test).
