# Enemy Aggro Model

## Goal

Close the three acquisition/leash defects the behavior-state-graph review panel
deferred: the leash gates retention but not acquisition (permanent oscillation
when `leashRange < detectionRange`), dead pawns stay valid targets (a downed
co-op player holds an enemy hostage), and a graph authored with no
disengagement edge is a silent level-wide pursuer. Target acquisition and leash
semantics inside the engine floor become symmetric, aliveness-aware, and
diagnosable — without moving any of them onto the authored surface.

## Scope

### In scope

- The engine floor's leash becomes a **target-relevance radius**: it bounds
  fresh acquisition on the same terms it already bounds retention. Applies only
  to brains that carry one — legacy `components.ai`, which seeds it from
  `leashRange`. Authored graphs carry none and are unaffected.
- A parse-time ordering rule on `components.ai`: `leashRange` must be `>=`
  `detectionRange`. A descriptor that inverts them is a validation error naming
  both values.
- Target candidacy gains an aliveness test, reusing the floor's existing
  predicate: a candidate must carry health that is positive and finite.
  Acquisition, retention, and re-ranking all inherit it.
- Two structural diagnostics on an authored behavior graph, emitted at
  descriptor validation as warnings: a graph whose engaging states have no edge
  to a non-engaging state (level-wide pursuer), and a graph with engaging states
  but no interrupt reading the has-target fact (the no-target distance sentinel
  trap).
- Author-facing documentation of the resulting contract: what the floor
  guarantees about acquisition range and aliveness, and what an authored graph
  still owns.

### Out of scope

- Any authored disengagement or acquisition range on `components.behavior`.
  Pinned by the predecessor: it would be a second spelling of disengagement with
  undefined precedence against the guards. Unchanged here.
- Perception of any kind — line of sight, sound, alert propagation, aggression
  profiles, memory/search. The visibility predicate on the selection chokepoint
  stays untouched and unused.
- Threat or priority ranking policies. Selection stays nearest-with-hysteresis.
- Changing the engagement radius, which is combat-slot spread and nothing else.
- Changing think-stride bands, switch hysteresis, the damage chokepoint, the
  aggro gate, or host-only evaluation.
- Wire or replication changes. All of this is host-side sim state.
- Retiring `components.ai`. Legacy lowering stays exactly as it is.

## Acceptance criteria

- [ ] A legacy enemy tuned with a leash smaller than its detection range no
      longer oscillates: seeded at rest with a pawn between the two radii, it
      stays at rest indefinitely, requests no destination, and changes state on
      no tick.
- [ ] Authoring `components.ai` with a leash range below its detection range is
      a validation error in both QuickJS and Luau, and the message names both
      field values.
- [ ] Every existing legacy AI behavior test with well-ordered tuning passes
      unchanged — same transition ticks, damage cadence, animations, facing,
      stride, and combat slots.
- [ ] An enemy engaged with a co-op pawn that drops to zero health releases it
      on the next tick and engages a live pawn standing beside it, even though
      the live pawn is within the switch-hysteresis margin of the dead one.
- [ ] When every pawn is dead, an engaged enemy stands down to its graph's
      initial state within one tick and requests no travel animation on the way.
- [ ] An enemy never acquires a pawn that carries no health at all.
- [ ] A behavior graph whose engaging states offer no edge to a non-engaging
      state validates successfully and logs one warning naming those states.
- [ ] A behavior graph with engaging states and no has-target interrupt
      validates successfully and logs one warning explaining the sentinel
      asymmetry.
- [ ] The shipped reference enemy graph triggers neither warning; a fixture
      graph exists for each warning.
- [ ] Warnings are emitted once per descriptor at validation, not per spawn and
      not per tick.
- [ ] The scripting reference documents the floor's acquisition contract:
      aliveness, the legacy leash's dual role, and that an authored graph owns
      its own disengagement.

## Tasks

### Task 1: Aliveness in target candidacy

In `crates/postretro/src/scripting/systems/ai/targeting.rs`, make the candidate
test consult health. `target_candidate` currently admits any entity carrying
`PlayerMovementComponent` and `Transform`; it must additionally require the
entity to satisfy `selected_target_alive` (already in that file: positive,
finite `HealthComponent.current`, and `false` when the component is absent).
Because `nearest_target_candidate` and the retained-candidate path both funnel
through `target_candidate`, this single edit covers acquisition, retention, and
re-ranking; no call-site signature changes. Keep the `selected_target_alive`
call at the attack gate in `ai/mod.rs` — it is now belt-and-braces rather than
the sole aliveness check, and removing it would put the damage chokepoint's
correctness at the mercy of a distant module. Re-point
`selected_dead_target_suppresses_attack_even_when_other_pawn_is_alive` in
`ai_tests.rs`: with the dead pawn no longer a candidate the enemy selects and
damages the live one, so the attack gate's own coverage moves to a fixture whose
only pawn is dead. Add the co-op release case and the health-less pawn case.

### Task 2: Leash bounds acquisition

