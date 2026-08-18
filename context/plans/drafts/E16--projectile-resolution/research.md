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
`HitDeclaration` carrying the fire's `shot_id`, on a later tick. `shot_id`, the
open-shots store, and `ShotVerdict` are reused unchanged; validation reuses `shot_id`
binding + target Health-bearing + range, **drops world-LOS for projectiles**, and **re-anchors the
range check to a snapshotted fire-time origin** (both the present-eye world-LOS and the present-eye
range check are fixed-hitscan assumptions that false-reject a moved shooter under deferred arrival —
see below).

**Consequence — no wire-format change.** The authority-critical path reuses the shipped
combat wire contract verbatim (no new message, no new field, no layout change). The net-new
authority mechanism is host-side and **off the wire**: a small **fire-time snapshot on `AuthorizedShot`**
— the shot's `ResolutionMode`, its **muzzle origin** (`Vec3`, for the re-anchored range check), a
**record cap of 1**, and a **per-shot open-shot timeout budget** `max(180-tick floor, ceil(min(range/speed,
lifetime)/tick) + RTT)` — the floor keeps the old fixed constant's client-hitch tolerance, the per-shot
term lifts the ceiling for slow/long projectiles the fixed 180 could not bound. Projectiles run **no world-LOS
check** — it uniquely defends only wallhacking, a PvP concern out of scope for this co-op-among-friends
engine, and its travel-time interaction is the one place it could false-reject a friend (see the
Co-op fairness note in `index.md`), so it is dropped for projectiles and kept only for same-tick
hitscan. Remote-observer visuals (Task 4)
ride the already-generalized `entity_class` snapshot record (valid on any Transform-bearing
record per the client-auth spec), so they too add no wire layout — only a new descriptor
classname and a classification arm. Both properties are stated as invariants to preserve,
not assumed.

## Grounded seams (verified this session)

