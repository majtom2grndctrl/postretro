# Enemy Behavior Descriptor Syntax

## Goal

Grow the `components.behavior` vocabulary so the roadmap's leash/pursuit
behaviors — retreat-to-start, patrol-area / return-to-patrol, unreachable-target
behavior, and chase-to-nearest-reachable / barrier behavior — become *expressible*
authored content on the shipped behavior-state-graph, not new hardcoded archetypes.
It adds position-goal motion verbs (a spawn anchor to move toward, a patrol route
to walk), the brain facts that guard them (distance-from-home, target
reachability), a minimal mutable-hostility hook so candidacy consults a per-entity
faction relation instead of hardcoding targets to player pawns, and applies the
composition shape the player descriptor set. Today the motion vocabulary is only
chase/hold/freeze and no position/home/reachability fact exists — these behaviors
cannot be written at all, and closing that gap is the work.

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
- **New brain facts** (append-only at `BRAIN_INPUTS` indices 10, 11):
  `@brain.distanceFromAnchor` (Number; the enemy's XZ distance from `home_anchor`,
  always meaningful, `0` at the anchor) and `@brain.targetReachable` (Bool; whether
  the nav floor can path enemy→selected-target this tick, `false` untargeted, `true`
  with no navmesh). Each lands in the foundation table + `BrainValidationScope`, the
  runtime `BrainScope` refresh, `BrainFacts`, and the SDK `brain` prelude (TS + Luau).
- **Minimal faction hook.** Candidacy (`target_candidate`, the gate both fresh
  acquisition and retained-target lookup pass) admits a pawn only when the evaluating
  enemy is hostile to it: `faction(enemy) != faction(candidate)`, faction read from
  the E16 `@state.faction` leaf (absent → 0.0). Enemies are seeded faction 1 at spawn,
  players read 0, so all existing targeting is unchanged. Faction is *mutated* through
  the existing E16 `@state` write path — this spec adds only the engine-floor read and
  the spawn seed.
- **Composition shape.** Applied per `player-descriptor-composition`: one transition
  grammar (already the `{to, when}` rows), closed engine vocabulary, data-only, new
  tuning added as composed graph-wide blocks (`patrol`) rather than flat flags. The
  named-attack-map instance of the "shared defaults + sparse override" pattern is
  *pinned* (see Coordination), landed by `E10--enemy-multi-attack`.
- **Split `ai/mod.rs`** (982 production lines) before extension: extract the facing
  helpers and combat-slot resolution into sibling modules.
- Reference enemy authored with a retreat-to-start + patrol demonstration; agent
  diagnostics overlay labels the new states; `docs/scripting-reference.md` documents
  the new verbs/facts/faction and the pinned attack-map grammar.

### Out of scope

- **The full faction / relationship model** — named alliances, neutrality, per-pair
  diplomacy, a declaration surface for initial faction, `@candidate.faction` in the
  authored candidate-filter IR, and enemy-vs-enemy infighting (broadening the
  targetable-kind set beyond `PlayerMovement`). Deferred to its own research→spec
  pass; the perf broad-phase for a widened candidacy set lives there. See Open questions.
- **The `attacks` named map + parameterized `attack` action verb** — grammar pinned
  here (Coordination), shipped by `E10--enemy-multi-attack`. This spec does not touch
  `AttackParams` or `ActionVerb`.
- **Random / wander patrol** — needs the seeded-deterministic-RNG story `E10--enemy-stagger`
  also defers; `patrol` is deterministic ordered points only.
- **A runtime-movable home anchor** — the anchor is the spawn position for the brain's
  life; re-homing is not expressible. Noted in Open questions.
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
(the two facts append at the tail). The engine-owned candidacy floor stays engine-owned
— the faction hook extends the floor's *rule* (hostility) without moving candidacy into
graph content. One divergence, argued: behavior-state-graph's "candidate filters admit
fresh candidates only; retention is graph policy" rule is *not* extended to faction —
hostility gates **both** acquisition and retention, because hostility is a target-validity
property of the floor (kin to `selected_target_alive`), not an acquisition-narrowing
policy; a target whose faction flips to friendly mid-chase must be dropped by the floor,
not left for the graph to notice. The player-descriptor-composition shape is applied.
The `E10--enemy-multi-attack` map grammar is pinned and coordinated, not overridden.

