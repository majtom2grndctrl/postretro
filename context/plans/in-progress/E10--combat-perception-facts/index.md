# Combat Perception Facts

## Goal

Enemies act on what they remember, not only on what they currently see. A hit
they can't source, and a target that slips out of sight, both leave a
**last-known position** the enemy investigates. New authored brain facts —
recency of damage, recency of sight, distance to the remembered spot, and the
bearing a hit came from — let a behavior graph decide the reaction (engage,
search, startle, flee), while a new `moveToLastKnown` motion verb steers there.
Fixes the "I shot him and he ignored me" gap without baking any aggression
policy into the engine.

## Scope

### In scope

- Four appended `@brain.*` facts (read-only guard inputs): `timeSinceDamageMs`,
  `timeSinceTargetVisible`, `distanceToLastKnown`, `damageBearing`.
- A `last-known target position` memory on `BrainComponent`, written from two
  sources: the live position of a **visible target** each tick, and the
  **attacker's position** at the moment of a hit.
- Damage-recency and sight-recency timers on `BrainComponent`, aged by the AI
  tick, reset at their trigger, sentinel ("never") until first triggered.
- A `damageBearing` snapshot: the enemy-relative yaw a hit came from, captured
  at the damage chokepoint.
- One new closed-vocabulary motion verb `moveToLastKnown`: a **non-engaged
  position goal** (holds no target, takes no combat slot, declares no action)
  that steers to the remembered spot and stands on arrival, holding when there
  is no memory.
- SDK surface for all five additions (typedefs, committed fixtures, primitive
  docs), in both QuickJS and Luau.
- The reference enemy graph gains an `investigate` state, damage-driven and
  lost-sight transitions, and a directional startle, with its Luau twin kept
  byte-equal.
- A behavior-preserving split of the `ai/mod.rs` tick orchestrator into cohesive
  sibling modules (`ai/steering.rs`, `ai/compute.rs`, `ai/apply.rs`), landed
  first (Phase 0) so the feature's additions extend small modules rather than
  growing the god file. No behavior change — verified by the existing AI suite
  (AC 2).

### Out of scope

- **Threat prioritization.** Damage makes an attacker's position *investigable*;
  it does not make the attacker the *preferred* target over a nearer hostile.
  "Most-recently-damaged wins" is a `select_target` ranking change
  (`enemy-aggro-model.md` dimension 2) and waits for its own consumer. v1 single
  attacker is the primary case.
- **Perception override on target selection.** The floor never makes an attacker
  behind cover a *selectable target* through walls. Behind-cover reaction is
  delivered by seeding the last-known-position memory and investigating it, not
  by tracking the attacker.
- **Directional flinch animation _authoring_ beyond exposing `damageBearing`.**
  The fact ships; a full directional-pain clip set (left/right/front/back stagger
  interrupts) is the stagger/pain feature's job (`E10--enemy-stagger`).
- **Alert propagation / pack aggro, per-archetype aggression profiles, sound as
  a distinct stimulus** (`enemy-aggro-model.md` candidate dimensions) — each
  waits for its own consumer.
- **Authored tuning of the sentinel/clamp constants.** The timers' "never"
  sentinel and clamp ceiling are engine constants in v1, upgradeable to a
  descriptor scalar later on the `NumberOrIr` precedent if a game needs it.

## Direction

**Problem.** An enemy's behavior graph can only read facts about the *single
retained target it currently perceives*. Being hit from an unseen direction, and
losing sight of a target, leave no fact and no memory — so an enemy shot from
behind cover, or sniped from beyond its authored engage distance, does nothing.
The cause is a missing substrate (no damage/sight recency, no positional
memory), not a missing tuning knob.

**Prior commitments.** `entity_model.md` §7c splits perception into *perceivable*
(engine floor, correctness) and *worth engaging* (authored taste), and states
**candidacy can only narrow the offer set, never widen it**. That forecloses a
purely-authored solution: an enemy cannot author its way into perceiving an
attacker it couldn't otherwise sense. So the floor owns the memory (a
correctness noun — *where a stimulus came from*), and the authored graph owns
the reaction (taste). `scripting.md` §1 names enemy aggression as an explicitly
descriptor-exposed spectrum with "no point baked in," which the taste split
honors: the facts ship; the reaction is authored. `enemy-aggro-model.md`
pre-committed the growth path and its governance — *"built incrementally, with
real consumers, never ahead of need"* — and this slice takes one consumer
(react-to-being-shot + search-after-losing-sight), deferring the rest of that
document's menu. The `@brain.*` fact table is append-only by contract
(`scripting.md` §11): new facts extend the tail, never reorder.

