# Enemy Behavior Descriptor Syntax

## Goal

Grow the `components.behavior` vocabulary so the roadmap's leash/pursuit
behaviors — retreat-to-start, patrol-area / return-to-patrol, unreachable-target
behavior, and chase-to-nearest-reachable / barrier behavior — become *expressible*
authored content on the shipped behavior-state-graph, not new hardcoded archetypes.
It adds position-goal motion verbs (a spawn anchor to move toward, a patrol route
to walk), the brain facts that guard them (distance-from-home, target
reachability), a minimal mutable-hostility hook so fresh acquisition consults a
per-entity faction relation instead of treating every player pawn as
unconditionally hostile — the targetable kind stays PlayerMovement, and applies
the same composition shape the player descriptor set established. Today the
motion vocabulary is only chase/hold/freeze and no position/home/reachability
fact exists — these behaviors cannot be written at all, and closing that gap is
the work.

## Scope

### In scope

- **Position-goal motion verbs.** Two new `MotionVerb` variants: `moveToAnchor`
  (steer toward the enemy's home anchor; on arrival, stand) and `patrol` (walk an
  authored route). The closed-vocabulary rule holds — the engine owns steering, the
  author picks the mode.
- **Home anchor.** Each brain records its spawn transform position as `home_anchor`
  at spawn. No descriptor or map field: placing the entity *is* authoring its home.
- **Patrol authoring.** An optional graph-wide `patrol` block: `points` (anchor-relative
  XZ offsets, so a route is placement-independent) and `mode` (`loop` | `pingPong`).
  The engine tracks a per-brain patrol cursor. `patrol` motion with no block, or an
  empty `points` list, is a parse error in both runtimes.
- **Stand-down destination is the untargeted-active resting state.** An any-state
  `not hasTarget` / `not targetHostile` stand-down interrupt must target the graph's
  untargeted-active resting state — the `patrol` state when a route is authored, else
  `idle`. `select_transition` skips a graph-wide interrupt only where `interrupt.to ==
  current`, so a stand-down whose destination is *not* the state the enemy rests in
  re-fires every tick that state is live: an `idle↔patrol` oscillation that never walks
  the route. Authoring constraint, taught by the reference comments; the multiple-resting-state
  general fix is deferred (Open questions).
- **New brain facts** (three, appended in order at `BRAIN_INPUTS` indices 10
  `distanceFromAnchor`, 11 `targetHostile`, 12 `targetReachable`; `targetHostile` belongs to
  the faction hook below): `@brain.distanceFromAnchor` (Number; the enemy's XZ distance
  from `home_anchor`, always meaningful, `0` at the anchor) and `@brain.targetReachable`
  (Bool; whether the nav floor can path enemy→selected-target this tick, `false`
  untargeted or with no navmesh). Each lands in the foundation table +
  `BrainValidationScope`, the runtime `BrainScope` refresh, `BrainFacts`, and the SDK
  `brain` prelude (TS + Luau).
- **Minimal faction hook** (two parts, split along the fresh-scan / retained-lookup
  seam). *Acquisition:* the fresh ranking scan (`nearest_target_candidate`) admits a
  pawn only when the evaluating enemy is hostile to it — `faction(enemy) !=
  faction(candidate)`, faction read from the E16 `@state.faction` leaf (absent → 0.0) —
  so a friendly pawn never masks a hostile one behind it. The retained-target lookup
  applies no faction test (`entity_model.md` §7c: candidacy filters fresh candidates
  only, never the retained target). *Retention:* the second brain fact
  `@brain.targetHostile` (Bool; whether the *selected* target is hostile, `false`
  untargeted) at `BRAIN_INPUTS` index 11; a target whose faction flips friendly
  mid-chase is stood down by an authored interrupt over it — the shape of the shipped
  `targetDied` stand-down. Enemies are seeded faction 1 on every enemy-assembly path,
  players read 0, so hostility holds for every enemy and existing targeting assertions
  are unchanged. Faction is *mutated* through the existing E16
  `@state` write path — this spec adds only the engine-floor read, the fact, and the
  spawn seed.
- **Composition shape.** Applied per `player-descriptor-composition`: one transition
  grammar (already the `{to, when}` rows), closed engine vocabulary, data-only, new
  tuning added as composed graph-wide blocks (`patrol`) rather than flat flags. The
  named-attack-map instance of the "shared defaults + sparse override" pattern is a
  coordination *recommendation* (see Coordination), landed by `E10--enemy-multi-attack`.
- **Split `ai/mod.rs`** (982 production lines) before extension: extract the facing
  helpers and combat-slot resolution into sibling modules.
- Reference enemy authored with a retreat-to-start + patrol demonstration; agent
  diagnostics overlay labels the new states; `docs/scripting-reference.md` documents
  the new verbs/facts/faction and the recommended attack-map grammar.

### Out of scope

- **The full faction / relationship model** — named alliances, neutrality, per-pair
  diplomacy, a declaration surface for initial faction, `@candidate.faction` in the
  authored candidate-filter IR, and enemy-vs-enemy infighting (broadening the
  targetable-kind set beyond `PlayerMovement`). Owned by the **Faction & relationship model**
  roadmap spec (Epic 10), where the widened-candidacy broad-phase also lives (gated on measured
  need). See Open questions (faction forward-compatibility).
- **The `attacks` named map + parameterized `attack` action verb** — grammar recommended
  here (Coordination), shipped by `E10--enemy-multi-attack`. This spec does not touch
  `AttackParams` or `ActionVerb`.
- **Random / wander patrol** — needs the seeded-deterministic-RNG story `E10--enemy-stagger`
  also defers; `patrol` is deterministic ordered points only.
- **A runtime-movable home anchor** — the anchor is the spawn position for the brain's
  life; re-homing is not expressible here. The additive write path is the **Runtime-movable
  home anchor** roadmap item (Epic 10), deferred to a set-piece consumer.
- **New perception inputs** (LOS, sound, alert propagation) and damage-based aggro —
  the roadmap's line-of-sight bullet and `research/enemy-aggro-model.md`.
- **An engine-side leash or acquisition range** — leash is authored via
  `@brain.distanceFromAnchor` guards, preserving behavior-state-graph's deliberate
  no-`leashRange` foreclosure.
- **Wire / replication changes** — anchor, patrol cursor, reachability, and faction
  are host-only sim state; clients keep consuming replicated animation-state names.
- **`and`/`or`/`not` IR opcodes** — the standing additive substrate follow-up; guards
  compose via `select` as today.

## Direction

**Problem.** The four leash/pursuit behaviors on roadmap line 62 are *unexpressible*
in `components.behavior` today — not blocked by a hardcoded archetype, but by a missing
vocabulary. The observation: the shipped motion verbs are chase/hold/freeze and every
brain fact is target-relative, so an author has no primitive for "move toward home,"
"walk a route," "how far am I from home," or "can I reach my target." The cause is
that behavior-state-graph shipped a deliberately minimal v1 vocabulary; this spec grows
it. Cause, not symptom: the deliverable is expressiveness primitives, not another behavior.