**Alternatives rejected.** The strongest rival is an engine-floor leash: a `leashRange`
scalar plus hardcoded retreat/patrol steering that reproduces these behaviors as engine
policy. Rejected on two counts — behavior-state-graph already foreclosed `leashRange`,
and it is "another hardcoded archetype," exactly what the roadmap says not to build; the
vocabulary-growth path keeps every behavior authored content. A second rival expresses
anchor/patrol entirely in graph content (author stores home in `@state.homeX/homeZ`,
does position math in IR): rejected because the IR has no vector type and no way to read
the spawn transform, and per-tick position arithmetic in `select` trees is unauthorable
in practice — a first-class anchor fact and verb are cleaner and the engine already holds
the position.

## Acceptance criteria

- [ ] **distanceFromAnchor.** A graph guarding on `@brain.distanceFromAnchor` fires
  when the enemy's XZ distance from its spawn position crosses the authored threshold;
  the fact reads `0` at the spawn point, is available with no selected target, and both
  runtimes accept a guard that binds it (Number). The value tracks the *spawn* position
  even after the entity is moved by a script.
- [ ] **moveToAnchor.** An enemy in a `moveToAnchor` state steers toward its spawn
  anchor and faces its direction of travel while moving; within the arrival epsilon it
  clears its destination and stands (no further `MoveTo` destination issued), and holds
  facing. Verified at the start (moving), the arrival tick (destination cleared), and a
  later stopped tick (still cleared).
- [ ] **patrol.** An enemy in a `patrol` state visits the authored anchor-relative
  points in order; `loop` wraps the cursor to 0 past the last point, `pingPong` reverses
  direction at each end, a single-point route degenerates to standing at that point, and
  the cursor persists across leaving and re-entering the patrol state (return-to-patrol
  resumes rather than restarting). `patrol` motion with no `patrol` block, and an empty
  `points` list, are parse errors with pathed messages in both runtimes.
- [ ] **Retreat-to-start, end to end.** A fixture graph with `chase → retreat` on
  `gt(distanceFromAnchor, leash)`, `retreat` using `moveToAnchor`, and `retreat → idle`
  on `le(distanceFromAnchor, arrivalEps)` walks the enemy home when a target leads it
  past the leash, then idles; a target re-entering range *during* retreat re-engages
  via an authored `retreat → alert` edge.
- [ ] **targetReachable.** With a selected target the fact reads whether the nav floor
  can path enemy→target; it reads `false` with no target and `true` with no navmesh
  present; it is recomputed on the think-stride acquisition cadence and a between-strides
  tick reuses the cached value. An authored `select(targetReachable, false, true)` guard
  routes to a hold state when the target is unpathable and back when it becomes pathable.
- [ ] **Faction hostility gate.** An enemy acquires and retains a `PlayerMovement` pawn
  only while hostile to it per `@state.faction`; a pawn whose faction is written (through
  the E16 `@state` path) to equal the enemy's faction is dropped on the next candidacy
  scan and never freshly acquired, and reverting it re-enables targeting. With default
  seeding (enemies faction 1, players 0) the full existing AI test suite passes unchanged.
- [ ] **Split.** `ai/mod.rs`'s facing helpers and combat-slot resolution move to sibling
  modules; its production line count drops below ~800; the full AI suite passes with
  import-only changes.
- [ ] **Both scopes, both facts.** `@brain.distanceFromAnchor` and `@brain.targetReachable`
  resolve identically in `BrainValidationScope` and the runtime `BrainScope` (drift test),
  occupy `BRAIN_INPUTS` indices 10 and 11, and appear in the SDK `brain` prelude in both
  runtimes (SDK drift test green).
