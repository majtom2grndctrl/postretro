# Enemy Aggro Model

## Goal

Close the three acquisition/leash defects the behavior-state-graph review panel
deferred. Two are engine-floor correctness: the leash gates retention but not
acquisition (permanent oscillation when `leashRange < detectionRange`), and a
graph authored with no disengagement edge is a silent level-wide pursuer. The
third — dead pawns holding enemies hostage — is not a missing check but a
missing vocabulary: the graph has no fact about the *target*, only about the
brain itself, so "worth attacking" is hardcoded in the engine. This plan gives
mods target-side facts and a candidacy filter, and moves that judgement to where
the rest of taste already lives.

## Scope

### In scope

- **Target-side brain facts.** New `@brain.*` guard inputs describing the
  selected target: its current health, its maximum health, and whether the
  engine's death latch has fired for it. Disengaging from a downed target
  becomes an authored interrupt instead of engine policy.
- **A per-graph candidate filter.** One optional IR predicate on the behavior
  block, compiled once and evaluated per candidate during the acquisition scan,
  over a candidate-facts namespace: engine facts about the candidate, plus the
  same `@state.*` per-entity leaves guards already read, resolved against the
  candidate rather than the enemy. The engine floor keeps deciding which
  entities are *offered* (player pawns, and later the visibility predicate); the
  mod decides which offered candidates are worth engaging. Unauthored, nothing
  is evaluated and behavior is bit-identical to today.
- **The floor's leash becomes symmetric**: a target-relevance radius bounding
  fresh acquisition on the same terms it already bounds retention. Legacy
  `components.ai` only — authored graphs carry no leash and are unaffected.
- **A parse-time ordering rule on `components.ai`:** `leashRange` must be `>=`
  `detectionRange`; inverting them is a validation error naming both values.
- **Two structural warnings on an authored graph**, emitted at descriptor
  validation: engaging states with no edge to a non-engaging state (level-wide
  pursuer), and engaging states with no has-target interrupt (the no-target
  distance sentinel trap).
- The reference enemy adopts the new vocabulary and becomes the worked example.

### Out of scope

- Any engine-side aliveness rule for targeting. The attack gate's existing
  aliveness check stays exactly where it is and keeps its current meaning; no
  new one is added to selection.
- Any authored disengagement or acquisition *range* on `components.behavior`.
  Pinned by the predecessor: a second spelling of disengagement with undefined
  precedence against the guards.
- Changing what legacy `components.ai` brains do. Lowering emits no new edges
  and no filter; v0 parity is preserved, hostage behavior included.
- Perception — line of sight, sound, alert propagation, aggression profiles,
  memory and search. The visibility predicate on the selection chokepoint stays
  untouched and unused; the candidacy filter sits downstream of it.
- Threat or priority *ranking* policies. The filter decides eligibility, never
  order; ranking stays nearest-with-hysteresis.
- Per-state candidacy. The filter is per-graph, deliberately.
- Writes of any kind from the filter or from guards. Both are read-only.
- Changing the engagement radius, think-stride bands, switch hysteresis, the
  damage chokepoint, the aggro gate, or host-only evaluation.
- Renaming the trigger system's `alive_players` misnomer (see `research.md` §4).
- Wire or replication changes. All of this is host-side sim state.

## Acceptance criteria

- [ ] A graph can author an interrupt that stands the brain down when its
      selected target's death latch has fired; an enemy engaged with a co-op
      pawn that dies releases it on the next tick and engages a live pawn
      standing beside it, even inside the switch-hysteresis margin.
- [ ] With no selected target, every target-side fact reads its type's zero, and
      the boolean death fact reads false. A guard reading a target-side fact
      untargeted never fires a stand-down that the has-target interrupt would not
      already have fired.
- [ ] A graph can author a candidate filter that excludes dead pawns; an enemy
      whose only nearby pawn is dead acquires nothing and stays at rest, while
      the same enemy with a live pawn in range engages normally.
