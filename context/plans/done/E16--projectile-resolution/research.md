# E16 — Projectile Resolution · Research Notes

Investigation backing `index.md`. Decisions live in the spec; this is the derivation.

## The load-bearing insight

The engine's combat authority model was **already shaped for projectiles.** From
`context/lib/networking.md` §"Combat authority: FIRE vs HIT" and the shipped
`plans/done/E16--client-authoritative-combat/`:

- **HIT is client-authoritative declaration.** The firing machine casts against the
  world it renders and sends `HitDeclaration { shot_id, Vec<HitRecord> }`; the host
  validates cheaply (four checks) and applies damage. Enemy HP is host-owned,
  replicated, never predicted.
- `HitDeclaration` is **standalone, not folded into the input command, precisely so a
  hit can arrive on a later tick than its fire — "projectile-ready"** (networking.md,
  the message-family note; client-auth spec Scope + AC).
- The design note is explicit: hitscan, pellet spread, and future projectiles are the
  **same shape** — "declare resolved hits against the rendered world, differing only in
  ray count and **arrival timing**, not in authority model."

So a projectile is not a host-authoritative replicated entity the client reconciles.
It is the **client-declared HIT deferred in time**: the firing machine simulates the
projectile locally, and when its own projectile lands, it emits the *same*
`HitDeclaration` carrying the fire's `shot_id`, on a later tick. `shot_id` and
`ShotVerdict` are reused unchanged; the open-shots store gains a host-internal fire-time
snapshot (`is_projectile` + origin + budget) and a per-shot prune; validation reuses
`shot_id` binding + target-alive + range (re-anchored to the snapshot origin), and
**drops world-LOS for projectiles** (see below — it defends only wallhacking, out of
scope for this co-op engine).

**Consequence — no wire-format change.** The authority-critical path reuses the shipped
combat wire contract verbatim (no new message, no new field, no layout change). The net-new
authority mechanism is **host-side and off the wire**: a **fire-time snapshot on `AuthorizedShot`**
(`is_projectile`, muzzle `origin`, per-shot timeout budget — the struct is never serialized) plus
**open-shot lifetime** (a projectile shot stays open until its declaration arrives or the re-sized
stale-shot prune retires it). Projectiles run **no world-LOS check** — it uniquely defends only
wallhacking, a PvP concern out of scope for this co-op-among-friends engine, so it is dropped for
projectiles and kept only for same-tick hitscan. The **range check is re-anchored to the snapshotted
fire origin** (a present-tick-eye range would false-reject a moved shooter, the same travel-time skew
that makes present-eye LOS unsound). Remote-observer visuals (Task 4)
ride the already-generalized `entity_class` snapshot record (valid on any Transform-bearing
record per the client-auth spec), so they too add no wire layout — only a new descriptor
classname and a classification arm. Both properties are stated as invariants to preserve,
not assumed.

## Grounded seams (verified this session)

