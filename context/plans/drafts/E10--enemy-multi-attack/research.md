# Research notes — Enemy Multi-Attack

Grounding verified against source 2026-08-16. Cite by symbol; treat any residual line
reference as a hint, not a contract.

## Attack tuning today

- `AttackParams { damage: f32, range: f32, cooldown_ms: f32 }` lives in
  `crates/foundation/src/data_descriptors/types/behavior.rs` (NOT combat.rs), derives `Copy`,
  and rides `BehaviorGraphDescriptor` as `attack: Option<AttackParams>`. Its single `range` gates
  DAMAGE; multi-attack's `maxRange` inherits that role as the sole engine-enforced reach — there is
  no `minRange` field. An author-side floor, where wanted, is a `ge(@brain.targetDistance, …)`
  transition guard, not a schema field. `AttackParams` stays `Copy`: every field the multi-attack
  entry adds (`max_range: f32`, `engagement_radius: Option<f32>`) is a plain scalar, so no field
  drops `Copy`.
- `BehaviorGraphDescriptor::engagement_radius()` resolves the combat-slot ring radius as
  authored `engagement_radius` field → `attack.range` → `DEFAULT_ENGAGEMENT_RADIUS` (2.0). The
  middle rung is `self.attack.map(|attack| attack.range)`, a by-value map over the singular
  `attack`. Multi-attack replaces the singular block with the `attacks` map, so there is no single
  block to fall back to; the resolver drops that rung and returns the graph-level default only
  (`self.engagement_radius`, else `DEFAULT_ENGAGEMENT_RADIUS`). Per-state standoff resolves
  separately, from the graph's `attacks` map directly — every stat is inline, so it needs no
  spawn-time table.