- [ ] **Zero-alloc preserved.** Snapshot refresh plus guard eval — now including the two
  new fixed slots and the anchor/reachability computation feeding them — performs zero
  heap allocations (alloc-probe).
- [ ] **Host-only, deterministic, no wire change.** Anchor, patrol cursor, reachability
  cache, and faction are host-only sim state; sim determinism tests stay green; a
  connected client observes only replicated animation-state names, with no new wire field.
- [ ] **Reference + diagnostics.** The reference enemy is authored with the retreat +
  patrol demonstration (the reachability demo gated on `E10--pursuit-wraparound-blocked`),
  the agent diagnostics overlay shows the new authored state names, and both typedef
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
to `mod.rs` and re-export the `pub(crate)` items the tick and `ai_tests.rs` use
(`FACING_TURN_RATE`, `slew_yaw`) so call sites change imports only. No behavior change; the full AI suite stays green. This lands first because every
later task edits the tick and both extracted clusters.

### Task 2: Thin slice — the `@brain.distanceFromAnchor` fact end to end

The narrow vertical path that falsifies the "add a brain fact" boundary the rest of the
spec repeats. Add `home_anchor: Vec3` to `BrainComponent` (`crates/entities/src/components/brain.rs`,
`#[serde(default)]` → `Vec3::ZERO` for pre-existing saves) and seed it at the spawn site
(`builtins/data_archetype.rs:582-590`, the block that already reads the brain to set
`aggro_armed`) from the entity's `Transform.position`. Append `@brain.distanceFromAnchor`
(Number) to `BRAIN_INPUTS` at **index 10** in `crates/foundation/src/brain.rs` with its
`BRAIN_DISTANCE_FROM_ANCHOR_INPUT` const; `BrainValidationScope` resolves it automatically
through the table. In the runtime `BrainScope` (`ai/brain_scope.rs`): grow the `fixed`
array (it is `[IrValue; BRAIN_INPUTS.len()]`, so the length follows the table) and write
slot 10 in `refresh` from a new `BrainFacts.distance_from_anchor: f32`. Compute that value
in the tick's compute pass as `distance_xz(snap.position, brain.home_anchor)` — every
tick, no stride, no target needed. Append the leaf to `sdk/lib/brain.ts` and `brain.luau`
(the frozen `brain` object + `BrainInputs` interface) per their stated sync obligation.
Regenerate the committed SDK typedef fixtures and update the drift-test expectations in
`crates/scripting-core/src/data_descriptors/tests/behavior.rs`. The `expected_fixed_value`
oracle in `brain_scope.rs` tests (no `_` arm) forces a matching case.

### Task 3: Position-goal motion verbs and patrol authoring

Add `MoveToAnchor` and `Patrol` to `MotionVerb` (`data_descriptors/types/behavior.rs`),
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
`MoveTo(Vec3)` variant and a `POSITION_GOAL_ARRIVAL_EPSILON` constant. Replace the pure
`steering_for(motion)` call in the compute pass with a resolver that, for `MoveToAnchor`,
targets `home_anchor` and returns `Clear` once within the arrival epsilon else
`MoveTo(anchor)`; for `Patrol`, targets `home_anchor + points[cursor]`, advances the
cursor per `mode` when arrived, and returns `MoveTo(point)`. The cursor and direction are
new `BrainComponent` fields (`patrol_cursor: usize`, `patrol_direction: i8`, serde-default),
mutated in the compute pass (the brain is already `&mut` there); they persist across state
changes. `steering_for` stays an exhaustive `match` — the two new arms are a compile error
until added. In the apply pass, add a `SteeringIntent::MoveTo(goal)` arm that calls
`agent_steering::set_destination(goal)`. `MoveToAnchor`/`Patrol` states are not `engaged`
(no target stake), so they drop the acquired target and take no combat slot — correct for
a retreating/patrolling enemy. Extend `is_locomotion_state` to treat `MoveToAnchor`/`Patrol`
(no action) as locomotion so their travel clip yields to the rest animation at a standstill
and drives walk-playback scaling; note the existing multi-locomotion `locomotion_animation`
BTreeMap-order collapse now spans chase/patrol (a documented, host/client-consistent v1
limitation, not a desync). Extend the facing gate so an enemy under a position-goal
`SteeringIntent::MoveTo` faces its travel velocity while moving (it faces nothing when
arrived/stopped) — today facing is gated on `outcome.engaged`, which position-goal motion
is not.