| Seam | Location | Shape / note |
|---|---|---|
| Fire resolution branch | `weapon/mod.rs::fire_hitscan` | `match resolution { ResolutionMode::Hitscan => … }` — single arm; the projectile arm lands here. Takes `registry: &EntityRegistry` (**immutable**) — cannot spawn; returns intent the mutable caller spawns. |
| Fire command | `weapon/mod.rs::WeaponFireCommand` | `{ button, aim_origin, aim_direction, can_fire }` — carries the launch ray on the **local** path. On the host authorization path (`run_remote_weapon_commands`, `commands.rs`) `aim_origin` is `Vec3::ZERO` ("the host casts no local aim ray"), so Task 3's fire-time origin is the pawn eye computed host-side (`Transform` + `capsule.eye_height`, as `attacker_eye`), **not** this field. |
| Impact record | `weapon/mod.rs::WeaponImpact` | `{ point, normal, target: Option<EntityId>, zone: Option<String>, outcome: ActivationOutcome }`. Not `Copy`. |
| Activation outcome | `weapon/mod.rs::ActivationOutcome` | `Hit(DamagePayload{amount})` | `Effect` | `Spawned(EntityId)` — `Spawned` is a dead stub; a launched projectile is its first constructor. |
| Client fire path | `weapon/mod.rs::resolve_client_fire` → `resolve_client_hitscan` → `ClientFireResolution { client_tick, hits: Vec<LocalHitRecord> }` | The connected-client prediction path; projectile client sim mirrors its post-loop placement. |
| Predicted-shot reconcile | `weapon/mod.rs::ClientPredictedShots` (`predict`, `reconcile_cooldown`, `apply_verdict`) | Keyed by `shot_id`; `apply_verdict` rolls back cooldown/FX/hitmarker on reject. Reused as-is. |
| Impact→damage chokepoint | `sim/weapon_stage/impact.rs::apply_authorized_weapon_impact_damage(registry, weapon_id, attacker, impact, credit_source, damage_amount)` | → `apply_weapon_impact_damage_with_source` → `entities::components::health::apply_damage_with_context(… DamageContext{ producer: DamageProducer::InTick })`. Both authoritative arms (local + host netcode) converge here (re-exported `sim/mod.rs`, used in `netcode/mod.rs`). |
| Per-impact apply loop | `sim/weapon_stage/commands.rs::run_local_weapon_command` (per-impact block) | Three steps a projectile impact copies: `spawn_impact_effect_at` → liveness re-check of pawn+target → `apply_authorized_weapon_impact_damage`; then `run_death_sweep` handles zero-HP. |
| Static world ray | `collision/mod.rs::cast_ray(world, origin: Point, dir: Vector, max_toi) -> Option<RayIntersection>` | `RayIntersection.time_of_impact` (distance, unit dir), `.normal`. Point = `origin + dir*toi`. |
| Targetable-entity ray | `scripting/systems/hit_zones.rs::nearest_entity_hit(registry, store, anim_time, origin, direction, range) -> Option<EntityRayHit>` | `EntityRayHit { toi, point, normal, target, zone }`. Requires unit dir. Task 1 extends it with a projectile-radius param. |
| Swept sphere vs. world | `collision/mod.rs::cast_capsule(world, pos: Point<f32>, capsule: &Capsule, dir: Vector<f32>, max_toi) -> Option<ShapeCastHit>` (wraps parry `cast_shapes`; builds the isometry internally) | Exists. A sphere is a degenerate `Capsule`. The projectile world sweep uses this (radius r) instead of `cast_ray`; the result is a `ShapeCastHit`, not a `RayIntersection`. |
| Radius-aware hit-zone narrow phase | `hit_zones.rs::ray_capsule_or_ball(origin, dir, a, b, radius, range)`; `zone_radius(zone)`; `expand_bound_for_finite_sphere(bound, c, radius)` | The entity query is **already radius-based** — ray-vs-capsule with per-zone `zone_radius`, broad-phase AABB inflated by it. Projectile radius r is the Minkowski add: `zone_radius + r` at the call, `+ r` on the bound. Hitscan/pellet callers pass `r = 0` → byte-identical. |
| Engine unit | `hit_zones.rs` doc ("Engine default capsule radius (meters)") | World units are **meters**. A projectile radius is ~0.1–0.3 m; a weapon range is tens of m. |
| Nearest-of tie-break | `weapon/mod.rs::resolve_nearest_hit` (private) | Calls both casts, prefers `entity.toi < world.toi` (wall wins ties). The projectile sweep reuses this shape. |
| Particle integrate/expire | `scripting/systems/particle_sim.rs::tick(registry, delta, gravity, live_counts)` | Two-pass: snapshot via `iter_with_kind`, integrate Euler, collect expired, **despawn after the walk** (never mutate mid-walk). The projectile advance mirrors this exactly. |
| FX spawn chokepoint | `weapon/impact.rs::spawn_impact_effect_at(registry, point, normal)` | Spawns self-despawning impact particles via `try_spawn`; sets a `SpriteVisual` per particle; no persistent emitter. |
| Projectile visual — 3 modes (all existing components) | (1) `ComponentKind::SpriteVisual = 4` — billboard body, set by `spawn_impact_effect_at`/particle sim; (2) `ComponentKind::Mesh = 9` `MeshComponent` — rigid model body via the Epic 10 mesh pass (rigid = degenerate single-bone), model loaded through the glTF/resource pipeline; (3) `ComponentKind::BillboardEmitter = 2` `BillboardEmitterComponent` — optional trail | Task 1 attaches the descriptor's visual component(s) on spawn (body + optional trail); Task 4's replicated presentation entity attaches the same. All presentation; none gates or replicates the hit. |
| Emitter on a moving host | `scripting/systems/emitter_bridge.rs::EmitterBridge::update` reads each emitter's **current** `Transform.position` each frame (`get_component::<Transform>(id)`) then spawns `ParticleState`+`SpriteVisual` particles (bridges run per frame, outside the core sim seam) | Verified: a `BillboardEmitter` mounted on a moving projectile emits a trail along its path with no new machinery. Particles are client-local presentation (off the wire per networking Phase-boundaries). |
| Content-relative asset path check | `foundation` `is_portable_content_relative_asset_path` (used by `WeaponDescriptor` `third_person_model` / `viewmodel` validation) | Task 2 validates the projectile visual path with the same check. |
| Fixed tick | `frame_timing.rs::TICK_DURATION = 16_667µs` (**60 Hz**); `sim/mod.rs::simulate_tick` → `simulate_tick_with_presentation_aim` | 10 ordered stages (`entity_model.md` §5, orders 0–9). Weapon fire = step 9 (`weapon_stage::run_local_weapon_command`), death sweep = step 10 (1-indexed). **Projectile advance inserts after weapon fire, before the death sweep**, reusing the threaded `on_impact` closure. |
| Runtime spawn | `spawner.rs`: `registry.try_spawn(transform, &[]) -> Option<EntityId>` then `registry.set_component` (`attach_descriptor_components` lives in `scripting/builtins/data_archetype.rs`, not `spawner.rs`) | Capacity-safe; warns once on exhaustion. |
| Spawn authority | `spawner.rs::SpawnContext.can_materialize_runtime_spawns` (`set_runtime_spawn_authority`) | `false` for a connected client (networking.md spawn-suppression). Task 4's host-spawned visual respects this: only the host spawns the replicated projectile. |
| Replication classify | `netcode/descriptor_class.rs::descriptor_entity_class` / `is_networked_ai_enemy` | Returns the snapshot `entity_class` (canonical descriptor name). A Transform-only projectile returns `None` today → not replicated; Task 4 adds a classification arm. |
| Client materialize | `netcode/remote_materialize.rs::materialize_armed_remote_enemy` (and siblings) | Resolves descriptor by `entity_class`, attaches presentation locally. Task 4 adds a projectile arm. |
| Open-shots store + prune | `netcode/mod.rs::OpenAuthorizedShots` (`record`/`get`/`retire`/`prune_stale`); `AuthorizedShot { shot_id, pawn, weapon, fire_tick, damage, range, pellet_count, credit_source }` | Already holds shots open across ticks; `prune_stale` retires shots older than `MAX_OPEN_SHOT_AGE_TICKS = 180` (3.0 s @ 60 Hz), wired in `host_flush_pending_hit_declarations`. Task 3 adds `is_projectile` + `origin: Vec3` + per-shot budget to `AuthorizedShot` and re-scopes the prune to that budget for projectiles. Host-internal, never serialized. |
| Declaration validation (4 checks) | `netcode/mod.rs::ingest_hit_declaration` (binding + consume) → `apply_valid_hit_record` | Check 1 `shot_id` binding + owner (`owners: MovementOwners`, **not** `WeaponOwners`), Check 2 target-alive (`HealthComponent`), Check 3 `has_static_world_los(eye, point)`, Check 4 `eye.distance(point) ≤ range × HIT_RANGE_TOLERANCE(=1.25)`. `eye = attacker_eye(registry, attacker)` at **ingest** — the present-tick eye. Task 3: projectiles skip Check 3 and anchor Check 4's distance to the snapshotted `origin`; the `is_projectile` branch reads the snapshot, not a weapon re-resolution. |
| Component registry | `entities/src/registry.rs::ComponentKind` (`#[repr(u16)]`, 0–20) | Add `Projectile = 21` **at the tail** (mid-enum insert renumbers wire discriminants — networking.md). Touch: enum, the hand-maintained `VARIANTS` array (**not** compiler-checked), `ComponentValue`, `kind()` match, `impl Component`. `spawn()` auto-seeds only Transform/EntityState/DeferredEffect; a projectile kind needs explicit `set_component`. |