- [ ] A candidate filter reads a mod-authored `@state.*` field on the candidate
      and acts on it: a pawn some impact policy marked untargetable is skipped by
      the acquisition scan while an unmarked pawn beside it is acquired, with no
      engine component naming that field. A candidate carrying no state
      component, or not that field, reads `0.0`.
- [ ] A graph with no candidate filter selects targets exactly as today —
      the full existing AI suite passes unchanged, legacy and authored alike.
- [ ] A candidate filter is compiled once per graph and evaluated per candidate
      with no heap allocation on the acquisition path, the interned `@state.*`
      snapshot included — it grows at bind and is written by index at refresh
      (alloc-probe assertion, matching the substrate invariant).
- [ ] A candidate filter that fails to bind is a validation error in both
      QuickJS and Luau, with the authored path in the message; a filter that
      produces a non-boolean is rejected the same way.
- [ ] The engine floor applies no aliveness rule to target selection: an enemy
      whose nearest pawn is dead and whose graph authors no filter still selects
      it and is still blocked from damaging it by the attack gate.
- [ ] A legacy enemy tuned with a leash smaller than its detection range no
      longer oscillates: seeded at rest with a pawn between the two radii, it
      stays at rest indefinitely, requests no destination, and changes state on
      no tick.
- [ ] Authoring `components.ai` with a leash range below its detection range is
      a validation error in both runtimes, naming both field values.
- [ ] A behavior graph whose engaging states offer no edge to a non-engaging
      state validates successfully and logs one warning naming those states.
- [ ] A behavior graph with engaging states and no has-target interrupt
      validates successfully and logs one warning explaining the sentinel
      asymmetry. Warnings fire once per descriptor validation, not per spawn and
      not per tick.
- [ ] The shipped reference enemy trips neither warning, authors both the
      target-death interrupt and the candidate filter, and is behavior-identical
      to its Luau twin.
- [ ] SDK typedef drift tests pass with the new brain facts, the `candidate`
      prelude, and the filter field in both committed fixtures; the scripting
      reference documents the floor's acquisition contract, the target-side
      facts' no-target readings, that an authored graph owns its own
      disengagement, and that a distance filter is the authored acquisition
      radius.

## Tasks

### Task 1: Target-side brain facts

Extend the fixed guard-input table with three facts about the selected target:
its current health, its maximum health, and whether the engine's one-shot death
latch (`HealthComponent.death_handled`) has fired for it. Add them in
`crates/foundation/src/brain.rs` as `@brain.`-prefixed constants appended to the
end of `BRAIN_INPUTS` — that table's order **is** the runtime read handle, so
append, never insert. Two Numbers and one Bool. With no selected target each
reads its declared type's zero: no new sentinel constant is introduced, and the
doc comments must say plainly that `@brain.hasTarget` is the only authoritative
presence test and that the existing `BRAIN_NO_TARGET_DISTANCE` remains the lone
exception to the zero convention. The Bool latch fact, not a health comparison,
is the death signal: it is unambiguous with no target, and it carries the death
sweep's full definition (which includes non-finite health) rather than an
author's re-derivation of it. Populate the facts in
`crates/postretro/src/scripting/systems/ai/brain_scope.rs`: `BrainFacts` gains
the selected target's `EntityId` as an `Option`, and `refresh` reads that
entity's `HealthComponent` the same way it already reads the evaluating enemy's
— absent component reads as zeros, absent target likewise. The tick site in
`ai/mod.rs` already computes the selected target immediately before building
`BrainFacts`, so it passes `target.map(|target| target.entity)` at that one call
site; nothing else moves. `BrainValidationScope` needs no change beyond the
table growing, since it resolves through `resolve_brain_input`. Extend the SDK's
hand-maintained `brain` prelude object in both runtimes with the three new
pre-wrapped input leaves, honoring the existing compile-time sync obligation
against the input table.

### Task 2: Per-graph candidate filter