- The **only runtime per-tick consumer** of `engagement_radius()` is
  `crates/postretro/src/scripting/systems/ai/combat_slots.rs` (`resolve_combat_slots`, feeding
  `CombatQuery::engagement_radius`). Other call sites are TESTS:
  `crates/entities/src/components/brain.rs` (in `from_graph_seeds_...`), the `behavior.rs` unit
  tests, and `crates/scripting-core/src/data_descriptors/tests/behavior.rs`. Task 3 updates the
  runtime consumer (now reading the firing state's attack for attack-firing states); Task 1/Task 4
  migrate the test call sites.
- `ActionVerb` is a unit-only enum (`Attack`) that deserializes from the bare string
  `"action": "attack"` — the round-trip test in `behavior.rs`
  (`the_wire_shape_round_trips_through_camel_case_json`) pins that string shape. Parameterizing to
  the newtype variant `Attack(String)` changes the wire shape to the object
  `action: { attack: "<name>" }` — externally-tagged serde wraps a newtype variant's payload
  directly under the variant key. A struct variant (`Attack { attack: String }`) would double-nest
  (`{ "attack": { "attack": "<name>" } }`), so the newtype form is load-bearing, not stylistic.
  `ActionVerb::ALL` and the `action_verb_all_is_exhaustive` walk both carry the variant; the
  successor-chain test's arm gains the field. `graph_eval::action_for_state`
  (`crates/postretro/src/scripting/systems/ai/graph_eval.rs`) returns `Option<ActionVerb>` — Task 3
  reads the `Attack(name)` payload to resolve which attack a state fires.

## Brain state and the cooldown fact

- `BrainComponent::attack_cooldown_remaining_ms: f32` (`crates/entities/src/components/brain.rs`)
  is the single cooldown scalar (a required field; `from_graph` seeds it `0.0`). Multi-attack
  replaces it with a name-indexed `BTreeMap<String, f32>`. It is transient sim state (re-armed on
  fire), so `#[serde(default)]` loads old brains empty (every attack ready), consistent with the
  component's other defaulted fields. A lookup miss for the current state's attack reads 0 (ready) —
  the same rule covers a fresh spawn and a name absent from the map. The map is not pruned on a
  graph swap or re-seat: a same-named attack in the newly-seated graph inherits its remaining
  sub-second timer (self-correcting within one cooldown); a stale name with no counterpart is a
  harmless dead entry. Clean-swap pruning is additive if a consumer ever needs it.
- Readers of the old scalar Task 2 must convert, beyond the AI tick:
  - `crates/postretro/src/scripting/systems/ai/mod.rs` — the per-tick decrement (line ~506, before
    the aggro-gate branch, so it runs every tick regardless of aggro state), the fire gate's
    `<= 0.0` check, the re-arm on fire, and the `BrainFacts::attack_cooldown_ms` feed.
  - `crates/postretro/src/spawner.rs` (~line 265) — a spawn-time **attack windup**:
    `brain.attack_cooldown_remaining_ms = brain.attack_cooldown_remaining_ms.max(MAX_DELAY_MICROS
    as f32 / 1000.0)`, so a freshly spawned enemy cannot attack before remote interpolation's
    maximum delay elapses and the remote presentation has arrived. With an empty map reading
    0-ready, an enemy would attack immediately — a regression. The seed must populate a windup
    entry for **every attack the graph declares**. Its test
    (`spawned_enemy_cannot_attack_before_interpolation_windup_expires`, asserting "the descriptor
    attachment must not overwrite the interpolation windup", ~line 483) migrates with it.
- `@brain.attackCooldownMs` (`BRAIN_ATTACK_COOLDOWN_MS_INPUT`, `crates/foundation/src/brain.rs`)
  is one of 13 registered `@brain.*` guard inputs (`BRAIN_INPUTS`), typed `IrType::Number`. It is a
  PUBLISHED SDK contract: present in `sdk/types/postretro.d.ts`, `sdk/types/postretro.d.luau`,
  `sdk/lib/brain.{ts,luau}`, and the committed typedef fixture
  `crates/postretro/src/scripting/typedef/tests/fixtures/expected.d.ts`. No shipped authored graph
  reads it (the reference enemy does not), but the contract surface exists. The fact reports the
  current state's attack's remaining cooldown (0 when the current state fires no attack) — shape
  unchanged, meaning generalized.
- The fact is fed at the brain-scope refresh in `ai/mod.rs` via `BrainFacts::attack_cooldown_ms`
  (`brain_scope.rs`); the tick's countdown decrements the timer. Both move to the per-attack map.
  The refresh runs inside the aggro-armed branch AFTER `current_index` is resolved and BEFORE
  `select_transition` picks the next state — so it keys off `current_index`, the pre-transition
  state, while the fire gate below keys off `next_index`, the post-transition state.

## Attack firing seam (`crates/postretro/src/scripting/systems/ai/mod.rs`)

- Compute pass: the attack gate reads `brain.graph.attack.is_some_and(|a| distance <= a.range)`,
  `action_for_state(&brain.graph, next_index) == Some(ActionVerb::Attack)`, the cooldown `<= 0.0`,
  and `selected_target_alive`; on firing it arms the cooldown from `attack.cooldown_ms`. Multi-attack
  resolves the `next_index` state's named attack from the `attacks` map, gates on that attack's own
  `maxRange` and its own cooldown-map entry, and re-arms only that entry.
- Apply pass: `apply_damage_with_context` routes the payload (`attack.damage`) with
  `DamageContext { source_id: ENEMY_ATTACK_SOURCE_ID, attacker: Some(id), weapon: None, zone: None,
  producer: DamageProducer::InTick }`, applied **directly to the selected target**; it pushes
  `ENEMY_ATTACK_EVENT` ("enemyAttack") and calls `restart_animation_clip` for the in-state swing
  replay (guarded on `!state_changed`). Multi-attack reads the fired attack's `damage` from the
  map; the `DamageContext` is unchanged. This is the shipped contact path — no ray, no
  `nearest_entity_hit`, no shooter-exclusion, no `HitZoneStore`, no zone scaling.
- `mod.rs` is ~923 lines — over the ~800 soft split threshold. Task 3's edits are localized to the
  attack seam (gate + apply pass) and stay inline; flag a split if the seam grows.

## Descriptor conventions

- Wire camelCase ↔ Rust snake_case via `#[serde(rename_all = "camelCase")]`. `AttackParams` and
  `BehaviorGraphDescriptor` both carry `deny_unknown_fields`, so an unknown per-entry key is
  rejected — the same posture the `attacks` entries inherit.
- Both runtimes funnel `components.behavior` through one serde parse + the shared
  `BehaviorGraphDescriptor::validate` (`crates/scripting-core/src/data_descriptors/js/entity.rs`,
  `lua/entity.rs`), so the twins cannot diverge; only the SDK typing files
  (`sdk/types/postretro.d.{ts,luau}`, `sdk/lib`) and committed typedef fixtures
  (`crates/postretro/src/scripting/typedef/tests/fixtures/`) need manual updates. The `attacks`
  map validation is all parse-time (AC1 has no spawn checks) because every stat is inline.
- SDK typedef rendering is a hand-declared registry in
  `crates/postretro/src/scripting/primitives/mod.rs`, decoupled from the Rust enum — not something the
  generator infers from `ActionVerb`'s shape. Today it registers `ActionVerb` via
  `register_enum("ActionVerb").variant("attack", ...)`, which renders the unit-string union
  (`"attack"`). Registering it instead as a struct — `register_type("ActionVerb").field("attack",
  "String")` — emits the object type `{ attack: string }` in both `.d.ts` and `.d.luau`, with no
  change needed to the generator itself
  (`crates/scripting-core/src/typedef/{ts,luau,common}.rs`); the `attacks` `Record` follows the
  existing `"BehaviorStates"` map-alias registration precedent there. This lands in Task 3, the first
  task where `crates/postretro` compiles.
- `components.mesh.animations` is the name-keyed map precedent, with pathed per-entry errors — the
  model `attacks` follows, and the framing (graph-wide vocabulary referenced by name) that keeps
  the statecharts successor's per-activity grouping additive.

## Reference archetype and clip availability

- `sdk/behaviors/reference/entities.{ts,luau}` author `referenceEnemyEntity` and
  `poseFixtureEnemyEntity`, each with a singular `attack: { damage: 8, range: 2, cooldownMs: 1200 }`
  block, an `engagementRadius: 2`, and one attack-firing state (`attack`, `action: "attack"`).
- The reference enemy's model (`content/dev/models/reference_enemy_kaykit_knight/scene.gltf`, the
  CC0 KayKit Adventurers Knight) is pruned to exactly four clips: `Idle`, `Walking_A`,
  `1H_Melee_Attack_Slice_Horizontal`, `Death_A`. **Only one attack clip exists.** A second melee
  attack therefore reuses the single slice clip through a distinct `mesh.animations` KEY (a distinct
  animation-state name backed by the same clip): the replicated animation-state name and the overlay
  label still differ per attack (satisfying AC10 (overlay) / AC11 (replication)), while the on-screen
  swing is shared. A
  genuinely distinct second-attack clip is a content dependency — re-pruning the KayKit knight to
  include another melee clip — and is an open question, not this spec's work.
- `reference_behavior_graph()` (`ai_tests.rs`) is a Rust hand-transcription oracle asserted equal to
  the shipped Luau by `the_reference_oracle_matches_the_shipped_authored_graph`
  (`shipped_reference_behavior_graph` evals the real `entities.luau`). The TS≡Luau twin is
  `the_shipped_reference_enemy_descriptor_is_identical_in_both_authorings`
  (`crates/scripting-core/src/data_descriptors/tests/behavior.rs`). Together: TS ≡ Luau ≡ oracle;
  all three migrate with the archetype.
- `trace_reference_fixture`/`BrainTrace` (`ai_tests.rs`) runs the reference oracle through a scripted
  approach and records per-tick `state`/`player_hp`/`animation`/`has_destination`/`acquired`.
  `reference_player_x(tick)` steps the player through out-of-detection → detection → contact (1.5 m)
  → back-off (6 m) → past leash (80 m). AC2 runs this trace on the single-entry `attacks` form only —
  the second attack never enters it, so the trace's connect distance, per-swing damage, and cooldown
  cadence stay exactly the shipped values with no dependence on declaration order between two
  attacks. Task 4 adds concrete numeric assertions on those three, rather than resting on
  suite-green. AC3's two-reach routing is verified separately, on the movement-feel fixture with the
  two-attack reference enemy.
- `assemble_agent_overlay_label` (`crates/postretro/src/agent_diagnostics.rs`) builds the label from
  `brain.state_name()` as `state:<name> …`. Task 4 extends it with the firing state's attack name.

## Boundary with statecharts / Epic 16

- `roadmap.md` Epic 10 "Hierarchical behavior (statecharts)" owns windup→commit→recover (a nested
  graph "when a layer needs its own state"), and is "the behavior substrate Epic 16's combat
  stances and planner build on."
- `roadmap.md` Epic 16 › Resolution Modes › melee owns "melee and quick-melee with a lunge (a
  combat↔movement impulse)." The commit that makes standoff-then-lunge safe is the statechart
  layer; the lunge impulse is the Epic 16 melee mode.
