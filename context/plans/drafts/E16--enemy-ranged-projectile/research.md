# E16 — Enemy Ranged Projectile — Research & Verification

Derivation, source-verification notes, corrections to stale docs, and the
lifecycle diagram. History lives here; `index.md` states only the current
design.

## Scope decision (owner, baked into the spec)

The roadmap bullet (`roadmap.md:264`) and `context/research/enemy-ranged-attacks.md`
frame the enemy ranged shot as **nearest-of hitscan on the player weapon ray path**.
The owner has decided the resolution mode is a **traveling projectile reusing the
shipped `E16--projectile-resolution` sim** (`context/plans/done/E16--projectile-resolution/`).
The spec is written to the projectile shape. Enemy *hitscan* stays a cheap additive
FUTURE mode (the enemy attack `resolution` branch is left open, not built).

## Source verification — confirmations

Every identifier below was read against current source this session.

- **Player is not targetable today.** `content/dev/scripts/player.ts` authors
  `health: { max: 100 }` with **no `hitbox`** (the file comments the omission
  explicitly). `nearest_entity_hit_ignoring`
  (`crates/postretro/src/scripting/systems/hit_zones.rs`) — the `None` (no
  zone-bearing model) arm is `hitbox.and_then(...)`, so "Health without a hitbox
  is not targetable" (comment at the arm). Confirmed against `entity_model.md` §7.
- **Projectile entity collision uses the same hitbox-gated path as hitscan.**
  `projectile_stage.rs::nearest_projectile_hit` calls
  `hit_zones::nearest_entity_hit_ignoring(..., radius, |id| projectile_collision_excludes(...))`.
  Same facility, same hitbox gate.
- **`nearest_entity_hit` already carries a `projectile_radius` param** (shipped by
  the projectile-resolution spec). Hitscan/pellet callers pass `0.0`. Radius
  plumbing is **done**; not new work.
- **`projectile_collision_excludes`** (`projectile_stage.rs`) excludes: the active
  projectile id, any `ComponentKind::Projectile` entity, and any
  `DescriptorProvenance` with `DescriptorSpawnPath::ProjectilePresentation`. It is
  **not** owner-aware (never excludes `owner_pawn`). Confirmed — the owner-exclusion
  gap is real.
- **`ProjectileComponent`** (`crates/entities/src/components/projectile.rs`) fields:
  `direction: [f32;3]`, `speed`, `radius`, `remaining_range`, `remaining_lifetime`,
  `damage`, `credit_source: String`, `owner_pawn: EntityId`, `owner_weapon: EntityId`,
  `spawned: bool`, `predicted_shot_id: Option<u64>` (`None` = host-authoritative
  standalone, the enemy case), `elapsed_flight_age`, `flipbook_active`,
  `impact_light`. Confirmed.
- **`spawn_projectile`** (`crates/postretro/src/sim/weapon_stage/commands.rs`) is
  `pub(crate) fn spawn_projectile(registry: &mut EntityRegistry, owner_pawn:
  EntityId, owner_weapon: EntityId, launch: weapon::ProjectileLaunch,
  predicted_shot_id: Option<u64>) -> Option<EntityId>`. Owner-agnostic — takes any
  entity id as `owner_pawn`. It sets `ProjectileComponent`, an optional
  `LightComponent`, the visual body (`SpriteVisual` or `MeshComponent::stateless`),
  and an optional trail `BillboardEmitterComponent`. It does **not** attach a
  `DescriptorProvenance` to the gameplay projectile (so the gameplay projectile is
  host-local, never replicated). Reachable from the AI module (`pub(crate)`, same
  crate).
- **`ProjectileLaunch`** (`weapon/mod.rs`): `{ origin, direction, speed, radius,
  range, lifetime, damage, credit_source: String, descriptor: ProjectileDescriptor }`.
  `descriptor` is required — it carries the visual union `spawn_projectile` reads.