**Prior commitments.** behavior-state-graph foreclosed a `leashRange` field as "a
second spelling of disengagement that silently outranks the guards" — *preserved*:
leash is a `distanceFromAnchor` guard, no engine leash field. Its "guards are
read-only" rule is preserved (facts read-only; faction is written via the existing
E16 `setState`, never a guard). The append-only `BRAIN_INPUTS` ordering is preserved
(the three facts append at the tail, indices 10-12). The engine-owned candidacy floor
stays engine-owned — the hostility filter extends the floor's *rule* without moving
candidacy into graph content, and follows behavior-state-graph's "candidate filters
admit fresh candidates only; retention is graph policy" rule exactly (`entity_model.md`
§7c): hostility narrows *fresh acquisition* on the ranking scan, and a target whose
faction flips friendly mid-chase is stood down by an authored interrupt over
`@brain.targetHostile` — the same shape as the shipped `targetDied` stand-down, not an
engine re-gate of the retained target. The player-descriptor-composition shape is
applied. The `E10--enemy-multi-attack` map grammar is coordinated, not overridden
(see Coordination).

**Alternatives rejected.** The strongest rival is an engine-floor leash: a `leashRange`
scalar plus hardcoded retreat/patrol steering that reproduces these behaviors as engine
policy. Rejected on two counts — behavior-state-graph already foreclosed `leashRange`,
and it is "another hardcoded archetype," exactly what the roadmap says not to build; the
vocabulary-growth path keeps every behavior authored content. A second rival expresses
anchor/patrol entirely in graph content (author stores home in `@state.homeX/homeZ`,
does position math in IR): rejected because the IR has no vector type and no way to read
the spawn transform, and per-tick position arithmetic in `select` trees is unauthorable
in practice — a first-class anchor fact and verb are cleaner and the engine already holds
the position. A third rival puts the hostility test in the shared candidacy predicate
(`target_candidate`, which the retained lookup also calls) so the floor drops a target
the instant its faction flips: rejected because `entity_model.md` §7c evaluates the
candidacy predicate only against fresh offered candidates, never the retained target —
re-gating retention there would re-split acquisition and retention against that rule.
Retention drop is authored instead, over `@brain.targetHostile`.

## Acceptance criteria

- [ ] **distanceFromAnchor.** A graph guarding on `@brain.distanceFromAnchor` fires
  when the enemy's XZ distance from its spawn position crosses the authored threshold;
  the fact reads `0` at the spawn point, is available with no selected target, and both
  runtimes accept a guard that binds it (Number). The value tracks the *spawn* position
  even after the entity is moved by a script.
- [ ] **moveToAnchor.** An enemy in a `moveToAnchor` state steers toward its spawn
  anchor and faces its direction of travel while moving. The goal resolver returns
  `Clear` (stands, holds facing) on any tick with `distanceFromAnchor <=
  POSITION_GOAL_ARRIVAL_EPSILON` and `MoveTo(anchor)` otherwise — arrival is *not*
  latched: an external push (separation, mover, knockback) past the epsilon after
  arrival re-issues `MoveTo(anchor)` the next tick. Verified at the start (moving), the
  arrival tick (`Clear`), a later tick still within epsilon (`Clear`), and a
  post-arrival displacement past epsilon (`MoveTo` re-issued).
- [ ] **patrol.** An enemy in a `patrol` state visits the authored anchor-relative
  points in order; `loop` wraps the cursor to 0 past the last point, `pingPong` reverses
  direction at each end, a single-point route degenerates to standing at that point, the enemy faces its
  travel direction while striding between points (the same MoveTo-path facing AC 2 pins),
  and the cursor persists across leaving and re-entering the patrol state (return-to-patrol
  resumes rather than restarting). `patrol` motion with no `patrol` block, and an empty
  `points` list, are parse errors with pathed messages in both runtimes.
- [ ] **Retreat-to-start, end to end.** A fixture graph with `alert/attack → retreat`
  on `gt(distanceFromAnchor, leash)`, `retreat` using `moveToAnchor`, and `retreat →
  patrol` on `le(distanceFromAnchor, arrivalEps)` (with `arrivalEps >=
  POSITION_GOAL_ARRIVAL_EPSILON`) walks the enemy home when a target leads it past the
  leash, then resumes patrol. `retreat` is non-engaged (Task 3), so it holds no target
  across strides — its only exits are `distanceFromAnchor`-based or the any-state
  stand-down, never a target-distance edge. A target lost mid-retreat on a non-due tick
  routes through the any-state stand-down to `patrol`, which steers back toward home;
  re-engagement then fires from `patrol → alert` on a fresh acquisition, not from a
  `retreat → alert` edge.
- [ ] **targetReachable.** With a selected target and navmesh the fact reads the nav floor's
  `find_path` verdict (enemy→target); it reads `false` with no target or no navmesh present,
  clearing any restored or non-due cached verdict immediately. It is recomputed on the
  think-stride acquisition cadence and a between-strides tick reuses the cached value only
  while navigation is present. The fact ships with this spec as the nav-floor verdict, not a
  ground-truth reachability oracle. The authored routing demo — a
  `select(targetReachable, false, true)` guard routing to a hold state when the target is
  unpathable and back when it becomes pathable — is gated on `E10--pursuit-wraparound-blocked`
  (AC 11): until that fix the verdict inherits pursuit's false-negative around freestanding
  walls, so a demo built on it would misfire at every wall corner.
- [ ] **Faction hostility gate.** On a fresh acquisition scan an enemy admits a
  `PlayerMovement` candidate only while hostile to it per `@state.faction`, so a friendly
  pawn never masks a hostile one and is never freshly acquired; the retained-target lookup
  applies no faction test. A retained target whose faction is written (through the E16
  `@state` path) to equal the enemy's faction is dropped by the reference enemy's authored
  `select(targetHostile, false, true)` stand-down interrupt on the next guard eval — not
  by candidacy — and reverting it lets the enemy re-acquire. With the enemy default faction
  seeded on every enemy-assembly path (including the shared test spawn helper), existing
  AI-test *assertions* are unchanged: the seed is a transparent harness addition, and the
  `select_target` unit test passes the enemy's faction argument.
- [ ] **Split.** `ai/mod.rs`'s facing helpers and combat-slot resolution move to sibling
  modules; its production line count drops below ~800; the full AI suite passes with
  import-only changes.
- [ ] **Both scopes, all three facts.** `@brain.distanceFromAnchor`, `@brain.targetHostile`,
  and `@brain.targetReachable` resolve identically in `BrainValidationScope` and the runtime
  `BrainScope` (drift test), occupy `BRAIN_INPUTS` indices 10, 11, and 12 respectively, and
  appear in the SDK `brain` prelude in both runtimes (SDK drift test green).
- [ ] **Zero-alloc guard window.** The `brain_scope` refresh+eval fixture
  (`refresh_and_guard_eval_perform_zero_heap_allocations`, `ai/brain_scope.rs`), extended
  with the three new fixed slots populated for a non-due tick — anchor distance, the cached
  reachability verdict (no `find_path` call), and hostility — performs zero heap
  allocations. `run_ai_tick` is not the probe target: its compute pass builds fresh
  outcome/snapshot/event `Vec`s every tick regardless of stride, so it can never be
  zero-alloc. The due-tick reachability probe calls `find_path`, which runs A* and
  allocates a `NavPath`; that allocation is explicitly *outside* the zero-alloc contract —
  it is strided and cached, so a between-strides tick reuses the cached verdict and
  allocates nothing.
- [ ] **Host-only, deterministic, no wire change.** Anchor, patrol cursor, reachability
  cache, and faction are host-only sim state; sim determinism tests stay green; a
  connected client observes only replicated animation-state names, with no new wire field.