Add an optional IR predicate to the behavior block that decides which offered
candidates an enemy will acquire. Descriptor side, in `postretro-foundation`:
`BehaviorGraphDescriptor` gains an optional raw `IrNode` field (camelCase wire
key per the boundary inventory), following the same descriptor-partition rule as
transition guards — the bound program is derived data the evaluator owns, never
a descriptor field. It needs its own input namespace and its own
declaration-time binding scope, both in a new module beside `brain.rs` rather
than inside it, and both shaped exactly like the brain pair. Two halves, as
there: a fixed table of `@candidate.`-prefixed engine facts — the candidate's XZ
distance from the enemy, its current health, its max health, and its death-latch
boolean — whose order **is** the runtime read handle, so it grows by appending
and never by insertion; and the `@state.*` per-entity leaves, resolved against
the candidate. One resolve function mirroring `resolve_brain_input` answers
both arms, a validation scope resolves through it, and a bind helper mirrors the
existing guard-bind helper. The shared `@state.` prefix is deliberate: a leaf
names a field, and the scope names whose field it is — `state("revivable")`
reads the enemy's in a guard and the candidate's in a filter, which is the
composition seam of `scripting.md` §11 reaching the acquisition path.
`BehaviorGraphDescriptor::validate` binds the filter when present and rejects an
unbindable or non-boolean one with the authored path, exactly as it already does
per transition. Runtime side, in the binary: a candidate `BindingScope` beside
the brain scope, the same two-snapshot shape — a fixed array written by index,
plus an interned `@state.*` name vector that grows only at bind. It refreshes
per candidate from the registry, reading components by reference; a candidate
with no `EntityStateComponent`, or without the named field, reads `0.0`, the
same emergent-field contract the brain scope honors. Binding rides the existing
evaluator side-table:
`BrainPrograms` holds one shared candidate scope alongside its brain scope, and
`BrainEntityPrograms` gains one optional bound filter program compiled during
the same `sync` pass that binds the graph's guards, keyed off the same `Arc`
pointer-identity staleness test. Evaluation goes in `targeting.rs`, inside the
ranking scan's per-pawn `filter_map` and **not** in the shared candidate lookup
that also resolves the retained target — filtering retention there would make
the engine decide disengagement again, which is the graph's job per Task 1.
Plumbing: the scan and the selection entry point take the enemy's bound filter
and the shared candidate scope as parameters, and `ai/mod.rs` passes them at its
existing selection call sites (the early non-engaged scan, the leash-escape
replacement, and both acquisition-due branches) from the side-table entry it
already looks up for guard evaluation. `None` means no filter: nothing is
evaluated and the scan is byte-for-byte today's. Lowering emits no filter, so
legacy brains are unchanged. Extend the SDK with a `candidate` prelude object in
both runtimes — pre-wrapped leaves for the fixed table, under the same
compile-time sync obligation Task 1 carries for `brain`. The existing
`state(name)` builder serves both scopes unchanged: it emits an `@state.` leaf
and the scope it binds against decides whose field that is.

### Task 3: Leash bounds acquisition

Give the floor's leash the same authority over fresh acquisition it already has
over retention. Today `BrainComponent::leash_range` (`Option<f32>`, `Some` only
for lowered legacy brains) is consulted in `ai/mod.rs` for the retained
candidate and for the replacement search on a leash-escape tick, but the
ordinary acquisition scan applies no range limit — which is the whole
oscillation. Move the rule into `targeting.rs` so the oversized tick module only
passes a value: the ranking scan and the selection entry point take the brain's
optional leash and reject any candidate beyond it, and `ai/mod.rs` passes
`brain.leash_range` at the same four call sites Task 2 touches. The existing
post-hoc leash filter on the replacement search collapses into the new parameter
rather than remaining a second spelling. A brain with no leash — every authored
graph — keeps today's unbounded behavior exactly. Separately, add the ordering
rule to `AiDescriptor::validate`
(`crates/foundation/src/data_descriptors/types/combat.rs`): after the existing
finite-and-positive loop, reject a descriptor whose `leashRange` is below its
`detectionRange`, with a message naming both values in the established
`components.ai.<field>` style. Both runtimes funnel through that validator, so
one edit covers QuickJS and Luau. Rust fixtures construct `AiDescriptor`
literally and bypass validation, which is why the floor rule must stand on its
own; the two Rust test fixtures using inverted tuning (`ai_tests.rs`, the
leash-versus-detection cases) become the non-oscillation regression rather than
being deleted.