- **`WeaponDescriptor`** (`crates/foundation/src/data_descriptors/types/combat.rs`)
  carries `damage`, `range`, `cooldown_ms` (serde `fireRateMs`), `resolution:
  ResolutionMode`, `projectile: Option<ProjectileDescriptor>`, `credit_source:
  Option<String>`. `ResolutionMode` = `{ Hitscan, Projectile }`. `validate()`
  requires `projectile: Some` and `pellet_count == 1` when `resolution` is
  `Projectile`. `ProjectileDescriptor` = `{ speed, radius, lifetime_ms, visual:
  ProjectileVisual }`.
- **`find_descriptor`** (`crates/postretro/src/scripting/builtins/data_archetype.rs`)
  scans `&[EntityTypeDescriptor]` by `canonical_name`. A weapon archetype is an
  `EntityTypeDescriptor` whose `.weapon` is a `WeaponDescriptor` (the shipped
  `defineEntity({ components: { weapon: … } })` shape). This is the resolution used
  for player default-weapon and `entity_class` lookups.
- **`AttackParams`** (`crates/foundation/src/data_descriptors/types/behavior.rs`):
  `#[derive(Copy)]`, `#[serde(rename_all = "camelCase", deny_unknown_fields)]`,
  fields `damage: f32`, `max_range: f32`, `cooldown_ms: f32`, `engagement_radius:
  Option<f32>`, `standoff_distance: Option<f32>`. `ActionVerb` = one variant
  `Attack(String)` keyed into `BehaviorGraphDescriptor.attacks: BTreeMap<String,
  AttackParams>` (`behavior/recursive.rs`).
- **AI fire seam** (`crates/postretro/src/scripting/systems/ai/mod.rs`,
  `run_ai_tick_with_navigation_and_impact`): the eval pass resolves the active
  attack via `brain.graph.attacks.get(name).copied()` and gates fire on
  `distance <= attack.max_range`, `attack_cooldown_remaining_ms <= 0`,
  `selected_target_alive`, `perception::fire_gate(target_perception)`, and
  `post_slew_facing_is_within_tolerance` — setting `attacked`/`attack_damage` and
  inserting `attack.cooldown_ms`. The apply pass calls
  `apply_damage_with_context(registry, target.entity, &DamagePayload{...},
  DamageContext{ source_id: ENEMY_ATTACK_SOURCE_ID, attacker: Some(outcome.id),
  weapon: None, zone: None, producer: DamageProducer::InTick })`. This is the seam
  the projectile branch extends.
- **Shipped AI floor (do not re-scope).** `AttackParams.standoff_distance` exists
  and is read via `brain.graph.standoff_distance_for_action(...)`; committed-aim
  keeps slewing (`facing_direction` under `committed_aim`), and fire gates on
  `perception::fire_gate` **and** `yaw_within_attack_tolerance(post_slew_yaw, …)`.
  The limitator (`content/dev/scripts/limitator.ts`) consumes both. Confirmed —
  the `enemy-ranged-attacks.md` "AI prerequisites" section is stale (see
  Corrections).