- [ ] **Reference + diagnostics.** The reference enemy is authored with the retreat +
  patrol demonstration (the reachability demo gated on `E10--pursuit-wraparound-blocked`),
  the agent diagnostics overlay shows the new authored state names,
  docs/scripting-reference.md documents the new verbs/facts/faction, and both typedef
  fixtures are regenerated.

## Tasks

### Task 1: Split `ai/mod.rs` before extension

Behavior-preserving split of `crates/postretro/src/scripting/systems/ai/mod.rs` (982
production lines; its tests already live in the external `ai_tests.rs`). Extract the
pure facing/yaw cluster (`MESH_FORWARD`, `FACING_TURN_RATE`, `yaw_rotation_toward`,
`yaw_from_rotation`, `slew_yaw`) into a new `ai/facing.rs`, and the combat-slot cluster
(`resolve_combat_slots`, `clear_combat_slot`, `retained_combat_slot`, `COMBAT_SLOT_HOLD_TICKS`)
into a new `ai/combat_slots.rs`. `LocomotionIntent`/`MOVE_SPEED_EPSILON` are used across the
apply pass as well as facing — leave them in `mod.rs` (or wherever the implementer sees the
cleaner seam); do not force them into `facing.rs`. Add `mod facing;` / `mod combat_slots;`
to `mod.rs`, and expose (`pub(super)`/`pub(crate)` as appropriate) and re-export every
extracted item the tick or `ai_tests.rs` references, so call sites change imports only.
The combat-slot functions read private `EnemyOutcome` fields (`combat_slot`, `engaged`,
`brain`, `id`, `position`, `target`, `prior_acquired_target`); moving them to
`combat_slots.rs` also requires widening those fields to `pub(super)` — extract the item
*and* the struct-field access it needs, not just the functions.
No behavior change; the full AI suite stays green. This lands first because every
later task edits the tick and both extracted clusters.

### Task 2: Thin slice — the `@brain.distanceFromAnchor` fact end to end

The narrow vertical path that falsifies the "add a brain fact" boundary the rest of the
spec repeats. Add `home_anchor: Vec3` to `BrainComponent` (`crates/entities/src/components/brain.rs`,
`#[serde(default)]` → `Vec3::ZERO` for pre-existing saves) and seed it from the enemy's
spawn position on every host enemy-assembly path (coordinate with Task 4's faction seed —
both seed at assembly): the archetype spawn block (`builtins/data_archetype.rs`, the
read-modify-write block that already reads the brain to set `aggro_armed`) seeds it from the
entity's `Transform.position` (attached by `try_spawn` before component seeding runs, so
it's already on the entity here); the shared test spawn helper (`spawn_enemy`, `ai_tests.rs`)
seeds it from its position argument. `from_graph` has no transform available and defaults
`Vec3::ZERO`; a client never constructs a host brain from a spawn transform — the
connected-client remote-enemy path is mesh-presentation-only, and any brain built off
the wire goes through `from_graph`, which has no transform — so `home_anchor` stays
`Vec3::ZERO` and is unread on clients (guard evaluation is host-only, `entity_model.md`
§7c). Append `@brain.distanceFromAnchor`
(Number) to `BRAIN_INPUTS` at **index 10** in `crates/foundation/src/brain.rs` with its
`BRAIN_DISTANCE_FROM_ANCHOR_INPUT` const; `BrainValidationScope` resolves it automatically
through the table. In the runtime `BrainScope` (`ai/brain_scope.rs`): grow the `fixed`
array (it is `[IrValue; BRAIN_INPUTS.len()]`, so the length follows the table) and write
slot 10 in `refresh` from a new `BrainFacts.distance_from_anchor: f32`. Compute that value
in the tick's compute pass as `crate::nav::distance_xz(snap.position, brain.home_anchor)`
(`pub(crate)` in `nav/mod.rs`; import it into `ai/mod.rs`) — every tick, no stride, no
target needed. Append the leaf to `sdk/lib/brain.ts` and `brain.luau`
(the frozen `brain` object + `BrainInputs` interface) per their stated sync obligation.
Regenerate the committed SDK typedef fixtures and update the drift-test expectations in
`crates/scripting-core/src/data_descriptors/tests/behavior.rs`. The `expected_fixed_value`
oracle in `brain_scope.rs` tests (no `_` arm) forces a matching case.

### Task 3: Position-goal motion verbs and patrol authoring

Add `MoveToAnchor` and `Patrol` to `MotionVerb`
(`crates/foundation/src/data_descriptors/types/behavior.rs` — the type lives in `foundation`;
`crates/scripting-core/.../data_descriptors/tests/behavior.rs` holds only the drift tests),
updating `MotionVerb::ALL` and the `motion_verb_all_is_exhaustive` successor chain (the
test fails until both are updated). Add an optional `patrol: Option<PatrolDescriptor>`
graph-wide block: `points: Vec<[f32; 2]>` (anchor-relative XZ, metres) and `mode:
PatrolMode { Loop, PingPong }` (camelCase serde). Validation alongside the existing
`BehaviorGraphDescriptor::validate`: any state whose `motion` is `patrol` requires a
present `patrol` block with a non-empty `points` list (mirroring the attack-action-requires-attack-block
rule), and each point's components must be finite — pathed messages
(`components.behavior.patrol.points[i]`). `behavior.rs` production is ~453 lines (its
`mod tests` bulk starts at line 454); it stays under the split threshold, so no split
is warranted here. Extend `SteeringIntent` (`engine_floor.rs`) with a data-carrying
`MoveTo(Vec3)` variant and a `POSITION_GOAL_ARRIVAL_EPSILON` constant (`0.5` m).
`SteeringIntent` currently derives `Eq`; `Vec3` holds `f32` and has no `Eq`,
so the `MoveTo(Vec3)` variant forces dropping `Eq` from the derive — keep `PartialEq` (`Vec3`
supports it) and `Copy` (both still hold). Its only consumers compare with `==`
(`graph_eval::engages` and the compute-pass steering match), which `PartialEq` satisfies; no
consumer needs `Eq`.

Two functions run over the two new motion verbs, and they are not the same function.
`steering_for(motion)` (`graph_eval.rs`) has no registry, no `home_anchor`, and no patrol
block, so it cannot compute a goal; it gains `MoveToAnchor`/`Patrol` arms returning a
non-`Chase`, non-`MoveTo` sentinel used only for *classification* — `engages`
(`steering_for(motion) == Chase || action.is_some()`) reads both as non-engaged. `steering_for`
stays exhaustive; the two arms are a compile error until added. Extend `is_locomotion_state`'s motion predicate
(`graph_eval.rs`, today `state.motion == MotionVerb::ChaseTarget && state.action.is_none()`)
to also admit `MoveToAnchor`/`Patrol`, so their travel clip yields to the rest animation
at a standstill and drives walk-playback scaling. The actual `MoveTo(goal)`/`Clear` intent comes from a *separate* goal resolver in the
compute pass, which holds `&BrainComponent` (hence `home_anchor`, the patrol block, and the
cursor): for `MoveToAnchor`, target `home_anchor`, returning `Clear` on any tick within
`POSITION_GOAL_ARRIVAL_EPSILON` else `MoveTo(anchor)` — arrival is *not* latched, it re-issues
`MoveTo` on any tick the enemy is pushed back past the epsilon; for `Patrol`, target
`home_anchor + points[cursor]`, advance the cursor per `mode` when arrived, and return
`MoveTo(point)`. The compute-pass steering resolution routes `MoveToAnchor`/`Patrol` through
this resolver and every other verb through `steering_for`. In the apply pass, add a
`SteeringIntent::MoveTo(goal)` arm that calls `agent_steering::set_destination(goal)`.