## Split-before-extend flags (files > ~800 lines)

| File | Lines | Plan's relationship |
|---|---|---|
| `scripting/systems/hit_zones.rs` | 3583 | **Extended** by a projectile-radius parameter on `nearest_entity_hit` (threaded to `ray_capsule_or_ball` + `expand_bound_*`). Small — a param + two `+ r` sites, not a new block — so flagged, not split; hitscan callers pass `r = 0`. |
| `sim/mod.rs` | 2872 | Gains a stage **call** + param threading only; the advance-stage **body** lives in a new module. Extension kept minimal; no split task, flagged as risk. |
| `entities/src/registry.rs` | 2106 | Append one `ComponentKind` variant — idiomatic table growth, not a split candidate. |
| `weapon/mod.rs` | 1944 | Gains the `fire_hitscan` projectile arm (small); projectile sim + component live in **new modules**, not here. |
| `collision/moving.rs` | 1162 | Reused unchanged. |
| `entities/components/health.rs` | 1137 | Reused unchanged (`apply_damage_with_context`). |

Decision: no dedicated split task. New behavior is directed into new modules
(`weapon/projectile.rs` or `sim/projectile_stage/`, `entities/components/projectile.rs`),
so the oversized files gain only a branch, a stage call, and an enum variant.

## Lifecycle

