# Research notes — Enemy Multi-Attack

Grounding verified against source 2026-08-16. Cite by symbol; treat any residual line
reference as a hint, not a contract.

## Attack tuning today

- `AttackParams { damage: f32, range: f32, cooldown_ms: f32 }` lives in
  `crates/foundation/src/data_descriptors/types/behavior.rs` (NOT combat.rs), derives `Copy`,
  and rides `BehaviorGraphDescriptor` as `attack: Option<AttackParams>`. Its single `range` gates
  DAMAGE; multi-attack's `maxRange` inherits that role as the sole engine-enforced reach — there is
  no `minRange` field. An author-side floor, where wanted, is a `ge(@brain.targetDistance, …)`
  transition guard, not a schema field.
- `BehaviorGraphDescriptor::engagement_radius()` resolves the combat-slot ring radius as
  authored `engagement_radius` field → `attack.range` → `DEFAULT_ENGAGEMENT_RADIUS` (2.0). The
  middle rung is `self.attack.map(|attack| attack.range)` — a by-value map that **depends on
  `AttackParams: Copy`**. Adding `weapon: Option<String>` drops `Copy`, so that rung cannot
  survive; the multi-attack design removes it (there is no singular `attack` to fall back to) and
  resolves the graph-level default only. Per-state standoff resolution does NOT move to a
  `BehaviorGraphDescriptor` method: a `weapon` entry's reach is unknown until spawn, so it resolves
  from Task 2's spawn-time per-attack tuning table instead (Task 3 reads it in
  `combat_slots.rs`).
- The **only runtime per-tick consumer** of `engagement_radius()` is
  `crates/postretro/src/scripting/systems/ai/combat_slots.rs` (`resolve_combat_slots`, feeding
  `CombatQuery::engagement_radius`). Other call sites are TESTS:
  `crates/entities/src/components/brain.rs` (in `from_graph_seeds_...`), the `behavior.rs` unit
  tests, and `crates/scripting-core/src/data_descriptors/tests/behavior.rs`. Task 3 updates the
  runtime consumer (now reading the tuning table for attack-firing states); Task 4 migrates the
  test call sites.
- `ActionVerb` is a unit-only enum (`Attack`) that deserializes from the bare string
  `"action": "attack"` — the round-trip test in `behavior.rs`
  (`the_wire_shape_round_trips_through_camel_case_json`) pins that string shape. Parameterizing to
  the newtype variant `Attack(String)` changes the wire shape to the object
  `action: { attack: "<name>" }` — externally-tagged serde wraps a newtype variant's payload
  directly under the variant key. A struct variant (`Attack { attack: String }`) would double-nest
  (`{ "attack": { "attack": "<name>" } }`), so the newtype form is load-bearing, not stylistic.

## Brain state and the cooldown fact

- `BrainComponent::attack_cooldown_remaining_ms: f32` (`crates/entities/src/components/brain.rs`)
  is the single cooldown scalar. Multi-attack replaces it with a name-indexed
  `BTreeMap<String, f32>`; it is transient sim state (re-armed on fire), so `#[serde(default)]`
  loads old brains empty (every attack ready), consistent with the component's other defaulted
  fields. A lookup miss for the current state's attack reads 0 (ready) — the same rule covers a
  fresh spawn and a name absent from the map. The map is not pruned on a graph swap or re-seat: a
  same-named attack in the newly-seated graph inherits its remaining sub-second timer
  (self-correcting within one cooldown); a stale name with no counterpart is a harmless dead entry.
  Clean-swap pruning is additive if a consumer ever needs it.
- `@brain.attackCooldownMs` (`BRAIN_ATTACK_COOLDOWN_MS_INPUT`, `crates/foundation/src/brain.rs`)
  is one of 13 registered `@brain.*` guard inputs (`BRAIN_INPUTS`), typed `IrType::Number`. It is a
  PUBLISHED SDK contract: present in `sdk/types/postretro.d.ts`, `sdk/types/postretro.d.luau`,
  `sdk/lib/brain.{ts,luau}`, and the committed typedef fixture
  `crates/postretro/src/scripting/typedef/tests/fixtures/expected.d.ts`. No shipped authored graph
  reads it (the reference enemy does not), but the contract surface exists. Design decision D3:
  the fact reports the current state's attack's remaining cooldown (0 when the current state fires
  no attack) — shape unchanged, meaning generalized.
- The fact is fed at the brain-scope refresh in `ai/mod.rs` via `BrainFacts::attack_cooldown_ms`
  from `brain.attack_cooldown_remaining_ms`; the tick's countdown decrements that scalar. Both
  move to the per-attack map.

## Attack firing seam (`crates/postretro/src/scripting/systems/ai/mod.rs`)

- Compute pass: the attack gate reads `action_for_state(&brain.graph, next_index) ==
  Some(ActionVerb::Attack)`, `brain.graph.attack.is_some_and(|a| distance <= a.range)`, the
  cooldown `<= 0`, and `selected_target_alive`; on firing it arms the cooldown from
  `attack.cooldown_ms`.
- Apply pass: `apply_damage_with_context` routes the payload (`attack.damage`) with
  `DamageContext { source_id: ENEMY_ATTACK_SOURCE_ID, producer: DamageProducer::InTick, .. }`,
  pushes `ENEMY_ATTACK_EVENT` ("enemyAttack"), and calls `restart_animation_clip` for the in-state
  swing replay. This is the seam Task 3 extends per-attack; it is shipped code, not a base-spec
  task reference.