Divergence: `enemy-aggro-model.md` frames perception growth around widening
`select_target`'s `visible` predicate (`bool → weight`). This spec does **not**
touch that predicate. Reacting to an unseen attacker is delivered by a
positional *memory* the graph investigates, not by making the attacker
perceivable. The `visible`-predicate widening remains available for
threat-ranking later; it is orthogonal, not superseded.

**Alternatives rejected.** *Perception override* — on damage, force the attacker
into the enemy's perceived/selectable set for a window, so existing facing and
pursuit track it. Rejected: it tracks the attacker through walls (an enemy that
"knows" your exact position behind cover reads as a cheat), special-cases target
selection, and puts the reaction on the floor rather than the graph. The
last-known-position memory delivers the same felt outcome (enemy advances on the
shooter, engages on re-sight) from stale information the graph chooses to act on
— more systemic, and it reuses the shipped `moveToAnchor` position-goal
machinery instead of adding a selection special case.

*Impact-policy / combat-event seam* — express the reaction through the shipped
`defineImpactEvent` / `@impact.*` surface (`scripting.md` §2, §10) rather than
polled brain facts. Rejected, and the shape is essentially forced: an impact
policy fires once per hit, so it cannot age a recency timer or recompute
`distanceToLastKnown` against the enemy's moving position each tick; the
sight-recency half has no damage event to hang on at all (losing sight fires
nothing); and the attacker's position is not a readable impact-scope leaf
(`@impact.*` exposes `amount` and health deltas plus a `@impact.source` command
token — no Transform). A polled fact backed by engine-owned memory is the only
shape that serves both halves and updates every tick.

*One generic stimulus slot* — collapse everything into a single stimulus record.
Partly adopted: `last_known_target_pos` **is** one dual-sourced slot (visible
target and damage seed write the same field), and a future sound stimulus
(`enemy-aggro-model.md`) would add a third source to it, not a new slot. The two
*timers* stay separate because they answer independent questions that are true
at once — "how long since I was hit" and "how long since I saw the target" — and
a graph reads them in different guards.

## Acceptance criteria

- [ ] The four facts parse and validate identically in QuickJS and Luau; a guard
  referencing each binds without error, and an unknown `@brain.*` name still
  produces a pathed parse error in both runtimes. SDK typedef drift tests pass
  with all four facts and the `moveToLastKnown` verb present in both committed
  fixtures.
- [ ] Archetypes that use none of the new facts or the new verb behave
  bit-for-bit as today: the full existing AI test suite passes unchanged (confirm
  no golden/digest snapshot pins the `BrainComponent` field set; appended
  serde-defaulted fields are otherwise compatible).
- [ ] A freshly spawned, never-damaged, never-having-seen-a-target enemy reads
  `timeSinceDamageMs` and `timeSinceTargetVisible` as the "never" sentinel (a
  guard `le(timeSinceDamageMs, W)` is false), and `distanceToLastKnown` as the
  large no-memory sentinel — a bare `gt(distanceToLastKnown, X)` reads true with
  no memory, so an investigate guard must conjoin a recency fact. No
  recency-conjoined new-fact guard fires at spawn.
- [ ] A hit on an enemy sets `timeSinceDamageMs` toward zero: on the AI tick
  after the hit it reads ≤ one tick's `dt_ms`, then climbs monotonically,
  clamped at the sentinel (it never wraps or grows unbounded over a long
  session). All three damage paths trigger it: player hitscan, another entity's
  melee, and the `applyDamage` script reaction. In live v1 play enemy melee never
  reaches a brain entity (dual-sourced-memory invariant), so the melee path is a
  chokepoint-level guarantee, exercised by driving the chokepoint directly.
