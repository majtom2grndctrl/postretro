# E16 — Projectile Resolution

> **Status:** draft (under review; open questions resolved).
> **Epic:** 16 — Combat. **Milestone:** Resolution Modes.
> **Prerequisites (shipped):** `E16--client-authoritative-combat` (FIRE/HIT split, `shot_id`,
> `HitDeclaration`, `ShotVerdict`), `E16--shotgun-pellet-spread`, `E16--impact-policy-substrate`,
> Epic 15 through Phase 3.5.
> **Design intent:** `context/research/weapon-model.md` §4, §8, §10. Derivation: `research.md`.

## Goal

Add **projectile** as a resolution mode — an engine-owned entity that travels over time and
resolves a **direct impact** (`ActivationOutcome::Hit`), filling the dead `ResolutionMode`
and `ActivationOutcome::Spawned` seams. It is the first non-instantaneous weapon and the
foundation the rocket launcher (projectile + AoE, a later spec) builds on. Co-op-correct by
reusing the shipped client-declared-HIT path: the firing machine simulates the projectile
and declares its hit on a later tick, so the authority-critical core needs **no new wire
contract**.

## Scope

### In scope

- `ResolutionMode::Projectile` and its authored tuning: launch **speed**, collision **radius**
  (the projectile's swept-sphere half-width, in engine **meters** — a bolt is ~0.1–0.3 m, not
  the metres-wide value a naive read of a large number implies), travel bound (**lifetime** and/or
  the existing `range`), and a **visual** — descriptor-owned, not FGD.
- A **visible** travelling projectile with a three-mode visual, each a single existing component
  attached on spawn: a **body** — a billboard `SpriteVisual` (the path particles/impact-FX use) OR a
  rigid `MeshComponent` model (the Epic 10 mesh pass, rigid = degenerate single-bone) — plus an
  **optional trail `BillboardEmitter`** (the emitter bridge reads the projectile's moving Transform
  each tick and emits particles along the path). So a plasma bolt is a sprite, a rocket is a model +
  smoke trail, a pure energy stream is emitter-only. Seen in single-player, by its firer, and (Task 4)
  by remote peers.
- An engine `ProjectileComponent` and a per-tick projectile-advance simulation: straight-line
  constant-velocity flight; per-tick **swept-sphere** collision of the `prev→cur` segment (a sphere
  of the projectile radius) against static world geometry (`cast_capsule`) and targetable entities
  (the radius-aware hit-zone query), nearest-of tie-break; resolve a **direct `Hit`** on first
  contact through the existing damage chokepoint; self-despawn on impact or travel-bound.
- Single-player and listen-host: fire → travel → impact → damage, fully local.
- Connected client (co-op): a **predicted** local projectile simulated at the rendered
  (interpolated) pose; on impact it emits the existing `HitDeclaration` carrying the fire's
  `shot_id` on a later tick; predicts/reconciles muzzle-FX / cooldown / hitmarker via the
  existing `ClientPredictedShots` + `ShotVerdict`; rolls back on reject. Enemy HP never predicted.