### Task 4: The `@brain.targetReachable` fact and its nav probe

Append `@brain.targetReachable` (Bool) to `BRAIN_INPUTS` at **index 11** (after Task 2's
index 10) with its const, wired the same way as Task 2 through `BrainValidationScope`, the
`BrainScope` `fixed` array + `refresh` slot 11, a new `BrainFacts.target_reachable: bool`,
the SDK `brain` prelude (TS + Luau), the SDK typedef fixtures, and the drift tests. Compute
reachability in the compute pass: when the brain is armed, has a selected target, and the
tick is acquisition-due (reuse the existing `evaluate_acquisition` stride gate — do not add
a stride constant), call the nav floor's `find_path(nav_graph, snap.position, target_pos).is_some()`
and cache it on a new `BrainComponent.target_reachable: bool` field; between strides reuse
the cache. With no selected target the fact reads `false`; with no `nav_graph` (a map
without a navmesh) it reads `true`, preserving today's chase-degradation behavior. This is
the one per-enemy nav query added; it is strided and cached so an idle-strided distant
enemy pays it at most once per stride band, within the combat-positioning query budget.
**Sequenced after `E10--pursuit-wraparound-blocked`:** that draft fixes `find_path`
returning a false `None` for a routable wraparound around a freestanding wall; without it
`@brain.targetReachable` reads `false` for a target reachable behind a wall, so any authored
unreachable-behavior fires spuriously at every corner — the fact would ship a bug. Do not
land Task 4 or its reference demo (Task 6) before that fix.

### Task 5: Minimal faction hostility hook