| Seam | Location | Shape / note |
|---|---|---|
| Fire resolution branch | `weapon/mod.rs::fire_hitscan` | `match resolution { ResolutionMode::Hitscan => … }` — single arm; the projectile arm lands here. Takes `registry: &EntityRegistry` (**immutable**) — cannot spawn; returns intent the mutable caller spawns. |
| Fire command | `weapon/mod.rs::WeaponFireCommand` | `{ button, aim_origin, aim_direction, can_fire }` — carries the launch ray already. |
| Impact record | `weapon/mod.rs::WeaponImpact` | `{ point, normal, target: Option<EntityId>, zone: Option<String>, outcome: ActivationOutcome }`. Not `Copy`. |
| Activation outcome | `weapon/mod.rs::ActivationOutcome` | `Hit(DamagePayload{amount})` | `Effect` | `Spawned(EntityId)` — `Spawned` is a dead stub; a launched projectile is its first constructor. |
| Client fire path | `weapon/mod.rs::resolve_client_fire` → `resolve_client_hitscan` → `ClientFireResolution { client_tick, hits: Vec<LocalHitRecord> }` | The connected-client prediction path; projectile client sim mirrors its post-loop placement. |
| Predicted-shot reconcile | `weapon/mod.rs::ClientPredictedShots` (`predict`, `reconcile_cooldown`, `apply_verdict`) | Keyed by `shot_id`; `apply_verdict` rolls back cooldown/FX/hitmarker on reject. Reused as-is. |
| Impact→damage chokepoint | `sim/weapon_stage/impact.rs::apply_authorized_weapon_impact_damage(registry, weapon_id, attacker, impact, credit_source, damage_amount)` | → `apply_weapon_impact_damage_with_source` → `entities::components::health::apply_damage_with_context(… DamageContext{ producer: DamageProducer::InTick })`. Both authoritative arms (local + host netcode) converge here (re-exported `sim/mod.rs`, used in `netcode/mod.rs`). |
| Per-impact apply loop | `sim/weapon_stage/commands.rs::run_local_weapon_command` (per-impact block) | Three steps a projectile impact copies: `spawn_impact_effect_at` → liveness re-check of pawn+target → `apply_authorized_weapon_impact_damage`; then `run_death_sweep` handles zero-HP. |
| Static world ray | `collision/mod.rs::cast_ray(world, origin: Point, dir: Vector, max_toi) -> Option<RayIntersection>` | `RayIntersection.time_of_impact` (distance, unit dir), `.normal`. Point = `origin + dir*toi`. |
| Targetable-entity ray | `scripting/systems/hit_zones.rs::nearest_entity_hit(registry, store, anim_time, origin, direction, range) -> Option<EntityRayHit>` | `EntityRayHit { toi, point, normal, target, zone }`. Requires unit dir. Task 1 extends it with a projectile-radius param. |
| Swept sphere vs. world | `collision/mod.rs::cast_capsule(world, iso, capsule: &Capsule, ...)` (wraps parry `cast_shapes`) | Exists. A sphere is a degenerate `Capsule`. The projectile world sweep uses this (radius r) instead of `cast_ray`. |
| Radius-aware hit-zone narrow phase | `hit_zones.rs::ray_capsule_or_ball(origin, dir, a, b, radius, range)`; `zone_radius(zone)`; `expand_bound_for_finite_sphere(bound, c, radius)` | The entity query is **already radius-based** — ray-vs-capsule with per-zone `zone_radius`, broad-phase AABB inflated by it. Projectile radius r is the Minkowski add: `zone_radius + r` at the `ray_capsule_or_ball` narrow-phase call, and `+ r` on the **query-time** broad phase (`transformed_zone_bound`) — **not** the load-time `expand_bound_*` model-AABB builders (cached per `ModelHandle`, shared across callers). Hitscan/pellet callers pass `r = 0` → byte-identical. |
| Engine unit | `hit_zones.rs` doc ("Engine default capsule radius (meters)") | World units are **meters**. A projectile radius is ~0.1–0.3 m; a weapon range is tens of m. |
| Nearest-of tie-break | `weapon/mod.rs::resolve_nearest_hit` (private) | Calls both casts, prefers `entity.toi < world.toi` (wall wins ties). The projectile sweep reuses this shape. |
| Particle integrate/expire | `scripting/systems/particle_sim.rs::tick(registry, delta, gravity, live_counts)` | Two-pass: snapshot via `iter_with_kind`, integrate Euler, collect expired, **despawn after the walk** (never mutate mid-walk). The projectile advance mirrors this exactly. |
| FX spawn chokepoint | `weapon/impact.rs::spawn_impact_effect_at(registry, point, normal)` | Spawns self-despawning impact particles via `try_spawn`; sets a `SpriteVisual` per particle; no persistent emitter. |
| Projectile visual — 3 modes (all existing components) | (1) `ComponentKind::SpriteVisual = 4` — billboard body, set by `spawn_impact_effect_at`/particle sim; (2) `ComponentKind::Mesh = 9` `MeshComponent` — rigid model body via the Epic 10 mesh pass (rigid = degenerate single-bone), model loaded through the glTF/resource pipeline; (3) `ComponentKind::BillboardEmitter = 2` `BillboardEmitterComponent` — optional trail | Task 1 attaches the descriptor's visual component(s) on spawn (body + optional trail); Task 4's replicated presentation entity attaches the same. All presentation; none gates or replicates the hit. |
| Emitter on a moving host | `scripting/systems/emitter_bridge.rs::EmitterBridge::tick` reads each emitter's **current** `Transform.position` per tick (`get_component::<Transform>(id)`) then spawns `ParticleState`+`SpriteVisual` particles | Verified: a `BillboardEmitter` mounted on a moving projectile emits a trail along its path with no new machinery. Particles are client-local presentation (off the wire per networking Phase-boundaries). |
| Content-relative asset path check | `foundation` `is_portable_content_relative_asset_path` (used by `WeaponDescriptor` `third_person_model` / `viewmodel` validation) | Task 2 validates the projectile visual path with the same check. |
| Fixed tick | `frame_timing.rs::TICK_DURATION = 16_667µs` (**60 Hz**); `sim/mod.rs::simulate_tick` → `simulate_tick_with_presentation_aim` | Ordered stages (not numbered in source). Weapon fire = `weapon_stage::run_local_weapon_command`, death sweep = `run_death_sweep`. **Projectile advance inserts immediately after `run_local_weapon_command`, before `run_death_sweep`**, reusing the threaded `on_impact` closure. |
| Runtime spawn | `spawner.rs`: `registry.try_spawn(transform, &[]) -> Option<EntityId>` then `set_component` / `attach_descriptor_components` | Capacity-safe; warns once on exhaustion. |
| Spawn authority | `spawner.rs::SpawnContext.can_materialize_runtime_spawns` (`set_runtime_spawn_authority`) | `false` for a connected client (networking.md spawn-suppression). Task 4's host-spawned visual respects this: only the host spawns the replicated projectile. |
| Replication classify | `netcode/descriptor_class.rs::descriptor_entity_class` / `is_networked_ai_enemy` | Returns the snapshot `entity_class` (canonical descriptor name). A Transform-only projectile returns `None` today → not replicated; Task 4 adds a classification arm. |
| Client materialize | `netcode/remote_materialize.rs::materialize_armed_remote_enemy` (and siblings) | Resolves descriptor by `entity_class`, attaches presentation locally. Task 4 adds a projectile arm. |
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