- **Resolved-stat home precedent.** `entity_model.md` §7c: "Bound guard programs
  are derived data. They live in the evaluator, never on the component, so they
  are never serialized … They rebuild from the retained graph whenever the entity
  is seen." Realized by `BrainPrograms` (`ai/brain_programs.rs`): `sync(&mut self,
  registry, warned)` rebinds an entity's programs when its `Arc<graph>` pointer
  changes. This is where a per-attack resolved-weapon table belongs. `sync` does
  **not** take `descriptors` today — threading it in is required plumbing.
- **Sim stage order** (`sim/mod.rs::simulate_tick_with_presentation_aim`): AI tick
  (`run_ai_tick_with_navigation_and_impact`, ~line 616) runs **before** the local
  weapon stage (~677) and the projectile advance stage (`projectile_stage::advance`
  with a `|_| true` all-projectiles matcher, ~700), which runs before
  `run_death_sweep` (~708). `descriptors` and `on_impact` are both in scope at the
  AI call site. So an enemy projectile spawned in the AI stage is advanced the same
  tick; the `spawned` grace flag skips its first pass, so it never impacts on the
  fire tick.
- **Co-op declaration path** (`netcode/mod.rs::ingest_hit_declaration`): validates
  a declaration against `open_shots.get(shot_id)` and rejects when
  `open.owner_client_id != client_id` or `owners.owner_of(open.shot.pawn) !=
  Some(client_id)`. `AuthorizedShot`s are minted only by
  `run_remote_weapon_commands` (client fires). Enemies own no client pawn and mint
  no shot, so they declare nothing and this path is untouched. Confirmed.
- **Player-fire self-hit is real and severe.** `resolve_nearest_hit` (`weapon/mod.rs`)
  calls `nearest_entity_hit(..., 0.0)` (ignores nothing). The player fires from the
  camera eye (`aim_origin`); a body-spanning hitbox contains the eye, and parry's
  ray-vs-AABB from an interior origin returns `toi = 0` — so once the player has a
  hitbox, every player shot self-resolves at range 0 unless the owner is excluded.
  This afflicts all three `resolve_nearest_hit` call sites in `weapon/mod.rs`:
  `fire_hitscan` (hitscan/pellet loop), `resolve_client_hitscan`, and
  `resolve_projectile_launch_pose` (convergence ray). The owner pawn id is not
  threaded to these today.

## Corrections to the task-prompt findings and stale docs

1. **Limitator placement count (task-prompt finding was wrong).** The finding said
   the limitator is "placed in `combat-demo.map` ×2 and `movement-feel.map`." Source:
   `combat-demo.map` contains **one** `"classname" "limitator"` (line 294);
   `movement-feel.map` contains **one** (line 792). One in each, not two in
   combat-demo. `content/dev/start-script.ts` imports and registers it (lines 18,
   115). The spec uses the correct count.

2. **`AttackParams` is `#[derive(Copy)]` (consequence not in the finding).** Adding a
   weapon-descriptor reference as a `String` field removes `Copy`. The eval pass
   reads the entry with `brain.graph.attacks.get(name).copied()`
   (`ai/mod.rs`); that call must become `.cloned()`, and any other `Copy`-dependent
   use of `AttackParams` updates in the same pass. Enumerated in the spec's schema
   task.

3. **Co-op client-visibility of the enemy projectile is NOT free (finding was too
   optimistic).** The finding said "presentation-mirror + host advance covers it …
   an enemy projectile needs NO prediction/declaration path." That is correct for
   **damage authority** (the host applies damage locally through
   `projectile_stage::advance` against the client pawn's Health+hitbox; no client
   declaration). It is **not** sufficient for **client visibility**:
   - `TickEvents.local_projectile_spawns` is fed **only** from the player weapon
     stage (`local_result.projectile_spawns`, `sim/mod.rs`); enemy AI-spawned
     projectile ids are never surfaced out of the AI tick (which returns only
     `Vec<Cow<str>>` event names). The host mirror
     (`host_spawn_projectile_presentations`, `main.rs`) iterates
     `local_projectile_spawns` — so an enemy projectile is mirrored to clients by
     nothing today.
   - `mirror_local_gameplay_projectile` (`netcode/projectile_presentation.rs`) is
     owner-agnostic on the projectile id, **but** its descriptor-class source,
     `local_projectile_presentation_source`, reads
     `DescriptorProvenance.canonical_name` off the projectile's **`owner_weapon`
     entity**. An enemy has no materialized weapon entity, so passing the enemy id
     as `owner_weapon` would resolve the enemy's own descriptor (e.g. `limitator`),
     materializing the wrong visual on the client.

   So co-op visibility requires real plumbing: surface enemy-spawned projectile ids
   from the AI tick, and give the mirror a weapon-entity-independent descriptor-class
   source (the resolved weapon canonical name the AI already holds). The spec's
   co-op task scopes this; "no new authority path" is preserved (damage is
   unchanged), but "no new machinery" for visibility is not accurate.

