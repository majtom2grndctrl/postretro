# Enemy Line-of-Sight + Cover — Research Notes

Grounding and derivation for `index.md`. Not an execution contract. Line numbers
are from `main` at the Limitator merge (14b1587) and drift on edit — cite
identifiers, re-open before trusting a number.

## Roadmap placement and dependency direction

- Open item: `context/plans/roadmap.md:66` — **Enemy line-of-sight + cover**
  (Epic 10). "exact eye-to-target occlusion via BVH ray queries against world
  geometry… Enemies break sight behind pillars, use geometry as cover,
  distinguish a clear head shot from a blocked one."
- Sequencing (`roadmap.md:69`): "Line-of-sight + cover follows combat
  positioning — it **upgrades that destination scoring from reachability to
  true visibility**." This spec is that upgrade.
- Ownership split recorded by commit `90e7505`: the enemy ranged/hitscan
  **weapon** feature (weapon-referencing attack kind, nearest-of ray, player
  hitbox, shooter self-exclusion, co-op hit authority, resolved-stat home) is a
  **future Epic 16 › Resolution Modes spec** that **depends on** this one
  ("an instant-damage standoff shot with no LOS gate fires through walls and
  floors"). That spec is out of scope here; this spec is its prerequisite.
- The two AI prerequisites commit `90e7505` filed under the Epic 16 bullet —
  standoff-band positioning and committed-aim facing — are pulled **into** this
  spec (owner decision), because both live in `combat_positioning` / the AI
  facing floor that this spec already edits.

## The Limitator today (the first consumer)

`content/dev/scripts/limitator.ts` (214 lines), registered in
`content/dev/start-script.ts`, placed in `combat-demo.map` and
`movement-feel.map`. Content-only: the entire Limitator feature added no
engine combat/AI Rust (only `crates/model/examples/socket_dump.rs`, an offline
tool).

- Components: `health` (max 100; **hitbox** `halfExtents [0.27, 0.91, 0.27]`,
  `offset [0, 0.91, 0]`; `zoneMultipliers { head: 2.5, leg: 0.5 }`), `mesh`
  (`models/limitator/model.gltf`; AR_4 rifle parented to the `hand_r` socket via
  the generic `attachments` map — cosmetic geometry, **not** a `WeaponDescriptor`
  reference; there is no weapon-mount system in tree), `behavior`.
- Ranged attack is `attacks.shoot` = an ordinary `AttackParams`
  (`damage 10, maxRange BREAK_RANGE+3, cooldownMs 750`), fired via
  `ActionVerb::Attack("shoot")`. Mechanically identical to melee: the AI tick
  applies **direct damage** to the selected target via
  `apply_damage_with_context(target.entity, …)` (`ai/mod.rs` attack decision
  ~683-729, damage apply ~916-935) with `weapon: None`. No ray, no projectile.
  The `// Hitscan-style` comment is aspirational.
- Offense nested graph (`limitator.ts` ~160-185): `close → aim → fire`.
  Constants: `FIRE_RANGE 8`, `BREAK_RANGE 9`, `ENGAGEMENT_RADIUS 6`,
  `LEASH_RANGE 70`, `AIM_MS 550`, `FIRE_MS 250`. `aim` uses `idle_aiming`;
  `fire` edge-fires `shoot`.
- Missing (this spec's target): no LOS anywhere; fires through walls whenever
  in range + off cooldown; no cover; holds naively at range; aim freezes facing.

## Existing seams this spec builds on

- **AI tick**: `run_ai_tick_with_navigation_and_impact`
  (`crates/postretro/src/scripting/systems/ai/mod.rs`), called from
  `crates/postretro/src/sim/mod.rs` (~600) with `nav_graph` and
  `Some(collision_world)`. Passes: snapshot → compute (per-enemy, target
  selection + transitions + motion + attack decision) → apply (writes, damage,
  animation). **`collision_world` is already a parameter** but is currently
  forwarded only to `resolve_combat_slots`; it is in scope in the function and
  can be used in the compute loop with no signature change.
- **`select_target`** (`scripting/systems/ai/targeting.rs`): carries
  `visible: Option<&dyn Fn(EntityId) -> bool>`, passed **`None`** at both call
  sites — the retained-target rescan and the fresh-acquisition scan. The seam
  doc comment names this spec. Threaded down into `nearest_target_candidate` /
  `target_candidate` (short-circuits on `visible.is_some_and(|f| !f(entity))`).
  **Footgun** (`entity_model.md` §7c): the raw nearest-hostile "offer" feeds
  think-stride pricing; filtering the *offered/stride* distance inverts stride
  cost (far enemies scan most). The predicate must gate acquisition candidacy
  only, never the stride offer. Retention/stand-down is graph policy — so
  supply `visible` to the **fresh-acquisition** call only, not the retained
  rescan.
- **LOS ray precedent**: `has_static_world_los(collision_world, eye, point) -> bool`
  at `crates/postretro/src/netcode/mod.rs` (~2013) — casts `collision::cast_ray`
  over the exact segment, rejects a hit before `distance - 1e-4`. `attacker_eye`
  (~2029) = `transform.position + Vec3::new(0, movement.capsule.eye_height, 0)`
  (**player** capsule). This is the primitive to generalize and re-home into
  `collision/`; netcode is an accidental home.
- **Ray casts**: `cast_ray` (static world, `collision/mod.rs` ~310);
  `cast_ray_combined` (static + movers, `collision/moving.rs` ~199) needs
  `movers: &[MoverCollider]` + a `MoverPoseSource` the AI tick does **not**
  receive. Static-world LOS is reachable with the existing param; mover-aware
  LOS needs extra plumbing → deferred (see below).
- **Enemy eye — genuine gap**: enemies carry `Transform + Agent + Brain`, no
  eye-height field (players use `movement.capsule.eye_height`). Nothing authored.
  Derivation reuses the authored **health hitbox** when present (top-center:
  `position + offset + (0, half_extents.y, 0)`), else falls back to
  `NavGraph::agent_params()` map-global `height` (`nav/mod.rs` ~194). The
  Limitator hitbox gives eye ≈ 1.82 m — a sane head-height ray origin, already
  authored for hit volumes.
- **Combat positioning**: `crates/postretro/src/combat_positioning.rs` (971
  lines). Ring-samples 8 dirs × 3 radii around the target; keeps slots that are
  nav-reachable (`find_path`) and statically occupiable
  (`capsule_static_placement_center`). Score (~277-278):
  `attack_band_error = |target_distance - engagement_radius|`,
  `score = attack_band_error + path_cost * weight`. **No visibility term.**
  Already receives `collision_world` (via `resolve_combat_slots`), so the LOS
  query is reachable here without new plumbing. The standoff defect
  (commit `90e7505` note): scoring to `engagement_radius` seats the enemy *at*
  that ring; a fire guard of `targetDistance ≤ FIRE_RANGE` with the ring outside
  it never crosses in. Fix: a standoff distance/band strictly inside the fire
  threshold, distinct from the ring radius.
- **Brain facts**: refreshed each tick (`ai/mod.rs` ~603/626); guards are IR
  over a brain vocabulary (`targetDistance`, `hasTarget`, `targetHostile`,
  `timeInActivityMs`, `distanceFromAnchor`, …). A new boolean fact
  `targetVisible` mirrors the existing boolean `hasTarget` end-to-end — this is
  the plumbing warrant for the fact task (mirror `hasTarget`'s Rust
  field → IR node → SDK TS/Luau surface → validation).
- **Engine floor vs authored graph** (`E10--behavior-state-graph`): target
  selection, retention/hysteresis, think-stride, aggro gate, combat slots,
  facing, and the damage chokepoint are unauthorable floor. LOS integrates as
  both: an engine-floor **fire-gate** (damage never applies without LOS) and an
  authorable **`targetVisible` fact** (content authors LOS-gated transitions).

## File-size flags (split-before-extend)

- `ai/mod.rs` 981 lines — near threshold; this spec adds fire-gate, facing, and
  candidate wiring. Mitigation: put new perception logic (eye, LOS predicate,
  fire-gate helper, fact computation) in a **new `ai/perception.rs` module**;
  keep `mod.rs` edits to call-site wiring. Avoids a split.
- `combat_positioning.rs` 971 lines — near threshold; visibility term + standoff
  band extend it. If the additions push it past ~1050, extract slot scoring into
  a submodule first (behavior-preserving) before adding the visibility term.

## Decisions pinned (and why)

- **Static-world LOS in v1; mover-aware (doors as sight-blockers) deferred.**
  `roadmap.md:66` scopes to "world geometry"; the primary case (pillars/walls)
  is static. Monster-closet reveals are trigger/spawn-driven, not LOS-driven, so
  enemies seeing through a *closed* door is not the signature-gameplay hole it
  first appears. `cast_ray_combined` needs mover-collider + pose plumbing into
  the AI tick that static LOS does not. Deferred to a set-piece that needs live
  movers to block enemy sight. Does **not** overlap E17-F (portal/rendering
  visibility) — this is a collision-ray mechanism.
- **Eye = eye→target, not muzzle→target, in v1.** The LOS gate is a tactical
  *decision* (should I fire / where can I see), for which head-height eye is the
  right granularity. Muzzle-socket-origin resolution is an Epic 16 fidelity
  concern (when the shot is a real ray).
- **Cover = enemy→target LOS scoring, not protective (target→enemy) exposure.**
  `roadmap.md:69` sanctions "reachability → true visibility" on the destination
  scorer. Offensive LOS (a slot you can shoot from) + the fire-gate produce the
  player-visible "breaks sight behind pillars / uses geometry" behavior.
  Protective cover scored by the player's *fire* exposure pairs with the player's
  shot becoming a ray (Epic 16) — deferred there.
- **No new `MotionVerb`.** Cover-seeking rides the existing combat-slot
  destination path (`SteeringIntent::Chase → set_destination(combat_slot)`): make
  the slot LOS-aware and the shipped steering carries it. Avoids a
  primitive-surface steering change. (A dedicated strafe/relative-velocity
  steering primitive stays unbuilt; steering is destination-based.)

## Review corrections (review-draft-spec panel)

Findings that reshaped tasks — recorded so the notes match the spec.

- **`visible` param reprices the stride (2 reviewers).** `select_target`'s
  `visible: Option<&dyn Fn>` is evaluated in `target_candidate`, upstream of
  **both** the `nearest`/`nearest_offered` (stride offer) and `eligible`
  accumulators in `nearest_target_candidate`. Using it for LOS would filter the
  stride price — inverting cadence per §7c. The authored `candidate_filter` is
  the mechanism proven separate (applied to `eligible` only). Task 5 therefore
  gates on the eligibility path, parallel to `candidate_filter`, never via
  `visible`, and computes `nearest_offered` over all hostiles. It also gates
  **both** scan branches — the retained-due rescan can switch to a closer new
  pawn (a fresh acquisition), so gating only the no-retained branch leaks it.
- **`hasTarget` is derived, not a stored field.** `@brain.hasTarget` =
  `IrValue::Bool(facts.target.is_some())` in the fixed-value match; there is no
  `BrainFacts` field to copy. The real stored-boolean precedents are
  `target_hostile` / `target_reachable` (`ai/brain_scope.rs`). `targetVisible`
  (an LOS verdict, not derivable from `target`) mirrors those for the field +
  match arm, and `hasTarget` only for the `BRAIN_INPUTS` const
  (`crates/foundation/src/brain.rs`) + SDK surfaces. `BrainFacts.target` carries
  only `(EntityId, distance)` today — thread the target **position** in so the
  fact and fire-gate ray to the same point.
- **Facing already slews for engaged states.** The `ai/mod.rs` facing match
  slews an engaged stopped enemy toward its target (`slew_yaw`,
  `FACING_TURN_RATE`). The `fire` beat carries an action so it is engaged and
  already faces; the `aim` beat (`motion:"hold"` → `SteeringIntent::Clear`, no
  action) is not engaged and freezes. Task 4's real work is the non-engaged aim
  beat, plus reading the **post-slew** heading in the attack decision (facing
  slew runs in apply, one pass after the compute-pass attack decision).
- **Edge-fire vs latch (owner decision → latch).** The attack edge-fires only on
  the state-entry tick (`take_entry_pending`, cleared on read), so a gate closed
  that tick drops the shot for the whole dwell. Owner chose a
  fire-once-per-dwell-on-first-gate-open **latch**: re-check LOS + facing each
  dwell tick, fire on the first clear one.
- **LOS debounce (owner decision → debounce).** LOS verdict holds `true` for a
  named `los_grace_ticks` after sight is lost (immediate on gain), so an engaged
  enemy lands a few shots as the player dives into cover — deliberate
  imperfection. Applies to the engaged fire-gate + `targetVisible`; fresh
  acquisition uses raw LOS.
- **Standoff (owner decision → first-class field, defer vocabulary overhaul).**
  Add a **per-attack** `standoffDistance`, a sibling of `AttackParams.engagement_radius`,
  defaulting to the attack's `engagement_radius_for_action` (so melee positioning
  — incl. the reference enemy's per-attack `slam` `engagementRadius` — is
  unchanged). Fed as the single `query.engagement_radius` value, which already
  drives both `generated_positions` and `score_candidate`, so one plumb covers
  ring and scorer. The Limitator already dodges the defect via `ENGAGEMENT_RADIUS
  6 < FIRE_RANGE 8`; the field makes standoff first-class without the author
  reverse-engineering the scorer. Rationalizing the whole ranged-distance
  vocabulary is deferred to Epic 16 combat stances (its real consumer).
- **Combat-positioning LOS endpoints (implementability).** `combat_positioning`
  is registry-decoupled (callers pass positions; `target_pos` is ground
  `Transform.position`) and has no access to the enemy hitbox. Slot LOS must
  thread the enemy eye offset + target eye into `CombatQuery` / `resolve_combat_slots`
  so it reuses Task 1's single eye/aim derivation — no divergent endpoints
  (Invariant 2). No AC runtime-tests the endpoints, so this is a review/grep
  guard, not a runnable one.
- **Slot LOS ≠ fire-gate LOS.** Slot LOS (slot→target, resolve time) is a
  positioning heuristic and must live in `score_candidate` so a held incumbent
  re-validates every tick; the fire-gate (enemy-eye→target, next tick, actual
  position) is the authority. Neither substitutes for the other.