Co-op (connected client) variant: the client runs the advance **post-loop** at the
interpolated (rendered) pose — mirroring the shipped client fire path — and on impact
emits `HitDeclaration{ shot_id, records }` (fire's `shot_id`, later tick; the client advance integrates
by the render-path per-frame wall-clock delta `frame_dt` — not the tick-quantized cooldown `elapsed_ms`
— so flight wall-clock ≈ range/speed regardless of frame rate). Host validates
(shot_id binding → target Health-bearing → range×tolerance **from the snapshotted fire origin**;
**projectiles skip world-LOS**) and applies. On expiry-without-hit the client sends an **empty**
declaration so the host retires the shot promptly; a **per-shot timeout** is the backstop for a
dropped client.

## Observer × lifecycle (the cross-product the flow hides)

| Observer | Projectile source | Sees flight? | Owns the hit? | Path |
|---|---|---|---|---|
| Single-player | own fire | yes (local sim) | yes (local authority) | Task 1: in-tick advance → `apply_authorized_weapon_impact_damage` |
| Listen-host | own pawn's fire | yes (local sim) | yes (local authority) | Task 1, same in-tick path |
| Connected client | **own** fire | yes (predicted local sim) | declares; host validates | Task 3: post-loop advance → `HitDeclaration` → reconcile `ShotVerdict` |
| Host | a client's fire | only via Task 4 visual | no (validates declaration) | Task 3 host side: hold open shot until declaration/timeout; **does not sim the client's gameplay projectile** |
| Remote peer (host or other client) | any peer's fire | only via Task 4 | no | Task 4: host-spawned replicated presentation projectile, interpolated |

The firing connected client renders its **predicted** projectile and suppresses the
host-replicated copy of its own shot (Task 4) — the same "the local pawn is the host's
pawn" suppression the movement path uses. Two paths diverge slightly (client uses full
camera aim; host visual uses the pawn's replicated `facing_yaw`+`aim_pitch`), acceptable
because the host visual is presentation-only for *remote* observers.

## Why straight-line flight (v1)

Straight-line constant-velocity flight is the v1 scope choice: it is the simplest travelling
projectile and covers the direct-impact bolt/rocket case. Gravity/arc, bounce, and homing are
additive later (descriptor gains fields; no rework).

Note on validation and travel time (the reason world-LOS is dropped for projectiles): client-auth
Check 3 casts a static-world ray from the shooter's **present-tick** eye. For same-tick hitscan that
is sound. For a projectile the declaration arrives a later tick, after the shooter has moved, so a
present-tick ray can be blocked by cover taken *after* firing — it would false-reject a legitimate
fire-and-take-cover hit. World-LOS only ever defends the **wallhack**
case, a PvP concern out of scope for this co-op engine, so projectiles run **no world-LOS** at all. The
range check (Check 4) has the *same* present-eye defect — a shooter who repositions during flight is
measured from where they now stand, not where they fired — so it is **re-anchored to the snapshotted
fire-time muzzle origin** (a cheap `Vec3` on `AuthorizedShot`, not the raycast+BSP-walk world-LOS would
need). Net: projectiles are validated by `shot_id` binding + target Health-bearing + fire-origin range,
which is strictly more client-honoring (no geometric false-reject surface). The net-new mechanism is the
fire-time snapshot (resolution + origin + per-shot timeout budget), all host-internal. Hitscan keeps its
cheap same-tick present-eye world-LOS and range check. See the Co-op fairness note in `index.md`.

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