### Task 4: Graph disengagement lints

Add two structural diagnostics for authored behavior graphs, in a new sibling
module beside `crates/foundation/src/data_descriptors/types/behavior.rs` (824
lines — do not extend it), exposing one entry point
`BehaviorGraphDescriptor::validate` calls after its existing structural checks
succeed. Both are pure functions of the descriptor. A state is ENGAGING when its
motion verb is the chase verb or it declares any action — the same predicate the
evaluator uses, restated over the descriptor rather than over a resolved state
index. Lint one: a graph with at least one engaging state where no engaging
state declares a transition to a non-engaging state warns that it pursues
without limit, naming the engaging states. Lint two: a graph with at least one
engaging state and no interrupt whose guard tree reads the has-target input
warns that target loss will be handled through distance guards, where the
no-target sentinel makes `gt`/`ge` read true. Detect the read by walking the
guard `IrNode` tree for an input node naming the has-target constant. Foundation
already depends on `log`, so emit via `log::warn!` — validation returns a
`Result` and these are not errors, so they do not join the return type. Messages
carry the offending state names and the `components.behavior` path prefix, and
fire once per descriptor validation, which is once per parse and not per spawn.
Confirm the shipped reference enemy trips neither, and add a fixture graph for
each.

### Task 5: Reference enemy, docs, and typedefs

Author the new vocabulary into the reference enemy
(`sdk/behaviors/reference/entities.{ts,luau}`, identical spellings): a
target-death stand-down interrupt declared immediately after the existing
has-target interrupt, and a candidate filter that excludes dead pawns and bounds
acquisition by distance. Its comments are the de-facto authoring documentation
and must teach four things — that target-side facts read zero with no target and
are only meaningful under `hasTarget`; that the death latch, not a health
comparison, is the death test and why; that candidacy is per-graph eligibility
while disengagement is per-state policy; and that `state("field")` names the
enemy's field in a guard and the candidate's in a filter, because the scope, not
the leaf, decides the entity. Extend `docs/scripting-reference.md` with the
floor's acquisition contract: what the engine offers as candidates, what it
never decides (aliveness), the legacy leash's dual acquisition/retention role,
that an authored graph owns its own disengagement, and that a filter over
`candidate.distance` **is** the authored acquisition radius — the reason no
descriptor field spells one, and the answer to the question the seed research
left open. In the typedef generator
(`crates/postretro/src/scripting/primitives/mod.rs`), update the
`components.ai` `detectionRange` and `leashRange` field descriptions to state
the ordering constraint and the leash's widened role, and emit the
behavior-block filter field plus the `candidate` input interface beside the
existing `brain` one. Regenerate and commit both typedef fixtures.

## Sequencing

**Phase 1 (concurrent):** Task 1, Task 4 — disjoint (guard-input table and
brain scope versus descriptor lint module).
**Phase 2 (sequential):** Task 2 — consumes Task 1's settled input table and
extends the same evaluator side-table.
**Phase 3 (sequential):** Task 3 — shares `targeting.rs` and the same four
selection call sites with Task 2.
**Phase 4 (sequential):** Task 5 — documents the contract the first four tasks
settle, and regenerates typedefs once.

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau |
|---|---|---|---|---|
| Target health fact | `BRAIN_TARGET_HEALTH_INPUT` | `"@brain.targetHealth"` | `brain.targetHealth` | `brain.targetHealth` |
| Target max health fact | `BRAIN_TARGET_MAX_HEALTH_INPUT` | `"@brain.targetMaxHealth"` | `brain.targetMaxHealth` | `brain.targetMaxHealth` |
| Target death latch fact | `BRAIN_TARGET_DIED_INPUT` | `"@brain.targetDied"` | `brain.targetDied` | `brain.targetDied` |
| Candidate filter field | `BehaviorGraphDescriptor::candidate_filter: Option<IrNode>` | `"candidateFilter"` | `candidateFilter?: RuntimeValue` | same |
| Candidate facts table | `CANDIDATE_INPUTS` | — | `candidate.*` prelude object | `candidate.*` |
| Candidate distance | `CANDIDATE_DISTANCE_INPUT` | `"@candidate.distance"` | `candidate.distance` | `candidate.distance` |
| Candidate health | `CANDIDATE_HEALTH_INPUT` | `"@candidate.health"` | `candidate.health` | `candidate.health` |
| Candidate max health | `CANDIDATE_MAX_HEALTH_INPUT` | `"@candidate.maxHealth"` | `candidate.maxHealth` | `candidate.maxHealth` |
| Candidate death latch | `CANDIDATE_DIED_INPUT` | `"@candidate.died"` | `candidate.died` | `candidate.died` |
| Candidate state leaf | `ENTITY_STATE_INPUT_PREFIX` (reused) | `"@state.<field>"` | `state("field")` | `state("field")` |