- [ ] While a target is visible (`@brain.targetVisible` true, grace included),
  `timeSinceTargetVisible` reads 0 and `distanceToLastKnown` tracks the target's
  live position; after visibility is lost it climbs monotonically and
  `distanceToLastKnown` freezes at the last-seen spot until movement changes it.
- [ ] After a hit whose attacker the enemy never saw, `distanceToLastKnown`
  reflects the **attacker's** position at hit time, and an enemy authored to
  `moveToLastKnown` steers to that spot; on reaching it (`distanceToLastKnown`
  within the arrival epsilon) the steering clears and it stands.
- [ ] `damageBearing` is captured relative to the enemy's facing at hit time:
  a hit from dead ahead reads ≈ 0, from directly behind reads ≈ ±π, and hits
  from the enemy's two sides read opposite signs. The value is meaningful only
  while `timeSinceDamageMs` is recent; the author gates it on recency.
- [ ] `moveToLastKnown` is non-engaged: a graph that declares an `action` on a
  `moveToLastKnown` activity is a parse error in both runtimes; an enemy in a
  `moveToLastKnown` activity retains no target and is assigned no combat slot;
  with no last-known position it holds (steering cleared) and, being a locomotion
  activity at a standstill, yields to the graph's rest (initial-state) animation
  rather than its own travel cycle (the locomotion-at-standstill rule,
  `entity_model.md` §7c).
- [ ] `BrainScope::refresh` projects every `BRAIN_INPUTS` entry in table order
  after the additions; the slot-index and array-length coupling tests pass (a
  reorder or a missed projection fails to compile or fails a slot test).
- [ ] A `BrainComponent` serialized before these fields deserializes with the
  memory empty and the timers at the sentinel; a round trip through
  `ComponentValue::Brain` preserves the new fields.
- [ ] Reference enemy, on the movement-feel fixture: sniped from beyond its
  engage distance with clear sight, it engages; shot from behind cover, it
  advances on the shot's origin and engages on re-sight; chasing a target that
  breaks line of sight, it searches the last-seen spot and stands down if
  nothing is there; a hit plays the authored directional startle. On a loopback
  co-op session the host enemy's memory and timers stay host-only (no wire-format
  change); the only replicated presentation cue is the mesh animation state name,
  so the client sees the enemy play its `walk` animation and move toward the
  last-known spot via the replicated `Transform`, not the `investigate` state
  itself — which shares the `walk` clip with patrol and chase and is therefore not
  client-observable. The distinct `flinch` startle is observable, but a startle
  beat shorter than one snapshot interval is not guaranteed to reach the client.
  This criterion is a manual playtest/review sign-off, not a runnable assertion:
  the felt behaviors are verified on the fixture and "no wire-format change" is a
  review gate; the one automatable piece is the twin parser byte-equality test.

## Tasks

### Task 0: Split `ai/mod.rs` (behavior-preserving)

Extract the tick orchestrator's cohesive units into sibling modules before the
feature adds to it. `crates/postretro/src/scripting/systems/ai/mod.rs` is
organized as three passes plus motion helpers; make that structure physical by
moving code verbatim, preserving order — a block move, not a reconstruction:
- `ai/steering.rs` — motion/steering resolution: `position_goal_steering` and the
  patrol helpers (`patrol_steering`, `patrol_goal`, `advance_patrol_cursor`),
  which resolve a brain's motion verb / patrol route to a `SteeringIntent`. Task
  2b's `MoveToLastKnown` arm lands here.
- `ai/compute.rs` — Pass 2: per-brain evaluation producing the `EnemyOutcome`
  vector, in source order — targeting, per-tick timer aging, fact/memory
  computes, scope refresh, then guard eval (aging and fact computes precede the
  refresh, so the 1-tick-latency read is preserved). Task 1/2a/3 fact + memory
  computes land here.
- `ai/apply.rs` — Pass 3: registry mutation from the outcomes, in source order —
  brain write-back, steering, facing, the damage chokepoint call, then animation.
