# Enemy Line-of-Sight + Cover

## Goal

Give enemies true per-frame line-of-sight so they stop acting on targets they
cannot see and use world geometry as cover. Enemies fire only on a clear
sightline, reposition to where they can actually shoot, and seat at a proper
standoff distance. Upgrades combat positioning from reachability to visibility
(`roadmap.md:66`, `:69`). First consumer: the shipped Limitator ranged enemy,
which today fires through walls and holds naively at range.

## Scope

### In scope

- A reusable static-world line-of-sight query, homed in the collision module.
- An engine-derived enemy eye point (ray origin), from the authored health
  hitbox where present, else the nav agent height.
- A single per-tick enemy-to-target LOS verdict, loss-grace **debounced**, shared
  by the fire-gate and the `targetVisible` fact.
- An engine-floor **fire-gate**: enemy attack damage never applies without a
  clear (debounced) eye-to-target sightline.
- An authorable **`targetVisible`** brain fact so content authors LOS-gated
  transitions.
- **Visibility-aware combat positioning**: slot scoring prefers slots with a
  clear enemy-to-target sightline (the reachability → visibility upgrade).
- **Standoff distance**: a first-class authored positioning target, so a ranged
  enemy seats strictly inside its fire threshold (AI prerequisite, `90e7505`).
- **Committed-aim facing** with a **latched fire**: the aim phase slews toward
  the target every tick, and the shot fires on the first tick within the
  fire-dwell that facing (post-slew) and LOS are both clear (AI prerequisite,
  `90e7505`).
- **LOS-as-candidacy** on fresh target acquisition (raw, undebounced), applied to
  the eligibility path only so think-stride pricing and retention are preserved.
- Limitator content update + a cover-bearing demo map that exercises the stack.

### Out of scope

- The enemy ranged/hitscan **weapon** feature — weapon-referencing attack kind,
  nearest-of ray resolution, the player hitbox, shooter self-exclusion, co-op
  hit authority, resolved-stat home. Owned by the future Epic 16 › Resolution
  Modes spec, which depends on this one (`enemy-ranged-attacks.md` Ownership;
  `roadmap.md` Epic 16 bullet). Enemy damage stays direct-in-tick here.
- **Overhaul of the ranged-combat distance vocabulary** (`max_range`,
  `engagement_radius`, fire/break thresholds). This spec adds one field
  (`standoffDistance`); rationalizing the whole set belongs with Epic 16 combat
  stances (`enemy-attack-modes.md`), where combat styles are defined.
- **Mover-aware LOS** (closed doors as sight-blockers). v1 casts against static
  world geometry only; `roadmap.md:66` scopes to "world geometry," and
  mover-aware casting needs collider/pose plumbing into the AI tick. Deferred.
- **Muzzle-origin** shot resolution. v1 gates on eye-to-target; muzzle-socket
  origin is Epic 16 fidelity.
- **Protective cover** scored by the player's fire exposure (target-to-enemy
  sightline). Pairs with the player's shot becoming a ray (Epic 16).
- A new steering **`MotionVerb`** (strafe / relative-velocity kiting). Cover-
  seeking rides the existing combat-slot destination path.
- **Per-archetype LOS-grace / eye-offset authoring.** v1 uses engine-constant
  defaults; making the grace window or eye offset per-archetype fields is an
  additive follow-on.
- Search / last-known-position behavior on sight loss — a scope choice, not a
  dependency wait: hierarchical statecharts have shipped
  (`plans/done/E10--hierarchical-behavior-statecharts`), so authoring a
  search/last-known state on that mechanism is left to a later behavior pass.

## Direction

**Problem.** Enemies act on targets they cannot see. The Limitator applies
damage whenever a target is within range and off cooldown, with no visibility
term anywhere in the AI tick or in combat positioning — so it shoots through
walls and floors, and combat positioning parks it at reachable-but-blind slots.
The cause is a missing per-frame sightline test, not a tuning problem: the
`select_target` `visible` seam ships wired to `None`, and `combat_positioning`
scores by nav path length only.

