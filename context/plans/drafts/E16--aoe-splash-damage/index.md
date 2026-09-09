# AoE / Splash Damage

## Goal

Add radial splash damage — a server-authoritative query that finds every live
damageable entity within a blast radius of a world point and applies a
distance-falloff-scaled damage payload to each, routed through the existing
damage chokepoint so credit, the contributor ledger, and impact policies fire
per target. First consumer: a rocket-launcher projectile that detonates on
impact. The durable asset is the engine's first **one-to-many, non-ray** entity
query, built caller-agnostic — a first-class capability the melee shape-sweep,
app-drain damage-over-time hazards, and self-centered / ground-targeted AoE will
each call, not a rocket feature. The rocket is merely its first caller.

## Scope

### In scope

- A net-new radial entity-overlap query as a caller-agnostic engine function
  (`entities_in_sphere(center, radius, exclude, occlude) → [(EntityId, nearest
  distance)]`), with no weapon or projectile knowledge — all live damageable
  entities whose damageable volume intersects a sphere at a point.
- A splash *emitter* over that query: linear distance falloff with an authored
  edge floor, owner / self-damage policy, per-target damage through the chokepoint.
  It takes a bare center point, not a projectile, so any resolution or future
  spell fills the center.
- Static-world occlusion of splash (a wall between blast and target spares it).
- A `splash` descriptor block on the weapon descriptor — a peer of `projectile`,
  not nested under it, so it composes onto any resolution — its runtime component
  plumbing, `effective()` exposure, TS + Luau typedefs, and validation.
- Wiring splash as the rocket launcher's projectile-impact detonation (the one
  consumer wired now — the center is the projectile impact point).
- Host-authoritative resolution; explosion VFX to remote co-op clients via the
  presentation channel.
- A dev-mod reference rocket launcher and a clustered-target demo map with a
  headless integration test.

### Out of scope

- Knockback / rocket-jump impulse — the Damage & Defenses "knockback" spec owns
  the impulse seam; `DamagePayload` gains no impulse field here.
- The `combat.wasSplash` / attack-method impact fact — not in the shipped impact
  fact vocabulary; a combat-events follow-on. Splash hits are indistinguishable
  from direct hits at the `@impact.*` level in this spec.
- Elemental / damage-type splash — `DamagePayload` stays amount-only.
- Friendly-fire / faction filtering — the faction model is unbuilt; splash hits
  every damageable entity in radius.
- Entity and mover occlusion of splash — only static world occludes. An authored
  wall-ignoring toggle is a serde-additive field later; not built now.
- The melee shape-sweep sibling query — this spec builds the radial member of the
  shared query family and leaves the sweep narrow phase to the melee spec.
- Further emitter consumers, each additive over the same caller-agnostic query and
  center-point emitter, none wired here: hitscan → splash-at-hit-point (the center
  is the host-validated hit point, so it wires into the client-authoritative hit
  seam, not the projectile path — its own increment); ground-targeted / caster-
  centered "magic" AoE (the center is an aim point or the caster's position);
  remote-detonation (secondary-activation); and the app-drain DoT hazard, which
  re-queries the same `entities_in_sphere` each tick through the app-drain
  producer rather than the in-tick emitter.
- A standalone `ResolutionMode::Aoe` — see Direction.
- Per-attack aggregation of splash impacts — the combat-events projectile
  aggregation window is deferred with projectiles.

## Direction

**Problem.** The engine has no one-to-many entity query: every damage path
resolves a single nearest target (ray / swept-sphere in `hit_zones.rs`). A rocket
explosion needs "damage all entities within R with falloff," which
`E16--projectile-resolution` explicitly deferred to this spec.

**Prior commitments.**
- `E16--projectile-resolution` deferred "AoE radial overlap — find all entities
  within R of a point, one payload each (one-to-many)" to "the AoE/melee spec"
  and named this as its dependent. `weapon-model.md` §10 models the rocket as
  projectile + AoE-on-impact.
- Splash is server-authoritative, per the client-authoritative-combat model: a
  host-owned rocket resolves its impact host-side, mints no client hit
  declaration, and there is no server rewind.
- `DamagePayload` stays amount-only (`combat-events.md` §8); falloff is computed
  before the chokepoint and passed as the per-target amount. This diverges from
  `weapon-model.md` §10's "distance falloff is the payload's falloff field" — but
  that doc is exploration, `combat-events.md` §8 (payload amount-only today) is the
  authoritative word, and pre-scaling the amount needs no payload change.
- `combat-events.md` §4 marks `combat.wasSplash` / `combat.distance` as "Now"-tier
  facts. This spec defers both (see Out of scope): they are not in the shipped
  `@impact.*` vocabulary, so splash is `@impact`-indistinguishable from a direct
  hit here; adding the method/distance facts is a combat-events follow-on.
- The replicable set gains no new `ComponentPayload` kind; splash damage reaches
  clients as replicated Health results (E15 Phase 3.5), VFX as a presentation
  event.
- **The durable capability, not a rocket feature.** The radial query is built as
  a first-class caller-agnostic engine function; the splash emitter takes a bare
  center point; `splash` is a peer of `projectile` on the weapon descriptor, not
  nested under it. So splash is resolution-agnostic — the *center* comes from
  whatever resolution locates a detonation point (projectile impact now; hitscan
  hit point, aim point, or caster position later), and the *query* is reused
  wholesale by the melee sweep and the app-drain DoT hazard. This is the "build
  more right faster" investment: it is nearly free (the query and emitter are
  built regardless), and it is what keeps non-rocket AoE — magic novas, ground
  targets, damage zones — additive rather than a rewrite. The rocket-only *wiring*
  is the deliberate breadth cut; the rocket-only *shape* is designed out.