The cursor and direction are new `BrainComponent` fields (`patrol_cursor: usize`,
`patrol_direction: i8`), mutated in the compute pass (the brain is already `&mut` there); they
persist across state changes. `patrol_cursor` is serde-persisted and carries `#[serde(default)]`
(like `home_anchor` — a newly-added non-defaulted field would reject every brain save written
before it existed, and no suite test loads a genuinely-old save, so the compiler will not catch
the omission). A save written against a
longer `points` list and loaded after the descriptor shrinks the list deserializes an
out-of-range cursor — clamp it with `cursor % points.len()` (preserves route phase over a reset-to-0) before
indexing, mirroring the `state_index` re-seat in `ai/mod.rs`. `patrol_direction` serde-defaults
to `0`, and `from_graph` (a full struct literal) would also seed `0`, which leaves
`pingPong`'s `cursor += direction` motionless; seed `patrol_direction = 1` explicitly in
`from_graph` and via `#[serde(default = ...)]` returning `1`, so `pingPong` advances from both
a fresh `from_graph` brain and a serde-default-deserialized one. `MoveToAnchor`/`Patrol`
states are not `engaged` (no target stake), so they drop the acquired target and take no
combat slot — correct for a retreating/patrolling enemy. Because `is_locomotion_state` treats
them as locomotion, the existing multi-locomotion `locomotion_animation` BTreeMap-order collapse
spans chase/patrol (a documented, host/client-consistent v1 limitation, not a desync). Extend the facing gate so an enemy under a position-goal
`SteeringIntent::MoveTo` faces its travel velocity while moving (it faces nothing when
arrived/stopped) — today facing is gated on `outcome.engaged`, which position-goal motion
is not. Crucially, a position-goal state may still hold a nearest pawn in `outcome.target`
(an unengaged state still runs `select_target` on a due tick and stores the nearest pawn), and
the current stopped branch faces `outcome.target`. The position-goal facing path must face
travel velocity *only* and face nothing when stopped — it must not fall through to the
`outcome.target`-facing branch, or a retreating/patrolling enemy that is arrived (or blocked
pre-move, speed ≤ ε) would spin to face a nearby pawn, violating AC 2's "faces nothing when
arrived/stopped." Pin this with a test (Task 6).

### Task 4: Minimal faction hostility hook

Split the hostility mechanism along the fresh-scan / retained-lookup seam `ai/targeting.rs`
already draws (`entity_model.md` §7c: the candidacy predicate is evaluated once per offered
candidate on a ranking scan, never against the target already retained).

**Acquisition — fresh scan only.** `target_candidate`, the per-entity gate both the
fresh scan (`nearest_target_candidate`) and the retained lookup (`select_target`)
call, keeps its existing checks (visibility, `PlayerMovementComponent`, `Transform`)
unchanged for both paths. Add the hostility filter to `nearest_target_candidate` only: for
each offered candidate read its `@state.faction`
(`registry.get_component::<EntityStateComponent>(entity)`, absent → 0.0) and admit it only
when it differs from the evaluating enemy's faction — under nearest-target ranking this is
necessary, since without it a nearer friendly pawn masks a hostile one behind it. The
retained lookup applies no faction test (its alive/died checks are unchanged), so a retained
target is never re-gated on hostility. Thread the enemy's faction scalar as a new `f32`
parameter `select_target` → `nearest_target_candidate` (read once per enemy in the compute
pass from its `EntityStateComponent`); `target_candidate`'s signature is untouched. This
changes the call the unit test `selection_keeps_retained_target_until_a_fresh_candidate_beats_hysteresis`
(`targeting.rs`) makes: it now passes the enemy's faction argument as a hardcoded
`1.0` literal (the test has no enemy entity to read it from), and its hysteresis
assertion is unchanged — both pawns read faction 0.0 (absent), the enemy passes `1.0`,
so both stay admitted.