- `ai/mod.rs` — orchestrator: entry points (`run_ai_tick*`), the pass-interface
  types (`EnemySnapshot`, `EnemyOutcome`, `AttackOutcome`, plus `AiTickResult`,
  `AiTickInputs`, `AiRuntime`, `LocomotionIntent`), Pass 1 (snapshot), and the
  driver that sequences the passes.
The smaller per-pass helpers (`facing_direction`, `should_switch_animation`,
`entity_faction`) move with the pass that consumes them. `EnemyOutcome` is
already `pub(super)`; raise `EnemySnapshot` and `AttackOutcome` to `pub(super)`
so the compute/apply modules share the pass interface. `SteeringIntent` stays in
`ai/engine_floor.rs`, unchanged. No behavior change: guards, ordering, and
outputs are identical. Verified by the full existing AI test suite passing
unchanged (AC 2); no new test. Later phases land their additions in these
modules (`ai/compute.rs`, `ai/steering.rs`).

### Task 1: Damage-recency fact (thin slice)

Deliver `@brain.timeSinceDamageMs` end to end, crossing every seam this spec
adds so the fact-table/refresh/SDK/chokepoint boundary is falsified before the
rest fans out. Add `time_since_damage_ms: f32` to `BrainComponent`
(`crates/entities/src/components/brain.rs`), serde-defaulting to a "never"
sentinel (a named default helper, not `0.0`, since `0.0` reads as "just
damaged"); initialize it to the sentinel in `from_graph`; add the pre-field
deserialization-defaults test and the `ComponentValue::Brain` round-trip.
`apply_damage_with_context` (`crates/entities/src/components/health.rs`) resets
it to `0.0` when the damaged entity carries a `BrainComponent` (no-op
otherwise), via the crate's clone-mutate-`set_component` idiom or
`get_component_value_mut(id, ComponentKind::Brain)`. The AI compute pass ages it
by `dt_ms` and clamps at the sentinel, aged unconditionally each tick like the
named attack cooldowns (not gated on `entry_pending` the way the activity timers
are), so a transition tick never skips an increment. Append the fact to `BRAIN_INPUTS` (`crates/foundation/src/brain.rs`) at
the tail as `Number` with its `BRAIN_*_INPUT` const and validation-twin entry,
plus the slot-index test; add its `BrainFacts` field and `BrainScope::refresh`
projection (`crates/postretro/src/scripting/systems/ai/brain_scope.rs`,
`.../ai/compute.rs`). Add the SDK `BrainInputs` interface entry in the `sdk_lib`
templates and regenerate the committed `postretro.d.ts` / `.d.luau` and the test
fixtures. Consumer: a unit test in `ai_tests.rs` with a hand-built graph whose
guard reads `timeSinceDamageMs` — not the shipped reference enemy (that lands in
Task 4). Prove: sentinel at spawn, ~`dt_ms` the tick after a hit, monotonic
climb, clamp; the three damage paths.

### Task 2a: Last-known-position memory and search facts

Add `last_known_target_pos: Option<Vec3>` (serde default `None`) and
`time_since_target_visible: f32` (serde default sentinel) to `BrainComponent`,
initialized in `from_graph`, with serde-default and round-trip coverage.
`apply_damage_with_context` additionally seeds `last_known_target_pos` from the
attacker's `Transform` position when `context.attacker` is `Some` and that
entity has a `Transform` (leave unchanged otherwise). The AI compute pass, at
the point the debounced `target_visible` verdict is known and the selected
target's world position is in hand: while visible, write
`last_known_target_pos = Some(target_position)` and reset
`time_since_target_visible = 0.0`; otherwise age `time_since_target_visible` by
`dt_ms` (clamped at the sentinel). Compute a `distance_to_last_known` from the
enemy's own position snapshot and `last_known_target_pos`
(`nav::distance_xz`), falling back to the large-value `BRAIN_NO_TARGET_DISTANCE`
no-memory sentinel when the memory is empty (so `gt`/`ge` guards read the
no-memory direction; see AC 3) — mirroring how `distance_from_anchor` is
computed. Append
`@brain.timeSinceTargetVisible` and `@brain.distanceToLastKnown` to
`BRAIN_INPUTS` (both `Number`), with consts, validation-twin entries,
slot-index tests, `BrainFacts` fields, `refresh` projections, SDK typedefs,
committed fixtures. The dual-sourced memory has one ordering rule: within a
tick, the compute-pass visible-cache is authoritative while the target is
visible; the damage-seed (a later stage) matters only when no target is visible
that tick (Invariants table). Test the visible-cache/age transition, the
damage-seed when unseen, and the sentinel fallbacks.

### Task 2b: `moveToLastKnown` motion verb

Add `MoveToLastKnown` to the `MotionVerb` enum and its `ALL` array (and the
array length) in `crates/foundation/src/data_descriptors/types/behavior.rs`
(serde `"moveToLastKnown"`). Resolve its destination in `position_goal_steering`
(`.../ai/steering.rs`, post-split): `MoveTo(last_known_target_pos)` when set and beyond the
arrival epsilon, else `Clear`; add the exhaustive arm to `steering_for` as well
(it must land in the `Clear`/non-chase set there). Add `MoveToLastKnown` to all four
non-engaged gates so it is treated exactly like `moveToAnchor`: the descriptor
validation that rejects an `action` on a position-goal leaf
(`.../types/behavior/recursive.rs`), and the `graph_eval.rs` `action_for_path`,
`activity_can_engage`, and `is_locomotion_activity` clauses. Register the SDK enum
variant doc and extend the `moveSpeed` doc's nav-driven-verb list in
`crates/postretro/src/scripting/primitives/mod.rs`; regenerate the `MotionVerb`
union in the committed typedefs and fixtures. Consumes Task 2a's
`last_known_target_pos`. Test: validation rejects an action on a
`moveToLastKnown` activity in both runtimes; a `moveToLastKnown` enemy retains
no target and gets no combat slot; steering resolves to `MoveTo` then `Clear` on
arrival, and `Clear` (hold) with no memory.

### Task 3: `damageBearing` fact

Add `damage_bearing: f32` (serde default `0.0`) to `BrainComponent`, initialized
in `from_graph`, with round-trip coverage. In `apply_damage_with_context`,
when the damaged entity carries a `BrainComponent` and `context.attacker` has a
`Transform`, compute the enemy-relative yaw from the enemy's facing
(`Transform.rotation` on the damaged entity — the AI apply pass writes the
enemy's visual facing to `Transform.rotation` via the `facing.rs` yaw helpers, and
the renderer reads it directly) and the XZ direction to the attacker, and store it.
Mesh forward is `+Z` (`MESH_FORWARD` in `ai/facing.rs`), so bearing 0 is the
attacker along the enemy's `Transform.rotation * +Z`; recompute the yaw inline in
`health.rs` — `facing.rs`'s helpers are `pub(super)` in the `postretro` binary
crate and are not importable from `postretro-entities`. Convention:
signed radians in `[-π, π]`, `0` = attacker dead ahead, `±π` = directly behind,
sign distinguishes the two sides. Append `@brain.damageBearing` to
`BRAIN_INPUTS` (`Number`) with const, validation twin, slot-index test,
`BrainFacts` field, `refresh` projection, SDK typedef, committed fixtures. Test
by pinning observable geometry: hits from ahead/behind/left/right yield
`≈0` / `≈±π` / opposite-signed values — pinning the convention without asserting
a raw quaternion result.

### Task 4: Reference enemy graph and integration

Rewrite the reference enemy graph (`content/dev/scripts/reference-enemy.ts` and
its byte-equal `reference-enemy.luau`) to exercise the whole feature: an
`investigate` activity (`animation: "walk"`, `motion: "moveToLastKnown"`); a
`patrol → investigate` transition on recent damage with an unreached memory
(`timeSinceDamageMs.le(ALERT_MS).and(distanceToLastKnown.gt(ARRIVE))` — the
recency conjunction is load-bearing; `distanceToLastKnown.gt` reads true with no
memory, so it gates on an actually-seeded spot, per AC 3); an
`engage → investigate` transition on lost sight
(`timeSinceTargetVisible.ge(SEARCH_AFTER_MS)`); `investigate → engage` on
re-sight (`targetVisible`) and `investigate → patrol` on arrival
(`distanceToLastKnown.le(ARRIVE)`); and a directional startle beat gated on
`timeSinceDamageMs` and `damageBearing`. Keep the twin parser test green.
Verify on the movement-feel fixture and on a loopback co-op session, both per
the final acceptance criterion.

## Sequencing

After the Phase 0 split, the feature's additions concentrate in a few shared
files — the `BRAIN_INPUTS` table (`foundation/brain.rs`), the damage chokepoint
(`entities/health.rs`), the compute pass (`ai/compute.rs`), and `brain_scope.rs`
— so tasks append to them in series rather than racing. The chain is inherently
layered (split → memory → facts → consumer), so this is sequential by nature,
not an artificial ordering.

**Phase 0 (sequential):** Task 0 — behavior-preserving `ai/mod.rs` split; lands
first so every later addition extends a small module. No behavior change (AC 2).
**Phase 1 (sequential):** Task 1 — thin slice; falsifies the fact-table,
refresh-projection, committed-SDK, and chokepoint→brain boundary before fan-out.
**Phase 2 (sequential):** Task 2a — memory + search facts; then Task 2b —
`moveToLastKnown`, which consumes 2a's `last_known_target_pos`.
**Phase 3 (sequential):** Task 3 — `damageBearing`; extends the chokepoint and
appends the last fact slot.
**Phase 4 (sequential):** Task 4 — reference graph + integration; consumes all
facts and the verb.

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS · Luau | FGD |
|---|---|---|---|---|
| damage-recency fact | `BRAIN_TIME_SINCE_DAMAGE_MS_INPUT` | `"@brain.timeSinceDamageMs"` | `brain.timeSinceDamageMs` | n/a |
| sight-recency fact | `BRAIN_TIME_SINCE_TARGET_VISIBLE_INPUT` | `"@brain.timeSinceTargetVisible"` | `brain.timeSinceTargetVisible` | n/a |
| last-known distance fact | `BRAIN_DISTANCE_TO_LAST_KNOWN_INPUT` | `"@brain.distanceToLastKnown"` | `brain.distanceToLastKnown` | n/a |
| damage bearing fact | `BRAIN_DAMAGE_BEARING_INPUT` | `"@brain.damageBearing"` | `brain.damageBearing` | n/a |
| investigate motion verb | `MotionVerb::MoveToLastKnown` | `"moveToLastKnown"` | `"moveToLastKnown"` | n/a |
| last-known memory (brain) | `BrainComponent::last_known_target_pos: Option<Vec3>` | serde default `None` | — | n/a |
| damage-recency timer (brain) | `BrainComponent::time_since_damage_ms: f32` | serde default = never-sentinel | — | n/a |
| sight-recency timer (brain) | `BrainComponent::time_since_target_visible: f32` | serde default = never-sentinel | — | n/a |
| damage bearing (brain) | `BrainComponent::damage_bearing: f32` | serde default `0.0` | — | n/a |

Fact names extend `BRAIN_INPUTS` at the tail in task order (slots 15–18). The
`Number` sentinel for the two timers and for `distanceToLastKnown` reuses the
large-value convention of the existing `BRAIN_NO_TARGET_DISTANCE`.
`distanceToLastKnown` therefore inherits that sentinel's `gt`/`ge` inversion trap
(documented at `BRAIN_NO_TARGET_DISTANCE`): with no memory a bare
`gt(distanceToLastKnown, X)` reads true — it doubles as the no-memory test — so an
"unreached spot" guard must conjoin a recency fact (`timeSinceDamageMs` /
`timeSinceTargetVisible`) that is recent only when the memory was seeded, as the
reference graph's `patrol → investigate` guard does. A recency reset with no
seeded attacker (the `applyDamage` script path) does not create a spot.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| `BRAIN_INPUTS` is append-only; a name's index is its runtime read handle | Tasks 1, 2a, 3 append at the tail | any insert/reorder silently re-points every bound program | slot-index tests (`foundation/brain.rs`); AC "refresh projects every entry" |
| `BrainScope::refresh` writes one value per `BRAIN_INPUTS` entry, in order | each fact task | array-length drift between table and refresh array | `refresh`'s fixed-array literal is a compile tripwire on length drift; `expected_fixed_value`'s no-`_` arm is a test-time panic on a missing case; AC 9 |
| Timers: `never`-sentinel until first trigger, `0` at trigger, monotonic aging clamped at the sentinel | Task 1 (damage), Task 2a (sight); reset at chokepoint / visible-tick, aged in compute pass | spawn default must be the large-value `BRAIN_NO_TARGET_DISTANCE` sentinel (`foundation/brain.rs`), not `0.0`; catch-up ticks age per tick; long sessions must not overflow | AC 3, 4, 5 |
| `moveToLastKnown` is non-engaged — no action, no target retention, no combat slot | Task 2b adds it to all four position-goal gates | a missed gate lets it declare an action or hold a target/slot | AC 8; validation-rejects-action test |
| Last-known memory is dual-sourced; the compute-pass visible-cache is authoritative each tick a target is visible, the damage-seed applies only when unseen | Task 2a (visible-cache), Task 1/2a (damage-seed), Task 3 (bearing rides the same seed site) | out-of-tick damage (player weapon stage 8; frame-end `applyDamage` drain) is the only seed path that reaches a brain entity in v1 — enemies target only `PlayerMovement` holders (`targeting.rs` `target_offers`), so in-tick enemy melee never damages a brain entity and the apply-pass snapshot write-back poses no live race; a future spec allowing enemy-damaged brain entities must fold the chokepoint-owned fields past that write-back | AC 5, 6; Orderings table (`research.md`) |
| The floor never makes an attacker a selectable target it could not otherwise perceive | whole spec (no `select_target`/`visible` change) | a future threat-ranking spec owns any widening | AC out-of-scope; no code path touches `select_target` |

## Script syntax examples

```ts
// Proposed design
import { brain, defineEntity, runtime } from "postretro";

const DETECTION_RANGE = 16;
const ALERT_MS = 4000;        // react to a hit taken within the last 4 s
const SEARCH_AFTER_MS = 800;  // lost sight this long → go look
const ARRIVE = 1.0;           // "reached the remembered spot"
const HALF_PI = Math.PI / 2;

// ...inside components.behavior...
activities: {
  idle:    { animation: "idle", motion: "hold" },
  patrol:  { animation: "walk", motion: "patrol" },
  engage:  { /* existing chase / attack sub-graph */ },
  investigate: { animation: "walk", motion: "moveToLastKnown" }, // non-engaged
  startle: { animation: "flinch", /* short committed beat */ },
  retreat: { animation: "walk", motion: "moveToAnchor" },
},
transitions: {
  "*": [
    // stand down only once calm, unshot, and the remembered spot is reached
    {
      to: "patrol",
      when: brain.hasTarget.not()
        .and(brain.timeSinceDamageMs.gt(ALERT_MS))
        .and(brain.distanceToLastKnown.le(ARRIVE)),
    },
    // a hit from the side/rear while unaware → brief directional startle
    {
      to: "startle",
      when: brain.timeSinceDamageMs.le(200).and(
        brain.damageBearing.gt(HALF_PI).or(brain.damageBearing.lt(-HALF_PI)),
      ),
    },
  ],
  patrol: [
    { to: "engage", when: brain.acquisitionDue.and(brain.targetDistance.le(DETECTION_RANGE)) },
    // shot but can't see the shooter → investigate where it came from
    { to: "investigate", when: brain.timeSinceDamageMs.le(ALERT_MS).and(brain.distanceToLastKnown.gt(ARRIVE)) },
  ],
  engage: [
    { to: "investigate", when: brain.timeSinceTargetVisible.ge(SEARCH_AFTER_MS) },
  ],
  investigate: [
    { to: "engage", when: brain.targetVisible },                 // re-sight → re-engage
    { to: "patrol", when: brain.distanceToLastKnown.le(ARRIVE) },// reached it, empty → give up
  ],
},
```

The memory-consumption signal is emergent: reaching the spot drives
`distanceToLastKnown` to ≈ 0, dropping `investigate → patrol`; a *new* hit
re-seeds the memory, so `distanceToLastKnown` jumps and `patrol → investigate`
re-triggers. No engine pulse or latch is needed — unlike the stagger design,
which needs a one-tick engine-written flag.

## Open questions

None.
