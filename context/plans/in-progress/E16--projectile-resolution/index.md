# E16 — Projectile Resolution

> **Status:** ready (reviewed — direction, draft-spec ×3, implementability, and two source-anchored verification rounds; awaiting orchestration).
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
  arrives; the host's existing stale-shot prune (`OpenAuthorizedShots::prune_stale`, re-sized
  per-projectile from the fire-time snapshot below) backstops a dropped client; the client sends
  an **empty** declaration on expiry-without-hit to retire promptly.
- **Fire-time authority snapshot** (host-internal, off the wire): the FIRE path records on each
  projectile's `AuthorizedShot` an `is_projectile` flag, the muzzle **origin** (`Vec3`, the firing
  pawn's fire-tick eye — computed host-side, **not** taken from `WeaponFireCommand.aim_origin`, which
  is `Vec3::ZERO` on the host authorization path by design), and a per-shot timeout budget.
  `AuthorizedShot` is a host-side struct, never serialized, so this adds no wire contract. It lets the
  host validate a later-arriving declaration without re-resolving the weapon (which may swap mid-flight)
  and anchors the range check to where the shot was fired.
- **Remote-observer visuals** (separable — the cut line for a leaner v1): the host spawns a
  presentation-only replicated projectile per fire (from the pawn's replicated aim + descriptor
  speed), deterministic straight-line, despawned on shot retire/timeout; remotes interpolate it;
  the firing client suppresses the replicated copy of its own shot.
- SDK typedefs (TS + Luau) and descriptor validation for the new mode; one reference projectile
  weapon authored in the dev mod.

### Out of scope (non-goals)

- **AoE / splash, and the rocket launcher.** The next Resolution-Modes spec; this fills only
  the projectile-that-resolves-`Hit`.
- **Gravity / arc, bounce / ricochet, penetration, homing.** Straight-line only in v1; additive
  later via descriptor fields, no rework.
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
host-internal fire-time snapshot on `AuthorizedShot` (`is_projectile`, muzzle origin, timeout budget) plus
open-shot lifetime — both host-side and off the wire (Co-op fairness, below). Projectiles skip world-LOS
(Check 3); if a future spec ever wants world-LOS for a projectile mode, the snapshotted origin already
carries the fire-time geometry it needs — a contained additive change. `ComponentKind::Projectile` must
append at the enum tail (mid-insert renumbers wire discriminants); this is a one-line ordering constraint,
not a door. Undoing the whole feature is deleting one sim stage, one component kind, the descriptor fields,
and the three snapshot fields — nothing else depends on its internals.