**Retention — authored, over a target-side fact.** Append `@brain.targetHostile` (Bool) to
`BRAIN_INPUTS` at **index 11** (after Task 2's index 10) with its
`BRAIN_TARGET_HOSTILE_INPUT` const, wired the full Task-2 way: `BrainValidationScope`
(via the table), the `BrainScope` `fixed` array + `refresh` slot 11, a new
`BrainFacts.target_hostile: bool`, the SDK `brain` prelude (TS + Luau), the SDK typedef
fixtures, the drift tests, and the `expected_fixed_value` oracle. The engine computes it in
the compute pass from the *selected* target: hostile (`faction(enemy) != faction(target)`) →
`true`; no valid hostile target — untargeted, or the selected target's faction equals the
enemy's — → `false`, following the target-side facts' no-target convention (`targetDied`
reads `false` untargeted), so an authored `select(targetHostile, false, true)` stand-down
reads true and fires exactly when the retained target is not hostile. Retention drop is then
authored: the reference enemy (Task 6) carries an any-state interrupt
`select(targetHostile, false, true)` → stand-down. This is the same *shape* as the shipped
`targetDied` stand-down (an any-state interrupt over a target-side bool → resting state), but
the untargeted polarity differs: `targetDied` reads `false` with no target and so does not
fire, whereas `select(targetHostile, false, true)` reads `true` untargeted, so it fires on
untargeted-*or*-friendly. It is correct in the reference only because the earlier-declared
`not hasTarget` interrupt (same `to`) wins first-true-wins on untargeted ticks; an author
lifting it must co-locate it with a `not hasTarget` stand-down sharing its `to`. So a target
whose faction flips friendly mid-chase stands the enemy down on the next guard eval.

Seed the enemy default faction (`FACTION_STATE_FIELD = "faction"`, `ENEMY_DEFAULT_FACTION =
1.0`, via `registry.entity_state_mut(id).set(..)`) on *every* enemy-assembly path that
produces a brain-bearing enemy — the archetype spawn block (`builtins/data_archetype.rs`, the
same read-modify-write block as Task 2's anchor seed), the shared test spawn helper
(`spawn_enemy` in `scripting/systems/ai_tests.rs`), and any other host spawn or reconstruction.
This is a constraint over the assembly paths, not a single call site: an enemy built by
`spawn_enemy`/`from_graph` with no `EntityStateComponent` reads `@state.faction` absent → 0.0,
matches the player's 0.0, and is excluded from the hostility-filtered `nearest_target_candidate`
— it would never acquire, collapsing the targeting/chase/attack suite. Do *not* use a
default-on-read shim: that would make a mod's `@state.faction` read 0 while the engine treats
an unseeded enemy as 1, a visible engine divergence. Players carry no `faction` key and read 0,
so enemy(1) ≠ player(0) stays hostile and existing test assertions hold. The targetable-kind
iteration set stays
`[ComponentKind::PlayerMovement]` (a named constant seam so the full model can widen it);
this keeps candidacy's iteration and perf identical to today and adds only an O(1) hostility
test per fresh candidate. Faction is mutated entirely through the shipped E16 `@state` write
path — no new reaction or output.

### Task 5: The `@brain.targetReachable` fact and its nav probe

Append `@brain.targetReachable` (Bool) to `BRAIN_INPUTS` at **index 12** (after Task 4's
index 11) with its `BRAIN_TARGET_REACHABLE_INPUT` const, wired the same way as Task 2 and
Task 4 through `BrainValidationScope`, the `BrainScope` `fixed` array + `refresh` slot 12, a
new `BrainFacts.target_reachable: bool`, the SDK `brain` prelude (TS + Luau), the SDK typedef
fixtures, the drift tests, and the `expected_fixed_value` oracle (its no-`_` panic arm forces
a matching case). Compute reachability in the compute pass: when the brain is
armed, has a selected target, a nav graph is present, and the tick is acquisition-due (reuse
the existing `evaluate_acquisition` stride gate — do not add a stride constant), call the nav
floor's `find_path(nav_graph, snap.position, target_pos).is_some()` and cache it on a new
`BrainComponent.target_reachable: bool` field marked `#[serde(default)]` (or
`#[serde(skip)]` — it is a recomputed cache that need not persist, and a plain field
with no default would reject any save written before it existed); between strides
reuse the cache. With no selected target the fact reads `false`; with no `nav_graph` (a map
without a navmesh) it also reads `false` immediately and clears the cache before a non-due
retained-target tick can reuse it. This intentional implementation correction does not add
no-nav direct steering or pursuit behavior; `E10--pursuit-wraparound-blocked` owns pursuit
route correctness. This is the one per-enemy nav query added; it is strided and cached so an
idle-strided distant enemy pays it at most once per stride band, within the combat-positioning
query budget.
**The fact ships here; only its reference demo waits.** `@brain.targetReachable` is defined
as the nav floor's `find_path` verdict (cached when a nav graph exists); without a nav graph it
conservatively reports `false`. It is not a ground-truth
reachability oracle. That draft fixes a `find_path` repair failure that returns a false `None`
for a genuinely-routable wraparound around a freestanding wall; until it lands, the verdict
inherits pursuit's known wraparound limitation around such walls. The fact and its wiring
therefore land with Task 5; only the authored reference *demo* (Task 6's `waiting` barrier-hold,
which would trigger at every wall corner and read as a malfunctioning reference enemy) waits on
the fix — per AC 11. Document the limitation alongside the fact so an author knows it tracks the
pathfinder's current capability, not ground truth.

### Task 6: Reference authoring, tests, diagnostics, docs

Author the reference enemy (`sdk/behaviors/reference/entities.{ts,luau}`, both spellings
identical) to demonstrate the new vocabulary. `patrol` is the untargeted-active resting state:
a `patrol` block (`pingPong` over a short route) and a `patrol` state that acquires on a fresh
scan (`patrol → alert` on `le(targetDistance, detection)`). A `retreat` state (`moveToAnchor`)
is reached from `alert`/`attack` on `gt(distanceFromAnchor, leash)` and exits *only* on
`le(distanceFromAnchor, arrivalEps)` → `patrol` (home reached, resume patrol) — no
target-distance exit, since `retreat` is non-engaged and holds no target across strides;
re-engagement rides the any-state stand-down back to `patrol` and then `patrol → alert`. The
two any-state stand-downs (`not hasTarget` and the new `select(targetHostile, false, true)`
friendly-flip drop, the `targetDied` shape) both target `patrol`, so they are skipped there
(`to == current`) and do not oscillate. Its comments are the de-facto authoring docs — they
must teach that the anchor is the spawn position, that leash is a `distanceFromAnchor` guard
(not an engine field), the patrol cursor's persistence, that a stand-down interrupt must target
the untargeted-active resting state or it oscillates, that an authored arrival guard threshold
must be `>= POSITION_GOAL_ARRIVAL_EPSILON` (a smaller threshold wedges — the resolver `Clear`s
at the epsilon and never reaches it), that retention drop on a friendly flip is authored
over `@brain.targetHostile` (and that `select(targetHostile, false, true)` fires on
untargeted-*or*-friendly, unlike `targetDied`, so it must co-reside with the `not hasTarget`
stand-down and share its `to`), and that a target-side-fact guard
(`targetReachable`/`targetHostile`/`targetDistance`) on a non-engaged state reads the
no-target sentinel on non-due ticks, so it needs an `acquisitionDue` conjunction (the
`patrol → alert` idiom). The reachability demo (a `waiting` state whose re-acquire guard carries the `acquisitionDue`
conjunction over `targetReachable`) is added only once Task 5's nav dependency lands;
until then the reference ships without it and the docs note the fact exists. Port/extend
`ai_tests.rs`, each test pinned by an Orderings row: retreat round-trip (leash exit, arrival →
`patrol`; target lost mid-retreat on a non-due tick routes to `patrol`, not stranded); patrol
lifecycle (loop wrap, `pingPong` reversal and the `patrol_direction = 1` seed from both a fresh
`from_graph` brain and a serde-default-deserialized one, single-point degenerate, persistence
across re-entry, an out-of-range persisted `patrol_cursor` clamped on load, and an untargeted
enemy holding `patrol` under an any-state stand-down while its cursor advances); the
`moveToAnchor` arrival not latched (a post-arrival push past epsilon re-issues `MoveTo`); the
position-goal facing-when-stopped case (an arrived — or blocked pre-move — `moveToAnchor`/`patrol`
enemy holding a nearby pawn in `outcome.target` faces *nothing*, not the pawn); the
authored-arrival-guard wedge (a guard threshold `<` epsilon never exits); the faction gate
(fresh-scan acquisition filter, no friendly acquisition, the authored `targetHostile` stand-down
dropping a flipped-to-friendly target, the trigger/touch vs `on_impact` faction-write ordering
seam — a trigger/touch `@state.faction` write lands before `run_ai_tick`, so the candidacy scan
and `targetHostile` read it the *same* tick, whereas an `on_impact` write inside the AI apply
pass is read by all brains the *next* tick uniformly (the compute pass completes before any
apply-pass mutation, so there is no intra-tick enemy-order dependence) — and default-seed
behavior identity); the reachability cache reused across a retained target's
movement; and the alloc-probe (`refresh_and_guard_eval_perform_zero_heap_allocations`) on a between-strides
tick with the three new fixed slots populated (the due-tick `find_path` allocation is out of
the zero-alloc contract). Update the
reference-enemy identity pins in `crates/scripting-core/src/data_descriptors/tests/behavior.rs`
(`the_shipped_reference_enemy_descriptor_is_identical_in_both_authorings`) to the new
authored shape — the pins are hand-written, so re-derive them from the rewritten reference
rather than silencing them. AC 10 (host-only,
deterministic, no wire) is covered by the existing sim/net determinism suite staying green plus
the by-construction host-only fields; add a determinism assertion over a retreat/patrol tick to
this list to pin it. Verify the agent diagnostics overlay renders the new authored state names
(`patrol`, `retreat`, `waiting`) — it reads `brain.state_name()` dynamically
(`agent_diagnostics.rs`), with no hardcoded state list, so the names already surface with no code
change; confirm this rather than hunting for a map to edit. Update
`docs/scripting-reference.md` with the new motion verbs, the three facts, the arrival-guard
`>= POSITION_GOAL_ARRIVAL_EPSILON` rule, the faction hook, and the recommended attack-map grammar
(see Coordination). The faction docs must frame `@state.faction` as the *interim* numeric hostility
input — an opaque identity token, hostility being a differing identity — whose durable authored
contract is the `targetHostile`/`hostile` fact, with named alliances / diplomacy roadmapped in
**Faction & relationship model**; so no authored content builds a permanent dependency on the
numeric surface (the forward-compat proviso the roadmap's migration relies on). Regenerate and commit both typedef fixtures.

### Coordination — attack-map grammar seam, multi-attack, stagger, weapon model

This spec surfaces one grammar recommendation it does not itself ship. It is not asserted
from here onto the other drafts — the owner records it in `E10--enemy-multi-attack` (the
ship vehicle) or a shared context doc, so the multi-attack and stagger drafts land
consistently on it:

- **Recommended attack grammar.** The attack tuning is a **named map**
  `components.behavior.attacks: BTreeMap<String, AttackParams>` and the action verb is
  **parameterized** `action: { attack: "<name>" }` naming a map entry — no privileged
  singular entry, matching the composition shape's "named map, reference by name." No bare
  `action: "attack"` and no singular `attack` block in the recommended grammar. *Type-name
  seam:* this spec names the source-true type `AttackParams` (`data_descriptors/types/behavior.rs`),
  but `E10--enemy-multi-attack` calls it `AttackTuning` throughout — the recommendation
  transcribes to that draft's `AttackTuning`; the two names denote the same type.
- **`E10--enemy-stagger` reconciliation.** Its examples use the singular `action: "attack"`
  and `attack: { ... }` block; both would migrate to `action: { attack: "<name>" }` and an
  `attacks` map when the map lands. Nothing else in stagger conflicts — its `@state.staggered`
  interrupt, `hold`-motion flinch state, and commitment-window guard are unaffected, and it
  may freely add `distanceFromAnchor`/`targetReachable` guards (e.g. stagger-then-retreat).
- **`E10--enemy-multi-attack` reconciliation.** Its Task 1 already defines the `attacks`
  map and `action: { attack }` — it is the ship vehicle for this grammar. The recommendation
  for it to record: (a) keep the map canonical with no singular fallback; (b) extend
  `AttackParams` additively with `weapon`/`minRange`/`maxRange`/`ResolutionMode` (contact
  preserved as today's behavior); (c) redefine the `engagementRadius()` fallback, which today
  reads the singular `attack.range` — with a map it would fall back to the **largest entry
  `range`/`maxRange`**, else the 2 m default; its open question ("positioning reads the
  current firing state's `maxRange`") reconciles to this. This spec leaves
  `AttackParams`/`ActionVerb` untouched so multi-attack owns the migration; the reference
  enemy keeps its singular `attack` block until then.
- **Weapon-model direction (enemy-as-wielder).** A named attack entry may reference a weapon
  descriptor (`AttackParams::weapon`, multi-attack), making the enemy the *wielder* of the
  same weapon-descriptor substrate player weapons use — not a melee-vs-ranged slot. This
  spec's grammar leaves `AttackParams` extensible for that field; it builds none of it.

## Sequencing

**Phase 1 (sequential):** Task 1 — the split unblocks every later tick edit.
**Phase 2 (sequential):** Task 2 — thin slice; falsifies the add-a-brain-fact boundary
(foundation table → validation scope → runtime scope → `BrainFacts` → `BrainComponent` →
spawn seed → SDK prelude → drift fixtures) before it is repeated.
**Phase 3 (sequential):** Task 3 — motion verbs; consumes Task 2's `home_anchor` and
`distanceFromAnchor`. Shares the compute/apply passes with Task 4.
**Phase 4 (sequential):** Task 4 — faction hook; shares the compute pass's target-selection
region with Task 3, so it follows rather than races it, and appends `@brain.targetHostile`
at `BRAIN_INPUTS` index 11, which must follow Task 2's index-10 append (append-only).
**Phase 5 (sequential):** Task 5 — reachability fact; appends `BRAIN_INPUTS` index 12 after
Task 4's index 11. The fact ships here (it faithfully reports `find_path`); only its reference
*demo* (Task 6) waits on `E10--pursuit-wraparound-blocked`.
**Phase 6 (sequential):** Task 6 — consumes the settled vocabulary; the reachability demo
half waits on Task 5's nav dependency.

Cross-spec: `E10--enemy-multi-attack` and `E10--enemy-stagger` reconcile onto the
recommended grammar (Coordination) as a separate follow-up, not part of this spec.

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| Motion verb (new) | `MotionVerb::{MoveToAnchor, Patrol}` | `"moveToAnchor"` / `"patrol"` | `"moveToAnchor" \| "patrol"` (union) | same | n/a |
| Patrol block | `BehaviorGraphDescriptor::patrol: Option<PatrolDescriptor>` | `"patrol"` | `patrol?` | `patrol?` | n/a |
| Patrol points | `PatrolDescriptor::points: Vec<[f32;2]>` | `"points"` (array of `[x, z]`) | `points: [number, number][]` | same | n/a |
| Patrol mode | `PatrolMode::{Loop, PingPong}` | `"loop"` / `"pingPong"` | `"loop" \| "pingPong"` | same | n/a |
| Distance-from-anchor fact | `BRAIN_INPUTS[10]` `@brain.distanceFromAnchor` (Number) | — | `brain.distanceFromAnchor` | same | n/a |
| Target-hostile fact | `BRAIN_INPUTS[11]` `@brain.targetHostile` (Bool) | — | `brain.targetHostile` | same | n/a |
| Target-reachable fact | `BRAIN_INPUTS[12]` `@brain.targetReachable` (Bool) | — | `brain.targetReachable` | same | n/a |
| Home anchor (sim state) | `BrainComponent::home_anchor: Vec3` | serde `home_anchor`, default `ZERO` | — (engine-internal) | — | n/a |
| Patrol cursor (sim state) | `BrainComponent::{patrol_cursor, patrol_direction}` | serde-default | — | — | n/a |
| Reachability cache (sim state) | `BrainComponent::target_reachable: bool` | serde default / skip (recomputed) | — | — | n/a |
| Faction leaf | `@state.faction` via `EntityStateComponent` | — | `state("faction")` (existing E16 write path) | same | n/a |
| Attacks map (recommended, not shipped) | `BehaviorGraphDescriptor::attacks: BTreeMap<String, AttackParams>` | `"attacks"` | `attacks: Record<string, …>` | same | n/a |
| Attack action param (recommended) | `ActionVerb::Attack { attack: String }` | `action: { "attack": "<name>" }` | `action: { attack: string }` | same | n/a |

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| `BRAIN_INPUTS` is append-only and the runtime `BrainScope.fixed` array length equals the table length, so table and refresh grow together in index order | Task 2 (index 10), Task 4 (index 11), Task 5 (index 12) | Any new fact must append and add its refresh slot in the same edit, or the crate will not compile; `expected_fixed_value` (no `_` arm) and the resolution-parity drift test guard it | AC 8, 9 |
| Facts are read-only; faction is the only new mutable `@state`, written solely through the E16 `setState`/`entity_state_mut` path, never by a guard | Task 4 | Any guard-side write; any new reaction claiming faction | AC 6 |
| Hostility filters fresh acquisition only, on the ranking scan (`entity_model.md` §7c); retention is stood down by an authored interrupt over `@brain.targetHostile`, never re-gated by candidacy | Task 4 (`nearest_target_candidate` filter + `@brain.targetHostile` fact) | Any move of the hostility test into the shared `target_candidate` gate — which the retained lookup also calls — would re-gate retention against §7c | AC 6 |
| No-target / no-nav conventions: `distanceFromAnchor` always meaningful; `targetHostile` `false` untargeted; `targetReachable` `false` untargeted or with no navmesh, with the absent-nav result taking priority over a cache | Task 2, Task 4, Task 5 | The refresh must apply these before eval, matching the `BRAIN_NO_TARGET_DISTANCE` sentinel precedent | AC 1, 5, 6 |
| All new state (anchor, patrol cursor, reachability cache, faction) is host-only sim state; no wire/replication field; clients see animation-state names only | Task 2, 3, 4, 5 | Any snapshot/replication edit that serializes these onto the wire | AC 10 |
| Leash is authored, not an engine field — expressed via `distanceFromAnchor` guards | Task 3, Task 6 | Any reintroduction of a `leashRange`-style field re-forecloses behavior-state-graph's decision | AC 4 |
| Guard window is zero-alloc on a between-strides tick — anchor distance and cached reachability feed `refresh` without allocating (refresh writes slots by index) | Task 2, Task 5 | The due-tick `find_path` runs A* and allocates a `NavPath`; that allocation is explicitly outside the zero-alloc contract, so the alloc-probe measures a non-due tick where the cached verdict is reused | AC 9 |
| Exactly one host anchor-seed site — every host gameplay spawn funnels `home_anchor` through the assembly seed (`attach_descriptor_components` plus the `spawn_enemy` test helper) | Task 2 | Any future host brain-attach added outside that path would read distance-from-world-origin (a `ZERO` anchor) | AC 1 |

## Orderings

| Scenario | Ordering | Expected outcome |
|---|---|---|
| Enemy at its spawn point, `moveToAnchor` state | `distanceFromAnchor == 0 <= POSITION_GOAL_ARRIVAL_EPSILON` | `SteeringIntent::Clear`; stands, holds facing; no `MoveTo` issued |
| `moveToAnchor` arrival, then external displacement past epsilon | arrival tick `Clear`; a push (separation/mover/knockback) moves it past epsilon next tick | arrival is NOT latched — `Clear` on any tick within epsilon, `MoveTo(anchor)` re-issued on any tick `distanceFromAnchor > epsilon` |
| Authored arrival guard threshold `<` engine epsilon | resolver `Clear`s at `POSITION_GOAL_ARRIVAL_EPSILON`, guard needs a smaller distance | state wedges — enemy stands, never reaches the smaller threshold, never exits; spec forbids an arrival guard `<` epsilon |
| Untargeted enemy in `patrol` under an any-state stand-down interrupt | stand-down `to == patrol == current`, skipped (`select_transition`) | brain stays in `patrol`; cursor advances over 4 untargeted ticks near home — not `idle↔patrol` alternation |
| Persisted `patrol_cursor >= points.len()` on load | descriptor shrank `points` since the save | resolver clamps (`cursor` → in-range), issues `MoveTo(anchor + points[clamped])`, no panic |
| `pingPong` from a fresh `from_graph` / default-deserialized brain | `patrol_direction` seeded `+1` on both paths | cursor advances `0 → 1` (direction non-zero), tested from both spawn and deserialize |
| Patrol, two coincident points | arrival test true every tick | cursor advances every tick (cycles); accepted — authoring artifact, not a crash |
| Patrol `pingPong` at an endpoint | cursor reaches first/last point | direction reverses; next goal steps back inward |
| Patrol, single point | one point, arrival true | degenerates to standing at that point (no advance) |
| Leave then re-enter `patrol` | cursor persists across the intervening states | resumes at the retained cursor (return-to-patrol continues the route) |
| Empty `points` + `patrol` motion | parse time | validation error, pathed |
| Reachability cache vs a retained target's movement | retained target moves reachable→unreachable between acquisition strides | `targetReachable` holds the last due-tick verdict up to one stride band; refreshed next due tick — accepted |
| Reachability, no navmesh on the map | `nav_graph == None`, including a retained target on a non-due tick with a restored cache | reads `false`, clears the cache immediately, and runs no probe |
| Reachability, target lost | `hasTarget` false this tick | reads `false`; cache cleared |
| Alloc probe, due tick with navmesh + target | acquisition-due → `find_path` runs | allocations occur (A* `NavPath`); zero-alloc holds only between strides — no-nav ticks also skip the probe |
| Retreat, target lost on a non-due tick | `retreat` non-engaged drops the target; non-due tick → `hasTarget` false | any-state `not hasTarget` stand-down (targets `patrol`) fires → routes to `patrol`, continues toward home; not stranded in place |
| Faction flips to friendly mid-chase | `@state.faction` written == enemy faction; next guard eval | `@brain.targetHostile` reads `false`; the authored `select(targetHostile, false, true)` stand-down drops the retained target; the fresh scan never re-offers it |
| Faction write source vs the AI tick | game-logic order: trigger dispatch / touch → `run_ai_tick` → `agent_steering::tick` (`sim/mod.rs`) | a trigger-command / touch-reaction `@state.faction` write lands before the tick — read the same tick by the candidacy scan and `targetHostile`; an `on_impact` write inside the AI apply pass is read by ALL brains the NEXT tick uniformly (the compute pass completes before any apply-pass mutation — no intra-tick enemy-order dependence) |
| Enemy moved by a script | transform changes, `home_anchor` unchanged | `distanceFromAnchor` grows; home stays the spawn point |
| `patrol` exited or entered this tick | steering resolver runs on the post-transition state's motion | on the tick `patrol` is exited, the cursor does not advance; on the tick `patrol` is entered, the resolver runs, so the cursor may advance on entry if already within epsilon of `points[cursor]` |
| Target-side-fact guard on a non-engaged state, distant target (stride > 1) | due tick vs non-due tick | due tick: `targetReachable`/`targetHostile`/`hasTarget` reflect the selected target; non-due tick: all read their no-target sentinel — an unengaged-state guard over them needs an `acquisitionDue` conjunction |
| Retreat homeward path by target proximity | near/leading target stays engaged every tick vs. far/lost target dropped on the first non-due tick | near: `retreat` walks to the exact anchor, then `retreat → patrol`; lost: drops `retreat → patrol` and steers toward `home_anchor + points[cursor]` (the patrol region near home, not the anchor) |
| Faction-write scan cadence, unengaged vs engaged enemy | unengaged enemy scans every tick; engaged runs the filtered scan only on acquisition-due ticks | an unengaged enemy sees a same-tick faction write and may fresh-acquire this tick; an engaged enemy first sees a newly-hostile third pawn on its next due tick (retention never re-gated) |
| Persisted `patrol_cursor >= points.len()` on load, resumed index | clamp is `cursor % points.len()` | resumes at `cursor % len` (route phase preserved), not index 0 |

## Script syntax examples

```ts
// Proposed design
import { defineEntity, brain, runtime } from "postretro";

export const sentry = defineEntity({
  canonicalName: "sentry",
  components: {
    health: { max: 60 },
    mesh: { /* model, animation states incl. "walk", "idle" */ },
    behavior: {
      initial: "patrol",
      moveSpeed: 3,
      attack: { damage: 8, range: 2, cooldownMs: 1200 }, // singular until multi-attack lands the map
      engagementRadius: 2,
      // Deterministic anchor-relative patrol route; the anchor is this entity's
      // spawn position, so the same descriptor patrols correctly wherever it is
      // map-placed. `patrol` is the untargeted-active RESTING state: the two
      // any-state stand-downs below target it, so they are skipped here
      // (to == current) instead of yanking the enemy out every tick.
      patrol: { mode: "pingPong", points: [[0, 0], [6, 0], [6, 6]] },
      interrupts: [
        // Any-state stand-downs route to the untargeted resting state `patrol`,
        // NOT `idle`: a stand-down whose `to` is not the state the enemy rests in
        // re-fires every tick that state is live (idle<->patrol oscillation).
        { to: "patrol", when: runtime.select(brain.hasTarget, false, true) },
        // Authored retention drop: stand down when the retained target is no
        // longer hostile (faction flipped friendly) — the `targetDied` *shape*, but
        // the polarity DIFFERS: select(targetHostile, false, true) reads `true`
        // untargeted, so it fires on untargeted-OR-friendly (unlike targetDied,
        // which reads false untargeted). It is correct only because the
        // `not hasTarget` stand-down above shares this `to` and wins first-true-wins
        // on untargeted ticks — so this row MUST stay below it and target the same
        // state. Reordering it, or lifting it without the hasTarget row, silently
        // breaks stand-down.
        { to: "patrol", when: runtime.select(brain.targetHostile, false, true) },
      ],
      states: {
        // Untargeted: walk the patrol route; acquire on a fresh scan.
        patrol: {
          animation: "walk", motion: "patrol",
          transitions: [{
            to: "alert",
            // acquisitionDue conjunction: the reference-enemy idiom
            // (sdk/behaviors/reference/entities.ts) — detection is
            // time-sliced by the engine's think stride.
            when: runtime.select(brain.acquisitionDue, runtime.le(brain.targetDistance, 16), false),
          }],
        },
        alert: {
          animation: "walk", motion: "chaseTarget",
          transitions: [
            { to: "attack",  when: runtime.le(brain.targetDistance, 2) },
            // Leash: authored, not an engine field.
            { to: "retreat", when: runtime.gt(brain.distanceFromAnchor, 20) },
            // Barrier hold (add once the wraparound nav fix lands):
            // { to: "waiting", when: runtime.select(brain.targetReachable, false, true) },
          ],
        },
        attack: {
          animation: "attack", motion: "chaseTarget", action: "attack",
          transitions: [
            { to: "alert",   when: runtime.gt(brain.targetDistance, 2) },
            { to: "retreat", when: runtime.gt(brain.distanceFromAnchor, 20) },
          ],
        },
        // Retreat-to-start: walk home, then resume patrol. `retreat` is
        // non-engaged, so it holds no target across strides — its only exit is
        // `distanceFromAnchor`-based (or the any-state stand-down to `patrol`),
        // never a target-distance edge. `arrivalEps` (1) must be
        // >= POSITION_GOAL_ARRIVAL_EPSILON or the state wedges: the resolver
        // Clears at the epsilon and never reaches a smaller authored threshold.
        retreat: {
          animation: "walk", motion: "moveToAnchor",
          transitions: [
            { to: "patrol", when: runtime.le(brain.distanceFromAnchor, 1) },
          ],
        },
      },
    },
  },
});
```

## Open questions

- **Faction forward-compatibility — decided (factions roadmapped, low priority).** The owner
  confirms named factions / per-pair diplomacy are wanted on the roadmap — not a priority for
  the first game, but PostRetro is a substrate for community-built games, so the full model must
  not be foreclosed. Already recorded as the **Faction & relationship model** roadmap item
  (`roadmap.md`), which names E10's faction seed + `hostile`/`targetHostile` facts as the durable
  contract the numeric storage migrates beneath. The minimal hook ships a
  numeric `@state.faction` with a differ-means-hostile rule over the player-pawn targetable
  set; the full model (named alliances, neutrality, per-pair diplomacy, an initial-faction
  declaration surface, `@candidate.faction`, and enemy-vs-enemy infighting) is the **Faction &
  relationship model** roadmap spec (Epic 10), where the widened-candidacy broad-phase also
  lives, gated on measured need. Forward-compatibility holds by construction: the durable
  authored contract is the `hostile` / `targetHostile` fact, and numeric `@state.faction` is
  interim storage that migrates beneath it (to a relation table, differ-means-hostile becoming
  the default relation) — provided the numeric leaf stays framed as the interim *input* to the
  fact, not published as the permanent allegiance surface. That proviso is the one discipline
  that keeps the door open, and it is enforced by Task 6's docs requirement (frame `@state.faction`
  as an opaque interim identity token whose durable contract is `targetHostile`), so the minimal
  hook is forward-compatible either way and this does not gate promotion. When `@candidate.faction`
  lands, acquisition narrowing migrates to the authored candidate filter and the engine floor
  keeps only the seed and the retention-side `@brain.targetHostile` read, with authored
  range-limiting staying on the acquire transition (detection) rather than the filter (candidacy)
  so the two ranges stay independently authorable — a sentry may be targetable at one range yet
  only engage from patrol at a tighter one.
- **Reachability: fact ships, demo waits (decided).** `@brain.targetReachable` is the nav
  floor's `find_path` verdict when a nav graph exists and `false` otherwise, so it ships with
  this spec — not a ground-truth oracle. It inherits pursuit's wraparound
  false-negative around freestanding walls until `E10--pursuit-wraparound-blocked` lands; only
  the authored reference *demo* (the `waiting` barrier-hold) waits on that fix, per AC 11.
  The same rule settles the softer `E10--mandatory-vertex-wedge-escapes` dependency: it keeps a
  chase-to-nearest-reachable barrier *hold* from jittering, so the demo waits on it too (a feel
  fix), while the fact does not. General rule: the fact ships; an authored reachability *demo*
  waits on whatever nav fix its feel needs.
- **State-scoped interrupts — deferred to statecharts.** The general fix for multiple
  untargeted-active resting states (an interrupt carrying the states it applies to or excludes)
  is the **Hierarchical behavior (statecharts)** roadmap spec's `"*"` scoped transitions; the
  single-resting-state convention holds here (search is out of scope, so one resting state
  suffices), and the first real consumer arrives with perception. A companion `behavior_lints`
  finding — an any-state `not hasTarget` / `not targetHostile` stand-down whose `to` is not the
  graph's untargeted-active resting state — could catch the oscillation at authoring time, but
  identifying that state statically is not clearly cheap; noted, not scoped.
- **Hierarchical composites — forward compatibility (owner, before promotion).** The flat
  state model is v1; the endpoint is a recursive statechart — composite activities, orthogonal
  layers, scoped transitions — owned by the *Hierarchical behavior (statecharts)* roadmap spec
  (Epic 10) and explored in this spec's `sample.ts`. Two per-activity migrations are deferred
  to it rather than pulled forward, keeping E10 minimal and the shipped `moveSpeed` contract
  untouched: per-activity `route` (from the graph-wide `patrol` block — E10-new, bounded) and
  per-activity `speed` (from the shipped graph-wide `moveSpeed` — a contract migration touching
  descriptor, validation, SDK, and replication, *not* cheap), each requiring per-state patrol
  cursors. E10 authors the graph-wide forms. Confirm nothing in the flat shape forecloses either
  migration or the composite envelope; the any-state `interrupts` grammar is the flat precursor
  of that spec's `"*"` scoped transitions — additively generalized, not replaced.
</content>
