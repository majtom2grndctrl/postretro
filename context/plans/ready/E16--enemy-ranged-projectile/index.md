# E16 — Enemy Ranged Projectile

> **Epic:** 16 — Combat. **Milestone:** Resolution Modes.
> **Prerequisites (shipped):** `E16--projectile-resolution` (the projectile sim,
> `ProjectileComponent`, `spawn_projectile`, `ResolutionMode::Projectile`,
> `nearest_entity_hit` radius param), `E16--client-authoritative-combat`,
> `E10--enemy-line-of-sight-cover` (standoff + committed-aim facing gate).
> **Design intent:** `context/research/enemy-ranged-attacks.md` (hitscan framing —
> superseded here by the projectile decision), `context/research/weapon-model.md` §4.
> **Derivation & source verification:** `research.md`.

## Goal

Give enemies a ranged attack that fires a **traveling projectile**, reusing the
shipped `E16--projectile-resolution` sim end to end. An enemy attack entry may name
a **weapon descriptor** (a projectile weapon) instead of inline contact stats; on
the AI fire tick the enemy launches a host-owned projectile that travels, occludes on
geometry, and damages the player through the existing chokepoint. This is the first
non-player producer of projectile launches and the first enemy attack that is not
direct-to-target contact.

## Scope

### In scope

- **The player becomes a first-class collidable/targetable presence** — an authored
  `health.hitbox` on the player pawn — and the player's own hitscan/pellet/projectile
  fire stops self-resolving against it (shooter exclusion). This reverses the
  documented hitbox-less-player invariant and is the load-bearing prerequisite for
  any enemy shot to reach the player.
- **Owner-aware projectile self-exclusion** — a projectile never impacts its own
  firing pawn after its one-tick spawn grace. Protects the player's own projectiles
  (now that the player is targetable) and every enemy projectile.
- **A `weapon`-referencing enemy attack entry** — an `attacks` map entry that names a
  weapon descriptor with `resolution: projectile`, resolving damage / reach / cooldown
  from that descriptor. Contact entries stay authored and behave unchanged. The entry
  `resolution` seam stays open for a future enemy *hitscan* mode.
- **A spawn-time resolved-stat home** — the referenced `WeaponDescriptor` is resolved
  once via `find_descriptor` into the brain's **derived guard-program side table**
  (rebuilt when the entity is seen; never serialized, per `entity_model.md` §7c),
  exposing the effective fire range (gating fire distance), damage, cooldown, and the
  `ProjectileDescriptor` the launch needs.
- **Enemy projectile launch at the AI fire seam** — when the selected firing leaf's
  attack is a projectile weapon, the AI apply pass builds a `ProjectileLaunch` and
  calls `spawn_projectile` with the enemy as `owner_pawn`, instead of applying contact
  damage. It reuses the shipped LOS / facing / cooldown / standoff gates unchanged.
- **Fire origin at `enemy_eye`** (a body-forward eye offset) for the first cut. The
  posed weapon-socket muzzle is the decided target, staged as a presentation follow-on
  (below).
- **Co-op:** the host-owned enemy projectile is advanced by the existing authoritative
  projectile-advance stage and damages a connected client's pawn host-side through the
  existing chokepoint (no client declaration — enemies mint no shot). The enemy
  projectile is made **visible in flight to a connected client** by reusing the
  existing presentation-mirror machinery, with the enemy-spawn ids surfaced to it and a
  weapon-name descriptor source.
- **Content:** convert the limitator's `shoot` attack to a projectile `weapon` entry;
  author a rifle projectile `WeaponDescriptor`; prove it on `combat-demo.map`.
- SDK typedefs (TS + Luau) and descriptor validation for the new attack-entry shape.

### Out of scope (non-goals)

- **Enemy hitscan resolution.** Deferred, not cancelled: the enemy attack `resolution`
  branch stays open so an instant-damage enemy shot is a cheap additive future mode.
  Owner: a future Resolution-Modes spec.
- **Friendly fire / enemy-on-enemy projectile impacts.** Owner-exclusion prevents a
  projectile *self*-hit but not an *ally*-hit; whether an ally impact deals damage is
  per-pair policy. Owner: **Faction & relationship model** (roadmap Epic 10, `[ ]`;
  `context/research/enemy-aggro-model.md`).
- **Per-attack aggregation of enemy projectile hits** (`onDamage` over a projectile's
  impacts, and the projectile aggregation window). Owner: **combat-events `onDamage`
  follow-on** (`context/research/combat-events.md` §3).
- **Client-side prediction/reconciliation of enemy projectiles.** The enemy projectile
  is a host-owned hazard; the client sees a presentation mirror, never a predicted
  gameplay projectile. Enemy HP and player-pawn HP from enemy damage are never
  client-predicted.
- **Target leading.** No leading precedent exists; the enemy fires at the target's
  current perceived position. A dodgeable slow projectile is desirable boomer-shooter
  feel. Owner: AI follow-on.
- **Posed weapon-socket muzzle origin.** The socket transform
  (`attachments.rs::sample_modified_world_pose`) is not sampled at the fixed AI/sim
  tick — the same split as the player's deferred muzzle-compose (`networking.md`
  "Fire origin composes on placement"). First cut uses `enemy_eye`; the socket muzzle
  is a presentation follow-on.