No FGD column: all of it is descriptor-owned tuning, never map-overridable. The
state leaf reuses the guard spelling on purpose — the scope binds it, so no
second name exists for the same field.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| The engine floor holds no aliveness policy for target selection; the attack gate's aliveness check is the floor's only health read and stays a damage gate | Task 2 (by not adding one) | Any future "obvious fix" adding a health test to candidacy or retention | AC 8, 5 |
| Target-side facts read their type's zero with no target; `@brain.hasTarget` is the sole authoritative presence test, and the distance sentinel remains the lone exception | Task 1 | Any new target-side fact choosing a non-zero no-target reading | AC 2 |
| The death latch, not a health comparison, is the authored death signal — it is unambiguous untargeted and carries the sweep's non-finite arm | Task 1 | Docs and reference authoring must not teach `le(targetHealth, 0)` as the death test | AC 1, 13 |
| Candidacy is per-graph eligibility; disengagement is per-state policy. The filter never runs against the retained target | Task 2 | The shared candidate lookup also resolves the retained target — the filter must sit in the ranking scan only | AC 1, 3 |
| The floor decides what is *perceivable* — which entities the scan is offered, and later the `visible` predicate — while the graph decides which offered candidates are worth engaging. The filter is strictly downstream of `visible` and can only narrow the offer set | Task 2 | A perception spec exposing a `@candidate.visible` fact and moving line-of-sight policy into the filter; any filter arm that widens what the scan considers | AC 3, 5 |
| The filter answers eligibility, never order: it produces a boolean, and ranking stays nearest-with-hysteresis | Task 2 | A threat or priority spec widening the filter to a score rather than adding its own seam | AC 3, 7 |
| `@state.*` names a field; the binding scope names whose. One spelling reaches the enemy from a guard and the candidate from a filter, so the `scripting.md` §11 composition seam holds on the acquisition path | Task 2 | A second, candidate-specific spelling for the same fields would fork the seam and strand mod-authored properties outside candidacy | AC 4 |
| Both fixed input tables are append-only — a name's index is its runtime read handle | Task 1, Task 2 | An insertion or reorder silently re-points every bound program | AC 5, 6 |
| An unauthored graph and every legacy brain behave bit-identically to today: no filter is evaluated, no new edge is lowered | Task 2, Task 3 | Any default filter, or a lowering that emits a target-death edge | AC 5, 8 |
| Bound programs stay derived data in the evaluator side-table, rebuilt via the `Arc` pointer-identity staleness test; the filter joins them rather than riding the component | Predecessor, extended by Task 2 | A filter program stored on `BrainComponent` would reintroduce serde and equality coupling | AC 6 |
| Acquisition-path evaluation is zero-alloc per tick; the filter adds a constant factor to an existing traversal, never a new walk | Task 2 | Candidate-scope refresh must not intern, clone, or collect — the `@state.*` snapshot grows at bind alone | AC 6 |
| The floor's leash is symmetric: a target beyond it is neither acquired nor retained | Task 3 | Any new acquisition path bypassing the selection chokepoint; the ordering validator alone cannot enforce it, since Rust fixtures bypass validation | AC 9, 10 |
| A brain with no leash keeps unbounded acquisition, with disengagement owned by its guards | Task 3 | Any default value substituted for the absent leash | AC 5 |
| Acquisition-range authority stays engine-side and legacy-only: no descriptor field spells disengagement range. An authored graph bounds acquisition with a distance filter instead | Predecessor (pinned), extended by Task 2 | Task 3 must thread the existing component field, never introduce an authored one; a future perception spec must reach for the filter before a new descriptor range | AC 5, 14 |
| Engagement radius stays combat-slot spread only — never acquisition, retention, or damage | Predecessor (pinned) | Any task tempted to reuse it as an acquisition radius | AC 5 |
| Graph diagnostics are warnings, never errors: a relentless pursuer is a legitimate authored design | Task 4 | Escalation to a validation error would reject shipping content | AC 11, 12 |