```mermaid
sequenceDiagram
  participant Fire as Weapon fire (step 9)
  participant Adv as Projectile advance (new, step 9.5)
  participant Sweep as Death sweep (step 10)
  participant Dmg as apply_authorized_weapon_impact_damage
  Note over Fire: ResolutionMode::Projectile
  Fire->>Adv: spawn ProjectileComponent (origin, dir, speed, range, damage, credit, shot_id)
  loop each fixed tick until impact or expiry
    Adv->>Adv: prev=cur; cur += dir*speed*dt (straight line)
    Adv->>Adv: sweep prev→cur (swept sphere r) vs world (cast_capsule) + entities (nearest_entity_hit, radius r), nearest-of
  end
  alt hit
    Adv->>Dmg: WeaponImpact{outcome: Hit} (host/SP) — spawn FX, liveness gate, apply
    Adv->>Sweep: target may be at 0 HP
    Adv->>Adv: despawn projectile
  else range/lifetime exceeded
    Adv->>Adv: despawn projectile, no damage
  end
```

The first advance pass after spawn is skipped (the ≥1-pass-of-flight marker), so the earliest
impact is tick N+1 (host) or the next frame (client) — never the spawn pass.

Co-op (connected client) variant: the client runs the advance **post-loop** at the
interpolated (rendered) pose — mirroring the shipped client fire path — and on impact
emits `HitDeclaration{ shot_id, records }` (fire's `shot_id`, later tick). Host validates
(shot_id binding → target alive → range×tolerance **anchored to the snapshotted fire origin**;
**projectiles skip world-LOS**) and applies. On expiry-without-hit the client sends an **empty**
declaration so the host retires the shot promptly; the re-sized stale-shot prune is the backstop
for a dropped client.

## Observer × lifecycle (the cross-product the flow hides)

| Observer | Projectile source | Sees flight? | Owns the hit? | Path |
|---|---|---|---|---|
| Single-player | own fire | yes (local sim) | yes (local authority) | Task 1: in-tick advance → `apply_authorized_weapon_impact_damage` |
| Listen-host | own pawn's fire | yes (local sim) | yes (local authority) | Task 1, same in-tick path |
| Connected client | **own** fire | yes (predicted local sim) | declares; host validates | Task 3: post-loop advance → `HitDeclaration` → reconcile `ShotVerdict` |
| Host | a client's fire | only via Task 4 visual | no (validates declaration) | Task 3 host side: hold open shot until declaration/timeout; **does not sim the client's gameplay projectile** |
| Remote peer (host or other client) | any peer's fire | only via Task 4 | no | Task 4: host-spawned replicated presentation projectile, interpolated |

The firing connected client renders its **predicted** projectile; the host suppresses
that client's replicated copy of its own shot **per client** (Task 4 — the presentation
record carries no owner on the wire, so the host omits it from the firer's own snapshot by
`owner_client_id`, not a client-side match). Two paths diverge slightly (client uses full
camera aim; host visual uses the pawn's replicated `facing_yaw`+`aim_pitch`), acceptable
because the host visual is presentation-only for *remote* observers.

## Why straight-line flight (v1)

Straight-line constant-velocity flight is the v1 scope choice: it is the simplest travelling
projectile and covers the direct-impact bolt/rocket case. Gravity/arc, bounce, and homing are
additive later (descriptor gains fields; no rework).

Note on validation and travel time (why Check 3 is dropped and Check 4 re-anchored for projectiles):
both shipped checks read the shooter's **present-tick** eye (`apply_valid_hit_record` reconstructs
`attacker_eye` at ingest and uses it for LOS *and* the range distance). For same-tick hitscan that is
sound — the present eye is the fire eye. For a projectile the declaration arrives a later tick, after
the shooter has moved. **World-LOS (Check 3):** a present-tick ray can be blocked by cover taken
*after* firing (false-rejecting a legitimate fire-and-take-cover hit); re-anchoring it to the fire
origin would fix the geometry, but world-LOS only ever defends the **wallhack** case, a PvP concern
out of scope here — so it is dropped, not re-anchored. **Range (Check 4):** it is a cheap
corruption/desync sanity bound worth keeping, but a present-eye range false-rejects a shooter who
fired near max range and retreated during flight — so it is **re-anchored to the snapshotted fire
origin** (`range` measures distance travelled from launch). The net-new authority mechanism is thus
the fire-time snapshot (`is_projectile` + origin + budget) plus open-shot lifetime; validation is
`shot_id` binding + target-alive + fire-origin range. Hitscan keeps its cheap same-tick present-eye
LOS + range. See the Co-op fairness note in `index.md`.