- **New wire message or field.** The co-op authority path reuses the shipped
  replication; damage rides ordinary Health replication and the presentation projectile
  rides the existing Transform + `entity_class` snapshot. No new wire contract.
- Materialized per-enemy weapon instance entities, enemy ammo/reload, dual-wield.
- **A weapon usage-restriction attribute (enemy-only / player-only / class- or
  faction-gated).** Weapons are weapons: there is one canonical `WeaponDescriptor`
  (`crates/foundation/src/data_descriptors/types/combat.rs:WeaponDescriptor`; SDK
  `WeaponDescriptor` in `sdk/types/postretro.d.ts`), authored as a `weapon` block on
  `defineEntity` and shared by player `inventory.loadout` references and enemy attack
  entries through the same `data_archetype::find_descriptor` lookup. This spec adds no
  player-vs-enemy weapon type and no branch on actor kind — the `enemy_rifle` canonical
  name is a naming convention, not a kind, and nothing prevents the same descriptor
  appearing in a player loadout. Any restriction on *who may wield* a weapon should be an
  attribute of the weapon, owned by the wieldable/weapon-model line (`weapon-model.md`)
  or the Faction & relationship model (roadmap Epic 10), never encoded as a separate
  weapon kind or an actor-type branch.

## Direction

**Problem.** Enemies can only attack by applying damage directly to the brain's
selected target within reach (the contact path). There is no way for an enemy to fire
a projectile that travels through space and can miss, be dodged, or be blocked by
geometry — and the shipped projectile sim, which does exactly this, has exactly one
producer: the player weapon fire tick. Observed by the limitator experiment
(`content/dev/scripts/limitator.ts`), a rifleman authored on the contact substrate,
which can aim and pace a firing cycle but has no projectile to send.