**Prior commitments.**
- `roadmap.md:66` scopes this as exact eye-to-target occlusion against world
  geometry; `:69` frames it as upgrading combat positioning's scoring from
  reachability to true visibility. This spec follows both.
- The engine-floor / authored-graph split (`E10--behavior-state-graph`): target
  selection, hysteresis, think-stride, the aggro gate, combat slots, facing, and
  the damage chokepoint are unauthorable floor. LOS lands as an engine-floor
  fire-gate **and** an authorable `targetVisible` fact — consistent with the
  split, not a divergence.
- Commit `90e7505` records that the enemy ranged **weapon** feature is a later
  Epic 16 spec depending on this one; this spec honors that boundary and does
  not bridge `AttackParams` to `WeaponDescriptor`. It pulls in only the two AI
  prerequisites that commit filed, because they live in code this spec edits.
- `select_target`'s `visible` predicate is named the plug point (`roadmap.md:57`),
  but it filters the think-stride offer as well as candidacy — so this spec
  applies the LOS gate on the **eligibility path** (parallel to the
  authored `candidate_filter`), not through the `visible` param. Divergence from
  the roadmap's naming, argued in Invariants; it preserves the stride contract
  §7c warns about.

**Alternatives rejected.** Adding a dedicated `MotionVerb` (strafe / kite) for
cover: rejected because cover-seeking is expressible through the existing
combat-slot destination path — make the slot LOS-aware and the shipped
`Chase → set_destination(combat_slot)` steering carries it, with no
primitive-surface steering change. A new steering primitive would be a one-way
contract addition bought before a consumer needs relative-velocity motion.

## Acceptance criteria

- [ ] **AC1.** An enemy with a clear eye-to-target sightline fires; the same
      enemy with world geometry persistently between its eye and the target — a
      sightline it never held, or lost more than `los_grace_ticks` ago — does not
      fire, and takes no attack action, while the target stays in range and off
      cooldown. (Fire during the loss-grace window is AC9.)
- [ ] **AC2.** Sightline is evaluated at the enemy eye point: an enemy whose feet
      are exposed under a low wall but whose eye is occluded does not fire; an
      enemy that can see the target's aim point over cover does.
- [ ] **AC3.** A single LOS query lives in the collision module, casts against
      static world geometry only (movers do not occlude), and is the sole
      sightline routine used by the fire-gate, the `targetVisible` fact, combat
      positioning, and the candidacy gate. The existing netcode consumer
      (`has_static_world_los`) behaves identically after being repointed at it.
- [ ] **AC4.** `targetVisible` is authorable in a behavior graph (TS and Luau) as
      a boolean brain fact; it reports the same sightline verdict the fire-gate
      uses (LOS clear to the current target, loss-grace debounced), independent of
      range, cooldown, and facing.
- [ ] **AC5.** Combat positioning prefers a slot with a clear sightline to the
      target over an equally-reachable blocked slot; given only blocked slots
      in-band, the enemy does not settle in the open and fire — it holds or
      repositions without firing.
- [ ] **AC6.** A ranged enemy authored with a `standoffDistance` settles at that
      distance, strictly inside its fire threshold, and begins firing — rather
      than parking just outside the guard and never crossing in (the Limitator
      standoff defect).
- [ ] **AC7.** During a committed aim the enemy's facing slews toward a moving
      target rather than freezing; the shot fires on the first tick within the
      fire-dwell that the post-slew facing is within tolerance **and** LOS is
      clear, and does not fire while facing away beyond tolerance.
- [ ] **AC8.** Fresh target acquisition does not select a hostile occluded from
      the acquiring enemy (raw LOS, no grace), whether on the pure-fresh scan or a
      retained-target switch to a closer pawn; think-stride cadence is unchanged
      for a distant occluded hostile (stride priced from the unfiltered nearest
      hostile).