- **Open-shot lifetime**: a projectile's authorized shot stays open until its declaration
  arrives; a **per-shot** host timeout (sized from the shot's authored travel time) backstops a
  dropped client; the client sends an **empty** declaration on expiry-without-hit to retire promptly.
- **Remote-observer visuals** (separable — the cut line for a leaner v1): the host spawns a
  presentation-only replicated projectile per fire (from the pawn's replicated aim + descriptor
  speed), deterministic straight-line, despawned on shot retire/timeout; remotes interpolate it;
  the firing client suppresses the replicated copy of its own shot.
- SDK typedefs (TS + Luau) and descriptor validation for the new mode; one reference projectile
  weapon authored in the dev mod.

### Out of scope (non-goals)

- **AoE / splash, and the rocket launcher.** The next Resolution-Modes spec; this fills only
  the projectile-that-resolves-`Hit`.
- **Gravity / arc, bounce / ricochet, penetration.** Straight-line only in v1; additive later via
  descriptor fields (integration tweaks), no rework. **Homing** is also out and is *not* a free
  descriptor add — it needs the advance stage to track and steer toward a target each tick (new sim
  logic), so it is a later behavioral spec, not a straight-line-additive field.
- **Richer projectile visuals.** Multiple stacked trail emitters, bespoke per-projectile particle
  physics beyond the existing `BillboardEmitter` config, and skinned/animated projectile models
  (v1 models are **rigid** only — the Epic 10 degenerate single-bone case). Each is additive.
- **AoE / radial overlap query.** Projectile *width* (a swept-sphere **cast** — nearest hit along
  the flight path with a radius) is in scope and cheap (Task 1). What is deferred is the distinct
  AoE *radial overlap* — "find **all** entities within R of a point, one payload each" (one-to-many)
  — the non-ray volume-query family the AoE/melee spec owns. The two share the word "radius" but are
  different operations; only the overlap is deferred.
- **Persistent tracked / deployable projectiles (the detonator pattern).** `ActivationOutcome::Spawned`
  is used here for a *transient* travelling projectile; a projectile that spawns a *persistent
  tracked entity* a secondary activation resolves later (`weapon-model.md` §10) is a later spec.
- **Client-side prediction/reconciliation of the remote-observer visual.** Remotes interpolate
  the host's replicated visual; it is presentation, never reconciled.
- **New wire message or field for the authority path.** A goal, verified as an invariant: the
  hit path reuses `HitDeclaration`/`ShotVerdict`/`shot_id` unchanged.
- Heat/cell resources, dual-wield, charge, secondary activation, viewmodel (other Weapon-Systems
  specs).

## Direction

**Problem.** The engine has exactly one weapon resolution mode — instantaneous hitscan
(`ResolutionMode` has a single `Hitscan` variant; `fire_hitscan` matches one arm). An entire
class of boomer-shooter identity weapons (rockets, plasma, nails) travels through space with
visible flight and travel-time, which the engine cannot express. The `ActivationOutcome::Spawned`
and single-arm `ResolutionMode` match are dead stubs waiting for exactly this.

**Prior commitments.** This consumes, without diverging from, the shipped client-authoritative
combat model (`plans/done/E16--client-authoritative-combat/`, `networking.md` §"Combat
authority: FIRE vs HIT"): FIRE host-authoritative + client-predicted cooldown; **HIT
client-declared** via a standalone `HitDeclaration` deliberately built to arrive on a later
tick than its fire ("projectile-ready"); `shot_id` binding as the security spine; `ShotVerdict`
reconcile; enemy HP host-owned, never predicted. The projectile is the intended later-arrival
consumer of that path — the design note names hitscan, pellet spread, and projectiles as one
shape "differing only in ray count and arrival timing." It also reuses the impact→`applyDamage`
chokepoint (`apply_authorized_weapon_impact_damage` → `apply_damage_with_context`, `DamageProducer::InTick`),
the zone-multiplier and credit-source path, the death sweep, the 60 Hz fixed tick and its
game-logic stage order, the entity-model runtime-spawn + prev==cur-on-spawn snapshot rule
(entity_model §3 names projectiles as a runtime-spawn case), and the descriptor-owned tuning
contract (no FGD KVPs; SDK typedefs updated in the same pass — index.md primitive-surface
principle). **Placement:** the projectile sim is engine-floor Rust (it needs the collision world,
the fixed tick, and the netcode authority model, none reachable from script — `scripting.md`
§1/§12); the only script-facing surface is descriptor tuning, mirroring how the ammo resource
is authored.

**Alternatives rejected.** The strongest rival is a **host-authoritative replicated projectile
entity that the client predicts and reconciles** — the literal reading of "client-predicted."
Rejected: it contradicts the shipped model (the host never casts a ray; it validates a declared
point), duplicates pawn-only predict/reconcile machinery for a non-pawn entity (absent today),
and touches the wire heavily — whereas the client-declared model reuses the entire shipped hit
path and adds no wire change. (Full alternatives incl. per-peer re-sim and bundling AoE: `research.md`.)

**Foreclosures / one-way doors.** Low and named. Straight-line-only flight forecloses arc/bounce/homing
*until a later additive spec* (descriptor gains fields; no rework). The net-new authority mechanism is a
small **fire-time snapshot on the existing `AuthorizedShot`** — the shot's `ResolutionMode` (to branch
validation), its **muzzle origin** (to range-check a later-arriving declaration against where the shot
was fired, not where the shooter now stands), and a **per-shot timeout budget** — all host-internal,
**off the wire** (the "no wire change" invariant holds). This re-prices the earlier "no fire-time-origin
snapshot" idea: the origin is one `Vec3` in the same host-side drawer that already holds `fire_tick`
and `damage`, not the raycast+BSP-walk that world-LOS would need — projectiles still run **no world-LOS**
(Co-op fairness, below). `ComponentKind::Projectile` must append at the enum tail (mid-insert renumbers
wire discriminants); this is a one-line ordering constraint, not a door. Undoing the whole feature is
deleting one sim stage, one component kind, the `AuthorizedShot` fire-time fields, and the descriptor
fields — nothing else depends on its internals.

**Co-op fairness (PvE-among-friends).** This is a co-op PvE engine — no PvP, no live service — so the
client firer must be honored exactly as much as the host firer. Both get identical *feel*: an instant
predicted projectile, muzzle FX, and hitmarker at their own screen's timing; the only difference is
*where* damage is applied (host locally, client via declaration), never *whether* a legitimate hit
counts. The host's validation of a client declaration is a **corruption/desync sanity check, not an
anti-cheat gate** (trust the friend; catch the bad packet), so **projectiles run no world-LOS check**
— it uniquely defends only the wallhack case, a PvP concern this engine rules out, and it is the one
check whose travel-time interaction risks false-rejecting a friend. A projectile is validated by
`shot_id` binding (never rejects legitimate play — a real hit always matches its own authorized fire),
target Health-bearing, and a generous range tolerance **measured from the snapshotted fire-time muzzle
origin, not the shooter's present eye** — because the declaration arrives after the shooter has moved, a
present-eye range check would false-reject a legitimate fire-and-reposition hit the same way world-LOS
would (both are fixed-hitscan assumptions that break under deferred arrival). The per-shot open-shot
timeout is sized from that shot's authored travel time so a legitimate slow projectile under bad latency
is never retired early (Invariant "no false-reject of a legitimate hit"). Hitscan keeps its cheap
same-tick world-LOS and its present-eye range check (sound when fire and hit are the same tick). The one
accepted, presentation-only asymmetry: a client's
*remote-observer* visual (Task 4) is drawn from the pawn's replicated aim rather than its exact camera
aim; it never affects whether or where the hit lands.

## Acceptance criteria

- [ ] A weapon authored with `resolution: "projectile"` and a launch speed fires a projectile
  that spawns at the muzzle, travels in a straight line at the authored speed, and applies its
  damage through the existing chokepoint **on the tick it reaches a target — never the fire tick**.
  A projectile has **≥1 tick of flight before it can contact** (a spawn-tick guard: the advance skips
  integration+sweep for a projectile spawned this tick, so its first real advance is tick N+1 regardless
  of speed) — so even a point-blank or very-fast shot resolves at N+1, not N. A projectile that reaches
  its travel bound (range/lifetime) without hitting anything applies **no** damage and despawns.
- [ ] A projectile is **visible while travelling** in single-player and to its own firer: the spawned
  entity carries its descriptor's visual component(s) — a `SpriteVisual` or `MeshComponent` body plus
  an optional `BillboardEmitter` trail — assertable on spawn (remote-observer visibility is the
  remote-observer AC / Task 4).
- [ ] Single-player and listen-host: a projectile striking a `Health`-bearing enemy credits and
  scales exactly like a hitscan hit (same zone-multiplier, credit-source, contributor-ledger,
  death-sweep path); a struck world surface applies no damage and spawns the impact FX.
- [ ] Damage from a single projectile is applied **at most once**: the projectile despawns on
  first contact, and a target already at 0 HP (killed by another source mid-flight) takes no
  further application (the existing liveness gate).
- [ ] A projectile whose **owner pawn despawned mid-flight** still applies its damage on contact,
  credited by the `credit_source` string with `attacker = None` (the projectile arm does not gate damage
  on owner liveness); a policy effect routed to `@impact.source` correctly no-ops.
- [ ] Existing hitscan and pellet-spread weapons are behaviorally unchanged — the projectile arm
  is additive to `fire_hitscan`'s `match resolution`, and non-projectile weapons never spawn a
  projectile entity.
- [ ] Connected client: firing a projectile weapon at a host-replicated enemy simulates a
  predicted projectile locally at the **rendered (interpolated)** pose, and on its local impact
  sends a `HitDeclaration` carrying the fire's `shot_id` on a **later tick** than the fire; the
  host validates it by `shot_id` binding + target Health-bearing + range-from-fire-origin only
  (projectiles skip world-LOS *and* the present-eye range anchor — see Task 3 · Direction Co-op fairness),
  authorizes exactly **one record slot** for the projectile shot, and applies damage crediting the
  authorized shot's pawn. No new wire message or field is added, and no version constant is bumped for the
  authority path (the added `AuthorizedShot` fields are host-internal).
- [ ] `shot_id` binding holds for projectiles: a fire the host rejected (cooling; empty magazine
  once ammo composes) mints no authorized shot, so a later projectile declaration binds to nothing
  and applies no damage. One authorized fire accepts at most one declaration.
- [ ] A projectile that expires without hitting sends an **empty** declaration and the host
  retires the shot; a projectile whose declaration never arrives (dropped client) is retired by a
  host **timeout**, and the open-shots store does not grow unbounded across a session. No legitimate
  hit is false-rejected: the timeout is **per-shot** — `max(180 ticks, ceil(min(range/speed, lifetime)/tick)
  + RTT margin)`, converting the ms lifetime to seconds before the `min` — so a slow/long projectile
  survives (the per-shot ceiling) and a fast projectile under a client frame-hitch survives (the 180-tick
  floor the old fixed window provided); a hitch longer than the whole window is an accepted loss; the range check is
  measured from the snapshotted **fire-time muzzle origin**; and projectiles run no world-LOS check at
  all — so a hit whose shooter moved or took cover during flight is accepted.
- [ ] The connected client predicts muzzle-FX and cooldown **on fire**, and the **hitmarker on the
  client's own local impact** (the tick it emits the record-bearing `HitDeclaration`) — never at fire,
  and never on the expiry/miss path (no phantom hitmarker for a projectile that misses); it reconciles
  all three against `ShotVerdict`, rolling back on reject. Enemy HP is never predicted (remote enemies
  carry no client `Health`), so there is no enemy-HP rollback.
- [ ] Remote-observer visual (if built): a projectile fired by any peer is visible in flight to
  other peers as a host-spawned, interpolated, presentation-only entity that despawns when its
  shot retires; a connected client spawns no local authoritative projectile for a remote peer's
  fire (spawn-suppression), and the firing client shows its own predicted projectile, not a
  doubled replicated copy.
- [ ] Author surface: `resolution: "projectile"` with an out-of-range speed/radius/lifetime, a missing
  launch speed, or a missing/invalid visual (no body, bad body path, or bad trail config) is rejected
  at descriptor validation with a field-named error; the TS and Luau typedefs expose the mode and its
  fields (speed, radius, lifetime, and the visual union — sprite/model body + optional trail); the dev
  mod contains reference weapons covering the modes (a sprite-body bolt and a model-body rocket with a trail).
- [ ] The deterministic harness exercises **all eighteen pinned Orderings rows (P1–P18)** — which cover
  the point-blank N+1 guard, the moved-shooter/take-cover acceptance, the slow/long and low-FPS timeout
  cases, the rejected-fire and undeclared-shot cases, the single-record and two-in-flight cases, and the
  expiry/retire cases — **plus the four named non-ordering behavioral tests**: radius-overlap width in
  **both** the zone-capsule and AABB-hitbox narrow phases; the `r = 0` byte-identical regression;
  visual-component-carry; and the P3 sub-tick-lifetime `validate()` rejection. No new `unsafe` (grep gate);
  and **no** authority version constant changed (`WIRE_VERSION` and all others unchanged).

## Tasks

### Task 1: Projectile resolution core — thin slice (single-player / host)

Build the whole projectile spine end-to-end for the locally-authoritative roles (single-player
and listen-host), the narrowest path that crosses every layer. **Descriptor:** add
`ResolutionMode::Projectile` to `postretro_foundation` combat.rs and the minimal authored tuning
the sim reads — a launch **speed** (m/s, finite > 0), a collision **radius** (meters, finite ≥ 0 —
the projectile's swept-sphere half-width), a travel **lifetime** (ms; the projectile also stops at
the existing `range`), and a **visual** reference (a content-relative sprite/billboard the projectile
renders as, validated like the existing weapon model-path fields) — as descriptor-owned fields with
`validate()` bounds mirroring the existing weapon-field checks. **Field grouping (decided):** the scalars
(Rust `projectile_speed`/`projectile_radius`/`projectile_lifetime_ms`; wire/TS/Luau camelCase
`projectileSpeed`/…) are **flat** on `WeaponDescriptor`,
mirroring the existing mode-specific flat fields `pellet_count`/`spread_degrees` (the nearest precedent —
scalars gated by resolution mode, not a discriminated union); the **visual** is a nested `projectileVisual`
union (body + optional trail), since it *is* a discriminated union (the shape the `resource` sub-block
uses). The **trail is a foundation-local emitter descriptor struct** (mirroring the `BillboardEmitter`
fields — rate/lifetime/spread/sprite-path): `WeaponDescriptor` lives in `foundation`, which has **no
`entities` dependency**, so the descriptor cannot reference `entities::BillboardEmitterComponent`
directly — the postretro spawn site converts the foundation struct into a `BillboardEmitterComponent` at
attach time (Task 2 validates the struct with foundation-local bounds, not the entities emitter's
validator). Do **not** add FGD KVPs (tuning is descriptor-owned). **Component:** add a new
`ComponentKind` variant **appended at the tail** (`Projectile = 21`; a mid-enum insert renumbers wire
discriminants — this is Invariant "component-kind tail-append") together with its `ComponentValue`
arm, `kind()` match, `impl Component`, the hand-maintained `VARIANTS` array (not compiler-checked),
and a new `entities/src/components/projectile.rs` `ProjectileComponent` carrying the flight and
resolution state the impact needs: direction (unit), speed, radius, remaining travel bound, `damage: f32`,
`credit_source: String`, the owner pawn and weapon `EntityId`, a **spawned-this-tick flag** (defaults **true** at construction — every constructor sets it, so a future
sweeping constructor cannot skip the guard; the advance skips a projectile still carrying it, clearing it
that pass, so the first real integration+sweep is the tick *after* spawn — the mechanized "≥1 tick of
flight" guarantee), (reserved for Task 3) the fire's `shot_id`, and a
**presentation-only flag** (defaults **false** = gameplay at construction; only Task 4's remote materializer
sets it true — the mirror of the guard's default-true rule, so a gameplay constructor can never
accidentally skip the sweep) for Task 4's replicated projectile — but **Task 1 implements the branch that
reads it** (`if presentation_only`, the advance integrates position only and skips the sweep, impact, and
despawn-on-contact), so Phase-2 Task 4 only *sets* the flag and never edits this advance module
(avoiding a concurrent edit with Task 3's client `frame_dt` call site). **Fire branch:** in `weapon/mod.rs::fire_hitscan`, add the `ResolutionMode::Projectile`
arm; because `fire_hitscan` borrows the registry immutably it cannot spawn — it returns a launch
**intent** carried on the existing fire-return path (`fire_hitscan` → `tick_resolved_component` →
`WeaponFireEvents`, which today has no launch-intent field — **add one as a sibling to its `impacts`**),
so the intent reaches `run_local_weapon_command`'s spawn site with the origin, direction, the resolution
stats above, **and the descriptor's projectile-visual union** (cloned from the weapon descriptor at
`tick_resolved_component` — its `String` asset paths are not `Copy` — and moved into the intent, so the
spawn site attaches the body/trail without re-resolving the descriptor). `WeaponImpact` cannot carry it (no speed/radius/lifetime/damage
fields), and `ActivationOutcome::Spawned(EntityId)` is constructed **after** the spawn with the real id,
since no entity id exists at fire time — that the mutable caller consumes. This spec is the first constructor of the dead `Spawned` variant; add a
**one-line doc-comment** on it stating that `Spawned(EntityId)` names an entity the activation produced,
and whether that entity is *transient* (self-resolving — this projectile, despawns on impact) or
*persistent/tracked* (instance-owned, resolved by a later secondary activation — the detonator pattern,
`weapon-model.md` §10) is the **consumer's** concern, not the variant's — so the later detonator spec
adds no tracked-vs-transient distinction to the variant. In v1 **nothing matches** `Spawned` (the
tracking consumer is the later detonator spec) — constructing it is what fills the dead seam, and the
spawn+advance is driven by the launch intent, not by a match on `Spawned`. **Spawn + advance:** in the
weapon stage's `run_local_weapon_command` (which holds `&mut registry`), spawn the projectile via
`registry.try_spawn(Transform{position: muzzle}, &[])` + `set_component(ProjectileComponent)` **and the
projectile's visual component(s) from the descriptor visual** so it is **visible** as it travels
(without a visual it is an invisible hit, not a real slice): attach the **body** — a `SpriteVisual`
(the billboard the particle/impact-FX path uses via `spawn_impact_effect_at`) or a rigid
`MeshComponent` (the projectile model loaded through the existing glTF/resource pipeline; rigid, no
skeleton) — plus, when the descriptor declares a trail, a `BillboardEmitterComponent` (the emitter
bridge already reads a moving host's Transform each tick and emits along the path). This is a spawn-time
branch on the descriptor visual kind; the visual is presentation only and never gates or replicates the
hit. Factor the descriptor-visual→component attach into a shared helper
(`attach_projectile_visual(registry, id, &visual)`) that Task 4's remote materializer reuses, so the
mapping is authored once. The spawn (entity + components) happens on the fire tick; the **spawned-this-tick
flag makes the first integration+sweep land on tick N+1**, so damage never resolves on the fire tick even
for a point-blank or very-fast shot (Invariant "≥1 tick of flight"; do **not** rely on the stage-0
prev==cur snapshot for this — that governs render interpolation, not the sim advance). Add a **new
projectile-advance module** (`sim/projectile_stage/` or `weapon/projectile.rs`) and a stage call in
`sim/mod.rs::simulate_tick_with_presentation_aim` **immediately after the weapon-fire stage
(`run_local_weapon_command`) and before the death sweep (`run_death_sweep`)**, reusing the threaded
`on_impact` closure and `tick_dt`. **Gate this tick-stage advance to local-authority roles (single-player /
listen-host).** `simulate_tick_with_presentation_aim` also runs on a connected client, but the client's
*predicted* projectile is advanced solely by Task 3's per-frame post-loop (at the interpolated pose); if the
tick stage also advanced it, it would double-move **and sweep/despawn it on contact before Task 3 can emit
the deferred `HitDeclaration`** — locally resolving a hit the client-declared model forbids the client to
apply. So the connected client runs **no** tick-stage projectile advance (its predicted projectile is
post-loop; its remote projectiles arrive as host snapshots); gate the stage by role, or skip entities
carrying the client-predicted marker. Keep the edit
to `sim/mod.rs` to the stage call + param threading; keep the edit to `weapon/mod.rs` to the fire
arm — the advance body lives in the new module (the oversized files must not grow a block). The
advance follows the particle two-pass discipline (`particle_sim::tick`): snapshot each projectile,
integrate `cur = prev + direction * speed * tick_dt` (segment length **capped at the remaining range
bound**, `min(speed·tick_dt, remaining_range)`, so a wall/target beyond `range` is never hit on
overshoot), sweep the `prev→cur` segment for the nearest
contact **as a swept sphere of the projectile radius** — `cast_capsule` (a radius-r sphere against
the static world; `cast_capsule` already exists and wraps parry `cast_shapes`) and a **radius-aware**
`nearest_entity_hit` over the segment length, taking the nearer with the wall-wins-ties rule of
`resolve_nearest_hit`. The entity query already tests ray-vs-capsule with a per-zone `zone_radius` and
inflates its broad-phase AABB bound by that radius, so adding the projectile radius is the Minkowski
sum — extend `nearest_entity_hit` with a projectile-radius parameter that adds `r` at **both** narrow
phases the query has — the zone-capsule path (`ray_capsule_or_ball`) **and** the authored-AABB path
(`ray_aabb_slab`/`aabb_hit`, the `None =>` branch for a Health+hitbox entity with no zone-bearing mesh;
inflate its half-extents/slab by `r`) — so width holds regardless of how an enemy's hitbox was authored,
and inflates the **query-time** broad phase (`transformed_zone_bound`) — **not** the load-time
`expand_bound_*` model-AABB builders, which cache a per-`ModelHandle` bound shared across callers, so a
per-projectile radius cannot flow through them; pass `r = 0` on the existing hitscan/pellet callers so
both narrow phases and the broad phase are **byte-identical** (a plumbing change, not a behavior change). A projectile-local nearest-of helper combines the swept
world cast and the radius-aware entity query rather than reusing the ray-only `resolve_nearest_hit`
verbatim. Then, **after the walk**, apply impacts and despawn (never mutate the registry mid-walk); a
**presentation-only** projectile is **skipped by this whole apply/impact/despawn-on-contact block** — it
integrates only and despawns on shot-retire or travel-bound (Task 4 sets the flag; the branch is here). On a contact, build a
`WeaponImpact { point, normal, target, zone, outcome: ActivationOutcome::Hit(DamagePayload{ amount:
damage }) }` and drive the three-step sequence the per-impact loop uses today —
`spawn_impact_effect_at` → liveness re-check of the **target** → `apply_authorized_weapon_impact_damage(
registry, weapon_id, attacker, &impact, credit_source, damage)` — then despawn the
projectile so damage applies at most once (Invariant "at-most-once"); `run_death_sweep` handles a
resulting 0 HP. **One deviation from the hitscan loop:** that loop `continue`s (skips damage) when the
owner pawn is gone; a projectile must **not** — it is a fair in-world object that outlives its firer, so
pass `attacker = registry.exists(owner_pawn).then_some(owner_pawn)` (`None` if despawned) and credit by
the stored `credit_source` string, gating only on **target** liveness (Orderings P9). `attacker = None` is
safe end-to-end (verified): the contributor ledger and kill-credit are **string-`source_id`-keyed** so
credit survives, and every `attacker`/`source` consumer is `Option`-typed — the only effect of a `None`
attacker is that a policy effect routed to `@impact.source` (e.g. grant-health-to-shooter) correctly
no-ops, since there is no live shooter to receive it. **No stable owner-identity snapshot is needed for
credit** — the stored `owner_pawn` id may dangle and is only liveness-checked (`registry.exists`). On reaching the travel bound (range or lifetime) with no contact, despawn with no
damage. AC: a projectile applies damage on the impact tick, not the fire tick; an expiring one
applies none; a projectile whose center-line would miss a target but whose radius overlaps it still
hits (width matters); the spawned projectile carries the descriptor's visual component(s) —
`SpriteVisual`, `MeshComponent`, and/or `BillboardEmitter` per the visual kind (assertable) — so it
renders while travelling; hitscan/pellet weapons are unchanged (their `r = 0` entity query is byte-identical).

### Task 2: Descriptor validation + SDK surface + reference weapon

Complete the author-facing surface for the projectile mode whose Rust shape Task 1 pins. Harden
`WeaponDescriptor::validate` for the projectile fields — speed finite and > 0, radius finite and ≥ 0
(meters), lifetime finite and ≥ one tick (a sub-tick lifetime yields no visible flight) — and the
**visual**, a discriminated union: a **body** of kind
`sprite` (a content-relative sprite path) or `model` (a content-relative glTF model path), plus an
optional **trail** emitter config. Validate each body path with the existing
`is_portable_content_relative_asset_path` check (the weapon model-path fields' check), the **trail with
foundation-local field bounds** authored here (rate/lifetime/spread finite and ≥ 0, sprite path via
`is_portable_content_relative_asset_path`) — **not** the `entities` emitter's validator, which is
unreachable from `foundation` (no `entities` dep; the trail is a foundation-local descriptor struct per
Task 1) — and reject a `resolution: "projectile"`
weapon that omits speed **or** a body (either missing fails validation). Each error is a field-named `DescriptorError::InvalidShape`
mirroring the existing weapon-field messages. Extend the TS and Luau SDK typedefs so
`resolution: "projectile"` and its tuning (speed, radius, lifetime, and the visual union) are
authorable and type-checked (the primitive-surface contract: SDK types and validation move in the
same pass as the Rust enum) — the typedefs are **generated** (the `scripting-core` typedef generator, e.g.
its `ResolutionMode` mapping), so **regenerate and update the expected `.d.ts` / `.d.luau` fixtures**
in the same pass; the new nested `projectileVisual`/trail descriptor types carry `serde(rename_all =
"camelCase")` to match the `WeaponDescriptor` wire contract. Mirror the discriminated-union-per-kind
pattern the existing `WeaponResource` / UI descriptors use. Author reference projectile weapons in the dev mod covering the
modes (a **plasma bolt** — sprite body; a **rocket** — model body + trail emitter) and place them so
the dev map can fire them. Update
the weapon-authoring reference docs to cover the mode. This is content + boundary typedefs only; it
adds no runtime behavior beyond Task 1's. AC: an out-of-range or speed-less projectile descriptor is
rejected with a field-named error; the TS and Luau typedefs expose the mode and fields; the dev mod
carries a working reference projectile weapon.

### Task 3: Co-op authority — client-predicted projectile + later-tick declaration + open-shot lifetime

Make the projectile co-op-correct for a **connected client** by reusing the shipped
client-authoritative-HIT path, the net-new mechanism being deferred declaration timing plus a small
host-internal **fire-time snapshot on `AuthorizedShot`** (resolution mode, muzzle origin, per-shot
timeout budget — all off the wire). On the client, a projectile weapon's fire spawns a **local** projectile
built like Task 1's (same `ProjectileComponent`, and it attaches its body/trail via the shared
`attach_projectile_visual` helper so it is **visible to its firer**, per the "visible while travelling"
AC), but at a **different seam**: the shipped client hit path (`resolve_client_fire` →
`resolve_client_hitscan`) returns synchronous `hits` that `run_client_fire_path_post_loop` (main.rs) sends
**inline at fire**; the **projectile arm of `resolve_client_hitscan` instead returns empty** (no
synchronous hit) and spawns a **client-local in-flight projectile** in that post-loop path, which persists
across frames and is advanced **once per frame, only there — never by Task 1's tick stage** (role-gated
off on the connected client), **reusing Task 1's advance body verbatim with `frame_dt` substituted for
`tick_dt`** — including the `min(speed·frame_dt, remaining_range)` **segment cap** and the
**contact-wins-over-expiry** rule, so one advance yields **exactly one** outcome (a record on contact XOR
an empty on travel-bound expiry, never both). It runs at the rendered (interpolated) pose — mirroring the
shipped client fire path's placement rationale (it must read the interpolated Transforms
`sample_into_registry` wrote and the render-stage anim clock), never inside the movement predict loop. The per-frame advance integrates by the render-path **per-frame wall-clock delta** (`frame_dt`, the delta
already threaded to the animation clock — **not** the cooldown path's `elapsed_ms`, which is a
tick-quantized accumulator computed *inside* the predict loop), so wall-clock flight ≈ `range/speed`
regardless of frame rate; otherwise a low-FPS client stretches flight past the host's tick-based timeout.
The client advance also honors the **spawned-this-tick guard** (Task 1), so a point-blank client shot
never impacts on its **spawn frame**; the authority path is unaffected regardless, since the declaration
always arrives a later *host* tick over the network. When the client projectile resolves — a record on
contact **or** an empty on travel-bound expiry, the single mutually-exclusive outcome of that advance —
send the **existing** `HitDeclaration { shot_id, records }` through the shipped
`client_send_hit_declaration` seam (the same send `run_client_fire_path_post_loop` uses at fire for
hitscan). Two **distinct** retirement mechanisms — do not conflate them: **(a)** the primary projectile's
shot is retired **only** by this deferred client-local send (record on contact, empty on expiry), and its
fire-tick record-bearing send is **suppressed**; **(b)** the existing `skip(1)` per-extra-tick empty loop
keeps its sole current role — retiring the *extra* authorized shots of a multi-tick catch-up frame (ticks
for which the client spawns no projectile), sent at fire as today — it does **not** cover the primary
shot's later expiry. Carry the **fire's** `shot_id` (computed at fire as today: pawn `NetworkId` high,
`client_tick` low) — now legitimately sent on a **later tick** than the fire; the record is the struck target's `NetworkId` + point + zone via the
existing client reverse map. No new message and no new field: the shipped `HitDeclaration`/`ShotVerdict`/`shot_id` contract already
admits a later, 0..N-record declaration, so **no version constant bumps** (Invariant "no wire
change"). Predict muzzle-FX and cooldown **at fire** through the existing `ClientPredictedShots` keyed by that
`shot_id`. The **hitmarker** needs a small new client-local hook: the shipped `predict()` sets
`hitmarker_visible` at fire from its `resolution.hits`, which are **empty** for a projectile (it hasn't
travelled), and nothing flips it true later — so add a `ClientPredictedShots::mark_hit_on_impact(shot_id)`
that sets `hitmarker_visible` on the still-`Pending` record at the tick the client's own projectile lands
(the record-bearing declaration's tick); the projectile path must **not** derive the hitmarker from
`predict()`'s fire-time hits. This is client-local presentation, off the wire, shown never at fire and
never on the expiry/miss path (a projectile can miss or land seconds later, so a fire-time hitmarker would
be a phantom). Reconcile FX / cooldown / hitmarker against `ShotVerdict`, rolling back on **reject**;
enemy HP is never predicted (remote enemies carry no client `Health`).

On the **host**, the authorized-shot store recorded on the FIRE path must keep a projectile's shot
**open across ticks** until its declaration binds and retires it (today a hitscan shot is declared
same-tick). At FIRE, snapshot on the shot's `AuthorizedShot` (beside the existing `damage`/`range`/
`pellet_count`/`fire_tick` — host-internal, no wire change): its **`ResolutionMode`** (or an
`is_projectile` bool), its **muzzle origin** (a `glam::Vec3` — the type of `WeaponFireCommand.aim_origin`
and `attacker_eye`; not the parry `Point3`), a **record cap of 1** (`pellet_count = 1` — a
projectile authorizes exactly one record slot; the shipped ingest short-circuits `pellet_count == 0` to
no-hit, so a projectile that left it 0 would silently apply no damage), and a **per-shot timeout budget**
= `max(MAX_OPEN_SHOT_AGE_TICKS, ceil(min(range / speed_s, lifetime_ms / 1000) / tick_dt_s) + RTT_margin_ticks)`.
Mind the units: `range / speed` is **seconds**, `lifetime` is authored in **milliseconds**, so convert
before the `min`. The shipped open-shot pruning is a single fixed `MAX_OPEN_SHOT_AGE_TICKS = 180` (3 s)
sized for same-tick hitscan; change `prune_stale` to compare each shot against **its own** stored budget
instead — it reads `current_tick - fire_tick <= shot.budget` (the budget field snapshotted above), not the
global constant.
The **`max(180, …)` floor is load-bearing**: the per-shot travel estimate alone can be *smaller* than 180
for a fast/short projectile, and the old fixed window was silently absorbing client frame-hitches (a long
GC/asset-load frame freezes the client projectile, delaying its declaration in wall-clock); flooring at
180 preserves that hitch tolerance, while the per-shot value lifts the ceiling for a slow/long projectile
(e.g. `speed=10, range=1000` ≈ 100 s, or the reference rocket's 4 s lifetime > 180 ticks) that a fixed
constant would evict mid-flight and false-reject (Invariant "no false-reject of a legitimate hit"). A client
hitch longer than the whole budget window is an **accepted** false-reject (the same bounded-tolerance
class as networking.md's peer-suspension timeout), not a case the budget can cover. On the client, send an
**empty** declaration when a projectile expires without hitting, so the host retires the shot promptly
rather than waiting for the timeout. Intra-frame, host pruning runs **before** declaration ingest
(confirmed: `prune_stale` then `drain_ready` in `host_flush_pending_hit_declarations`), and `prune_stale`
**retains** while `current_tick - fire_tick <= budget` — so the first *pruned* tick is `elapsed = budget + 1`,
and a declaration arriving then is rejected (timeout wins the tie). Keep the `<=` comparison; the budget's
RTT margin gives headroom so a legitimate declaration always arrives at `elapsed ≤ budget`. On host
demotion / level change or a client disconnect, the open-shots store is cleared through the shipped
`remove_client`/`remove_pawn` paths (networking.md: any exit from participating clears combat), so a
client's in-flight projectile shot is dropped, its predicted local projectile is cleared with the
registry, and a stale later declaration binds to nothing (Orderings P15). **Projectiles skip world-LOS (Check 3) *and* re-anchor the range check (Check 4).** Check 3
casts a static-world ray, and Check 4 measures range, both from the shooter's *present-tick* eye — sound
for same-tick hitscan, but for a projectile the declaration arrives a later tick after the shooter has
moved: Check 3 would false-reject a fire-and-take-cover hit (and only ever defends the wallhack case, a
PvP concern this engine rules out), and Check 4 from the moved eye would false-reject a fire-and-reposition
hit. So a **projectile** declaration is validated by Check 1 (`shot_id` binding — the security spine),
Check 2 (target resolvable + `HealthComponent`-bearing via `NetworkId` — liveness itself is the downstream
death latch, not this check), and Check 4 **measured from the snapshotted fire-time muzzle origin, not the
present eye** — no world-LOS. The host branches on the authorized shot's **snapshotted `ResolutionMode`**
(read directly off `AuthorizedShot` — no pawn/`WeaponOwners` hop). `apply_valid_hit_record` (where Checks
3/4 live) must receive **both** a skip-LOS flag **and** the fire-origin `Vec3` value (a flag alone is
insufficient — Check 4's distance is computed from the origin), threaded from `ingest_hit_declaration`, or
the `AuthorizedShot` reference it reads them from. The projectile branch must also **not** early-return on
`attacker_eye == None` the way the shipped function does (it ranges from the fire origin, not the present
eye; a despawned owner clears the shot via `remove_pawn` anyway, so this is belt-and-suspenders). A
projectile skips Check 3 and re-anchors Check 4; **hitscan and pellet weapons keep the shipped present-eye Check 3 and Check 4
unchanged**. Damage application and the credit path are the shipped ones. AC: a connected client's projectile hit is
declared on a later tick and applied host-side regardless of the shooter's movement or intervening
geometry during flight (fire-origin range + no world-LOS to false-reject it); a single-record projectile
declaration applies damage (`pellet_count = 1`); a slow/long projectile under latency is accepted (per-shot
timeout); a rejected fire yields no projectile damage (`shot_id` binding); an undeclared shot is retired by
the per-shot timeout; the open-shots store does not grow unbounded; no wire constant bumps.

### Task 4: Remote-observer replicated projectile visuals (separable)

Give remote peers a view of a projectile in flight, as presentation only — the separable task and
the cut line for a leaner v1. On every projectile fire, the **host** spawns a presentation-only
projectile entity that travels the deterministic straight-line path from the pawn muzzle along the
pawn's aim (the listen-host's own camera aim for its pawn; a connected client's pawn aim
reconstructed from the already-replicated `facing_yaw` + `aim_pitch` — verify both are available at
the weapon stage, which runs after the host camera stage; if `aim_pitch` is not exposed there, source it
from the host camera stage's per-pawn output, or fall back to the replicated Transform facing, since the
visual is presentation-only and reduced pitch precision is acceptable) at the descriptor speed, and despawns it
when the shot retires (declaration or timeout) or its own path reaches the travel bound. **For a host-local
fire (SP / listen-host) there is no client declaration or open-shot to retire it**, so tie this
presentation copy's despawn to its **sibling Task 1 gameplay projectile** — despawn it when that gameplay
projectile despawns (on impact or travel-bound) — so remote peers do not watch it overshoot past an enemy
the gameplay projectile already hit. **Do not invent a
second mover:** this entity carries a `ProjectileComponent` tagged **presentation-only** (a flag the Task 1
advance reads) so the *same* Task 1 advance stage integrates its straight-line motion each host tick — but
a presentation-only projectile **never sweeps for contact, resolves an impact, or applies damage** (the
gameplay hit is the firer's client declaration, or the host's own Task 1 gameplay projectile); it only
moves so its replicated Transform snapshots show flight. The host entity carries a synthesized
`DescriptorProvenance` (the owning weapon's `canonical_name`, plus any `spawn_path`/owned-component fields
the struct requires — the projectile arm keys **solely on `ComponentKind::Projectile`**, so `spawn_path`
is irrelevant to classification; pick any valid value) — required, since `descriptor_entity_class`
classifies **via `DescriptorProvenance.canonical_name`** and early-returns `None` without it, so a
Transform-only entity would never replicate. Classify
this entity for replication: add a projectile arm to `netcode/descriptor_class.rs::descriptor_entity_class`
**gated on the entity carrying `ComponentKind::Projectile`** (so a projectile snapshot is classified as a
projectile and not confused with the weapon descriptor of the same `canonical_name`, nor with a pawn/enemy
— those carry no `Projectile` component), returning a class string the client routes to a **projectile
materializer arm** in `netcode/remote_materialize.rs`. That arm attaches only presentation — the **same
visual component(s) Task 1 attaches locally** (the `SpriteVisual`/`MeshComponent` body plus any
`BillboardEmitter` trail, via the shared `attach_projectile_visual` helper) — by resolving the weapon
descriptor named by `canonical_name` and reading its `projectileVisual` union (its `resolution ==
projectile` confirms the routing; descriptors are shared content) — no `Health` or gameplay component beyond the
`DescriptorProvenance` marker — so the entity
rides the **existing** Transform + `entity_class` snapshot record
(valid on any Transform-bearing record since the client-auth spec) and interpolates like any
replicated entity, adding **no wire layout change** (Invariant "no wire change"; verify the classname
string is the only new value). A connected client spawns **no** local authoritative projectile for a
**remote** peer's fire — respect `SpawnContext.can_materialize_runtime_spawns` (false for a connected
client), so remote projectiles arrive solely as host snapshots (networking.md spawn-suppression). The
firing connected client suppresses the host-replicated copy of **its own** shot (it already renders
its Task 3 predicted projectile), the same "the local pawn is the host's pawn" suppression the
movement path uses — coordinate with Task 3 on the client render path so the predicted projectile is
shown and the replicated duplicate is hidden. **The listen-host has the same double-entity for its own
fire** — its Task 1 gameplay projectile (not replicated) plus this presentation copy — so it likewise
suppresses local render of its *own* pawn's presentation projectile, showing only the gameplay one.
Accept that the host visual (pawn replicated aim) and
the firer's predicted projectile (full camera aim) may diverge slightly; the visual is presentation
for remote observers only. AC: a peer's projectile is visible in flight to other peers and despawns
on shot retire; a connected client spawns no authoritative projectile for a remote fire; the firing
client shows its predicted projectile, not a doubled copy; no wire layout change.

### Task 5: Tests

Extend the weapon-stage and netcode `predict_reconcile_harness` scaffolding to cover the projectile
behaviors deterministically. This includes a **harness extension** to drive the client's per-frame
`frame_dt` advance and the `sample_into_registry` interpolated poses (P10 and the AC's "rendered/interpolated
pose" depend on the harness controlling `frame_dt` and pose), and `#[cfg(test)]` observability of
`open_shots.len()` and `hitmarker_visible` — scope that extension as part of this task. The Orderings pin
table (P1–P18) is the source of truth for the **ordering** assertions; the **non-ordering** behavioral tests (radius-overlap width, the `r = 0` regression,
visual-component-carry, and the P3 sub-tick-lifetime validation) are named here directly, not as P-rows.
Single-player / host: a **point-blank** projectile applies damage at **N+1, not the fire tick** (P1 — a
far-target test passes even with the ≥1-tick guard broken, so the point-blank case is mandatory); a
high-speed projectile whose one-tick travel exceeds range resolves at N+1 with no tunneling and no
beyond-range overshoot (P2); a projectile that reaches its travel bound applies no damage and despawns;
two rapid-fire projectiles resolve independently, and two hitting the same enemy on the same advance tick
**both** apply (P4); a projectile whose owner pawn despawned mid-flight still applies with `attacker = None`,
credited by string (P9); a projectile whose target died mid-flight applies no second hit (P17).
**Non-ordering behavioral tests:** a projectile whose center-line misses a target but whose radius overlaps
it registers a hit **in both the zone-capsule and the AABB-hitbox narrow phases**; an existing
hitscan/pellet weapon's entity query is byte-identical to today (`r = 0` regression, both narrow phases +
broad phase); a spawned projectile carries the descriptor-specified visual components (sprite- vs.
model-body, and a trail `BillboardEmitter` when declared); a sub-tick lifetime is rejected at `validate()`
(P3, in Task 2's descriptor-validation suite). Co-op: a connected client's later-tick declaration is
validated and applied host-side crediting the authorized pawn; a **single-record** declaration applies
damage (`pellet_count = 1`) (P12); a **moved/repositioned-shooter** declaration is accepted (fire-origin
range, no world-LOS) (P7, P8); a **slow/long** projectile beyond the old 180-tick constant is accepted
under the per-shot budget (P5); a **low-FPS client** (`frame_dt` advance) is accepted (P10); a fire the
host rejected yields no projectile damage even with a plausible later declaration (`shot_id`-binding
security test) (P14); a **second declaration for an already-retired shot** is rejected (P13); a declaration
on the first pruned tick is rejected (prune-before-ingest) (P6); an undeclared open shot is retired by the
per-shot timeout with the store bounded (P18); an expiry-without-hit empty declaration retires the shot
with no phantom hitmarker, and the client hitmarker appears at **local impact** via `mark_hit_on_impact`,
never at fire (P11); a hitscan/pellet weapon still runs its present-eye Check 3/Check 4 unchanged (P8); the connected client's
**predicted projectile advances exactly once per frame and is neither swept nor despawned by the fixed-tick
advance stage** (which is role-gated off on the client) — assert it survives to emit its deferred
declaration.
**Host demotion / level change (authority, unconditional):** a client projectile's shot is cleared from
`open_shots` on demotion, its later declaration rejected, and its in-flight predicted projectile cleared
with the registry (P15 authority half). If Task 4 landed: a replicated projectile visual materializes on a
remote and despawns on shot retire, a connected client spawns none locally for a remote fire, and the
replicated visual is cleared on demotion too (P15 presentation half, P16). Two explicit guards the
order-independent `VARIANTS`/`COUNT` array does not give for free: an `assert_eq!(ComponentKind::Projectile
as u16, 21)` discriminant drift-guard (Invariant "component-kind tail-append"), and a
grant-health-to-`@impact.source` policy fixture asserting **no grant** when `attacker = None` (the P9/AC5
no-op sub-claim). Assert no new `unsafe` (grep gate) and, for the authority path, that **no** version constant changed
(`WIRE_VERSION` still `19` and all others unchanged). AC: the harness exercises each pinned ordering row
(P1–P18) plus the named non-ordering behavioral tests deterministically.

## Sequencing

**Phase 1 (sequential):** Task 1 — thin slice through descriptor → fire branch → component →
advance stage → collision → damage chokepoint, single-player/host. Falsifies the boundary
assumption (a deferred impact reuses the `WeaponImpact` → `apply_authorized_weapon_impact_damage`
path) before any co-op or content fan-out.
**Phase 2 (concurrent):** Task 2 (descriptor/SDK/content), Task 3 (co-op authority), Task 4
(remote-observer visuals) — disjoint seams on top of Task 1. Task 3 and Task 4 both touch the
connected-client path (Task 3 renders the predicted projectile; Task 4 suppresses the replicated
duplicate of the same shot) — merge-coordinate the client render/suppression seam.
**Phase 3 (sequential):** Task 5 — verifies the surface once behavior lands.

## Rough sketch

Grounded seams, exact signatures, the lifecycle diagram, the observer × lifecycle cross-product,
and the split-before-extend flags live in `research.md`. New code lands in **new modules**
(`entities/src/components/projectile.rs`, `sim/projectile_stage/` or `weapon/projectile.rs`) so the
oversized files (`sim/mod.rs` 2872, `weapon/mod.rs` 1944, `registry.rs` 2106, `hit_zones.rs` 3583)
gain only a stage call, a fire arm, an enum-tail variant, and a reused query respectively.

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| Projectile mode | `ResolutionMode::Projectile` | descriptor JSON `"resolution": "projectile"` (content data, shared by both peers — **not** replicated) | `"projectile"` | `"projectile"` | n/a (tuning is descriptor-owned) |
| Launch speed | `WeaponDescriptor` speed field (m/s, >0) | descriptor JSON camelCase | typedef field | typedef field | n/a |
| Collision radius | `WeaponDescriptor` radius field (meters, ≥0) → swept-sphere half-width; Minkowski-added at **both** entity narrow phases (`ray_capsule_or_ball` + `ray_aabb_slab`) and the query-time broad phase (`transformed_zone_bound`); `r = 0` byte-identical for hitscan/pellet | descriptor JSON camelCase | typedef field | typedef field | n/a |
| Projectile visual | `WeaponDescriptor` visual — discriminated union: body `sprite`→`SpriteVisual` \| `model`→`MeshComponent`, + optional `trail` (a **foundation-local** emitter descriptor struct, converted to `entities::BillboardEmitterComponent` at the spawn site — `foundation` cannot depend on `entities`) | descriptor JSON, `kind`-tagged (camelCase) | typedef union per kind | typedef union per kind | n/a |
| Travel lifetime | `WeaponDescriptor` lifetime field (ms, ≥ one tick); `range` bounds distance | descriptor JSON camelCase | typedef field | typedef field | n/a |
| Projectile component | `ComponentKind::Projectile = 21` (tail), `ComponentValue::Projectile`, `ProjectileComponent` | not replicated for the gameplay projectile; Task 4's **presentation** projectile rides the existing Transform + `entity_class` record | n/a | n/a | n/a |
| Hit declaration | **reused** `HitDeclaration { shot_id, Vec<HitRecord> }` | **unchanged** — no new message/field; later-tick arrival already legal | n/a | n/a | n/a |
| Authorized-shot fire-time snapshot | `AuthorizedShot` gains `ResolutionMode` (or `is_projectile`), muzzle **origin** (`Vec3`), record cap (`pellet_count = 1`), and a **per-shot timeout budget** `max(180, ceil(min(range/speed, lifetime)/tick) + RTT)` — snapshotted at FIRE | **host-internal, not serialized** — no wire change | n/a | n/a | n/a |

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| **At-most-once damage** per projectile | Task 1 (despawn on first contact; liveness gate) | double-resolve if the projectile isn't despawned on contact; a re-declared shot (Task 3 retires on accept) | AC "at most once"; Task 5 P4 + target-died-mid-flight (P17) |
| **≥1 tick of flight** (scoped to the **local** SP/host advance) — damage never lands on the fire tick; on the co-op host the client's declaration always arrives a later host tick regardless | Task 1 (spawned-this-tick flag; first advance is N+1) | a point-blank or very-fast shot resolving on tick N; a reliance on the render-only prev==cur rule | AC "on the impact tick, never the fire tick"; Task 5 P1 point-blank + P2 high-speed |
| **Shot always retires** (declaration or per-shot timeout); open-shots store bounded | Task 3 (per-shot timeout + client empty declaration) | a projectile that never declares; a client that drops mid-flight | AC "expiry → empty declaration; dropped → timeout; store bounded"; Task 5 P18 timeout + P11 empty-declaration |
| **No false-reject of a legitimate hit** — projectiles run no world-LOS and range-check from the **snapshotted fire origin** (only `shot_id` + Health-bearing + fire-origin range), and the timeout is **per-shot** = `max(180-tick floor, ceil(min(range/speed, lifetime)/tick) + RTT margin)` | Task 3 (skip Check 3; re-anchor Check 4; per-shot timeout budget on `AuthorizedShot`) | a fixed timeout shorter than a slow projectile's flight; a present-eye range check on a moved shooter; a boundary-tick declaration under prune-before-ingest | AC "no legitimate hit is false-rejected"; Task 5 P5 slow-under-latency + P7 moved-shooter + P8 take-cover + P10 low-FPS |
| **Enemy HP never client-predicted** | Task 3 (reuses client-auth structure) | any client write to enemy `Health` (structurally absent — remotes carry no `Health`) | AC "enemy HP is never predicted … no enemy-HP rollback" (structural gate) |
| **No wire-format change on the authority path** — the `AuthorizedShot` fire-time snapshot (resolution/origin/timeout/record-cap) is host-internal, never serialized | Task 3 (reuse `HitDeclaration`/`ShotVerdict`; host-only `AuthorizedShot` fields), Task 4 (reuse `entity_class` record) | a new message/field or a version-constant bump; serializing an `AuthorizedShot` fire-time field | AC "no new wire message or field … no version constant bumped" + the Task 4 "no wire layout change" AC; Task 5 no-constant-changed assertion |
| **`ComponentKind::Projectile` appended at the enum tail** | Task 1 | a mid-enum insert renumbering later wire discriminants | Task 5 `assert_eq!(ComponentKind::Projectile as u16, 21)` drift-guard |
| **Hitscan/pellet entity query byte-identical** (existing hitscan/pellet callers pass `r = 0` to the shared `nearest_entity_hit` radius param) | Task 1 (radius param, `r = 0` on existing callers) | the shared query gaining radius behavior; a caller omitting `r = 0` | AC "existing hitscan and pellet-spread weapons are behaviorally unchanged"; Task 5 `r = 0` regression |

## Orderings

Each row is a pinned **ordering** scenario Task 5 tests deterministically (the source of truth for the
ordering assertions). Non-ordering behavioral tests — radius-overlap width, the `r = 0` regression,
visual-component-carry, descriptor validation — are named in Task 5 directly, not as P-rows.

| # | Scenario | Ordering | Expected outcome |
|---|---|---|---|
| P1 | Point-blank fire (target within one tick's travel) | spawn tick N → advance skips the spawned-this-tick projectile; first real sweep at N+1 | Damage lands at **N+1, never N**. The far-target test passes even with the guard broken, so this case is mandatory. |
| P2 | High-speed projectile, `speed·tick_dt ≥ range` | first real advance N+1 sweeps the whole range as one segment | Impact at **N+1**, one full swept-sphere segment **capped at the remaining range** (`min(speed·tick_dt, remaining_range)`), no tunneling and no beyond-range overshoot hit. |
| P3 | Sub-tick lifetime `0 < lifetime < tick_dt` | authored below one tick | **Rejected at `validate()`** (lifetime ≥ one tick), so it never spawns. |
| P4 | Two rapid-fire projectiles hit the same enemy on the same advance tick | pass 1 both detect contact; pass 2 applies both, then despawns both | **Both** apply (per-projectile at-most-once, **not** per-target); a second application past 0 HP no-ops via the death latch in `apply_damage_with_context`. (SP/host: two distinct projectile *entities*, no `shot_id`s — those exist only on the client-declaration path.) |
| P5 | Slow/long projectile whose flight exceeds the old fixed 180-tick constant | fire host tick N; legitimate declaration at N+k, k > 180 | **Accepted** — per-shot budget = `max(180, ceil(min(range/speed, lifetime)/tick) + RTT)`, whose per-shot ceiling exceeds 180 here. |
| P6 | Declaration arrives on the first *pruned* tick (`elapsed = budget + 1`, since `prune_stale` retains while `elapsed ≤ budget`) | host frame: `prune_stale` (retire) → drain+ingest (miss) → `ShotVerdict(accept=false)` | **Rejected** (timeout wins the intra-frame tie); keep the `<=` comparison; the RTT margin gives headroom so a *legitimate* declaration always arrives at `elapsed ≤ budget`. |
| P7 | Shooter dashes far during flight, then declares | fire N at origin O; shooter at O′ by declaration tick; host range-checks | **Accepted** — Check 4 measures from the snapshotted fire-time origin O, not the moved eye. |
| P8 | Shooter takes cover (wall between present eye and impact) during flight | fire N with LOS; wall interposed by N+k | **Accepted** — projectile arm skips world-LOS; a hitscan/pellet arm still runs present-eye Check 3/Check 4. Assert both branches. |
| P9 | Owner pawn despawns mid-flight, projectile then contacts enemy (host/SP) | owner despawn N+2; contact N+3 | Damage **applies** with `attacker = None`, credited by the `credit_source` string (the projectile arm drops the hitscan owner-liveness `continue`). |
| P10 | Low-FPS client fires a normal projectile | client advances by **`frame_dt`** (per-frame wall-clock) per frame, not fixed `tick_dt` | **Accepted** — wall-clock flight ≈ `range/speed`, declaration arrives before the per-shot timeout; the 180-tick floor + RTT margin covers steady low-FPS and bounded hitches (an unbounded hitch is the accepted-loss case). |
| P11 | Projectile expires without hitting → empty declaration | client expiry → `HitDeclaration{records:[]}` → host ingest | Shot retired promptly; no damage; **no phantom hitmarker** on the client (hitmarker is impact-only). |
| P12 | Single valid projectile hit, host ingest | declaration with 1 record; projectile `pellet_count = 1` | Record processed, damage applies (a `pellet_count = 0` would short-circuit to no-hit). |
| P13 | Second declaration for an already-retired shot | ingest retires on first accept; second `HitDeclaration` same `shot_id` | Second finds no open shot → `accept=false`. One authorized fire ⇒ at most one accepted declaration. |
| P14 | Rejected fire (cooling/empty) then a plausible later projectile declaration | fire rejected → no `AuthorizedShot` minted; declaration binds to nothing | **No damage** (`shot_id` binding). |
| P15 | Host level change / demotion with a client projectile in flight | demote clears `open_shots` (`remove_client`/`remove_pawn`); later declaration arrives; registry teardown | **Authority (Task 3, unconditional):** declaration rejected (shot gone); the in-flight predicted projectile is cleared with the registry; no dangling impact next level. **Presentation (Task 4):** the replicated visual is cleared too. |
| P16 | Remote-observer trail sampled per frame over a multi-tick catch-up advance | advance runs k ticks in one frame; snapshot/`emitter_bridge` samples once | Presentation-only: accept trail gaps / interpolation lag; the gameplay hit is unaffected. Pin as presentation-tolerance, not a sim bug. |
| P17 | Target dies (another source) while the projectile is in flight (host/SP) | death precedes impact | On impact the death latch no-ops the second application; the projectile still despawns; no double-kill credit. |
| P18 | A projectile whose declaration never arrives (dropped client), no expiry-empty either | fire host tick N; no declaration; `elapsed` passes the per-shot budget | Retired by the per-shot timeout; `open_shots` stays bounded across a session. |

## Script syntax example

```ts
// Proposed — a direct-impact projectile weapon (dev-mod reference).
const plasmaRifle = defineEntity({
  components: {
    weapon: {
      damage: 25,
      range: 128,                          // meters
      fireRateMs: 180,
      fireMode: "auto",
      resolution: "projectile",
      // projectile tuning (descriptor-owned; never an FGD KVP):
      projectileSpeed: 80,                 // m/s, straight-line (crosses 128 m in ~1.6 s)
      projectileRadius: 0.2,               // meters — swept-sphere half-width (a bolt, not an explosion)
      projectileLifetimeMs: 4000,          // travel time cap (range also bounds distance)
      projectileVisual: {                  // discriminated union: a body (+ optional trail)
        body: { kind: "sprite", sprite: "sprites/plasma_bolt.png" },
        // rocket variant:
        //   body:  { kind: "model", model: "models/rocket/rocket.gltf" },
        //   trail: { sprite: "sprites/smoke.png", rate: 60, lifetime: 0.6 /* …BillboardEmitter cfg */ },
      },
      creditSource: "plasma",              // damage-attribution key (source-id ledger), NOT an ammo type
    },
  },
});
```
(Field grouping is decided: flat `projectile*` scalars on the weapon descriptor + a nested
`projectileVisual` union — see Task 1 and Decisions below. Exact identifier spellings are Task 1/2's to
finalize against the SDK naming convention.)

## Decisions

No open questions remain; the four prior unknowns are resolved, each with its warrant, and folded into
the task that implements it.

- **Field grouping → flat scalars + nested visual union** (Task 1). The scalars
  (`projectileSpeed`/`projectileRadius`/`projectileLifetimeMs`) are flat on `WeaponDescriptor`; the visual
  is a nested `projectileVisual` union. Warrant: the nearest codebase precedent for resolution-mode-specific
  scalars is the flat `pellet_count`/`spread_degrees` pair; the sub-block form (`resource`) is for
  discriminated unions of shapes, which the scalars are not — the visual, which *is* a discriminated union,
  takes the nested form.
- **Visual union shape → body (`sprite`|`model`) + optional `trail` emitter** (Task 1/2; boundary
  inventory). Warrant: the canonical rocket (model body + smoke trail) must be expressible together; the
  cheaper flat one-of-three union (`sprite`|`model`|`emitter`) cannot do model + trail simultaneously, and
  the emitter bridge already trails a moving host for free.
- **Owner-despawn credit → apply with `attacker = None`, credit by `credit_source` string; no owner-id
  snapshot** (Task 1; Orderings P9). Warrant: verified against source — the contributor ledger and
  kill-credit are string-`source_id`-keyed, every `attacker`/`source` consumer is `Option`-typed, and the
  only effect of `None` is that a policy effect routed to `@impact.source` correctly no-ops (there is no
  live shooter to receive it). No consumer requires a live attacker id.
- **`ActivationOutcome::Spawned` semantics → add a one-line doc-comment; no variant change** (Task 1).
  Warrant: `weapon-model.md` §8 gives the consumer sketch (`Spawned(id) => track on this instance`),
  showing the transient-vs-tracked distinction lives in the consumer's match arm, not the variant; §10
  frames `Spawned` for a persistent tracked charge (contrasted with `Hit`). This spec introduces the
  *transient* use, and the doc-comment records that the distinction is the consumer's concern — so the
  later detonator spec retrofits nothing.