**Prior commitments.** This builds on and does not diverge from: the shipped
projectile sim (`context/plans/done/E16--projectile-resolution/` — `ProjectileComponent`,
the owner-agnostic `spawn_projectile`, the `|_| true` authoritative advance stage, the
`ResolutionMode`/`ProjectileDescriptor` descriptor contract); the "attacks are
weapons/wieldables" model (`weapon-model.md`), where a `weapon`-referencing attack
entry resolves stats from a `WeaponDescriptor` so player and enemy share one authoring
substrate; the client-authoritative combat model (`networking.md` "Combat authority:
FIRE vs HIT") — enemies mint no `AuthorizedShot` and declare no hit, so the host applies
enemy-projectile damage locally with no client authority path; the derived-data rule
for resolved brain state (`entity_model.md` §7c — bound guard programs live in the
evaluator, rebuilt when the entity is seen, never serialized); and the shipped AI floor
(`E10--enemy-line-of-sight-cover`) — first-class `standoffDistance`, committed-aim
facing, and a fire tick gated on LOS *and* post-slew facing.

**Placement.** The projectile sim, the AI fire seam, the resolved-stat table, and the
collision/exclusion changes are all engine-floor Rust: they need the collision world,
the fixed tick, the registry, and the netcode authority model, none reachable from
script (`scripting.md` §1). The only script-facing surface is the attack-entry schema
(a weapon reference on an `attacks` entry) and the referenced weapon descriptor —
authored data, validated in `foundation`. This matches how the player's projectile
weapon is authored: descriptor tuning, no live script.

**One divergence from the design note, stated.** `enemy-ranged-attacks.md` and
the `roadmap.md` combat bullet frame the enemy ranged shot as **nearest-of hitscan**. The owner has
decided the resolution is a **traveling projectile** reusing the shipped sim. The
hitscan mode is not built and not cancelled — the enemy attack-entry `resolution` seam
is left open so an additive enemy-hitscan mode is cheap later. (The design doc's "AI
prerequisites" section is also stale: both prerequisites it lists as open shipped in
`E10--enemy-line-of-sight-cover`; this spec does not re-derive them. Doc rewrite is a
follow-up for the owner — see `research.md`.)

**Alternatives rejected.** (1) *Enemy hitscan* (the doc's framing) — cheaper, but the
owner wants the dodgeable-projectile feel and to reuse the shipped sim; kept as an
additive future mode. (2) *Materialize a weapon instance entity per enemy* so
`owner_weapon` and the presentation mirror resolve like the player's — rejected:
enemies carry no `Inventory` and this drags in switching/inventory machinery the enemy
does not need; a derived resolved-stat table plus a weapon-name descriptor source is
far cheaper. (3) *Bake resolved weapon stats into the retained graph* — rejected per
`entity_model.md` §7c (resolved data is derived, never serialized). (4) *Resolve the enemy
projectile against the player through a narrower seam the player's own fire never tests* —
a projectile-only or enemies-only target shape — rather than making the player generally
ray-targetable and excluding self at the player-fire sites. Rejected: the `hit_zones`
facility (`hit_zones.rs`) knows only the authored AABB hitbox and zone-bearing skinned
models — it has no notion of the player's movement capsule, so a narrower carve-out is a
new special-case target kind, strictly more complex and less general than "the player is a
first-class target, exclude self at the source" (the same `ignored` seam the projectile sim
already uses). Full notes: `research.md`.

**Foreclosures / one-way doors.** Low and named. The player becoming targetable is a
reversal of a documented invariant, load-bearing in three places (self-exclusion,
co-op hit authority for enemy→player, friendly-fire reachability) — handled here
(self-exclusion, host-local damage) or explicitly deferred (friendly fire). Adding a
`String` weapon reference to `AttackParams` removes its `#[derive(Copy)]` — a contained
mechanical change enumerated in Task 3. `enemy_eye` fire origin forecloses nothing; the
posed-socket muzzle is additive. No new wire contract, so co-op is reversible.

## Acceptance criteria

- [ ] AC1 — The player pawn is a targetable/collidable presence: a test ray or
  projectile aimed at the player resolves a hit on the player (it did not before), and
  the player's authored `health.hitbox` spans its body like the limitator's.
- [ ] AC2 — The player's own fire no longer self-resolves: firing a hitscan/pellet
  weapon, and launching a projectile weapon, from the player never registers a hit on
  the player's own pawn (no range-0 self-hit, no self-converging muzzle aim), while a
  shot aimed at any *other* targetable entity still resolves exactly as before the
  player gained a hitbox.
- [ ] AC3 — A projectile spawned inside its own firing pawn's hitbox never impacts that
  pawn: after the one-tick spawn grace it passes through the owner and continues, and
  resolves normally against a *different* targetable entity or world geometry. This
  holds for both a player-fired and an enemy-fired projectile.
- [ ] AC4 — An enemy attack entry that names a projectile `weapon` descriptor is
  accepted at descriptor validation; a contact entry (inline `damage`/`maxRange`/
  `cooldownMs`) authored today validates and behaves identically. An entry that both
  names a weapon and inlines contact stats is rejected at descriptor validation with a
  field-named error (a single-descriptor check). An entry naming a weapon that does not
  resolve, or resolves to a non-projectile weapon, is caught at spawn-time resolution
  (Task 4) — warned once with the attack/weapon named, configuring no attack so it never
  fires; cross-descriptor resolvability is a resolution-time concern, since no load-time
  pass sees the whole descriptor set.
- [ ] AC5 — On the fire tick, when the selected firing leaf's attack is a projectile
  weapon and every shipped gate passes (cooldown elapsed, target inside the **effective
  weapon range**, target alive, LOS clear, post-slew facing within tolerance), the enemy
  spawns a host-owned projectile at `enemy_eye` aimed at the target's perceived
  position, and applies its per-attack cooldown — instead of applying contact damage.
  A contact-attack enemy is behaviorally unchanged (still applies direct damage, spawns
  no projectile).
- [ ] AC6 — The enemy projectile reuses the shipped sim: it does not impact on the fire
  tick (spawn grace), travels in a straight line at the descriptor speed, is occluded by
  world geometry (a shot into a wall applies no damage and self-despawns), and on
  reaching the player applies its damage through the existing chokepoint with the
  enemy's credit source. It never impacts the firing enemy. A projectile that reaches
  its travel bound (range/lifetime) without contact applies no damage and despawns.
- [ ] AC7 — Effective range gating: an enemy whose target is beyond the resolved weapon
  range does not fire even when the target is visible and facing is satisfied; the fire
  distance gate reads the weapon's effective range, not a stale inline value.
- [ ] AC8 — Co-op damage: on a listen-host with a connected client, an enemy projectile
  that reaches the connected client's pawn applies damage to that pawn host-side through
  the existing chokepoint, replicated as ordinary Health; no `HitDeclaration` is sent
  for the enemy shot and no `AuthorizedShot` is minted for it, and no new wire message
  or field or version constant is introduced.
- [ ] AC9 — Co-op visibility: a connected client sees the enemy projectile in flight as
  a host-spawned, interpolated presentation entity that despawns on impact/expiry, and
  sees its impact FX; the presentation resolves the projectile weapon's authored visual
  (not the firing enemy's own descriptor visual). Enemy HP and the client's own pawn HP
  are never client-predicted for the enemy shot.
- [ ] AC10 — Content proof on `combat-demo.map`: the limitator, its `shoot` attack
  converted to a projectile `weapon` entry backed by an authored rifle
  `WeaponDescriptor`, fires a visible traveling projectile that is blocked by geometry
  between it and the player, damages the player on contact, and does not damage itself;
  the enemy still holds its standoff and gates fire on LOS and facing.
- [ ] AC11 — The deterministic harness exercises: a targetable-player ray/projectile hit
  that a hitbox-less player passed through before; a player shot that no longer
  self-hits; a projectile spawned inside its owner's hitbox that never self-impacts; an
  enemy projectile applying damage on a later tick (not the fire tick) through the shared
  chokepoint with the enemy credit source; an enemy projectile occluded by a wall
  applying none; an out-of-effective-range enemy not firing; and (co-op) an enemy
  projectile damaging a connected client's pawn host-side with no declaration and no wire
  constant change. No new `unsafe` (grep gate).

## Tasks

### Task 1: Player becomes targetable + player-fire shooter exclusion

Make the player pawn a first-class targetable/collidable presence and stop the player's
own fire from resolving against it. **Content:** add an authored `health.hitbox` block
to `content/dev/scripts/player.ts` (the pawn authors `health: { max: 100 }` with no
hitbox today). Author `halfExtents`/`offset` arrays spanning the pawn body, mirroring
the limitator's authored hitbox and the pawn's movement capsule (radius 0.2, halfHeight
0.8, eyeHeight 0.5) — pick a body-spanning box; exact extents are yours to pin, the
constraint is that the box contains the pawn body and the camera eye sits inside its
y-range (that is why self-exclusion below is mandatory). Once the player has a hitbox it
becomes targetable through the shared `nearest_entity_hit_ignoring` facility
(`crates/postretro/src/scripting/systems/hit_zones.rs`), whose no-zone-model arm gates
on `HealthComponent.hitbox`. **Self-exclusion:** the player's own fire currently resolves
through `weapon/mod.rs::resolve_nearest_hit`, which calls `nearest_entity_hit(…, 0.0)`
(ignoring nothing). It is called from three sites in `weapon/mod.rs` — `fire_hitscan`
(the hitscan/pellet loop), `resolve_client_hitscan`, and `resolve_projectile_launch_pose`
(the convergence ray that aims a projectile launch through the muzzle). A body-spanning
hitbox contains the camera eye, and a parry ray from an interior origin returns
`toi = 0`, so without exclusion every player shot self-hits at range 0 and every
projectile launch converges on the player. Thread the **owner pawn `EntityId`** to
`resolve_nearest_hit` and have it call `nearest_entity_hit_ignoring` with a closure that
excludes the owner pawn (the facility already exposes the `ignored: impl Fn(EntityId) ->
bool` parameter — this is the same seam the projectile sim uses). The owner pawn is
available at the weapon-stage caller (`run_local_weapon_command` holds `pawn`) and at the
client-fire caller; plumb it down through the fire functions — do not leave the fire
functions guessing. Non-player callers that pass no owner (test `tick` paths) pass a
"none" exclusion so behavior is unchanged for them. Verified by AC1 (player now
targetable), AC2 (own fire no longer self-hits; a shot at another target is unchanged —
pin both sides), and AC11's targetable-hit and no-self-hit rows.

### Task 2: Owner-aware projectile self-exclusion

Make a projectile never impact its own firing pawn. `projectile_stage.rs::projectile_collision_excludes(registry, active_projectile, candidate)` today excludes the active
projectile id, any `ComponentKind::Projectile` entity, and any `DescriptorProvenance`
with `DescriptorSpawnPath::ProjectilePresentation` — but never the firing pawn. Harmless
while the only firer (the player) had no hitbox; once the player is targetable (Task 1)
its own projectile self-collides after the one-tick `spawned` grace, and an enemy firer
(which has a hitbox) would self-collide likewise. Pass the projectile's `owner_pawn`
into the exclusion decision and skip it: `advance_matching` (`projectile_stage.rs`) clones
each active `ProjectileComponent` into its per-tick snapshot and holds it (`component`,
carrying `owner_pawn`) at the `nearest_projectile_hit` call site, so thread `owner_pawn`
from there through `nearest_projectile_hit` into `projectile_collision_excludes` and return
`true` for `candidate == owner_pawn`. This is a self-exclusion only — it deliberately does not
exclude other pawns (ally-hit is the deferred friendly-fire concern). Verified by AC3
(a projectile spawned inside its owner's hitbox passes through the owner and resolves
against a different target/world) and AC11's self-impact row. This owner-exclusion
upholds the shipped **at-most-once** damage invariant for the new firer case (a
projectile must not resolve on its owner and then be unavailable for its intended
target).

### Task 3: `weapon`-referencing attack-entry schema + validation + SDK

Extend the enemy attack vocabulary so an `attacks` map entry can name a projectile
weapon descriptor instead of carrying inline contact stats, and keep contact entries
behaving identically. **Schema:** `AttackParams`
(`crates/foundation/src/data_descriptors/types/behavior.rs`) is `#[serde(deny_unknown_fields)]`
with required `damage`/`max_range`/`cooldown_ms` and optional `engagement_radius`/
`standoff_distance`, and it derives `Copy`. Add an optional `weapon: Option<String>`
(serde `weapon`) naming a weapon descriptor's `canonical_name`, and make `damage`/
`max_range`/`cooldown_ms` optional so a weapon entry can omit them (resolved from the
descriptor in Task 4). The positioning fields (`engagement_radius`, `standoff_distance`)
stay inline on both kinds — they are AI-positioning concerns, not weapon stats. Adding a
`String` field removes `#[derive(Copy)]`: change the derive and update the eval-pass read
in `ai/mod.rs`, which currently does `brain.graph.attacks.get(name).copied()`, to
`.cloned()`, plus any other `Copy`-dependent use of `AttackParams`. **Validation**
(`behavior/recursive.rs::validate_attacks`): a **contact** entry (no `weapon`) still
requires finite `damage`/`maxRange`/`cooldownMs` with the existing bounds — reject a
contact entry missing them, so today's authored contact entries validate byte-identically
and a malformed one is still rejected. A **weapon** entry (`weapon` present) must NOT also
inline `damage`/`maxRange`/`cooldownMs` (reject with a field-named error naming the
conflict), and its positioning fields keep their existing bounds. Cross-descriptor
resolvability (the named weapon exists and is a projectile weapon) is not checkable here —
the enemy descriptor validator sees only `AttackParams`, and no load-time pass sees the
whole descriptor set (every `foundation` `validate()` is single-descriptor; all
cross-descriptor resolution runs at runtime through `find_descriptor` with a graceful
fallback). So resolvability lands at Task 4's resolution site as a warn-once + no-fire, not
a validation-time reject. **SDK:** extend the TS and Luau typedefs
so an attack entry is a discriminated shape — a contact entry (inline stats) or a weapon
entry (`weapon: string` + optional positioning) — type-checked, mirroring the
discriminated-union pattern the existing weapon `resolution`/`resource` typedefs use (the
primitive-surface contract: SDK types and validation move in the same pass as the Rust
schema). This task adds no runtime behavior beyond the schema and validation. Verified by
AC4 (weapon entry accepted; contact entry unchanged; the weapon-plus-inline-stats conflict
rejected at validation — pin the accepted case and the conflict rejection). The
unresolvable / non-projectile cases resolve at Task 4 (warn-once + no-fire), not here.

### Task 4: Spawn-time resolved-stat home (derived guard-program table)

Resolve a weapon-referencing attack entry's effective stats once, into the brain's
**derived** side table, so the fire seam reads descriptor-local numbers per tick without
re-resolving. Per `entity_model.md` §7c, bound guard programs are derived data that live
in the evaluator (`ai/brain_programs.rs::BrainPrograms`), rebuilt from the retained graph
whenever the entity is seen (`sync`, keyed on the `Arc<graph>` pointer), never serialized.
Follow that shape: alongside the bound programs, build a per-attack-name resolved table
for weapon entries, populated during `sync`. `sync(&mut self, registry, warned)` does not
take the descriptor set today — thread `descriptors: &[EntityTypeDescriptor]` into `sync`
(the value is in scope at the AI stage call site in
`sim/mod.rs::simulate_tick_with_presentation_aim` and must be plumbed into
`run_ai_tick_with_navigation_and_impact` → `sync`). For each `attacks` entry naming a
`weapon`, resolve it with `data_archetype::find_descriptor(descriptors, name)` and read
its `WeaponDescriptor`; record the effective **fire range** (the descriptor `range`),
**damage**, **cooldown** (`cooldown_ms`), the `credit_source`, and a clone of the
`ProjectileDescriptor` (`weapon.projectile`, which `WeaponDescriptor::validate` guarantees
is `Some` for a projectile weapon). A weapon that fails to resolve or is not a projectile
weapon warns once (mirroring the existing bind-time warn set) and configures no attack, so
the entry never fires (matches the "unresolved action name configures no range and no
damage, so it never attacks" behavior at the fire gate). Expose accessors so the eval pass
can read the effective range (for the fire distance gate), cooldown, damage, credit
source, and the `ProjectileDescriptor` for a given attack name. Verified by AC7 (fire
distance gate reads the effective weapon range) and, transitively, AC5/AC6 (the launch
uses the resolved damage/speed/visual).

### Task 5: Enemy projectile launch at the AI fire seam (+ fire origin)

Make the enemy fire a projectile at the AI seam when its selected attack is a projectile
weapon, reusing every shipped gate. In `ai/mod.rs`, the eval pass sets `attacked` /
`attack_damage` and inserts the per-attack cooldown after the shipped gates pass
(cooldown elapsed, `distance <= attack.max_range`, target alive,
`perception::fire_gate(target_perception)`, `post_slew_facing_is_within_tolerance`); the
apply pass then calls `apply_damage_with_context(target, …)` for a contact attack. Extend
this: when the selected attack is a **weapon** entry (Task 3), (a) the fire **distance
gate** reads the **effective weapon range** from the resolved table (Task 4) instead of
the inline `max_range` (which a weapon entry omits); every other gate is unchanged and
reused as-is. (b) On a passing fire tick, carry the resolved `ProjectileDescriptor`,
effective damage, and credit source on the outcome, and in the apply pass build a
`weapon::ProjectileLaunch { origin, direction, speed, radius, range, lifetime, damage,
credit_source, descriptor }` and call
`crate::sim::weapon_stage::spawn_projectile(registry, enemy_id, owner_weapon, launch,
None)` (`predicted_shot_id: None` = host-authoritative standalone). Do **not** call
`apply_damage_with_context` for a weapon attack — the projectile applies damage on
contact later. A contact attack keeps its exact current path (direct
`apply_damage_with_context`, no projectile). The cooldown insert and
`record_successful_attack_fire` stay as they are for both kinds. **Fire origin and
direction:** origin = the enemy eye (`target_perception.enemy_eye`, already computed for
facing); direction = the unit vector toward the target's perceived aim point
(`target_perception.target_aim - enemy_eye`, normalized — the same segment the facing
tolerance gate reads). The posed weapon-socket muzzle is out of scope (presentation
follow-on; see Non-goals). **`owner_weapon`:** the enemy has no materialized weapon entity;
`spawn_projectile` requires a non-`Option` `owner_weapon: EntityId`. Pass the enemy id:
`apply_authorized_weapon_impact_damage` (`sim/weapon_stage/impact.rs`) never resolves a
`WeaponComponent` on `owner_weapon` — it uses the id only for a log line and stores it as
`DamageContext.weapon` — and `DamageContext.weapon` flows only to the engine-internal
`last_weapon` damage record (`health.rs`); no script reaction addresses `@impact.weapon`
(reactions target only `@impact.source`/`@impact.target`), so nothing distinguishes
`Some(enemy_id)` from `None`. The AI stage runs before the projectile-advance stage in the same
tick, and the `spawned` grace flag guarantees the projectile does not impact on the fire
tick. Verified by AC5 (spawns instead of applying contact damage; gates reused; contact
enemy unchanged), AC6 (reuses the sim: no fire-tick impact, wall occlusion, chokepoint
damage with enemy credit, no self-hit, travel-bound expiry), and AC11's enemy-projectile
rows.

### Task 6: Co-op — enemy-projectile client visibility + host-side damage validation

Make the enemy projectile correct in co-op: damage a connected client's pawn host-side
with no client authority path, and be visible in flight to a connected client. **Damage
authority (reuse, confirm):** the host runs the AI tick and the authoritative
projectile-advance stage (`projectile_stage::advance` with the `|_| true` matcher), which
applies damage to any targetable Health+hitbox entity — including a connected client's
pawn (host-owned, now carrying the Task 1 hitbox) — through
`apply_authorized_weapon_impact_damage` → `apply_damage_with_context`, replicated as
ordinary Health. Enemies mint no `AuthorizedShot` and send no `HitDeclaration`, so
`ingest_hit_declaration`'s ownership check (`netcode/mod.rs`) is untouched; add **no** new
wire message, field, or version constant. **Remote player pawns stay non-targetable on
clients** — the reason friendly fire is pure policy here and not a latent client-authority
hole: a remote player pawn is materialized mesh-only, with no client-side `Health`
(`netcode/remote_materialize.rs::materialize_armed_remote_player` attaches only the
descriptor mesh — "NONE of `Brain`/`Agent`/`Health`/`Weapon`/`PlayerMovement`"), and the
Task 1 player hitbox is an authored AABB on `Health`, not a zone-bearing model with an
uploaded hit-zone-store entry. A client's local hit query (`nearest_entity_hit_ignoring`,
`hit_zones.rs`) resolves only against a Health+hitbox AABB or a zone-bearing skinned model,
so it resolves against neither for a remote pawn — no client→player-pawn hit-declaration
path opens. Confirm with a listen-host + connected-client test and manual run. **Visibility (new plumbing, not free):** a connected client does not
see the host's gameplay projectile (it is not replicated — `spawn_projectile` attaches no
`DescriptorProvenance`). Today the host mirrors only the **player's** weapon-stage spawns:
`TickEvents.local_projectile_spawns` is fed from `local_result.projectile_spawns`
(`sim/mod.rs`) and consumed by `host_spawn_projectile_presentations` (`main.rs`) via
`mirror_local_gameplay_projectile`. Two gaps: (1) enemy-spawned projectile ids are never
surfaced out of the AI tick (it returns only event names) — surface the ids Task 5 spawns
(e.g. through the AI stage return / `TickEvents`) and feed them to the host mirror
alongside `local_projectile_spawns`. (2) `mirror_local_gameplay_projectile`'s
descriptor-class source (`local_projectile_presentation_source`,
`netcode/projectile_presentation.rs`) reads `DescriptorProvenance.canonical_name` off the
projectile's **`owner_weapon` entity**; an enemy has none, so passing the enemy id would
resolve the enemy's own descriptor (e.g. `limitator`) and materialize the wrong visual —
give the mirror a weapon-entity-independent descriptor-class source (the weapon's
`canonical_name` — the attack entry's `weapon` field, which Task 4 resolves against; carry
it on the surfaced spawn so the mirror uses it directly). The presentation projectile then rides the existing
Transform + `entity_class` snapshot and interpolates like any replicated entity — no wire
layout change. Enemy HP and the client's own-pawn HP are never client-predicted for the
enemy shot. Verified by AC8 (host-side damage, no declaration, no wire change) and AC9
(client sees the projectile with the correct weapon visual; no prediction).

### Task 7: Content flip + dev-map proof

Prove the feature on real content. Author a rifle projectile `WeaponDescriptor` in the
dev mod (`defineEntity({ components: { weapon: { resolution: "projectile", projectile: {
speed, radius, lifetimeMs, visual }, range, damage, fireRateMs, … } } })`) with a visible
projectile (a sprite bolt or a model + trail, reusing the shipped projectile-visual union
— the dev mod already authors reference projectile weapons for the player). Convert the
limitator's `attacks.shoot` (`content/dev/scripts/limitator.ts`) from the inline contact
entry (`damage: 10, maxRange, cooldownMs, standoffDistance`) to a **weapon** entry naming
that rifle descriptor, keeping `standoffDistance` inline (the AI-positioning concern) and
dropping the inline contact stats (now resolved from the weapon). Ensure the rifle
descriptor is imported/registered where the limitator resolves it
(`content/dev/start-script.ts` registers the limitator; register the rifle descriptor
beside it). The limitator is placed once in `combat-demo.map` (line 294) and once in
`movement-feel.map`; use `combat-demo.map` for the proof. Prove: the limitator fires a
visible traveling projectile; a wall between it and the player blocks the shot (no
damage); an unobstructed shot damages the player on contact; the projectile never damages
the limitator; the enemy still holds its `STANDOFF_DISTANCE` and gates fire on LOS and
facing (the shipped AI floor, unchanged). Update the enemy-behavior authoring reference
docs to cover the weapon-entry attack shape. Verified by AC10.

### Task 8: Tests

Extend the deterministic AI, weapon-stage, projectile-stage, and netcode harnesses.
Single-player / host: a ray or projectile aimed at the (now hitboxed) player resolves a
hit that a hitbox-less player passed through (Task 1); a player shot no longer self-hits
while a shot at another target is unchanged (Task 1, both sides); a projectile spawned
inside its owner's hitbox never self-impacts and resolves against a different target
(Task 2); an enemy whose selected attack is a projectile weapon spawns a projectile at
the fire tick and applies damage on a **later** tick (not the fire tick) through the
shared chokepoint with the enemy credit source (Task 5); an enemy projectile into a wall
applies no damage and despawns (Task 5); an enemy whose target is beyond the effective
weapon range does not fire though visible and facing-satisfied (Task 4/5); a contact-attack
enemy is byte-identical to today (Task 5 regression). Co-op: a listen-host enemy projectile
damages a connected client's pawn host-side with no `HitDeclaration` and no
`AuthorizedShot`, and no wire/version constant changes (Task 6); if visibility landed, the
client materializes the presentation projectile with the weapon's visual and none of the
enemy's own descriptor visual (Task 6). Assert no new `unsafe` (grep gate). Verified by
AC11 and, for the co-op no-constant assertion, AC8.

## Sequencing

**Phase 1 (sequential):** Task 1 — player targetable + player-fire self-exclusion. The
load-bearing invariant reversal; it changes existing player fire behavior and every later
proof depends on a targetable player. Falsifies "the player can be made targetable without
breaking its own fire."
**Phase 2 (concurrent):** Task 2 (owner-exclusion, `projectile_stage.rs`), Task 3 (schema
+ SDK: `foundation`, typedefs, and the `ai/mod.rs` eval read's `.copied()`→`.cloned()`) —
disjoint files (Task 2 touches only `projectile_stage.rs`), independent.
**Phase 3 (sequential):** Task 4 — resolved-stat home; consumes Task 3's weapon-entry
schema.
**Phase 4 (sequential):** Task 5 — enemy projectile launch + fire origin; the producer
that ties the chain together, consuming Task 4's resolved stats, Task 3's schema, Task 2's
owner-exclusion, and Task 1's targetable player. Its SP proof (an enemy projectile occludes
on geometry and damages the targetable player, not itself) is the second falsifying
integration.
**Phase 5 (concurrent):** Task 6 (co-op — netcode/mirror), Task 7 (content flip + dev-map
proof) — both consume Task 5; disjoint surfaces.
**Phase 6 (sequential):** Task 8 — tests, once behavior lands.

## Rough sketch

Grounded seams (exact signatures, the lifecycle diagram, and the co-op vantage
cross-product) live in `research.md`. Key touch points: `hit_zones::nearest_entity_hit_ignoring`
(hitbox gate, `ignored` closure); `weapon/mod.rs` `resolve_nearest_hit` +
`fire_hitscan`/`resolve_client_hitscan`/`resolve_projectile_launch_pose` (owner
plumbing); `projectile_stage.rs` `projectile_collision_excludes`/`nearest_projectile_hit`
(owner-exclusion); `foundation` `behavior.rs` `AttackParams`/`ActionVerb` +
`behavior/recursive.rs` `validate_attacks` (schema); `ai/brain_programs.rs`
`BrainPrograms::sync` (resolved-stat table); `ai/mod.rs` fire seam (launch);
`sim/weapon_stage/commands.rs` `spawn_projectile` (owner-agnostic spawn);
`sim/mod.rs` `simulate_tick_with_presentation_aim` (stage order, `descriptors` in scope);
`netcode/projectile_presentation.rs` + `main.rs::host_spawn_projectile_presentations`
(co-op mirror). `find_descriptor` (`scripting/builtins/data_archetype.rs`) resolves the
weapon by canonical name. No file past ~800 lines gains a large block: `hit_zones.rs`
(3583) is unchanged — it already exposes the `ignored` closure param; `weapon/mod.rs`
(~1944) gains an owner param and switches its `resolve_nearest_hit` callers from
`nearest_entity_hit` to `nearest_entity_hit_ignoring`; `ai/mod.rs` gains a launch branch;
`projectile_stage.rs` changes only the `projectile_collision_excludes` helper (the advance
body is reused verbatim).

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| Player hitbox | `HealthComponent.hitbox: Option<Hitbox>` | content (`.prl`/descriptor), not replicated as a new field | `health.hitbox: { halfExtents, offset }` | same | n/a |
| Weapon attack entry | `AttackParams.weapon: Option<String>` (canonical name); `damage`/`maxRange`/`cooldownMs` become optional | descriptor JSON camelCase; contact JSON unchanged | attack entry union: contact vs `{ weapon: string, standoffDistance?, engagementRadius? }` | same | n/a |
| Resolved weapon stats | derived table on `BrainPrograms` (range/damage/cooldown/credit/`ProjectileDescriptor`) | **not serialized** (derived data, §7c) | n/a | n/a | n/a |
| Enemy projectile | `spawn_projectile(reg, enemy_id, owner_weapon, launch, None)`; `ProjectileComponent.predicted_shot_id = None` | gameplay projectile **not replicated**; co-op **presentation** projectile rides the existing Transform + `entity_class` snapshot | n/a | n/a | n/a |
| Enemy→player damage (co-op) | `apply_authorized_weapon_impact_damage` host-side; replicated as Health | **unchanged** — ordinary Health replication, no new message/field | n/a | n/a | n/a |

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| **Player is targetable, own fire excludes the owner** | Task 1 (hitbox + owner-excluding `resolve_nearest_hit`) | any `resolve_nearest_hit` caller omitting the owner exclusion → range-0 self-hit; a non-owner shot wrongly excluded → missed legitimate hit | AC1, AC2 (both sides); AC11 |
| **A projectile never self-impacts its owner** | Task 2 (owner-aware `projectile_collision_excludes`) | the owner not excluded after the spawn grace (player or enemy firer); excluding a non-owner pawn (would hide a legitimate ally hit — deferred, must stay a self-only exclusion) | AC3; AC11 |
| **At-most-once damage per projectile** (shipped) | shipped sim; upheld for the new firer by Task 2 | a projectile resolving on its owner then unavailable for the target; double-resolve | AC6; AC11 |
| **Resolved weapon stats are derived, never serialized** | Task 4 (table on `BrainPrograms`, rebuilt in `sync`) | writing resolved numbers back into the retained `attacks` graph; leaking them into component equality/serde | AC7 (behavioral) |
| **Enemy shot needs no client authority path** | Task 6 (host-local damage; enemies mint no shot, declare nothing) | any client `HitDeclaration`/`AuthorizedShot` for an enemy shot; a new wire message/field/version constant | AC8; AC11 |
| **No wire-format change** (damage rides Health; visibility rides the existing snapshot) | Task 6 (reuse Health replication + `entity_class` presentation record) | a new message/field or a bumped version constant for either damage or visibility | AC8, AC9; AC11 |

## Orderings

| Scenario | Ordering | Expected outcome |
|---|---|---|
| Enemy fires tick N | AI stage (step 8) spawns; projectile-advance (step 9) runs same tick; `spawned` grace skips the first pass | No impact on tick N; first integration next pass; earliest impact N+1. |
| Enemy projectile into a wall | world contact precedes any entity contact along the segment | No damage; despawn; no self-hit. |
| Enemy projectile spawned inside its own hitbox | spawn grace, then owner-exclusion on every subsequent pass | Passes through the firing enemy; resolves only on the player/world. |
| Target beyond effective weapon range, visible, facing satisfied | range gate reads the resolved weapon range | Enemy does not fire (AC7). |
| Firing enemy despawns while its projectile is in flight | despawn precedes impact | Projectile resolves (it carries its own damage + string credit source): target liveness gates the hit; owner-pawn liveness only downgrades `attacker` to `None` (shipped advance behavior). |
| Player killed by the enemy projectile | impact drops HP to ≤0; death sweep (step 10) runs after the advance | The authored death policy handles the downed player; damage applied at most once. |
| Co-op: enemy projectile reaches a connected client's pawn | host advance applies damage; Health replicates | Client pawn takes damage host-side; no declaration; client's own HP never predicted. |

## Script syntax examples

```ts
// Proposed — a projectile rifle weapon descriptor (dev-mod reference), authored
// like the player's reference projectile weapons.
const enemyRifle = defineEntity({
  canonicalName: "enemy_rifle",
  components: {
    weapon: {
      damage: 10,
      range: 12,                       // meters — the enemy's effective fire range
      fireRateMs: 750,
      fireMode: "auto",
      resolution: "projectile",
      projectile: {
        speed: 40,                     // m/s straight-line — dodgeable
        radius: 0.15,                  // meters — swept-sphere half-width
        lifetimeMs: 4000,
        visual: { body: { kind: "sprite", sprite: "sprites/enemy_bolt.png" } },
      },
      creditSource: "enemy.rifle",
    },
  },
});

// Proposed — the limitator's attack becomes a weapon entry (Task 7 flip).
// standoffDistance stays inline (AI positioning); damage/maxRange/cooldownMs are
// resolved from enemy_rifle, so they are omitted here.
behavior: {
  attacks: {
    shoot: { weapon: "enemy_rifle", standoffDistance: 6 },
  },
  // ... activities/transitions unchanged ...
}
```

## Open questions

None outstanding. The `owner_weapon` id is decided in Task 5 (pass the enemy id), the
weapon-entry contact-stat conflict is decided in Task 3 (reject at validation), and the
locus of cross-descriptor resolvability is decided in Task 3/Task 4 (a resolution-time
warn-once + no-fire, since no load-time pass sees the whole descriptor set).