## Script syntax examples

```ts
// Proposed design
behavior: {
  initial: "idle",
  // Per-GRAPH eligibility, asked once per candidate during acquisition.
  // Not a guard: guards run against a target that has already been chosen.
  //
  // Three clauses: not dead, within this graph's acquisition radius, and not
  // flagged untargetable by whatever impact policy owns that field. The radius
  // lives HERE — the floor spells no acquisition range for an authored graph,
  // so a distance clause is how a graph gets one. `state()` reads the
  // CANDIDATE's field in this position and the enemy's inside a guard: the
  // scope decides the entity, not the leaf. No `and` opcode — conjunction is a
  // nested `select(a, b, false)`.
  candidateFilter: runtime.select(
    candidate.died,
    false,
    runtime.select(
      runtime.le(candidate.distance, ACQUIRE_RANGE),
      runtime.eq(state("untargetable"), 0),
      false,
    ),
  ),
  interrupts: [
    // Target loss first, as always — it outranks every range guard.
    { to: "idle", when: runtime.select(brain.hasTarget, false, true) },
    // Then target death. `brain.targetDied` is false with no target, so this
    // edge is inert untargeted and the interrupt above stays the one that
    // handles loss. Do NOT spell this `le(brain.targetHealth, 0)`: with no
    // target that reads zero and fires for the wrong reason.
    { to: "idle", when: brain.targetDied },
  ],
  // ... states as before
}
```

## Open questions

- Warning and error messages name state names and the `components.behavior`
  path but not the owning entity's canonical name, which the descriptor
  validators do not have in hand. Threading it would widen every descriptor
  type's validator signature. Accepted as-is; revisit if authors report the
  diagnostics as hard to place.
- **Where line of sight lands once it exists.** This plan puts perception
  upstream of candidacy: the floor decides what is perceivable, the filter
  narrows what is worth engaging. That leaves one call for the perception spec —
  whether visibility stays a floor gate on the offer set, or is *also* published
  as a `@candidate.visible` fact so a graph can author "engage only what I can
  see" as taste. Both read as consistent with the split; only the second lets a
  mod hold fire on an unseen target without an engine change, and only the first
  keeps one answer to "what can this enemy perceive". Not decided here because
  no resolver exists yet to decide it against.
- **Where target ranking lands.** Two homes are now plausible and the seed
  research assumes the first: widening the `visible` predicate from `bool` to a
  weight, or a second per-graph IR expression producing a score over the same
  `@candidate.*` namespace this plan builds. The second is the cheaper one after
  this plan lands — namespace, scope, binding, and staleness are all in place —
  and it puts ranking policy on the taste side with eligibility. Whichever wins,
  the filter stays boolean: a score belongs in its own seam, not in a widened
  return type.
- **Threat and last-attacker policies have no seam.** "Prefer whoever just hurt
  me" is a relation over enemy × candidate, and both scopes are per-entity: the
  brain scope names one enemy, the candidate scope one candidate. Expressing it
  needs per-pair memory on the brain, which nothing here provides and no task
  should improvise. The dimension is deferred whole, not half-seamed.