- `mod.rs` is ~923 lines — over the ~800 soft split threshold. Task 3's edits are localized to the
  attack seam (gate + apply pass), so a split is not sequenced here; flag it if the seam grows.

## Weapon ray path (`crates/postretro/src/weapon/mod.rs`)

- Fire is pawn-agnostic already: `tick_resolved` / `resolve_nearest_hit` compute nearest-of world
  (`collision::cast_ray`) vs. entity (`hit_zones::nearest_entity_hit`). Player coupling is all at
  the call site (camera-aimed command, damage + zone scaling applied by the caller).
  `resolve_nearest_hit` already takes no `WeaponComponent` — it is a private `fn`, so calling it
  from the AI fire path (Task 3) is a visibility change, not a decoupling.
- `nearest_entity_hit` (`crates/postretro/src/scripting/systems/hit_zones.rs`) has **no
  shooter-exclusion parameter** — a ray from inside the firer's hitbox can self-hit. Zero-HP
  targets are already skipped. Adding an ignore-shooter parameter touches its callers:
  `weapon/mod.rs`'s player fire path is the **sole production caller** (via `resolve_nearest_hit`);
  every other call site is a test — `weapon/mod.rs`'s own unit test, the `hit_zones.rs` test
  module, and `impact_policy.rs`'s `downed_target_ray_hit_reaches_later_impact_policy` test (inside
  its `#[cfg(test)]` module, not production code) — group that last one with the `hit_zones.rs`
  tests when enumerating callers to update.
- The weapon-entry hitscan origin (Task 3) is the firing enemy's Health AABB center — Transform
  position plus the descriptor's `hitbox.offset` (`combat.rs`'s `HitboxDescriptor`). Hitbox center
  fully serves this spec's only origin consumers, occlusion and self-exclusion; a posed
  weapon-socket origin and a precise muzzle-tip offset matter once a shot is visibly emitted from
  the origin (a beam, muzzle flash, or traveling projectile), which this spec adds none of —
  deferred together to Epic 16 › projectile. `ai/mod.rs` needs no `HitZoneStore` handle or
  per-entity animation time for this.
- Map-placed archetypes spawn with `attach_weapon: false`; enemies carry no `WeaponComponent`, and
  the dense per-kind columns forbid one entity carrying two — hence spawn-time stat resolution into
  brain tuning rather than companion wieldable entities.
- A `weapon` reference on an `attacks` entry resolves through `find_descriptor`
  (`crates/postretro/src/scripting/builtins/data_archetype.rs`) — a linear scan by
  `canonical_name` over the same `descriptors: &[EntityTypeDescriptor]` slice the `entity_class`
  KVP and a player's default-weapon name already resolve through.

## Descriptor conventions

- Wire camelCase ↔ Rust snake_case via `#[serde(rename_all = "camelCase")]`; `FireMode` /
  `ResolutionMode` enum values are camelCase. `ResolutionMode` is single-variant (`Hitscan`) in
  `combat.rs`; `Contact` appends there.
- Both runtimes funnel `components.behavior` through one serde parse + the shared
  `BehaviorGraphDescriptor::validate` (`js/entity.rs`, `lua/entity.rs`), so the twins cannot
  diverge; only the SDK typing files and committed typedef fixtures need manual updates.
- `components.mesh.animations` is the name-keyed map precedent, with pathed per-entry errors — the
  model `attacks` follows, and the framing (graph-wide vocabulary referenced by name) that keeps
  the statecharts successor's per-activity grouping additive.

## Boundary with statecharts / Epic 16

- `roadmap.md` Epic 10 "Hierarchical behavior (statecharts)" owns windup→commit→recover (a nested
  graph "when a layer needs its own state"), and is "the behavior substrate Epic 16's combat
  stances and planner build on."
- `roadmap.md` Epic 16 › Resolution Modes › melee owns "melee and quick-melee with a lunge (a
  combat↔movement impulse)." The commit that makes standoff-then-lunge safe is the statechart
  layer; the lunge impulse is the Epic 16 melee mode.
- Multi-attack keeps attacks instantaneous precisely so it needs none of the above: no commit
  means reach-based distance-guard routing on the flat graph is a complete selection mechanism.

## Coordination

- `E10--enemy-combat-positioning` (shipped): `CombatQuery::engagement_radius` composes onto the
  chase-destination write. Multi-attack feeds it per-attack from Task 2's spawn-time tuning table,
  keyed by the firing state's named attack — not a `BehaviorGraphDescriptor` method, since a
  `weapon` entry's reach is unknown until spawn.
- `E10--enemy-facing-slew` (shipped): `FACING_TURN_RATE` governs yaw slew; the deferred
  attack-windup facing lock stays unowned (windup is out of scope here too).
- Replication: only `Transform` + mesh animation-state name cross the wire, so per-attack states
  replicate for free; in-state clip restarts do not (the repeat-attack open question).

## Attack modes / commit-lunge

Design-intent for stances (constraint-sets over the `attacks` vocabulary, per-mode engagement
radius, an attacks-fired counter, telegraph/commit/recover) is captured in
`context/research/enemy-attack-modes.md`, feeding the statecharts and Epic 16 combat-stances specs.