4. **`DamageContext.weapon` for the enemy projectile.** `spawn_projectile` requires
   a non-`Option` `owner_weapon: EntityId`. `apply_authorized_weapon_impact_damage`
   uses it only for a log line and as `DamageContext.weapon: Some(weapon_id)`.
   Enemy *contact* attacks conventionally pass `weapon: None` (`ai/mod.rs`). The
   projectile path will carry `Some(owner_weapon)`. Open question in the spec:
   which id the enemy passes as `owner_weapon`, and whether `@impact.weapon`
   consumers care.

5. **`enemy-ranged-attacks.md` and `roadmap.md:264` need a projectile-framing
   rewrite (follow-up for the owner — do NOT edit here).** Both describe the enemy
   ranged shot as nearest-of hitscan and list the two AI prerequisites (standoff
   inside fire threshold; committed-aim facing gate) as OPEN. Both prerequisites
   shipped in `E10--enemy-line-of-sight-cover` and are consumed by the limitator.
   The roadmap bullet already notes the AI prereqs shipped, but still frames the
   resolution as hitscan. These are documentation follow-ups, not part of this
   drafting task.

6. **`projectile_stage.rs` is a single file**, not a `sim/projectile_stage/`
   directory (minor path note — the finding cited it correctly as a file).

## Lifecycle — enemy projectile, fire to impact (host / single-player)

```mermaid
sequenceDiagram
    participant AI as AI tick (step ~8)
    participant WS as Weapon stage (player)
    participant PS as Projectile advance (step ~9, |_| true)
    participant DS as Death sweep (step ~10)
    Note over AI: tick N — fire gate passes (LOS + facing + range + cooldown)
    AI->>AI: resolve weapon (derived table) → build ProjectileLaunch (origin = enemy_eye)
    AI->>PS: spawn_projectile(reg, enemy_id, owner_weapon, launch, None) → sets spawned=true
    Note over PS: tick N — spawned grace: clears flag, no integration (no fire-tick impact)
    Note over PS: tick N+1..N+k — integrate cur=prev+dir*speed*dt; swept-sphere vs world + entities
    PS->>PS: nearest_projectile_hit excludes owner (Task: owner-aware)
    PS->>DS: on contact → apply_authorized_weapon_impact_damage(target=player) → despawn
    Note over DS: player at 0 HP handled by the sweep (authored death policy)
```

## Co-op vantage × lifecycle (connected client)

| Vantage | Fire | Flight | Impact |
|---|---|---|---|
| Host (authoritative) | AI spawns gameplay projectile | `projectile_stage::advance` integrates | applies damage to the client pawn's Health+hitbox (host-side); replicated as ordinary Health |
| Connected client | sees nothing until mirror | sees the **presentation** projectile (host-spawned, interpolated) — requires enemy-spawn surfacing + weapon-name descriptor source | sees the impact FX via the presentation flight's contact; enemy HP never predicted; own-pawn HP arrives via Health replication |

Damage authority is identical to single-player (host-local). Only the client's
*view* of the projectile needs the mirror plumbing (Correction 3).

## Alternatives considered (rejected)

- **Enemy hitscan (the doc's original framing).** Cheaper (no traveling entity),
  but the owner wants the boomer-shooter dodgeable-projectile feel and to reuse the
  shipped projectile sim. Left as an additive future mode behind the same
  `resolution` branch.
- **Materialize a weapon instance entity per enemy** (so `owner_weapon` and the
  mirror descriptor source work like the player). Rejected: enemies do not carry an
  `Inventory`, and materializing a wieldable instance per enemy is the switching/
  inventory machinery this spec does not need. The derived resolved-stat table plus
  a weapon-name descriptor source for the mirror is far cheaper.
- **Bake resolved weapon stats into the retained graph.** Rejected per
  `entity_model.md` §7c: resolved data is derived, rebuilt when the entity is seen,
  never serialized. Baking into the retained `attacks` map would serialize resolved
  numbers and violate the derived-data invariant.