- Multi-attack keeps attacks instantaneous precisely so it needs none of the above: no commit
  means reach-based distance-guard routing on the flat graph is a complete selection mechanism.

## Deferred: ranged / hitscan enemy attacks

- Ranged attacks — `weapon`-referencing entries, the shared weapon-descriptor substrate, nearest-of
  ray resolution — defer to a future Epic 16 combat-layer spec. The load-bearing reason is the
  player is deliberately hitbox-less (`content/dev/scripts/player.ts`) and enemy damage reaches it
  by direct `apply_damage` to the selected target, so nearest-of hitscan cannot hit it; making the
  player a first-class hitscan target reverses that invariant and pulls in self-exclusion and co-op
  hit-authority consequences. Design intent: `context/research/enemy-ranged-attacks.md`.

## Coordination

- `E10--enemy-combat-positioning` (shipped): `CombatQuery::engagement_radius` composes onto the
  chase-destination write. Multi-attack feeds it per-attack, keyed by the firing state's named
  attack, resolved directly from the retained graph's `attacks` map.
- `E10--enemy-facing-slew` (shipped): `FACING_TURN_RATE` governs yaw slew; the deferred
  attack-windup facing lock stays unowned (windup is out of scope here too).
- Replication: only `Transform` + mesh animation-state name cross the wire, so per-attack states
  replicate for free; in-state clip restarts do not (the repeat-attack open question).

## Attack modes / commit-lunge

Design-intent for stances (constraint-sets over the `attacks` vocabulary, per-mode engagement
radius, an attacks-fired counter, telegraph/commit/recover) is captured in
`context/research/enemy-attack-modes.md`, feeding the statecharts and Epic 16 combat-stances specs.