**Co-op fairness (PvE-among-friends).** This is a co-op PvE engine — no PvP, no live service — so the
client firer must be honored exactly as much as the host firer. Both get identical *feel*: an instant
predicted projectile, muzzle FX, and hitmarker at their own screen's timing; the only difference is
*where* damage is applied (host locally, client via declaration), never *whether* a legitimate hit
counts. The host's validation of a client declaration is a **corruption/desync sanity check, not an
anti-cheat gate** (trust the friend; catch the bad packet). A projectile declaration arrives a **later
tick** than its fire, after the shooter has moved — so the two shipped checks that read the shooter's
*present-tick* eye are unsound for it. **World-LOS (Check 3) is skipped**: it uniquely defends only the
wallhack case, a PvP concern this engine rules out, so rather than re-anchor it we drop it. **The range
check (Check 4) is kept but re-anchored** to the snapshotted fire-time origin (not the present-tick eye),
because range measures how far the projectile travelled from its launch — anchoring it to a moved
shooter's current eye would false-reject a legitimate fire-and-retreat hit. A projectile is thus validated
by `shot_id` binding (never rejects legitimate play — a real hit always matches its own authorized fire),
target-alive, and a fire-origin-anchored range tolerance; the open-shot timeout budget is sized so a
legitimate slow projectile under bad latency is never retired early (Invariant "no false-reject of a
legitimate hit"). Hitscan keeps its cheap same-tick world-LOS and present-eye range. The one accepted,
presentation-only asymmetry: a client's *remote-observer* visual (Task 4) is drawn from the pawn's
replicated aim rather than its exact camera aim; it never affects whether or where the hit lands.

## Acceptance criteria

- [ ] A weapon authored with `resolution: "projectile"` and a launch speed fires a projectile
  that spawns at the muzzle, travels in a straight line at the authored speed, and applies its
  damage through the existing chokepoint **on the tick it reaches a target — not the fire tick**;
  a projectile that reaches its travel bound (range/lifetime) without hitting anything applies
  **no** damage and despawns.
- [ ] Single-player and listen-host: a projectile striking a `Health`-bearing enemy credits and
  scales exactly like a hitscan hit (same zone-multiplier, credit-source, contributor-ledger,
  death-sweep path); a struck world surface applies no damage and spawns the impact FX.
- [ ] Damage from a single projectile is applied **at most once**: the projectile despawns on
  first contact, and a target already at 0 HP (killed by another source mid-flight) takes no
  further application (the existing liveness gate).
- [ ] Existing hitscan and pellet-spread weapons are behaviorally unchanged — the projectile arm
  is additive to `fire_hitscan`'s `match resolution`, and non-projectile weapons never spawn a
  projectile entity.
- [ ] Connected client: firing a projectile weapon at a host-replicated enemy simulates a
  predicted projectile locally at the **rendered (interpolated)** pose, and on its local impact
  sends a `HitDeclaration` carrying the fire's `shot_id` on a **later tick** than the fire; the
  host validates it by `shot_id` binding + target-alive + range **anchored to the fire-time origin
  snapshot** (projectiles skip world-LOS; the snapshot is host-internal, off the wire — see Task 3 ·
  Direction Co-op fairness) and applies damage crediting the authorized shot's pawn. No new wire
  message or field is added, and no version constant is bumped for the authority path.
- [ ] `shot_id` binding holds for projectiles: a fire the host rejected (cooling; empty magazine
  once ammo composes) mints no authorized shot, so a later projectile declaration binds to nothing
  and applies no damage. One authorized fire accepts at most one declaration.
- [ ] A projectile that expires without hitting sends an **empty** declaration and the host
  retires the shot; a projectile whose declaration never arrives (dropped client) is retired by the
  host's existing stale-shot prune, and the open-shots store does not grow unbounded across a session.
  No legitimate hit is false-rejected: the per-shot timeout budget exceeds the maximum projectile
  travel time (`min(range ÷ speed, lifetime)`) plus a generous RTT margin, so a slow-projectile hit
  under latency survives; projectiles run no world-LOS check, and the range check is anchored to the
  snapshotted fire-time origin (not the shooter's present eye), so a hit whose shooter moved or took
  cover during flight is accepted.
- [ ] The connected client predicts muzzle-FX / cooldown / hitmarker on fire and reconciles them
  against `ShotVerdict`, rolling back on reject; enemy HP is never predicted (remote enemies carry
  no client `Health`), so there is no enemy-HP rollback.
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
- [ ] The deterministic harness exercises: an SP projectile applying damage on the impact tick
  (not the fire tick); an expiring projectile applying none; a connected-client later-tick
  declaration validated and applied host-side; a rejected fire yielding no projectile damage
  (`shot_id` binding); an undeclared shot retired by timeout; two in-flight projectiles resolving
  independently. No new `unsafe` (grep gate).

## Tasks

### Task 1: Projectile resolution core — thin slice (single-player / host)

Build the whole projectile spine end-to-end for the locally-authoritative roles (single-player
and listen-host), the narrowest path that crosses every layer. **Descriptor:** add
`ResolutionMode::Projectile` to `postretro_foundation` combat.rs and the minimal authored tuning
the sim reads — a launch **speed** (m/s, finite > 0), a collision **radius** (meters, finite ≥ 0 —
the projectile's swept-sphere half-width), a travel **lifetime** (ms; the projectile also stops at
the existing `range`), and a **visual** reference (a content-relative sprite/billboard the projectile
renders as, validated like the existing weapon model-path fields) — as descriptor-owned fields with
`validate()` bounds mirroring the existing weapon-field checks. Do **not** add FGD KVPs (tuning is
descriptor-owned). **Component:** add a new
`ComponentKind` variant **appended at the tail** (`Projectile = 21`; a mid-enum insert renumbers wire
discriminants — this is Invariant "component-kind tail-append") together with its `ComponentValue`
arm, `kind()` match, `impl Component`, the hand-maintained `VARIANTS` array (not compiler-checked),
and a new `entities/src/components/projectile.rs` `ProjectileComponent` carrying the flight and
resolution state the impact needs: direction (unit), speed, radius, remaining travel bound, `damage: f32`,
`credit_source: String`, the owner pawn and weapon `EntityId`, a **spawned-this-pass marker** (a
`spawned` flag skipped once by the advance then cleared; see the ≥1-pass-flight guarantee below), and
(reserved for Task 3) the fire's `shot_id`. **Fire branch:** in `weapon/mod.rs::fire_hitscan`, add the `ResolutionMode::Projectile`
arm; because `fire_hitscan` borrows the registry immutably it cannot spawn — it returns a launch
**intent** (origin, direction, and the resolution stats above; `ActivationOutcome::Spawned` is the
natural tag for the launched entity) that the mutable caller consumes. **Spawn + advance:** in the
weapon stage's `run_local_weapon_command` (which holds `&mut registry`), spawn the projectile via
`registry.try_spawn(Transform{position: muzzle}, &[])` + `set_component(ProjectileComponent)` **and the
projectile's visual component(s) from the descriptor visual** so it is **visible** as it travels
(without a visual it is an invisible hit, not a real slice): attach the **body** — a `SpriteVisual`
(the billboard the particle/impact-FX path uses via `spawn_impact_effect_at`) or a rigid
`MeshComponent` (the projectile model loaded through the existing glTF/resource pipeline; rigid, no
skeleton) — plus, when the descriptor declares a trail, a `BillboardEmitterComponent` (the emitter
bridge already reads a moving host's Transform each tick and emits along the path). This is a spawn-time
branch on the descriptor visual kind; the visual is presentation only and never gates or replicates the
hit. All on the fire tick. Guarantee **≥1 advance pass of flight** with the marker: the advance **skips a
projectile's first pass after spawn** (then clears the flag), so its first integration is the next pass —
next tick on the host (earliest impact N+1), next frame on the client (Task 3) — never the spawn pass. A
projectile never resolves on the fire tick (contrast same-tick hitscan; AC 1, Orderings row 1); this
tick/frame-agnostic phrasing is what lets Task 3 reuse the same advance body. The stage-0 prev==cur-on-spawn rule is a *presentation* snapshot (interpolation, no render
pop — entity_model §5 order 0); it does not defer the sim, so it is not the mechanism here. Add a **new
projectile-advance module** (`sim/projectile_stage/` or `weapon/projectile.rs`) and a stage call in
`sim/mod.rs::simulate_tick_with_presentation_aim` **after the weapon-fire stage (step 9, 1-indexed) and
before the death sweep (step 10)** — entity_model §5 numbers these orders 8 and 9 (0-indexed) — reusing
the threaded `on_impact` closure and `tick_dt`. Keep the edit
to `sim/mod.rs` to the stage call + param threading; keep the edit to `weapon/mod.rs` to the fire
arm — the advance body lives in the new module (the oversized files must not grow a block). The
advance follows the particle two-pass discipline (`particle_sim::tick`): snapshot each projectile,
integrate `cur = prev + direction * speed * tick_dt`, sweep the `prev→cur` segment for the nearest
contact **as a swept sphere of the projectile radius** — `cast_capsule` (a radius-r sphere against
the static world; `cast_capsule` already exists and wraps parry `cast_shapes`) and a **radius-aware**
`nearest_entity_hit` over the segment length, taking the nearer with the wall-wins-ties rule of
`resolve_nearest_hit`. The entity query already tests ray-vs-capsule with a per-zone `zone_radius` and
inflates its broad-phase AABB bound by that radius, so adding the projectile radius is the Minkowski
sum — extend `nearest_entity_hit` with a projectile-radius parameter that adds `r` to the zone
capsule radius at the `ray_capsule_or_ball` call and to the broad-phase bound (`expand_bound_*`
helpers); pass `r = 0` on the existing hitscan/pellet callers so their behavior is **byte-identical**
(a plumbing change, not a behavior change). A projectile-local nearest-of helper combines the swept
world cast and the radius-aware entity query rather than reusing the ray-only `resolve_nearest_hit`
verbatim. Then, **after the walk**, apply impacts and despawn (never mutate the registry mid-walk). On a contact, build a
`WeaponImpact { point, normal, target, zone, outcome: ActivationOutcome::Hit(DamagePayload{ amount:
damage }) }` and drive the impact sequence the per-impact loop uses today —
`spawn_impact_effect_at` → liveness gate → `apply_authorized_weapon_impact_damage(registry, weapon_id,
attacker, &impact, credit_source, damage)` — with **one deliberate divergence** from that loop
(`sim/weapon_stage/commands.rs`): it `continue`s (drops the impact entirely) when the **pawn** is gone,
but a projectile outlives its owner (Orderings, owner-despawn), so here **target** liveness gates the
impact while **pawn** liveness only downgrades `attacker` to `None` (`apply_authorized_weapon_impact_damage`
takes `Option<EntityId>`; credit survives via the `credit_source` string). Then despawn the
projectile so damage applies at most once (Invariant "at-most-once"); `run_death_sweep` handles a
resulting 0 HP. On reaching the travel bound (range or lifetime) with no contact, **clamp the final
segment to the remaining bound and sweep that clamped segment before declaring expiry**
(contact-wins-over-expiry — a projectile reaching a target and its bound in the same tick still hits);
only if that clamped sweep finds no contact, despawn with no damage. Factor the advance body to take
the timestep `dt` as a parameter (Task 1 passes `tick_dt`); Task 3 reuses it verbatim with `frame_dt`. AC: a projectile applies damage on the impact tick, not the fire tick; an expiring one
applies none; a projectile whose center-line would miss a target but whose radius overlaps it still
hits (width matters); the spawned projectile carries the descriptor's visual component(s) —
`SpriteVisual`, `MeshComponent`, and/or `BillboardEmitter` per the visual kind (assertable) — so it
renders while travelling; hitscan/pellet weapons are unchanged (their `r = 0` entity query is byte-identical).

### Task 2: Descriptor validation + SDK surface + reference weapon

Complete the author-facing surface for the projectile mode whose Rust shape Task 1 pins. Harden
`WeaponDescriptor::validate` for the projectile fields — speed finite and > 0, radius finite and ≥ 0
(meters), lifetime finite and > 0 — and the **visual**, a discriminated union: a **body** of kind
`sprite` (a content-relative sprite path) or `model` (a content-relative glTF model path), plus an
optional **trail** emitter config. Validate each body path with the existing
`is_portable_content_relative_asset_path` check (the weapon model-path fields' check), the trail
against the same bounds the existing `BillboardEmitterComponent` validation enforces — **mirrored into
`foundation`** (or checked at a higher layer that sees both), since that validation lives in `entities`
and `WeaponDescriptor::validate` is in `foundation`, which has no `entities` dep — and reject a
`resolution: "projectile"` weapon that omits both speed and a body. Each error is a field-named `DescriptorError::InvalidShape`
mirroring the existing weapon-field messages. Extend the TS and Luau SDK typedefs so
`resolution: "projectile"` and its tuning (speed, radius, lifetime, and the visual union) are
authorable and type-checked (the primitive-surface contract: SDK types and validation move in the
same pass as the Rust enum) — mirror the discriminated-union-per-kind pattern the existing
`WeaponResource` / UI descriptors use. Author reference projectile weapons in the dev mod covering the
modes (a **plasma bolt** — sprite body; a **rocket** — model body + trail emitter) and place them so
the dev map can fire them. Update
the weapon-authoring reference docs to cover the mode. This is content + boundary typedefs only; it
adds no runtime behavior beyond Task 1's. AC: an out-of-range or speed-less projectile descriptor is
rejected with a field-named error; the TS and Luau typedefs expose the mode and fields; the dev mod
carries a working reference projectile weapon.

### Task 3: Co-op authority — client-predicted projectile + later-tick declaration + open-shot lifetime

Make the projectile co-op-correct for a **connected client** by reusing the shipped
client-authoritative-HIT path, with the only new mechanisms being deferred declaration timing, a
host-internal fire-time snapshot on `AuthorizedShot`, and open-shot lifetime. On the client, a projectile weapon's fire predicts and spawns a **local**
projectile the same way Task 1 does, but advances it **post-loop, once per frame** at the rendered
(interpolated) pose — mirroring the shipped client fire path's placement rationale (it must read the
interpolated Transforms `sample_into_registry` wrote and the render-stage anim clock), never inside
the movement predict loop, and reusing Task 1's factored advance body with `dt = frame_dt` (its radius
sweep, wall-wins-ties, and clamp-then-contact-before-expiry rule, unchanged). The host's fixed-tick
advance stage lives in `simulate_tick_with_presentation_aim`, which a connected client never runs
(`sim/mod.rs` — "a connected client never runs the host simulation seam"), so the client's predicted
projectile is advanced by this post-loop path alone; there is no double-advance to guard against. On
the client projectile's local impact, emit the **existing**
`HitDeclaration { shot_id, records }` carrying the **fire's** `shot_id` (computed at fire as today:
pawn `NetworkId` high, `client_tick` low) — now legitimately sent on a **later tick** than the fire;
the record is the struck target's `NetworkId` + point + zone via the existing client reverse map. No
new message and no new field: the shipped `HitDeclaration`/`ShotVerdict`/`shot_id` contract already
admits a later, 0..N-record declaration, so **no version constant bumps** (Invariant "no wire
change"). Predict muzzle-FX / cooldown / hitmarker through the existing `ClientPredictedShots` keyed
by that `shot_id`, and reconcile against `ShotVerdict` — rolling back FX/cooldown/hitmarker on
reject; enemy HP is never predicted (remote enemies carry no client `Health`). On the **host**, the
FIRE path records the authorized shot in the existing open-shots store (`OpenAuthorizedShots`), which
already holds a shot **open across ticks** until its declaration binds and retires it — hitscan
declarations already arrive ticks later over RTT, so no new "keep it open" mechanism is needed. What
IS net-new is a **fire-time authority snapshot** on `AuthorizedShot` (host-internal, never serialized):
an `is_projectile` flag, the muzzle **origin** (`Vec3`), and a **per-shot timeout budget**. The origin
is the firing pawn's fire-tick eye, computed host-side at authorization in `run_remote_weapon_commands`
from the pawn's registry `Transform` + `PlayerMovementComponent.capsule.eye_height` (the construction
`attacker_eye` uses) — **not** `WeaponFireCommand.aim_origin`, which that path hardcodes to `Vec3::ZERO`
("the host casts no local aim ray"); the pawn is live at fire and the movement stage precedes the weapon
stage, so the eye is available. The store
is already pruned by `OpenAuthorizedShots::prune_stale` (`netcode/mod.rs`), which today retires any shot
older than `MAX_OPEN_SHOT_AGE_TICKS` (180 ticks = 3.0 s at 60 Hz) — below a legitimately-authorable slow
projectile's flight + latency (the reference weapon authors a 4 s lifetime). So **re-scope the prune to a
per-shot budget** rather than adding a second timer: for a projectile, budget =
`max(MAX_OPEN_SHOT_AGE_TICKS, ceil(min(range ÷ speed, lifetime_ms ÷ 1000) ÷ tick_dt_s) + RTT_margin_ticks)`
(units explicit: `range ÷ speed` and `lifetime_ms ÷ 1000` are seconds, `÷ tick_dt_s` converts to ticks),
keeping 180 as the hitscan floor, so a legitimate slow-projectile hit under latency is never retired early
(Invariant "no false-reject on late hit"). On the client, send an **empty** declaration when a projectile
expires without hitting (the existing surplus-tick empty-declaration pattern), so the host retires the shot
promptly rather than waiting for the budget — a *distinct* mechanism from the dropped-client prune, not a
reuse of it. **Validation branches on the snapshotted `is_projectile` flag** (never re-resolve the weapon
at ingest: `WeaponOwners` is a presentation attachment-dirty queue, not a pawn→weapon map, and
`AuthorizedShot.weapon` could be swapped or despawned across a multi-second flight — the flag is frozen at
fire). A **projectile** declaration is validated by Check 1 (`shot_id` binding — the security spine),
Check 2 (target alive via `NetworkId`), and Check 4 (range) **only**, with two changes from the hitscan
path: **Check 3 (world-LOS) is skipped** — a present-tick static-world ray from the moved shooter defends
only the wallhack case (a PvP concern this engine rules out) and would false-reject a fire-and-take-cover
hit; and **Check 4 (range) is re-anchored to the snapshotted fire origin** instead of `attacker_eye` at
ingest (`apply_valid_hit_record` today measures `eye.distance(point)` from the present-tick eye — for a
deferred projectile the shooter has moved, so a present-eye range would false-reject a legitimate
fire-and-retreat hit; the snapshotted origin is where the shot's `range` is measured from). **Hitscan and
pellet weapons keep the shipped live-eye Check 3 and present-eye Check 4 unchanged** (their declarations
are same-tick, so the present eye *is* the fire eye). Damage application and the credit path are the
shipped ones. If the firing client's pawn is torn down mid-flight, the host's pawn teardown already
removes its open shots (`open_shots.remove_pawn`), so the shot is **retired** and a late declaration
binds to nothing — a retirement, not a false-reject (the pawn is gone); whether an in-game death
despawns the pawn at all is the death/respawn model's concern, so this case may only arise on
disconnect. AC: a connected client's projectile hit is
declared on a later tick and applied host-side regardless of the shooter's movement or intervening
geometry during flight (no world-LOS to false-reject it); a rejected fire yields no projectile damage
(`shot_id` binding); an undeclared shot is retired by timeout; the open-shots store does not grow
unbounded; no wire constant bumps.

### Task 4: Remote-observer replicated projectile visuals (separable)

Give remote peers a view of a projectile in flight, as presentation only — the separable task and
the cut line for a leaner v1. On every projectile fire, the **host** spawns a presentation-only
projectile entity that travels the deterministic straight-line path from the pawn muzzle along the
pawn's aim (the listen-host's own camera aim for its pawn; a connected client's pawn aim
reconstructed from the already-replicated `facing_yaw` + `aim_pitch` — verify both are available at
the weapon stage, which runs after the host camera stage) at the descriptor speed, and despawns it
when the shot retires or its own path reaches the travel bound. "Shot retires" has a referent only for
a **connected client's** fire (which mints an `AuthorizedShot`); the **listen-host's own** pawn fire is
locally authoritative and mints no open shot (`authorized_shots` comes only from
`run_remote_weapon_commands`), so for host-own fires tie the presentation entity's despawn to the Task 1
gameplay projectile's despawn (and suppress the presentation copy in the host's own render, since the host
already sees its gameplay projectile) — otherwise the presentation copy would outlive the gameplay impact
and the host would draw both. Classify this entity for replication: add a projectile arm to
`netcode/descriptor_class.rs::descriptor_entity_class` returning its descriptor `canonical_name` (the
host-spawned presentation entity must carry `DescriptorProvenance` for the weapon descriptor, since
`descriptor_entity_class` gates on it, so the entity enters the replicable set), and a client materializer arm in
`netcode/remote_materialize.rs` that attaches only presentation — the **same visual component(s) Task 1
attaches locally** (the `SpriteVisual`/`MeshComponent` body plus any `BillboardEmitter` trail), built
from the weapon descriptor's projectile-visual union and resolved client-side from the replicated
`entity_class` (descriptors are shared content) — no `Health`, no gameplay component — so the entity
rides the **existing** Transform + `entity_class` snapshot record
(valid on any Transform-bearing record since the client-auth spec) and interpolates like any
replicated entity, adding **no wire layout change** (Invariant "no wire change"; verify the classname
string is the only new value). A connected client spawns **no** local authoritative projectile for a
**remote** peer's fire — respect `SpawnContext.can_materialize_runtime_spawns` (false for a connected
client), so remote projectiles arrive solely as host snapshots (networking.md spawn-suppression). The
firing connected client already renders its Task 3 predicted projectile, so the host must **not** also
show that client the replicated copy of its own shot. The movement path's "the local pawn is the host's
pawn" suppression does **not** transfer: the presentation projectile rides a Transform + `entity_class`
record that carries **no owner or `shot_id`** on the wire (and the no-wire-change invariant forbids adding
one), so the client cannot identify which replicated projectile is its own. Instead suppress it
**host-side, per client**: the host knows the firing client from `OpenAuthorizedShot.owner_client_id`, and
snapshots are already encoded per client (`host.rs` `encode_in_batch(client_id, …)`), so the host omits a
client's own presentation projectile from *that* client's snapshot while still sending it to every other
peer. Task 4 adds a **per-client entity-exclusion set** to the replication tracker for this — host-internal
(omitting a record from one client's snapshot is not a wire change). Today per-recipient variation exists
only as movement-authority *metadata* (e.g. `local_player`), not entity exclusion, so this is net-new
tracker state, not a reuse of an existing per-pawn suppression. Coordinate with Task 3 so the firer sees its predicted
projectile and no doubled copy. Accept that the host visual (pawn replicated aim) and
the firer's predicted projectile (full camera aim) may diverge slightly; the visual is presentation
for remote observers only. AC: a peer's projectile is visible in flight to other peers and despawns
on shot retire; a connected client spawns no authoritative projectile for a remote fire; the firing
client shows its predicted projectile, not a doubled copy; no wire layout change.

### Task 5: Tests

Extend the weapon-stage and netcode `predict_reconcile_harness` scaffolding to cover the projectile
behaviors deterministically. Single-player / host: a projectile spawned on the fire tick applies
damage on a **later** tick (assert the damage lands on the impact tick, not the fire tick, by
stepping ticks and observing HP), scaled/credited like a hitscan hit through the shared chokepoint;
a projectile that reaches its travel bound applies no damage and despawns; two in-flight projectiles
from rapid fire resolve independently; a projectile whose target died mid-flight applies no second
hit; a projectile whose center-line misses a target but whose radius overlaps it registers a hit,
and an existing hitscan/pellet weapon's entity query is byte-identical to today (`r = 0` regression);
a spawned projectile carries the descriptor-specified visual components (sprite- vs. model-body, and a
trail `BillboardEmitter` when declared). Co-op: a connected client's projectile declaration arriving on a later tick is validated and
applied host-side crediting the authorized pawn; a fire the host rejected yields no projectile damage
even with a plausible later declaration (the `shot_id`-binding security test); an undeclared open
shot is retired by the host timeout and the store stays bounded; an expiry-without-hit empty
declaration retires the shot; a legitimate declaration whose shooter moved behind cover during flight
is accepted (projectiles run no world-LOS check); a hitscan/pellet weapon still runs its live-eye
world-LOS unchanged (the branch is projectile-only). If Task 4 landed: a replicated projectile visual
materializes on a remote and despawns on shot retire, and a connected client spawns none locally for a
remote fire. Assert no new `unsafe` (grep gate) and, for the authority path, that no version constant changed. AC:
the harness exercises each listed behavior deterministically.

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
| Launch speed | `WeaponDescriptor` speed field (units/s, >0) | descriptor JSON camelCase | typedef field | typedef field | n/a |
| Collision radius | `WeaponDescriptor` radius field (meters, ≥0) → swept-sphere half-width; added to `zone_radius` at the entity query (Minkowski) | descriptor JSON camelCase | typedef field | typedef field | n/a |
| Projectile visual | `WeaponDescriptor` visual — discriminated union: body `sprite`→`SpriteVisual` \| `model`→`MeshComponent`, + optional `trail`→`BillboardEmitterComponent` | descriptor JSON, `kind`-tagged (camelCase) | typedef union per kind | typedef union per kind | n/a |
| Travel lifetime | `WeaponDescriptor` lifetime field (ms, >0); `range` bounds distance | descriptor JSON camelCase | typedef field | typedef field | n/a |
| Projectile component | `ComponentKind::Projectile = 21` (tail), `ComponentValue::Projectile`, `ProjectileComponent` | not replicated for the gameplay projectile; Task 4's **presentation** projectile rides the existing Transform + `entity_class` record | n/a | n/a | n/a |
| Fire-time authority snapshot | `AuthorizedShot` gains `is_projectile`, `origin: Vec3`, per-shot timeout budget (host-internal) | **not serialized** — `AuthorizedShot` is a host-side store, off the wire | n/a | n/a | n/a |
| Hit declaration | **reused** `HitDeclaration { shot_id, Vec<HitRecord> }` | **unchanged** — no new message/field; later-tick arrival already legal | n/a | n/a | n/a |

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| **At-most-once damage** per projectile | Task 1 (despawn on first contact; liveness gate) | double-resolve if the projectile isn't despawned on contact; a re-declared shot (Task 3 retires on accept) | AC 3; Task 5 target-died-mid-flight + at-most-once |
| **Shot always retires** (declaration, empty declaration, or prune); open-shots store bounded | Task 3 (re-sized `prune_stale` per-shot budget + client empty declaration — two distinct mechanisms) | a projectile that never declares; a client that drops mid-flight | AC 7; Task 5 timeout + empty-declaration |
| **No false-reject of a legitimate hit** — projectiles skip world-LOS (Check 3), the range check (Check 4) is anchored to the snapshotted fire origin (not the present eye), and the per-shot budget ≥ max travel time + RTT margin | Task 3 (skip Check 3; re-anchor Check 4 to the fire-time origin snapshot; per-shot budget sizing) | budget shorter than a slow projectile's flight + latency; a present-eye range or LOS check false-rejecting a moved shooter | AC 7; Task 5 slow-projectile-under-latency + take-cover-still-hits |
| **Enemy HP never client-predicted** | Task 3 (reuses client-auth structure) | any client write to enemy `Health` (structurally absent — remotes carry no `Health`) | AC 8 (structural gate) |
| **No wire-format change on the authority path** (the fire-time snapshot is host-internal, never serialized) | Task 3 (reuse `HitDeclaration`/`ShotVerdict`; snapshot off the wire), Task 4 (reuse `entity_class` record) | a new message/field, a version-constant bump, or serializing the snapshot | AC 5, AC (Task 4); Task 5 no-constant-changed assertion |
| **`ComponentKind::Projectile` appended at the enum tail** | Task 1 | a mid-enum insert renumbering later wire discriminants | AC (implicit); drift-guard tests on the discriminant order |
| **Hitscan/pellet entity query byte-identical** (the shared `nearest_entity_hit` radius param defaults `r = 0`) | Task 1 (radius param, `r = 0` on existing callers) | the shared query gaining radius behavior; a caller omitting `r = 0` | AC 4; Task 5 `r = 0` regression |

## Orderings

| Scenario | Ordering | Expected outcome |
|---|---|---|
| Projectile fired tick N, reaches target tick N+k (k≥1) | spawn precedes impact by ≥1 tick | Damage applies at N+k, not N (contrast hitscan same-tick). |
| Projectile reaches range/lifetime before any contact | expiry precedes any impact | No damage; despawn; client sends empty declaration; host retires the shot. |
| Shooter moves (strafes behind cover) between fire tick N and declaration tick N+k | movement interleaves flight | Accepted — Check 3 (world-LOS) is skipped and Check 4 (range) is anchored to the snapshotted fire origin, not the shooter's present eye, so movement/cover during flight cannot false-reject a legitimate hit (validation is `shot_id` + target-alive + fire-origin range). |
| Two projectiles in flight from rapid fire | independent entities, independent `shot_id`s | Each resolves on its own; no cross-talk. |
| Target dies (other source) while projectile in flight | death precedes impact | On impact the liveness gate no-ops; no second application. |
| Client declares on impact, host already retired the shot (timeout fired first) | timeout precedes declaration | Declaration rejected; client rolls back FX/hitmarker; enemy HP untouched (never predicted). Timeout sizing (Invariant) makes this reachable only for a genuinely dropped/oversized case. |
| Owner pawn despawns while its projectile is in flight | despawn precedes impact | Projectile resolves its impact (it carries its own damage + string credit-source): **target** liveness gates the hit, **pawn** liveness only downgrades `attacker` to `None` (unlike the local per-impact loop, which skips the impact when the pawn is gone). Credit survives via the `credit_source` string. (This is the local Task 1 projectile — SP/host; on the connected-client declaration path the shot is instead **retired** on pawn teardown via `open_shots.remove_pawn` — Task 3.) (Open question if attacker-entity credit is required.) |
| Level unload / registry teardown with projectiles in flight | teardown clears all entities | In-flight projectiles cleared with the registry; no dangling impact next level. |

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
(Exact field names/nesting — flat on `weapon` vs. a `projectile` sub-block — are Task 1/2's to pin;
the constraint is that they are authored tuning on the weapon descriptor.)

## Open questions

- **Field grouping.** Whether the projectile tuning is flat on `WeaponDescriptor` (like
  `pellet_count`/`spread_degrees`) or a `projectile` sub-block (like the `resource` tagged union).
  Lean flat for the scalars (speed/radius/lifetime); the visual is a nested union regardless. Decide in Task 1.
- **Visual union shape (decided; alternative noted).** The projectile visual is a **body**
  (`sprite` | `model`) **plus an optional `trail` emitter**, so the canonical rocket (model + smoke
  trail) is expressible, not just body-xor-particles. The simpler alternative — a flat one-of-three
  union (`sprite | model | emitter`) — is cheaper but can't do model + trail together. Body+trail is
  the recommendation; flip to the flat union if the extra field isn't wanted.
- **Attacker credit after owner despawn.** A projectile outliving its owner pawn resolves with a
  stale `attacker: EntityId`. The source-id ledger is string-keyed (credit survives), but
  `DamageContext.attacker` wants a live id. v1 default: apply with the attacker liveness-checked
  (None if despawned), credit by `credit_source` string. Confirm this satisfies the impact-policy /
  scoring consumers, or pin an owner-snapshot if not.
- **Task 4 in or out of v1.** Remote-observer visuals are the separable cut line. Recommended in
  (a co-op projectile invisible to teammates is a visible defect, and the host already has the
  replicated aim), but a leaner v1 can ship Tasks 1–3 + 5 and defer Task 4 as a fast-follow.
- **`ActivationOutcome::Spawned` semantics (owner-confirm).** This spec is the first constructor of
  the dead `Spawned(EntityId)` variant and binds it to a *transient, self-resolving* projectile.
  `weapon-model.md` §4/§10 also reserves `Spawned` for the *persistent tracked-entity / detonator*
  pattern (an instance-owned live set a later secondary activation resolves). Confirm that setting the
  transient precedent here does not force the later detonator spec to retrofit a tracked-vs-transient
  distinction onto the variant — if it might, a one-line doc note on `Spawned` now (transient vs.
  tracked is the consumer's concern, not the variant's) forecloses the retrofit cheaply.