## Rejected alternatives (detail)

1. **Host-authoritative replicated projectile entity, client predicts + reconciles.**
   This is the literal reading of "client-predicted," and the generic framing a code map
   suggests. Rejected: it contradicts the shipped combat model (the host **never casts a
   ray**; it validates a declared point — networking.md), duplicates the pawn-only
   predict/reconcile machinery for a non-pawn entity (absent today), and touches the wire
   heavily — while the client-declared model reuses the entire shipped
   `HitDeclaration`/`shot_id`/`ShotVerdict` path and adds no wire change. The protocol's
   own "a hit can arrive on a later tick… projectile-ready" note is the tell that the
   client-declared model is the intended consumer.

2. **Deterministic per-peer re-simulation from a broadcast fire event.** Rejected for the
   *authoritative hit* (the engine forbids one machine re-running another's roll;
   `scripting.md` §12, networking.md RNG posture — and the hit is client-declared anyway).
   It is, however, the right shape for the *presentation* visual, which is why Task 4 uses
   a host-deterministic replicated path rather than instructing remotes to re-sim.

3. **Bundle AoE/splash to ship a rocket launcher in one spec.** Rejected per the scope
   decision (direct-impact only). AoE is the next Resolution-Modes spec and the rocket is
   the projectile+AoE payoff after both halves exist. The `Spawned`/tracked-entity
   detonator pattern (`weapon-model.md` §10) is likewise a later spec; this fills the
   projectile-that-resolves-`Hit` case.