- **Divergence, argued.** The roadmap files AoE under "Resolution Modes … each a
  sibling under `ActivationOutcome`." This spec makes splash an
  **impact-triggered effect composed onto the projectile resolution, not a fourth
  `ActivationOutcome` variant.** The rocket's *fire* activation already resolves
  to `Spawned(projectile)`; the explosion is a property of that projectile's
  *impact*, resolved host-side, not a value returned from the fire tick. The
  chokepoint entry hard-requires a single `Some(target)`, so a per-target payload
  set has no home in a single-`Hit` return. The projectile-impact site branches
  on the presence of a `splash` block instead; `ActivationOutcome` is untouched.
  ("Resolution mode" in the roadmap is already used loosely — melee is described
  as "exposed both as a resolution mode and as a universal quick-melee
  activation," not necessarily an enum variant.)

**Alternatives rejected.**
- *A `ResolutionMode::Aoe` sibling (mirroring `Projectile`).* The rocket launcher
  must travel — its `resolution` is `Projectile` — so a non-traveling Aoe mode
  cannot be the rocket launcher. Splash-as-impact-effect composes with projectile
  (and later hitscan) rather than competing with it.
- *Looping the existing `nearest_entity_hit` outward.* It is a nearest-of ray with
  tie-break, not a set query; there is no clean way to enumerate all-within-R from
  it. A purpose-built overlap query is smaller than bending the ray facility.
- *Nesting `splash` under `ProjectileDescriptor`.* Reads natural (rockets are the
  first case) but bakes the rocket shape into the authoring contract — a
  ground-targeted or caster-centered magic AoE has no projectile to hang it on and
  would need a second, parallel authoring surface. A peer block on the weapon,
  composed onto whatever resolution yields the center, costs the same now and
  keeps the non-projectile cases additive. This is the corner the design avoids.
- *Building the whole shared non-ray query family now* (the radial member and the
  melee shape-sweep the roadmap pairs with it). Rejected as premature: the sharing
  the roadmap names lives at the candidate walk and broad-phase, which this spec
  already reuses (`for_each_hittable_candidate`, the `touch.rs` distance math), not
  at the shape math. The sweep needs a different narrow phase with no consumer in
  this spec, so generalizing the shape now is unexercised surface; the melee spec
  adds the sweep member against the same candidate walk with no rework here.

## Acceptance criteria

1. Firing a rocket-launcher weapon (`resolution: "projectile"` with a `splash`
   block) spawns a host-owned projectile that, on impact at point P, damages
   every live damageable entity whose damageable volume intersects the sphere of
   the authored `splash.radius` centered at P — each once, through the damage
   chokepoint, so the weapon's `creditSource` is stamped into the per-target
   contributor ledger and exactly one `@impact.*` dispatch fires per damaged
   target. An entity whose volume lies wholly outside the radius takes no damage.
2. Per-target damage scales with distance from P to the nearest point of the
   target's damageable volume: a target whose volume contains P (distance 0) takes
   full `effective().damage`; a target at the radius edge takes
   `damage · splash.minFraction`; between, linearly. Beyond the radius, none.
3. Static-world occlusion: a target whose nearest-point segment from P is blocked
   by static world geometry takes no splash; an otherwise-identical target with a
   clear segment at the same distance takes its falloff-scaled splash. Movers and
   other entities never occlude.
4. `splash.selfDamage`: with the default (`true`), the firing owner pawn, when
   live and in radius, takes falloff-scaled splash; with `false`, the owner takes
   no splash even when in radius. Non-owner targets are unaffected by this flag.
5. A projectile carrying a `splash` block applies **no** separate single-target
   direct hit — the struck entity takes splash only (no double-count). A
   projectile with no `splash` block keeps the current single-target direct hit,
   unchanged.
6. Splash resolves host-side only: a connected client's predicted rocket applies
   no local splash damage; splash HP changes reach every client as replicated
   Health results, and the explosion VFX reaches remote clients as a presentation
   event on the unreliable presentation channel. No new replicated component kind
   is added.
7. The `splash` block is authorable in TS and Luau with emitted typedefs (fixtures
   blessed) and is validated at load: `radius` finite and > 0, `minFraction` in
   [0, 1]; an invalid block is rejected with a diagnostic.
8. On a dev-mod clustered-target demo map, one reference-rocket shot damages
   several grouped enemies with damage that decreases with distance, spares an
   enemy standing behind a wall, and — with `selfDamage` on — harms the firing
   player at point-blank range; asserted by a headless integration test.

## Tasks

### Task 1: Radial splash core + projectile-impact wiring (host/SP)

Build the net-new radial query and the splash emitter, and wire them as the
rocket launcher's impact detonation, on the host / single-player path end to end.
The query is a caller-agnostic engine function taking a bare world center, radius,
and an exclusion predicate — no weapon, projectile, or splash type in its
signature, so the melee sweep and a future app-drain DoT hazard call it unchanged.
(Task 2 adds the optional occlusion argument; the caller-agnostic shape is set
here.) It enumerates candidates through the existing hittable-candidate
walk over Health-bearing entities, tests each candidate's broad-phase damageable
volume (the `Hitbox` AABB, or a zone-bearing model's derived bound) for overlap
with the sphere at the center, applies the caller-supplied exclusion predicate
(owner, inert deferred-effect entities, presentation proxies) and the shipped
liveness gate, and returns each surviving target with its nearest-point distance.
The splash emitter is a thin layer over the query that also takes a bare center
point (not a projectile): for each surviving target it computes the falloff-scaled
amount
(AC 2, `minFraction` authored, default 0) from the nearest-point distance and
applies it through `apply_authorized_weapon_impact_damage` with the weapon's
resolved `creditSource`, so each target that takes damage produces exactly one
impact dispatch (AC 1); a computed amount of 0 is not applied. The caller then
drains impact policies exactly as the direct-hit path does. Add the full
`SplashDescriptor { radius, minFraction, selfDamage }` block to the weapon
descriptor (Boundary Inventory), its runtime component field, `effective()`
exposure, TS + Luau typedef registration with blessed fixtures, and load-time
validation of `radius > 0` and `minFraction ∈ [0,1]` (AC 7) — the whole boundary,
so Task 2 wires new behavior without editing the descriptor or typedefs. In the
authoritative projectile-impact closure, branch on the presence of a `splash`
block: present → resolve entirely through the emitter, passing the projectile's
`owner_pawn` for the emitter's exclusion predicate, and apply no separate direct
hit; absent → the current single-target direct hit is unchanged (AC 5). Place the
query and emitter in a new sim module; expose the candidate-walk and
volume/distance helpers at crate visibility rather than growing the oversized
files. Here the owner is a normal candidate (matching the `selfDamage: true`
default); occlusion and the `selfDamage: false` branch land in Task 2, which is
why the emitter takes the whole splash descriptor and owner up front. Focused unit
tests cover enumeration (multiple targets in/out of radius, empty result), falloff
endpoints and a mid distance, the per-target credit path, and the projectile
branch both ways. Host / single-player only; delivers AC 1, 2, 5, 7.

### Task 2: Occlusion + self-damage policy

Extend the radial query with the two policy filters that decide *whether* a
candidate in radius is a valid splash target, without adding a second damage path
(Invariants — the chokepoint stays the only sink). Add static-world occlusion as an
**optional query argument** (so a caller that wants no occlusion — a future melee
sweep, a wall-ignoring effect — passes it off, keeping the query caller-agnostic);
the splash emitter passes it on. When on, for each in-radius candidate the query
tests the shipped static-world line-of-sight segment from the center to the nearest
point of the candidate's damageable volume, and drops a blocked candidate before
the chokepoint (AC 3). Movers and other entities do not participate. Wire the `selfDamage: false` branch: when `false`,
the owner pawn is added to the query's exclusion predicate; when `true` (the
default Task 1 already exhibits), the owner is a normal candidate (AC 4). Both
read from the `SplashDescriptor` the emitter already receives and edit only the
query module, so the descriptor, typedefs, and the projectile-impact call site are
untouched. Focused unit tests pin
both sides of each: a target behind a wall spared and a clear target at the same
distance hit; the owner harmed at point-blank with `selfDamage` on and spared with
it off. Delivers AC 3, 4.

### Task 3: Co-op authority + splash VFX presentation

Confirm and enforce host-authoritative splash under co-op, and make the explosion
visible to remote clients (Invariants — splash resolves host-side only; no new
replicated component kind). The predicted-client projectile path must not invoke the
splash emitter — only the authoritative flight advance does — so a client's
predicted rocket produces no local splash damage; its HP outcomes arrive as the
already-replicated Health results. Push the explosion VFX at the detonation point
to remote clients as a transient fact on the unreliable presentation channel
(`ServerPresentationMessage`), reusing the host/single-player local spawn Task 1
already performs; add no new `ComponentPayload` kind, and no tuning-payload field
(splash params are archetype-level and read from each end's locally-loaded
descriptor). Verify on a two-process loopback that a remote observer sees the
explosion and that damaged entities' HP converges. Delivers AC 6.

### Task 4: Reference rocket launcher + integration proof

Ship the first real consumer and prove the foundation. Author a rocket-launcher
wieldable in the dev mod (`resolution: "projectile"` + a tuned `splash` block) and
a demo/fixture map placing a cluster of enemies at varied distances from a fixed
aim point plus one enemy shadowed by a wall, with the player positioned for a
point-blank self-damage case. Add a headless integration test that fires the
reference rocket at the cluster and asserts: multiple enemies take damage
decreasing with distance, the wall-shadowed enemy is unharmed, and the firing
player takes self-damage at point-blank (AC 8). Consumes the query (Task 1), its
occlusion and self-damage policy (Task 2), and reuses existing combat-test spawn
fixtures where they fit.

## Sequencing

**Phase 1 (sequential):** Task 1 — thin slice; falsifies the query / chokepoint /
impact-policy / projectile-impact boundary assumptions end to end on the host path.
**Phase 2 (concurrent):** Task 2, Task 3 — independent; Task 2 edits the query
module + descriptor + typedefs, Task 3 edits the projectile closure + net code, no
shared file, and both read the splash descriptor the Task 1 emitter already
receives.
**Phase 3 (sequential):** Task 4 — integration proof; consumes Task 1's query and
Task 2's occlusion + self-damage policy.

## Boundary inventory

`splash` is authored in TS/Luau and parsed by serde into a Rust struct; no FGD
surface (it is script-declared weapon data, not a map KVP). Casing: camelCase on
the wire and in both script runtimes (matching `ProjectileDescriptor`), snake_case
in Rust.

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| splash block | `WeaponDescriptor.splash: Option<SplashDescriptor>` (`#[serde(default)]`) | `"splash"` | `splash?` | `splash?` | n/a |
| descriptor | `SplashDescriptor` | struct, `rename_all = "camelCase"` | object | object | n/a |
| blast radius | `radius: f32` | `"radius"` | `radius: number` | `radius: number` | n/a |
| edge floor | `min_fraction: f32` | `"minFraction"` | `minFraction?: number` | `minFraction?: number` | n/a |
| owner policy | `self_damage: bool` | `"selfDamage"` | `selfDamage?: boolean` | `selfDamage?: boolean` | n/a |

`splash.radius` (blast) is distinct from the sibling `projectile.radius` (the
projectile's collision-sphere width); both stay their own fields, scoped by block.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| Splash resolves host-authoritatively only — never applied client-side | Task 1 (emitter runs inside the authoritative flight advance) | Task 3 — the predicted-client path must not call the emitter; any new emitter caller must be host-only | AC 6 |
| Every splash target routes through the single damage chokepoint; one impact dispatch per target | Task 1 (per-target `apply_authorized_weapon_impact_damage`) | Task 2 adds occlusion / owner filters *before* the chokepoint, never a second damage path; Task 4 asserts per-target credit | AC 1 |
| `DamagePayload` stays amount-only; falloff is pre-scaled into the amount | Task 1 | Task 2's filters never touch the payload; no falloff field is added | AC 2 |
| Replicable set gains no new `ComponentPayload` kind | existing contract | Task 3 — splash damage replicates by Health result, VFX via the presentation channel | AC 6 |

## Orderings

| Scenario | Ordering / input | Expected |
|---|---|---|
| Point-blank direct hit | Rocket strikes an enemy; that enemy's volume contains P | Enemy takes full `damage` (distance 0), once, via splash — no separate direct hit (AC 5) |
| Cluster at range | N enemies at increasing nearest-point distances ≤ radius | N impact dispatches, amounts strictly decreasing with distance, edge = `damage · minFraction` (AC 1, 2) |
| Wall between | Target in radius, static geometry across the P→nearest-point segment | Target spared; a clear target at equal distance hit (AC 3) |
| `minFraction = 0`, edge target | Nearest-point distance == radius | Amount computes to 0 → not applied (no dispatch, no ledger entry); just beyond radius → not enumerated (AC 2) |
| Owner in radius, `selfDamage: true` | Owner pawn live within radius | Owner takes falloff-scaled splash (AC 4) |
| Owner in radius, `selfDamage: false` | Owner pawn live within radius | Owner takes none; other in-radius targets unaffected (AC 4) |
| Dead / inert target in radius | Candidate fails the liveness gate or is an inert deferred-effect entity | Excluded; no dispatch (AC 1) |
| Empty blast | No live damageable entity in radius | Zero dispatches; VFX still plays (AC 1, 6) |
| Connected-client predicted rocket | Client predicts the launch; impact predicted locally | No local splash damage; HP arrives as replicated Health result; VFX as presentation event (AC 6) |

## Rough sketch

- New sim module (e.g. `sim/splash.rs`) owns the radial query and the emitter;
  oversized files (`projectile_stage.rs`, `hit_zones.rs`, `weapon/mod.rs`) gain
  only call sites or `pub(crate)` exposure.
- Query reuses `for_each_hittable_candidate` (candidate walk over
  `iter_with_kind(Health)`), each candidate's broad-phase world volume (`Hitbox`
  AABB / model derived bound), point-to-volume nearest-point distance (the
  `sphere_capsule_distance_squared` / AABB nearest-point math already in
  `touch.rs`), `health::is_damage_target_eligible` (liveness), and the
  exclusion-predicate pattern (`projectile_collision_excludes` / owner id).
- Occlusion reuses `collision::line_of_sight(P, nearest_point, world)`.
- Emitter loops targets → `apply_authorized_weapon_impact_damage(registry,
  owner_weapon, owner_pawn, &impact_for_target, credit_source, scaled_amount)` →
  drain impact policies via the closure the projectile path already threads.
- Descriptor lands beside `ProjectileDescriptor` in `combat.rs`; typedef
  registration beside the `projectile?` field; fixtures re-blessed
  (`expected.d.ts` / `.d.luau`).
- VFX reuses the projectile's existing impact-effect spawn for the burst; Task 3
  pushes it to remote clients via `netcode/presentation.rs`.

## Script syntax examples

```ts
// Reference rocket launcher — a weapon block on defineEntity.
defineEntity({
  name: "rocket-launcher",
  components: {
    weapon: {
      damage: 120,                 // blast-center splash damage (effective())
      fireRateMs: 800,
      fireMode: "semi",
      resolution: "projectile",
      creditSource: "weapon.rocket",
      projectile: {
        speed: 1600,
        radius: 12,                // projectile collision-sphere width
        lifetimeMs: 4000,
        visual: /* … */,
      },
      splash: {
        radius: 256,               // blast radius (world units)
        minFraction: 0.2,          // edge damage fraction; default 0
        selfDamage: true,          // owner takes splash in radius; default true
      },
      resource: { kind: "ammo", type: "rocket", magazine: 4,
                  costPerShot: 1, reserve: 20, reloadMs: 2200,
                  reloadStyle: "magazine" },
    },
  },
})
```

## Open questions

None open. Decisions of record:

- **Placement ratified** (owner, 2026-09-09): splash is a resolution-agnostic
  impact-composed effect over a caller-agnostic radial query — not a fourth
  `ActivationOutcome` / `ResolutionMode` variant. See Direction for the argument.
- **`selfDamage` defaults `true`** (owner, 2026-09-09): boomer-authentic and the
  seam a future rocket-jump knockback rides; an author-set flag either way.