Give the floor's leash the same authority over fresh acquisition it already has
over retention. Today `BrainComponent::leash_range` (`Option<f32>`, `Some` only
for lowered legacy brains) is consulted in `ai/mod.rs` for the retained
candidate and for the replacement search on a leash-escape tick, but the
ordinary acquisition scan applies no range limit — which is the whole
oscillation. Move the rule into `targeting.rs`, the natural home, so the
oversized tick module only passes a value: `select_target` and
`nearest_target_candidate` take the brain's optional leash and reject any
candidate beyond it, and `ai/mod.rs` passes `brain.leash_range` at each of its
call sites (the early non-engaged scan, the leash-escape replacement, and both
acquisition-due branches). The existing post-hoc leash filter on the replacement
search becomes redundant and should collapse into the new parameter rather than
stay as a second spelling. A brain with no leash — every authored graph — keeps
today's unbounded behavior exactly. Separately, add the ordering rule to
`AiDescriptor::validate`
(`crates/foundation/src/data_descriptors/types/combat.rs`): after the existing
finite-and-positive loop, reject a descriptor whose `leashRange` is less than
its `detectionRange`, with a message naming both values in the established
`components.ai.<field>` style. Both runtimes funnel through that validator, so
one edit covers QuickJS and Luau. Rust-side fixtures that build `AiDescriptor`
literally bypass the validator, which is why the floor rule above must stand on
its own; the two Rust test fixtures using inverted tuning
(`ai_tests.rs`, around the leash-versus-detection cases) become the
non-oscillation regression instead of being deleted.

### Task 3: Graph disengagement lints

Add two structural diagnostics for authored behavior graphs. Put them in a new
sibling module beside
`crates/foundation/src/data_descriptors/types/behavior.rs` (824 lines — do not
extend it), exposing one entry point that
`BehaviorGraphDescriptor::validate` calls after its existing structural checks
succeed. Both are pure functions of the descriptor; neither needs runtime state.
A state is ENGAGING when its motion verb is the chase verb or it declares any
action — the same predicate the evaluator's `engages` uses, restated here over
the descriptor rather than over a resolved state index. Lint one: if the graph
has at least one engaging state and no engaging state declares a transition
whose destination is a non-engaging state, warn that the graph pursues without
limit, naming the engaging states. Lint two: if the graph has at least one
engaging state and no entry in `interrupts` has a guard tree containing an input
read of the has-target brain fact, warn that target loss will be handled through
distance guards, where the no-target sentinel makes `gt`/`ge` read true. Detect
the input read by walking the guard's `IrNode` tree for an `Input` node whose
name equals the has-target constant in `crates/foundation/src/brain.rs`;
`foundation` already depends on `log`, so emit through `log::warn!` — validation
returns a `Result` and these are not errors, so they do not join the return
type. Warnings carry the offending state names and the `components.behavior`
path prefix; they fire once per descriptor validation, which is once per parse,
not per spawn. Confirm the shipped reference enemy graph
(`sdk/behaviors/reference/entities.{ts,luau}`) trips neither, and add a fixture
graph for each.

### Task 4: Author-facing contract

Document the resulting floor contract in `docs/scripting-reference.md` beside
the existing behavior-graph material, and update the `components.ai` field
descriptions in the typedef generator
(`crates/postretro/src/scripting/primitives/mod.rs`, the `detectionRange` and
`leashRange` field strings) so the emitted SDK typedefs state the ordering
constraint and the leash's dual acquisition/retention role. Regenerate and
commit both typedef fixtures. Extend the reference enemy's authoring comments to
name the two new warnings, so the file that already teaches the sentinel
asymmetry also tells an author what the engine will say when they skip the
lesson. Keep the TypeScript and Luau spellings identical, as the predecessor
requires.

## Sequencing

**Phase 1 (concurrent):** Task 1, Task 3 — disjoint files (targeting versus
foundation descriptor validation).
**Phase 2 (sequential):** Task 2 — shares `targeting.rs` with Task 1 and builds
on its candidate shape.
**Phase 3 (sequential):** Task 4 — documents the contract the first three tasks
settle.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| The floor's leash is symmetric: a target beyond it is neither acquired nor retained | Task 2 | Any new acquisition path that bypasses the selection chokepoint; the ordering validator alone does not enforce it, since Rust fixtures bypass validation | AC 1, 2, 3 |
| Acquisition-range authority stays engine-side and legacy-only: no descriptor field on the behavior block spells disengagement | Predecessor (pinned) | Task 2 must thread the existing component field, never introduce an authored one | AC 3 |
| A brain with no leash behaves exactly as today — unbounded acquisition, disengagement owned by its guards | Task 2 | Any default value substituted for the absent leash | AC 7, 8 |
| Aliveness is decided once, by one predicate, for candidacy and for damage | Task 1 | The attack gate keeps its own call; the two must not drift to different definitions | AC 4, 5, 6 |
| Engagement radius stays combat-slot spread only — never acquisition, retention, or damage | Predecessor (pinned) | Any task tempted to reuse it as an acquisition radius | AC 3 |
| Graph disengagement diagnostics are warnings, never errors: a relentless pursuer is a legitimate authored design | Task 3 | Any escalation to a validation error would reject shipping content | AC 7, 8, 9 |
| Warnings are per-descriptor, not per-entity: validation runs at parse, spawn does not repeat it | Task 3 | Moving the lint to the spawn/attach path would make it per-instance | AC 10 |

## Open questions

- The warning messages name state names and the `components.behavior` path but
  not the owning entity's canonical name, which the descriptor validator does
  not have in hand. Threading it would mean widening the validator's signature
  across every descriptor type. Accepted as-is; revisit if authors report the
  warnings as hard to place.