- [ ] **AC9.** A retained target that steps behind cover is not hard-dropped by
      LOS loss; the enemy lands shots through the loss-grace window as the target
      enters cover, then holds fire; disengagement remains graph policy
      (transitions/leash), not an LOS side-effect.
- [ ] **AC10.** No through-wall damage (runnable): in a scripted scenario, within
      `los_grace_ticks` of the player stepping fully behind a pillar, the enemy
      stops dealing damage and emits no attack event (assert HP + events). The
      emergent half — the Limitator then repositions to reacquire a sightline and
      resumes fire — is a demo-map integration/manual acceptance on
      `combat-demo.map`, not a tick-exact unit metric (it composes nav + slot +
      standoff + latch).
- [ ] **AC11.** Existing melee enemy behavior is unchanged: a melee enemy
      adjacent to its target still attacks. The eye-to-aim segment at contact
      range is shorter than the nearest static hit, so the fire-gate's `cast_ray`
      returns no blocking hit before `distance - 1e-4`.
- [ ] **AC12.** A melee enemy with no authored `standoffDistance` shows unchanged
      combat positioning: `standoffDistance` defaults to the attack's
      `engagement_radius_for_action`, so the ring scoring is identical to today.
      The parity test must cover an attack carrying a per-attack `engagementRadius`
      override (as the reference enemy's `slam` does), not only the graph default.

## Tasks

### Task 1: LOS query, enemy eye, shared debounced verdict, and the fire-gate (thin slice)

Add a single reusable static-world sightline query to the collision module —
`line_of_sight(eye, aim, &CollisionWorld) -> bool` — generalized from the
existing `has_static_world_los(collision_world, eye, point) -> bool` (currently
in `netcode/mod.rs`, casting `collision::cast_ray` over the exact segment and
blocking on a hit with time-of-impact `< distance - 1e-4`); carry its
`distance <= 1e-5 → false` zero-length early-out verbatim, and repoint the netcode
caller at it, behavior-preserving (AC3). Movers must not occlude — use the static-world
`cast_ray`, not `cast_ray_combined`. Pin **one** enemy eye derivation and **one**
target aim point, used verbatim by every consumer (fact, fire-gate, slot LOS):
enemy eye = `position + hitbox.offset + (0, hitbox.half_extents.y, 0)` when the
enemy's `HealthComponent` carries a `hitbox`, else `position + eye_factor *
NavGraph::agent_params().height` with a single named `eye_factor` constant; the
target aim point = the target pawn's eye, `position + capsule.eye_height * Y`
(enemy targets are always `PlayerMovement` pawns; the same derivation covers all
player pawns in co-op). Compute the enemy→selected-target LOS **once per tick**
in the per-enemy compute loop and apply a loss-grace debounce: the verdict is
`true` immediately on gaining sight, and holds `true` for a named
`los_grace_ticks` window after sight is actually lost (immediate on gain, graced
on loss). The grace requires persisted per-enemy state (a countdown / last-seen
tick) that exists in neither `BrainComponent` nor `AiSystem` today: home it as a
**host-only** entity-keyed grace map on `AiSystem` (following the
`blocked_warned` entity-keyed-map precedent, but persisted across ticks and
pruned on grace-expiry / despawn, not each tick). Host-only is correct because
the AI tick is host-authoritative and clients neither compute the verdict nor
evaluate transitions — so the grace state needs no replication and no boundary
entry, keeping co-op deterministic (the authority is the sole computer). Feed
this single debounced verdict to both the fire-gate and the `targetVisible` fact
(Task 2). Wire the engine-floor fire-gate into the AI tick attack decision: an
attack is suppressed (no damage, no `enemyAttack` event) unless the debounced
verdict is clear. When `collision_world` is `None` (headless / no-world tick),
treat the sightline as clear — there is no geometry to occlude, matching
pre-LOS behavior. The AI tick already receives `collision_world` as a parameter
(today forwarded only to `resolve_combat_slots`); use it in the per-enemy compute
loop — no signature change. The fire-gate reads the selected target's position
from the outcome target binding (`EnemyOutcome.target` already carries
`.position`); additionally thread the target **position** into `BrainFacts.target`
(today `(EntityId, distance)` only) so the Task 2 fact rays to the same point as
the gate. Place the eye computation, the shared LOS-verdict
computation + debounce, and the fire-gate helper in a new
`scripting/systems/ai/perception.rs` module to keep `ai/mod.rs` (981 lines) from
growing; `mod.rs` gets the call-site wiring only. Thin vertical slice: exercise
on the Limitator (behind vs. clear of a wall) to falsify the eye-derivation,
LOS-query, and tick-plumbing assumptions before the fan-out. Satisfies AC1, AC2,
AC3, AC11; establishes the shared verdict AC4/AC9 depend on. Tests assert pin
rows P1, P4, P5, P16, P18.

### Task 2: `targetVisible` brain fact

Expose the Task 1 debounced LOS verdict to the authored behavior graph as a
boolean brain fact `targetVisible`, populated in the per-tick brain-facts refresh
(which runs in the same per-enemy compute loop as the fire-gate, so it has the
same `collision_world`, eye inputs, and threaded target position). The fact reads
the one shared verdict — it does not recompute LOS — so the fact and the engine
fire-gate never disagree **on the LOS verdict** (the fire-gate additionally
requires facing-within-tolerance and cooldown, which the fact does not report, so
the fact is true in states where the gate still forbids a shot).
`targetVisible` is `false` whenever there is no
selected target, and is populated identically in **both** brain-facts refresh
calls (the resolve-cooldown-zero pass and the real-cooldown pass), never carried
stale. Add the fact as a stored boolean the way `target_hostile` /
`target_reachable` are done (real `BrainFacts` fields read directly in the
fixed-value match in `ai/brain_scope.rs`) — not the way `hasTarget` is done
(`hasTarget` is *derived* from `facts.target.is_some()` and has no stored field,
so it is the wrong template for the field itself). Mirror `hasTarget` only for the
**other** surfaces: the `BRAIN_INPUTS` entry typed `IrType::Bool` in
`crates/foundation/src/brain.rs`, and the SDK authoring surface in both TS and
Luau (`brain.targetVisible`), plus the typedef surface test. Per the
primitive-surface-is-a-contract principle, update SDK types and guard validation
in the same pass. Do not route the engine fire-gate through the authored fact —
the gate is floor and must hold for graphs that never read `targetVisible`.
Satisfies AC4. Tests assert pin rows P4, P17.

### Task 3: Visibility-aware combat positioning + standoff distance

Extend `combat_positioning.rs` slot selection with a visibility term and a
first-class standoff distance.

Visibility: evaluate the Task 1 `line_of_sight` query per candidate slot **inside
`score_candidate`** (not only the challenger-generation loop), so a held incumbent
is LOS-re-validated every tick; a clear-sightline slot is strongly preferred, and
when every in-band slot is blocked the solver yields no firing slot (caller
holds/repositions without firing). `collision_world` is already threaded via
`resolve_combat_slots`, but the ray **endpoints** are not: `combat_positioning` is
registry-decoupled (callers pass positions), and today `target_pos` is the
target's ground `Transform.position`. Thread the enemy **eye offset** (from
Task 1's single eye derivation) and the **target aim point** (target eye) into
`CombatQuery` / `resolve_combat_slots`, and ray `(slot + eye_offset) →
target_eye` — reusing Task 1's one eye derivation and one aim point so Invariant
row 2 (no divergent endpoints) holds. Slot LOS is a positioning heuristic
(slot→target at resolve time) and is **not** the fire authority — the Task 1
fire-gate (enemy-eye→target, next tick, from the enemy's actual position) remains
authoritative; neither substitutes for the other.

Standoff: add a **per-attack** `standoffDistance` — a sibling of the existing
`AttackParams.engagement_radius: Option<f32>` — and feed it as the positioning
distance in place of the ring radius (the current `|target_distance -
engagement_radius|` scoring seats the enemy at the ring, just outside a
`≤ FIRE_RANGE` guard, so it never crosses in — commit `90e7505`). The single
`query.engagement_radius` value already drives both `generated_positions`
(`× RADIAL_MULTIPLIERS`) and `score_candidate`'s band error, so passing the
resolved standoff as that one value covers ring and scorer together — one plumb,
no dual edit. Resolve it **action-relative**: `standoffDistance` defaults to the
attack's `engagement_radius_for_action` (not the graph-level value), so an enemy
authoring a per-attack `engagementRadius` (as the melee reference enemy does for
`slam`) positions identically — AC12. Mirror `AttackParams.engagement_radius`
across every surface it already has: the field on `AttackParams`
(`foundation/.../data_descriptors/types/behavior.rs`), its `validate()` (finite
`> 0`), the SDK attack-authoring type in TS + Luau (`standoffDistance`), and the
typedef golden; carry it through `outcome.engagement_radius` /
`prior_engagement_radius` into `CombatQuery` and the
`retained_standoff_matches_committed_state` hysteresis exactly as
`engagement_radius` flows today. If the visibility + standoff additions push
`combat_positioning.rs` (971 lines) past ~1050, extract slot scoring into a
behavior-preserving submodule first. Satisfies AC5, AC6, AC12. Tests assert pin
rows P10, P11.

### Task 4: Committed-aim slew + latched fire

Make a committed aim slew the enemy's facing toward a moving target every tick,
and convert the attack from edge-fire-on-entry to a latch that fires on the first
tick within the fire-dwell that both gates are clear. Two shipped facts drive
this: facing already slews toward the target for **engaged** stopped enemies
(`facing.rs::slew_yaw`, `FACING_TURN_RATE`, applied in `ai/mod.rs`), but the
`aim` beat is **not** engaged (its `motion:"hold"` resolves to
`SteeringIntent::Clear`, no action verb), so facing freezes for the whole aim; and
the attack today edge-fires only on the state-entry tick (`take_entry_pending`),
so a gate closed on that one tick drops the shot for the entire dwell. Fixes: (a)
extend the engaged-facing test so the committed-attack aim phase slews toward the
target every tick (make the aim activity report engaged, or make the facing test
independent of `engages_path` for the attack phase); (b) read the **post-slew**
heading in the fire decision — either apply the facing slew before the attack
decision, or re-derive the tolerance against this tick's slewed yaw — so an enemy
that reaches tolerance this tick can fire this tick; (c) replace the entry-edge
attack with a **fire-once-per-dwell-on-first-open latch**: while the brain dwells
in a firing state, re-check LOS (Task 1 debounced verdict), facing, and the
existing `selected_target_alive` gate every tick and fire on the first clear tick,
at most once per dwell. The latch runs whenever the active leaf resolves a firing
action **every tick** (`action_for_path`, not `action_for_entry_path`), while
`take_entry_pending` / `take_entry_event_pending` still drive animation and
`onEnter`. Track "already fired this dwell" via `attacks_fired_in_activity` read
at the **firing-leaf depth** (`active_depth() - 1`) — not depth 0, which
`record_successful_attack_fire` also increments — zeroed on activity entry, so no
new brain field is needed and the dwell scopes to the firing leaf. The facing
gate: the
shot resolves only when the enemy forward is within a named tolerance angle of the
eye-to-target direction and LOS is clear. This shares `ai/mod.rs` and the
facing/`graph_eval` seam with Task 5 — sequence before Task 5 and coordinate the
shared file. Satisfies AC7. Tests assert pin rows P1, P2, P3, P12, P13, P14.

### Task 5: LOS-as-candidacy on fresh acquisition

Gate fresh target acquisition on **raw** (undebounced) LOS — an enemy does not
newly acquire a target it cannot see — applied on the **eligibility path only**.
The existing `visible: Option<&dyn Fn>` param on `select_target` is *not* the
seam: it is evaluated in `target_candidate` upstream of both accumulators in
`nearest_target_candidate`, so it would filter the `nearest`/`nearest_offered`
value that prices the think-stride, inverting cadence. Instead apply the LOS
eligibility gate parallel to the authored `candidate_filter` (which is already
applied only to the `eligible` fold, never to `nearest`), as an engine-floor step
distinct from the authored filter; `nearest_offered` (stride pricing) must be
computed over all hostiles regardless of LOS. `select_target` /
`nearest_target_candidate` take neither `collision_world` nor an enemy eye today,
so add this eligibility-only LOS predicate as a new parameter (not the existing
`visible`, which is applied upstream of both accumulators in `target_candidate`)
and thread `collision_world` + the Task 1 enemy eye into both call sites, raying
enemy-eye → each candidate's target-eye (Task 1's one eye derivation and one aim
point). Apply the gate to the candidate
scan in **both** branches where a new pawn can become the selected target: the
pure-fresh scan **and** the retained-target due-tick rescan (which can switch to a
meaningfully-closer new pawn — also a fresh acquisition). The retained-target
**lookup** argument stays ungated, so an engaged incumbent is never LOS-dropped;
disengagement stays graph policy (ordered transitions, leash). Fresh-acquisition
LOS is raw, not debounced — the grace applies to the engaged fire-gate/fact, not
to waking. Applying the exact raycast at acquisition (not per-frame per candidate)
bounds cost; when the view-independent cell-visibility substrate is built (gated
on measured need, `research/cell-visibility-substrate.md`), it fronts this same
eligibility gate as a cheap broad-phase before the exact raycast, additive and not
foreclosed. Sequence after Task 4 (shared `ai/mod.rs` / `targeting.rs`). Satisfies
AC8, AC9. Tests assert pin rows P6, P7, P8, P9.

### Task 6: Limitator content + cover demo map (consumer proof)

Update `content/dev/scripts/limitator.ts` to consume the new capabilities: guard
the `aim → fire` transition (and/or a reposition transition) on
`brain.targetVisible` so the graph expresses "aim and fire only when I can see
you, reposition when I can't," and set an explicit `standoffDistance` on the
`shoot` attack (per-attack, in its `attacks` entry), inside the fire threshold —
replacing the current work-around of authoring `engagementRadius` below the guard. Add or extend a demo map (the existing `combat-demo.map`) with
cover geometry — pillars / low walls between typical engagement positions — so the
behavior is exercisable: the Limitator must break the player's sight behind a
pillar, reposition to reacquire a sightline, and resume fire, with no through-wall
damage beyond the intended loss-grace. Keep the melee reference enemy on the map
as the unchanged-behavior control. End-to-end consumer proof; consumes Tasks 2–5.
Satisfies AC10; exercises the corpse and despawn orderings P14, P15.

## Sequencing

**Phase 1 (sequential):** Task 1 — thin slice; falsifies the eye-derivation,
LOS-query, shared-verdict, and AI-tick-plumbing assumptions before any fan-out.
**Phase 2 (concurrent):** Task 2 (brain fact — scripting surface + brain-facts
refresh) ‖ Task 3 (combat positioning — isolated `combat_positioning.rs`). Both
consume Task 1's shared verdict / query; primary files are disjoint.
**Phase 3 (sequential):** Task 4 — consumes Task 1's attack decision and the
facing seam; converts the fire to a latch.
**Phase 4 (sequential):** Task 5 — shares `ai/mod.rs` / `targeting.rs` with
Task 4; runs after it to avoid contention.
**Phase 5 (sequential):** Task 6 — content + map; consumes Tasks 2–5.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| Enemy attack damage never applies without a clear (debounced) eye-to-target sightline and, in a firing state, facing within tolerance (engine floor; holds for graphs that never read `targetVisible`) | Task 1 (fire-gate), Task 4 (facing + latch) | The in-tick damage apply (`apply_damage_with_context`) sits behind the gate; the latch re-checks each dwell tick; Task 6 authors transitions but the gate is never their only enforcement | AC1, AC2, AC7, AC10 |
| One LOS routine, in `collision/`, static-world only (movers do not occlude); one enemy eye derivation and one target aim point; the enemy→target verdict computed once per tick and shared | Task 1 | Tasks 2, 3, 5 consume the one routine/verdict; no second sightline routine, no divergent endpoints | AC3, AC4 |
| `targetVisible` fact and the engine fire-gate report the same sightline verdict every tick | Task 1 (shared verdict), Task 2 (fact reads it) | Both read the one debounced verdict on the one target binding; fact `false` with no target, identical in both refreshes | AC4 |
| Think-stride pricing uses the unfiltered nearest-hostile distance; the LOS gate touches the eligibility path only, never `nearest`/`nearest_offered`, and never via the `visible` param | Task 5 | The LOS eligibility gate is applied parallel to `candidate_filter`; `nearest_offered` is computed over all hostiles | AC8 |
| LOS loss does not hard-drop a retained target; the loss-grace debounce lets an engaged enemy fire briefly into cover, then hold; acquisition LOS is raw, applied to both scan branches, never to the retained lookup | Task 1 (debounce), Task 5 (eligibility gate, both branches; lookup ungated) | Retained-target lookup arg stays ungated; disengagement stays ordered-transition / leash policy | AC8, AC9 |
| Melee behavior unchanged: LOS trivially clear at contact, and `standoffDistance` defaults to `engagement_radius` | Task 1, Task 3 | Contact-range eye→aim segment shorter than nearest static hit; standoff default leaves ring scoring intact | AC11, AC12 |

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| target-visible brain fact | `BrainFacts.target_visible: bool` (stored, like `target_hostile`) | n/a (not serialized to PRL) | `brain.targetVisible` | `brain.targetVisible` | n/a |
| standoff distance | `AttackParams.standoff_distance: Option<f32>` (per-attack sibling of `engagement_radius`; validated finite `> 0`; default = action `engagement_radius_for_action`) | n/a | `standoffDistance` (in an `attacks` entry) | `standoffDistance` (in an `attacks` entry) | n/a |

## Orderings

Pin table — the ordering contract task tests assert by id. One `tick` = one
game-logic tick (Input → Game logic → Audio → Render → Present). The AI tick runs
snapshot → compute (target selection → cooldown/timers → facts refresh →
transitions → motion → attack decision) → `resolve_combat_slots` → apply (brain
write → steering → facing slew → damage apply → animation).

| # | Scenario | Ordering | Expected outcome |
|---|---|---|---|
| **P1** | LOS blocked while in a firing state | latch re-checks each dwell tick; verdict blocked (past grace) | no damage, no `enemyAttack` that tick; latch keeps re-checking; fires the first dwell tick the verdict clears; if the dwell ends still blocked, no shot |
| **P2** | Facing out of tolerance at fire-state entry, LOS + cooldown ready | enemy slews toward the moving target during aim; on a dwell tick the post-slew residual ≤ tolerance | shot fires that tick (latch), not dropped; a tick with residual > tolerance does not fire |
| **P3** | Facing reaches tolerance exactly on a dwell tick | this-tick slew brings yaw within tolerance | fire decision reads the post-slew yaw and fires this tick (no one-tick lag) |
| **P4** | `targetVisible` fact vs fire-gate, same tick, same target | both read the one shared debounced verdict on the one target binding | fact verdict == gate sightline verdict, always; a thin-occluder sweep flips both together |
| **P5** | Divergent-endpoint / mover regression guard | attempt to ray with a different eye or a mover between enemy and target | impossible: one eye derivation, one aim point, static-world `cast_ray`; a mover between enemy and target does not block LOS |
| **P6** | Stride pricing under a sole occluded hostile | one hostile at distance d, fully occluded; acquisition LOS gate active | stride priced from d (unfiltered); no fresh acquisition (eligibility gated); `acquisition_due` cadence unchanged |
| **P7** | Retained target, LOS lost | enemy engaged on A; A steps behind cover | A retained; enemy fires through the `los_grace_ticks` window, then holds; `targetVisible` false after grace; no disengage from LOS alone |
| **P8** | Retained A, closer occluded B on a due tick | retained-due rescan; B meaningfully closer but occluded | B not acquired (eligibility LOS-gated on the retained-due branch too); enemy keeps A |
| **P9** | Full disengage, then occluded target nearby | enemy leashes → retreat → idle; target still occluded and close | on the fresh idle scan the target is not reacquired (acquisition needs raw LOS); intended v1 consequence |
| **P10** | Held combat slot goes blocked mid-hold | slot resolved with LOS clear; held with `scan_challengers=false`; target moves so slot→target is now occluded | incumbent re-scored in `score_candidate` fails LOS → slot cleared that tick → enemy holds; re-scan next tick picks a visible slot or holds; never fires from the blocked slot |
| **P11** | All in-band slots blocked | every ring candidate lacks LOS | `resolve_combat_slots` yields no firing slot; enemy holds/repositions without firing |
| **P12** | AIM_MS authored at 0 (or < one tick) | close → aim enters and exits same/next tick | with the latch + per-tick aim slew, the enemy still fires once facing/LOS clear within the following fire-dwell; if the target is un-faceable within the dwell, no shot that cycle (no silent permanent miss) |
| **P13** | FIRE_MS authored at 0 | fire entered; dwell is one tick | latch fires on that tick if the gate is clear; FIRE_MS controls dwell length, not whether a shot can fire |
| **P14** | Target dies mid-aim (corpse present) | dead pawn keeps `PlayerMovement`+`Transform`; brain enters fire | attack gate `selected_target_alive` false → no corpse damage, no event; aliveness authoritative in the engine gate, not the authored guard; loops until `hasTarget` clears |
| **P15** | Target despawns mid-aim | pawn loses `PlayerMovement`/`Transform` before this tick's compute | `select_target` → None → `hasTarget` false → `"*"` → idle; aim abandoned cleanly; no fire |
| **P16** | LOS flicker across ticks (thin-pillar strafe) | verdict would flip tick-to-tick raw | the loss-grace debounce holds the verdict `true` across brief flicker; the enemy keeps firing through the grace window (intended imperfection); the fact does not chatter |
| **P17** | `targetVisible` with no target | `selected_target` None | `target_visible` = false in both refreshes; never stale `true` |
| **P18** | N enemies fire at one target in a tick (incl. N=0) | all compute-pass attack decisions read last-tick Health; damage applied sequentially in apply | LOS evaluated per-enemy in the compute loop against snapshot position, so one enemy's verdict never depends on another's apply-pass writes; N=0 → no events |

## Open questions

- **Eye factor / hitbox-top exactness.** v1 derives eye from hitbox top-center or
  `eye_factor * agent_height`. If playtest shows the ray origin reading too
  high/low for an archetype, an authored per-archetype eye offset is the additive
  follow-on (the derived eye needs no authoring and covers the Limitator). Owner:
  playtest.
- **`los_grace_ticks` value.** The loss-grace window is an engine constant in v1;
  its value (how many shots leak into cover) is a feel knob to tune on the demo
  map, and a per-archetype authoring field is the additive follow-on. Owner:
  playtest.