Replace candidacy's hardcoded player-pawn admission with a hostility gate in
`ai/targeting.rs`. `target_candidate` (:30) — the single gate both the fresh scan
(`nearest_target_candidate`) and the retained lookup (`select_target`) call — additionally
reads the candidate's `@state.faction` (`registry.get_component::<EntityStateComponent>(entity)`,
absent → 0.0) and admits it only when it differs from the evaluating enemy's faction, so
both acquisition and retention are gated in one edit. Plumb the enemy's faction scalar
through `select_target` → `nearest_target_candidate` → `target_candidate` (a new `f32`
parameter; the enemy's faction is read once per enemy in the compute pass from its
`EntityStateComponent`). Seed the enemy default faction at the spawn site
(`builtins/data_archetype.rs:582-590`, same block as Task 2's anchor seed):
`registry.entity_state_mut(id).set(FACTION_STATE_FIELD, ENEMY_DEFAULT_FACTION)` where
`ENEMY_DEFAULT_FACTION = 1.0` and `FACTION_STATE_FIELD = "faction"`. Players carry no
`faction` key and read 0, so enemy(1) ≠ player(0) stays hostile and every existing test is
unchanged. The targetable-kind iteration set stays `[ComponentKind::PlayerMovement]` (a
named constant seam so the full model can widen it); this keeps candidacy's iteration and
perf identical to today and adds only an O(1) hostility test per candidate. Faction is
mutated entirely through the shipped E16 `@state` write path — no new reaction or output.

### Task 6: Reference authoring, tests, diagnostics, docs

Author the reference enemy (`sdk/behaviors/reference/entities.{ts,luau}`, both spellings
identical) to demonstrate the new vocabulary: a `retreat` state (`moveToAnchor`) reached
from `alert`/`attack` on `gt(distanceFromAnchor, leash)` and exiting to `idle` on
`le(distanceFromAnchor, arrivalEps)` and to `alert` on target re-entry, plus a `patrol`
block and an `idle → patrol` edge for the untargeted case. Its comments are the de-facto
authoring docs — they must teach that the anchor is the spawn position, that leash is a
`distanceFromAnchor` guard (not an engine field), and the patrol cursor's persistence. The
reachability demo (a `waiting` state on `select(targetReachable, false, true)`) is added
only once Task 4's nav dependency lands; until then the reference ships without it and the
docs note the fact exists. Port/extend `ai_tests.rs`: retreat round-trip (leash exit,
arrival, re-engage edges), patrol cursor (loop wrap, pingPong reversal, single-point
degenerate, persistence across re-entry), the faction gate (drop-on-flip, no fresh
acquisition, default-seed behavior identity), and the alloc-probe with the two new fixed
slots populated. Update the agent diagnostics overlay to label the new states. Update
`docs/scripting-reference.md` with the new motion verbs, the two facts, the faction hook,
and the **pinned attack-map grammar** (see Coordination). Regenerate and commit both
typedef fixtures.

### Coordination — attack-map grammar seam, multi-attack, stagger, weapon model

This spec pins one grammar decision it does not itself ship, so the multi-attack and
stagger drafts land consistently on it:

- **Canonical attack grammar.** The attack tuning is a **named map**
  `components.behavior.attacks: BTreeMap<String, AttackParams>` and the action verb is
  **parameterized** `action: { attack: "<name>" }` naming a map entry — no privileged
  singular entry, matching the composition shape's "named map, reference by name." There
  is no bare `action: "attack"` and no singular `attack` block in the canonical grammar.
- **`E10--enemy-stagger` reconciliation.** Its examples use the singular `action: "attack"`
  and `attack: { ... }` block; both migrate to `action: { attack: "<name>" }` and an
  `attacks` map when the map lands. Nothing else in stagger conflicts — its `@state.staggered`
  interrupt, `hold`-motion flinch state, and commitment-window guard are unaffected, and it
  may freely add `distanceFromAnchor`/`targetReachable` guards (e.g. stagger-then-retreat).
- **`E10--enemy-multi-attack` reconciliation.** Its Task 1 already defines the `attacks`
  map and `action: { attack }` — it is the ship vehicle for this grammar. It must (a) keep
  the map canonical with no singular fallback; (b) extend `AttackParams` additively with
  `weapon`/`minRange`/`maxRange`/`ResolutionMode` (contact preserved as today's behavior);
  (c) redefine the `engagementRadius()` fallback, which today reads the singular `attack.range`
  — with a map it falls back to the **largest entry `range`/`maxRange`**, else the 2 m default;
  its open question ("positioning reads the current firing state's `maxRange`") reconciles to
  this. This spec leaves `AttackParams`/`ActionVerb` untouched so multi-attack owns the
  migration; the reference enemy keeps its singular `attack` block until then.
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
`distanceFromAnchor`. Shares the compute/apply passes with Task 5.
**Phase 4 (sequential):** Task 5 — faction hook; shares the compute pass's target-selection
region with Task 3, so it follows rather than races it.
**Phase 5 (sequential):** Task 4 — reachability fact; appends the second `BRAIN_INPUTS`
entry after Task 2's, and is **blocked on `E10--pursuit-wraparound-blocked`** landing.
**Phase 6 (sequential):** Task 6 — consumes the settled vocabulary; the reachability demo
half waits on Task 4's nav dependency.

Cross-spec: `E10--enemy-multi-attack` and `E10--enemy-stagger` reconcile onto the pinned
grammar (Coordination) as a separate follow-up, not part of this spec.

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| Motion verb (new) | `MotionVerb::{MoveToAnchor, Patrol}` | `"moveToAnchor"` / `"patrol"` | `"moveToAnchor" \| "patrol"` (union) | same | n/a |
| Patrol block | `BehaviorGraphDescriptor::patrol: Option<PatrolDescriptor>` | `"patrol"` | `patrol?` | `patrol?` | n/a |
| Patrol points | `PatrolDescriptor::points: Vec<[f32;2]>` | `"points"` (array of `[x, z]`) | `points: [number, number][]` | same | n/a |
| Patrol mode | `PatrolMode::{Loop, PingPong}` | `"loop"` / `"pingPong"` | `"loop" \| "pingPong"` | same | n/a |
| Distance-from-anchor fact | `BRAIN_INPUTS[10]` `@brain.distanceFromAnchor` (Number) | — | `brain.distanceFromAnchor` | same | n/a |
| Target-reachable fact | `BRAIN_INPUTS[11]` `@brain.targetReachable` (Bool) | — | `brain.targetReachable` | same | n/a |
| Home anchor (sim state) | `BrainComponent::home_anchor: Vec3` | serde `home_anchor`, default `ZERO` | — (engine-internal) | — | n/a |
| Patrol cursor (sim state) | `BrainComponent::{patrol_cursor, patrol_direction}` | serde-default | — | — | n/a |
| Faction leaf | `@state.faction` via `EntityStateComponent` | — | `state("faction")` (existing E16 write path) | same | n/a |
| Attacks map (pinned, not shipped) | `BehaviorGraphDescriptor::attacks: BTreeMap<String, AttackParams>` | `"attacks"` | `attacks: Record<string, …>` | same | n/a |
| Attack action param (pinned) | `ActionVerb::Attack { attack: String }` | `action: { "attack": "<name>" }` | `action: { attack: string }` | same | n/a |

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| `BRAIN_INPUTS` is append-only and the runtime `BrainScope.fixed` array length equals the table length, so table and refresh grow together in index order | Task 2 (index 10), Task 4 (index 11) | Any new fact must append and add its refresh slot in the same edit, or the crate will not compile; `expected_fixed_value` (no `_` arm) and the resolution-parity drift test guard it | AC 8, 9 |
| Facts are read-only; faction is the only new mutable `@state`, written solely through the E16 `setState`/`entity_state_mut` path, never by a guard | Task 5 | Any guard-side write; any new reaction claiming faction | AC 6 |
| Hostility gates both acquisition and retention (a target-validity property of the floor), diverging from candidate-filter's fresh-only rule | Task 5 (`target_candidate`) | A future refactor that moves the hostility test out of the shared `target_candidate` gate would re-split the two paths | AC 6 |
| No-target / no-nav conventions: `distanceFromAnchor` always meaningful; `targetReachable` `false` untargeted, `true` with no navmesh | Task 2, Task 4 | The refresh must apply these before eval, matching the `BRAIN_NO_TARGET_DISTANCE` sentinel precedent | AC 1, 5 |
| All new state (anchor, patrol cursor, reachability cache, faction) is host-only sim state; no wire/replication field; clients see animation-state names only | Task 2, 3, 4, 5 | Any snapshot/replication edit that serializes these onto the wire | AC 10 |
| Leash is authored, not an engine field — expressed via `distanceFromAnchor` guards | Task 3, Task 6 | Any reintroduction of a `leashRange`-style field re-forecloses behavior-state-graph's decision | AC 4 |
| Per-tick guard window stays zero-alloc — the anchor distance and cached reachability feed `refresh` without allocating | Task 2, Task 4 | The `find_path` probe runs before the armed alloc probe (strided, cached); refresh writes slots by index | AC 9 |

## Orderings

| Scenario | Ordering | Expected outcome |
|---|---|---|
| Enemy at its spawn point, `moveToAnchor` state | `distanceFromAnchor == 0 <= arrivalEps` | `SteeringIntent::Clear`; stands, holds facing; no `MoveTo` issued |
| `moveToAnchor` arrival tick | distance crosses below `arrivalEps` this tick | destination cleared once; not re-issued on later ticks in-state |
| Patrol, two coincident points | arrival test true every tick | cursor advances every tick (cycles); accepted — authoring artifact, not a crash |
| Patrol `pingPong` at an endpoint | cursor reaches first/last point | direction reverses; next goal steps back inward |
| Patrol, single point | one point, arrival true | degenerates to standing at that point (no advance) |
| Leave then re-enter `patrol` | cursor persists across the intervening states | resumes at the retained cursor (return-to-patrol continues the route) |
| Empty `points` + `patrol` motion | parse time | validation error, pathed |
| Reachability, target changes between strides | new target acquired on a non-due tick | cached reachability is one stride stale; refreshed next due tick — accepted |
| Reachability, no navmesh on the map | `nav_graph == None` | reads `true` (chase degrades as today), no probe run |
| Reachability, target lost | `hasTarget` false this tick | reads `false`; cache cleared |
| Faction flips to friendly mid-chase | `@state.faction` written == enemy faction, next candidacy scan | retained target dropped by `target_candidate`; not re-acquired |
| Faction flip on the same tick as acquisition | write ordered before the candidacy scan | scan reads the new value; write after → next tick |
| Enemy moved by a script | transform changes, `home_anchor` unchanged | `distanceFromAnchor` grows; home stays the spawn point |

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
      initial: "idle",
      moveSpeed: 3,
      attack: { damage: 8, range: 2, cooldownMs: 1200 }, // singular until multi-attack lands the map
      engagementRadius: 2,
      // Deterministic anchor-relative patrol route; the anchor is this
      // entity's spawn position, so the same descriptor patrols correctly
      // wherever it is map-placed.
      patrol: { mode: "pingPong", points: [[0, 0], [6, 0], [6, 6]] },
      interrupts: [
        { to: "idle", when: runtime.select(brain.hasTarget, false, true) },
      ],
      states: {
        // Untargeted: walk the patrol route.
        idle: {
          animation: "idle", motion: "hold",
          transitions: [
            { to: "alert", when: runtime.le(brain.targetDistance, 16) },
            { to: "patrol", when: runtime.le(brain.distanceFromAnchor, 8) },
          ],
        },
        patrol: {
          animation: "walk", motion: "patrol",
          transitions: [{ to: "alert", when: runtime.le(brain.targetDistance, 16) }],
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
        // Retreat-to-start: walk home, re-engage if the target closes again.
        retreat: {
          animation: "walk", motion: "moveToAnchor",
          transitions: [
            { to: "alert", when: runtime.le(brain.targetDistance, 8) },
            { to: "idle",  when: runtime.le(brain.distanceFromAnchor, 1) },
          ],
        },
      },
    },
  },
});
```

## Open questions

- **Full faction / relationship model — owner decision.** The minimal hook ships a
  numeric `@state.faction` with a differ-means-hostile rule over the existing
  player-pawn targetable set. The full model (named alliances, neutrality, per-pair
  diplomacy, a per-archetype initial-faction *declaration* surface — `EntityStateComponent`
  has none today — `@candidate.faction` in the authored candidate-filter IR, and
  enemy-vs-enemy infighting by widening the targetable-kind set, with the O(N²) candidacy
  broad-phase that requires) is a separate research→spec pass. Owner: confirm the numeric
  field and hostility rule are forward-compatible with the intended model before promotion,
  and decide whether infighting is wanted soon enough to co-design the broad-phase.
- **Reachability ship vs. the nav fix.** Task 4 is sequenced after
  `E10--pursuit-wraparound-blocked`. If that fix slips, the owner may prefer to ship the
  `@brain.targetReachable` fact with a documented "unreliable around freestanding walls"
  caveat and no reference demo, rather than block this spec. `E10--mandatory-vertex-wedge-escapes`
  is the softer dependency (it keeps a chase-to-nearest-reachable barrier *hold* from
  jittering); decide whether the barrier demo waits on it too.
- **Runtime-movable anchor.** The anchor is fixed at spawn. A future "re-home" (a guard-post
  rotation, a patrol leader relocating its followers) would need a write path to
  `home_anchor` — deferred; note whether any near-term content wants it.
</content>
