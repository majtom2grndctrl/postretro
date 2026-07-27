# Enemy Aggro Model

> **Superseded — historical reference only.** Do not implement this plan's
> `components.ai` or leash architecture. Active work:
> [`E10--retire-legacy-ai`](../E10--retire-legacy-ai/).

## Goal

Close three acquisition/leash defects the behavior-state-graph review panel
deferred. Two are engine-floor correctness: the leash gates retention but not
acquisition, so a brain tuned `leashRange < detectionRange` oscillates forever;
and a graph with no disengagement edge is a silent level-wide pursuer. The
third — dead pawns holding enemies hostage — is a missing vocabulary, not a
missing check. The graph carries facts about the brain and none about its
target, so "worth attacking" is hardcoded in the engine. This plan gives mods
target-side facts and a candidacy filter, and moves that judgement where the
rest of taste already lives.

## Scope

### In scope

- **Target-side brain facts.** New `@brain.*` guard inputs describing the
  selected target: current health, max health, and whether the engine's death
  latch has fired for it. Disengaging from a downed target becomes an authored
  interrupt instead of engine policy.
- **A per-graph candidate filter.** One optional IR predicate on the behavior
  block. Compiled once per bound brain, evaluated per candidate on every ranking
  scan, over a namespace of engine facts about the candidate: distance, health,
  max health, and its death latch. The floor keeps deciding which entities are
  *offered* — player pawns, and later the visibility predicate; the mod decides
  which offered candidates are worth engaging. Unauthored, nothing is evaluated
  and behavior is bit-identical to today.
- **A symmetric floor leash**: a target-relevance radius bounding fresh
  acquisition on the same terms it already bounds retention. Legacy
  `components.ai` only — authored graphs carry no leash.
- **A parse-time ordering rule on `components.ai`:** `leashRange >=
  detectionRange`. Inverting them is a validation error naming both values.
- **Two structural warnings on an authored graph**, emitted at descriptor
  validation: engaging states with no edge to a non-engaging state (level-wide
  pursuer), and engaging states with no has-target interrupt (the no-target
  sentinel trap).
- The reference enemy adopts the new vocabulary and becomes the worked example.

### Out of scope

- Any engine-side aliveness rule for targeting. The attack gate's aliveness
  check stays where it is and keeps its meaning; selection gains none.
- Any authored disengagement or acquisition range *field* on
  `components.behavior`. Pinned by the predecessor: a second spelling of
  disengagement with undefined precedence against the guards. A distance clause
  inside the filter is how an authored graph bounds acquisition instead (AC 15).
- Changing what legacy lowering emits. No new edges, no filter; hostage behavior
  is preserved. Legacy *acquisition* still changes — it gains the floor's leash
  bound. Task 3 records why that bound cannot be a lowered filter.
- Perception — line of sight, sound, alert propagation, aggression profiles,
  memory and search. The visibility predicate on the selection chokepoint stays
  untouched and unused; candidacy sits downstream of it.
- Threat or priority *ranking* policies. The filter decides eligibility, never
  order; ranking stays nearest-with-hysteresis.
- Per-state candidacy. The filter is per-graph, deliberately.
- Mod-authored `@state.*` leaves on the candidate scope. The filter reads engine
  facts only. See "Where the deferred dimensions land" for the write-path
  constraint that has to lift first.
- Writes from the filter or from guards. Both are read-only.
- Changing the engagement radius, think-stride bands, switch hysteresis, the
  damage chokepoint, the aggro gate, or host-only evaluation.
- Renaming the trigger system's `alive_players` misnomer (see `research.md` §4).
- Network replication changes.

## Acceptance criteria

- [ ] A graph can author an interrupt that stands the brain down when its
      selected target's death latch has fired. An enemy engaged with a co-op
      pawn that dies releases it on the tick after the death sweep commits, and
      engages a live pawn beside it even inside the switch-hysteresis margin. A
      pawn killed by another enemy inside the same AI tick reads `targetDied`
      false that tick and true the next. The graph under test authors the filter
      as well as the interrupt: standing down clears the target, so the next
      scan is offered the corpse with no retained target to seed hysteresis, and
      without the filter the corpse is simply re-acquired. An integration
      criterion over Tasks 1 and 2, not a Task 1 one. The latch is set by a
      different system than the one under test — `sweep_deaths`
      (`health.rs:77`) has one non-test caller, `impact_effects.rs:360`, and
      `run_ai_tick` does not run it — so the fixture drives the latch itself
      between ticks, directly or by calling `sweep_deaths`.
- [ ] With no selected target, each of the three new target-side facts reads its
      type's zero and the boolean death fact reads false. `@brain.targetDistance`
      is excluded: it is a target-side fact and keeps `BRAIN_NO_TARGET_DISTANCE`,
      the lone exception to the zero convention. A graph with no has-target
      interrupt is exactly what lint two warns about.
- [ ] A graph can author a candidate filter that excludes dead pawns. An enemy
      whose only nearby pawn is dead acquires nothing and stays at rest; the
      same enemy with a live pawn in range engages normally.
- [ ] A graph with no candidate filter and a brain with no leash select targets
      exactly as today. Review gate, not a runnable test: no existing AI test
      changes its asserted behavior, with the exceptions each owning task names —
      the three reference-enemy parity tests in Task 5, which track authored
      content rather than engine behavior, and the one retuned stride fixture in
      Task 3. Within the target-selection
      surface — `targeting.rs` and `ai/mod.rs`'s selection block — the
      mechanical edits are exactly these:

      | Edit | Detail |
      |---|---|
      | `target_candidate` | Signature unchanged |
      | `select_target`, `nearest_target_candidate` | Gain the leash parameter, the bound-filter and `&mut CandidateScope` parameters, and the two-value return. Both already take `&EntityRegistry` |
      | Doc comments | `select_target`'s is rewritten; it is the only one of the three carrying one (`targeting.rs:83-96`) |
      | New in `targeting.rs` | `retained_is_outside_leash` |
      | Call sites | Four `select_target` sites in `ai/mod.rs`, six direct ones in `ai_tests.rs:728-806` |
      | `BrainFacts` | Target identity folds in at the one tick site (`ai/mod.rs:557`) |

      This bounds that surface only; every task adds its own new types and
      fields elsewhere — including Task 2's mechanical churn across the 30
      `BehaviorGraphDescriptor` struct literals, which is outside this bound.
      Also a review gate: `engagementRadius` is still read for combat-slot
      spread alone.
- [ ] A candidate filter is compiled once per bound brain, inside `sync`, on the
      same `Arc` pointer-identity staleness edge that rebinds the graph's
      guards — never on a tick where the `Arc` is unchanged. It evaluates per
      candidate with no heap allocation on the acquisition path: the fixed fact
      array is written by index at refresh and never grown (alloc-probe
      assertion, matching the substrate invariant). "Compiled never per
      candidate" is a review gate — no alloc probe or staleness test reaches it,
      since a per-candidate rebind inside the armed window would show up as an
      allocation but one outside it would not.
- [ ] A candidate filter that fails to bind is a validation error in both
      QuickJS and Luau, with the authored path in the message. A filter
      producing a non-boolean is rejected the same way.
- [ ] Target selection remains aliveness-free; no task adds a rule.
      `selected_target_alive` keeps gating the attack decision — cooldown,
      damage, attack event, clip restart — and never selection: an enemy whose
      nearest pawn is dead and whose graph authors no filter still selects it
      and is still blocked from attacking it. Review gate, except for the
      runnable half: extend `no_attack_or_event_when_player_already_dead`
      (`ai_tests.rs:1337`) to assert the acquired target is still the dead pawn.
- [ ] A legacy enemy tuned with a leash smaller than its detection range no
      longer oscillates. Seeded at rest with a pawn between the two radii, it
      stays at rest indefinitely, requests no destination, and changes state on
      no tick.
- [ ] Authoring `components.ai` with a leash range below its detection range is
      a validation error in both runtimes, naming both field values.
- [ ] A behavior graph whose engaging states declare no state-local transition
      to a non-engaging state validates successfully and returns one finding
      naming every engaging state, in `states` key order, asserted on directly
      rather than through captured log output. Graph-level interrupts do not
      count as such an edge — see Task 4 for why.
- [ ] A behavior graph with engaging states and no has-target interrupt
      validates successfully and returns one finding explaining the sentinel
      asymmetry. Findings are produced once per descriptor validation, not per
      spawn and not per tick — a review gate, held today by `validate` having
      exactly two non-test callers, both parse-time bridges.
- [ ] The shipped reference enemy trips neither warning, authors both the
      target-death interrupt and the candidate filter, and is behavior-identical
      to its Luau twin. It declares the has-target interrupt before the
      target-death interrupt, so a target-side fact read untargeted never
      reaches a stand-down first.
- [ ] SDK typedef drift tests pass with the new brain facts, the `candidate`
      prelude, and the filter field in both generated artifacts
      (`sdk/types/postretro.d.ts`, `sdk/types/postretro.d.luau`).
      `brain_input_typedefs_match_the_foundation_table` is order-sensitive
      (`Vec<String>` equality) and its `CANDIDATE_INPUTS` twin must be too;
      `brain_sdk_helpers_cover_every_brain_input` compares sets and counts and
      is deliberately not. Byte-exact equality of the two committed artifacts is
      a third test, `committed_sdk_types_match_current_registry`. Review gates,
      not runnable tests: the `components.ai` `leashRange` description states
      measurement from the enemy's current position rather than its origin, both
      range descriptions state the ordering rule, and the scripting reference
      documents the floor's acquisition contract, the target-side facts'
      no-target readings, that an authored graph owns its own disengagement,
      that a distance filter is the authored acquisition radius, and that
      `le(targetHealth, 0)` is not the death test.
- [ ] The filter is never consulted for the retained-target lookup. An enemy
      already engaged with a pawn its filter excludes is not dropped by the
      filter; it is displaced only by switch hysteresis, by the floor's leash,
      or by a guard standing the brain down.
- [ ] An authored graph whose filter is `le(candidate.distance, R)` acquires a
      pawn at `R - ε` and does not acquire one at `R + ε`, with no `leashRange`
      in play.
- [ ] A relevance rule never reprices the think stride. Under the
      stride-inverting wiring the relevance rule empties `current_distance` and
      `acquisition_due` reads `None` as due every tick — and every other
      criterion here, plus all three legacy leash fixtures, passes under it. So
      this criterion is the only thing standing between the plan and that
      inversion, and each half needs an observable the inverted wiring actually
      fails. Calling `acquisition_due` directly is not one: the test would
      supply its own distance and re-derive the answer rather than observe the
      wiring. The two halves observe it differently, because the states differ
      in how long they last:

      | Half | State | Observable |
      |---|---|---|
      | Legacy (Task 3) | Transient — one tick. The floor clears an out-of-leash retained target on the tick it sees it, and the brain rescans from non-engaged thereafter | A single tick. Engaged, `think_stride_counter` at 0, retained pawn at far-band distance outside the leash, a second pawn inside it. Correct: the far band's divisor makes tick 1 not due, the strided replacement does not run, and `acquired_target` is `None` after it. Inverted: `current_distance` is `None`, tick 1 is due, and the replacement search returns the second pawn |
      | Filter (Task 2) | Stable — the filter never drops a retained target (AC 14), so a far-band engaged enemy holds it indefinitely | A window. The graph authors an edge guarded on `@brain.acquisitionDue` and the test counts state flips across it. Correct: the far band's divisor. Inverted: every tick |

      `think_stride_for_distance` (`engine_floor.rs:35`) is `pub(crate)`, so both
      derive the divisor from the band rather than hardcoding it; `STRIDE_FAR`
      itself is private and stays that way.
- [ ] Populated, the health facts read the entity's component. With a selected
      target carrying a `HealthComponent`, `@brain.targetHealth` and
      `@brain.targetMaxHealth` read its current and maximum values; a target
      without that component reads zero for both. The same holds for
      `@candidate.health` and `@candidate.maxHealth` against each offered
      candidate during a ranking scan. AC 2 covers only the untargeted zeros, so
      without this nothing asserts these four facts are wired to anything. Brain
      half is Task 1's, candidate half Task 2's. The brain half needs its
      fixture retuned as well as extended: `seeded_registry`
      (`brain_scope.rs:232`) gives both entities `max: 40.0`, so a
      `targetMaxHealth` wired to the *enemy's* component passes. The two
      entities must carry distinct `max` values.

## Tasks

### Task 1: Target-side brain facts

Extend the fixed guard-input table with three facts about the selected target.
Add them in `crates/foundation/src/brain.rs`, appended to the end of
`BRAIN_INPUTS` — that table's order **is** the runtime read handle, so append,
never insert. The array length literal `[(&str, IrType); 7]` grows to 10.

| Constant | Wire name | Type |
|---|---|---|
| `BRAIN_TARGET_HEALTH_INPUT` | `"@brain.targetHealth"` | Number |
| `BRAIN_TARGET_MAX_HEALTH_INPUT` | `"@brain.targetMaxHealth"` | Number |
| `BRAIN_TARGET_DIED_INPUT` | `"@brain.targetDied"` | Bool |

The third reads `HealthComponent.death_handled`, the engine's one-shot death
latch. With no selected target each fact reads its declared type's zero. No new
sentinel is introduced. Doc comments must state plainly that
`@brain.hasTarget` is the only authoritative presence test and that
`BRAIN_NO_TARGET_DISTANCE` remains the lone exception to the zero convention.

**Why the latch, not a health comparison.** Two reasons. It is unambiguous with
no target: every target-side fact reads zero untargeted, so `le(targetHealth, 0)`
fires on *no target* exactly as loudly as on a corpse, while `targetDied` reads
false. And it carries the death sweep's full definition, the non-finite arm
included, rather than an author's re-derivation.

Neither fact is coupled to AI iteration order, and this plan must not claim
otherwise. The tick is three passes: Pass 2 (`ai/mod.rs:420-646`) evaluates
every brain — `BrainScope::refresh` at `:554` included — before Pass 3 (`:650`)
applies any damage at `:796`. No enemy's swing is visible to another enemy's
guards on the tick it lands, so both facts are uniform across observers and
`targetDied` turns true on the tick after death commits. What the floor does
*not* provide is same-tick overkill suppression: `selected_target_alive`
(`targeting.rs:76`) is read at `ai/mod.rs:615`, inside the compute pass, and
gates the attack decision against health as of the tick's start, so two enemies
both selecting the same low-health pawn both pass it and both damage it in Pass
3. Existing floor behavior; unchanged here.

Populate the facts in `crates/postretro/src/scripting/systems/ai/brain_scope.rs`.
`BrainFacts` folds distance and identity into one binding —
`target: Option<(EntityId, f32)>` — preserving the existing "cannot disagree"
guarantee; its doc comment updates to say so. `refresh` reads that entity's
`HealthComponent` the way it already reads the evaluating enemy's: absent
component reads as zeros, absent target likewise. The tick site in `ai/mod.rs`
already computes the selected target immediately before building `BrainFacts`,
so it builds the folded binding at that one call site. Nothing else moves.
`BrainValidationScope` needs no change beyond the table growing, since it
resolves through `resolve_brain_input`.

Extend the hand-maintained `brain` prelude object in both runtimes with three
pre-wrapped input leaves. In `brain.ts` each leaf must be spelled exactly
`{property}: Object.freeze(runtime.read("{name}"))` — the TS half of the drift
test below text-matches that literal. The `BrainSdk.brain` doc comment in
`brain.luau` (`:25-28`) enumerates the field names in prose and no test reads
it, so it goes stale silently; extend it too. Two drift tests guard two
different surfaces:

| Test | Location | Guards |
|---|---|---|
| `brain_sdk_helpers_cover_every_brain_input` | `crates/scripting-core/src/data_descriptors/tests/behavior.rs:372` | Both preludes: `brain.luau` is evaluated and its `brain` table's keys set-compared to `BRAIN_INPUTS` (`behavior.rs:398`) with per-leaf `op`/`name` checks (`:403-407`); `brain.ts` is text-matched per input, with its `"@brain.` literal count compared to `BRAIN_INPUTS.len()` (`:417-421`) |
| `brain_input_typedefs_match_the_foundation_table` | `crates/postretro/src/scripting/typedef/tests/surface.rs:584` | The emitted `BrainInputs` typedef block |

New doc-comment prose in `brain.ts` must not contain `"@brain.`, or the count
inflates and the first test fails.

The `BrainInputs` block is hand-written text in two typedef templates:
`crates/scripting-core/src/typedef/templates/sdk_lib.d.ts` (`export interface
BrainInputs {`, line 842) and `sdk_lib.luau` (`export type BrainInputs = {`,
line 1073). Task 1 edits both in the same phase, so the typedef drift test never
goes red. `committed_sdk_types_match_current_registry` (`committed.rs:8`) is the
plan's one expected-red window: committed artifacts are regenerated only in
Phase 4, so it stands red from Phase 2 until Task 5 closes it.
`virtual_module.luau` needs no Task 1 edit; it names the type without declaring
its fields.

Sites that break under the `BrainFacts` fold:

| Site | Why |
|---|---|
| `brain_scope.rs:260` (`engaged_facts()`) | Construction |
| `brain_scope.rs:404` | Struct-update literal naming `target_distance` explicitly |
| `brain_programs.rs:716`, `:754`, `:801`, `:858` | Construction |
| `ai_tests.rs:373` | Construction, in the guard-evaluation harness |
| `brain_scope.rs:114-122` | Seven-element fixed-slot array against `[IrValue; BRAIN_INPUTS.len()]` |
| `brain_scope.rs:346`, `:348` (`expected_fixed_value` body) | Field reads of `facts.target_distance` |

`expected_fixed_value` (`brain_scope.rs:344`) ends in a `panic!` catch-all, not
a wildcard, so a missing arm fails
`refresh_projects_engine_facts_and_health_into_the_fixed_slots` at test runtime
with a naming message rather than at compile time. The genuine compile failures
are the array literal, every `BrainFacts` construction site, and the two
`facts.target_distance` reads in `expected_fixed_value`. The `panic!`
catch-all defers only the missing-arm failure to test runtime.

Task 1 owns the runnable tests for AC 2 — every target-side fact reads its
type's zero with no selected target and `targetDied` reads false — and the
brain half of AC 17: `@brain.targetHealth` and `@brain.targetMaxHealth` read a
selected target's `HealthComponent`, and zero when that component is absent.
Both extend `refresh_projects_engine_facts_and_health_into_the_fixed_slots`.
That test's helper takes one health component — `expected_fixed_value(name,
facts, health: &HealthComponent)` — and it is the *enemy's*. The three new arms
need the target's, so the helper gains a second health parameter. Retune
`seeded_registry` (`:232`) as AC 17 requires: its two entities differ in
`current` but share `max: 40.0`, which lets a `targetMaxHealth` wired to the
enemy pass.

### Task 2: Per-graph candidate filter

Add an optional IR predicate to the behavior block deciding which offered
candidates an enemy will acquire.

**Descriptor side**, in `postretro-foundation`. `BehaviorGraphDescriptor` gains
`candidate_filter: Option<IrNode>`, wire key `candidateFilter`. The descriptor
is `rename_all = "camelCase"`, so no explicit rename is needed. It also carries
`deny_unknown_fields`, so the key needs `#[serde(default)]` and old serialized
graphs must still deserialize. That is the only direction that matters:
`deny_unknown_fields` means a reader predating this change rejects a graph
carrying `candidateFilter`, which is why the key is additive content rather than
replicated state. The raw node follows the same descriptor-partition rule as
transition guards — the bound program is derived data the evaluator owns, never
a descriptor field. `primitives/mod.rs` is the typedef-generation registry, not
the parse path, so script authoring of `candidateFilter` works from Phase 3
through the descriptor's serde; Task 5 registers it for the emitted types.

The descriptor has no `Default` derive and is built by 30 struct literals across
seven files — `behavior.rs:498`, `behavior_lowering.rs:70`,
`entities/src/components/brain.rs:590`, `brain_programs.rs`, `ai_tests.rs`,
`builtins/data_archetype_test_fixtures.rs`, and
`netcode/enemy_replication_harness_test.rs` — so the new field compile-breaks
all of them. The additions are mechanical `candidate_filter: None`; AC 4 bounds
the target-selection surface and does not cover this churn. While editing the
type, correct its doc comment (`behavior.rs:130`), which gives the engine
"target selection" outright — the same claim the promotion prerequisites amend
in `entity_model.md`.

The filter needs its own namespace and its own declaration-time binding scope,
both in a new module `crates/foundation/src/candidate.rs`, beside `brain.rs`
rather than inside it, and both shaped like the brain pair's fixed half. A table
of `@candidate.`-prefixed engine facts under `CANDIDATE_INPUT_PREFIX =
"@candidate."`, whose order **is** the runtime read handle, so it grows by
appending and never by insertion:

| Constant | Wire name | Meaning |
|---|---|---|
| `CANDIDATE_DISTANCE_INPUT` | `"@candidate.distance"` | XZ distance from the enemy |
| `CANDIDATE_HEALTH_INPUT` | `"@candidate.health"` | Current health |
| `CANDIDATE_MAX_HEALTH_INPUT` | `"@candidate.maxHealth"` | Max health |
| `CANDIDATE_DIED_INPUT` | `"@candidate.died"` | Death latch |

`died`, not `targetDied` — the scope already names the entity. The table is the
namespace's whole content: a filter reads engine facts about the candidate and
nothing else. One resolve function mirroring `resolve_brain_input` answers it, a
validation scope resolves through it, and a bind helper mirrors the existing
guard-bind helper. `BehaviorGraphDescriptor::validate` binds the filter when
present and rejects an unbindable or non-boolean one with the authored path
`components.behavior.candidateFilter`, exactly as it already does per
transition. Both runtimes emit that string.

**Runtime side**, in the binary. A candidate `BindingScope` in a new module
`crates/postretro/src/scripting/systems/ai/candidate_scope.rs`, beside
`brain_scope.rs`. One snapshot: a fixed array written by index, sized to
`CANDIDATE_INPUTS`, never grown. Its refresh signature is `refresh(&mut self,
registry: &EntityRegistry, candidate: EntityId, from: Vec3)`, mirroring
`BrainScope::refresh`, with `from` the enemy position
`nearest_target_candidate` already holds.
`nearest_target_candidate` and `select_target` already take `&EntityRegistry`
(`targeting.rs:47`, `:98`), so the scan needs no new registry plumbing to call
it. A candidate with no `HealthComponent` reads zeros for the three health
facts, the same absent-component contract the brain scope honors.

Binding rides the existing evaluator side-table. `BrainPrograms` holds one
shared candidate scope alongside its brain scope, and `BrainEntityPrograms`
gains one optional bound filter program compiled during the same `sync` pass
that binds the graph's guards, keyed off the same `Arc` pointer-identity
staleness test.

**Where evaluation goes.** Inside `nearest_target_candidate` (`targeting.rs:46`),
the ranking scan — and **not** inside `target_candidate` (`:27`), the shared
lookup that also resolves the retained target and that `ai/mod.rs:434` calls
directly to price the think stride. Filtering there would make the engine decide
disengagement again, which the graph owns: see the "Candidacy is per-graph
eligibility" invariant and AC 14.

Task 3 already turned that scan into a single fold with two accumulators — the
nearest candidate over every offered pawn, and the nearest *eligible* one — and
gave `select_target` its two-value return. The filter is the second relevance
rule to ride that shape: it joins the leash on the eligible accumulator and
never touches the raw-distance one. The scan's shape does not change here.

`select_target` gains the bound filter and the shared candidate scope as two
parameters, never `&BrainPrograms`, which would hand `targeting.rs` guard
programs it has no business reading. `ai/mod.rs` passes them at the four call
sites Task 3 established: the early non-engaged scan (`ai/mod.rs:437`), the
leash-escape replacement (`:455`), and both acquisition-due branches (`:471`,
`:488`). The six direct `select_target` call sites in `ai_tests.rs:728-806` are
compile-broken by this signature change and updated here; their assertions are
unchanged.

Getting both out of the side-table at once needs one accessor: the filter is a
shared borrow, the scope a mutable one, and the scan holds the filter across
repeated per-candidate refreshes. Split them by field:

```rust
// Proposed design
pub(crate) fn candidate_filter_context(
    &mut self,
    entity: EntityId,
) -> (Option<&BoundProgram<CandidateScope>>, &mut CandidateScope) {
    (
        self.entries.get(&entity).and_then(|entry| entry.candidate_filter()),
        &mut self.candidate_scope,
    )
}
```

The borrow checker splits `&mut self` across distinct fields inside the body, so
this needs no interior mutability and costs nothing at runtime. It is the
discipline the evaluator already follows: `select_transition` takes the graph,
the entry, and the scope as separate parameters rather than the table itself,
precisely so the caller owns the split.

`CandidateScope::refresh` therefore stays `&mut self`. The `&mut` is what makes
"written by index, never grown" a compile-time fact rather than a convention. A
`RefCell` on the fixed array would buy the same call pattern and give that up,
and the alloc probe would not notice, since `RefCell` does not allocate. The
`RefCell`s on `BrainScope` are a *bind*-phase need — `BindingScope::resolve_input`
takes `&self` — and are not a precedent for the refresh path.

Cost is bounded by construction. Refresh writes four slots and reads one
`HealthComponent`, per *(enemy, candidate)* pair, over an offer set of player
pawns. There is no interning, no name vector, and nothing that grows with the
number of bound filters, so one graph's filter cannot tax another's scan.

`None` means no filter: nothing is evaluated and the scan is byte-for-byte
today's. Lowering emits no filter, so legacy brains are unchanged.

**SDK surface.** Extend both runtimes with a `candidate` prelude object of
pre-wrapped leaves for the fixed table. `candidate` is a third top-level export
of the **existing** `sdk/lib/brain.{ts,luau}` module, not a new SDK module:
`sdk/lib/brain.ts` already owns the guard-input vocabulary, and `candidate` is
a second view of the same enemy-behavior authoring surface, so a new module
would fork one vocabulary across two files. Unlike a new key on the `brain`
object, a new top-level export is
gated by an explicit allowlist mirrored in six places:

| Site | What changes |
|---|---|
| `BRAIN_LUAU_FIELDS` (`crates/scripting-core/src/luau_prelude.rs:222`) | `&["brain", "state"]` — the module's *exports*, not the `brain` object's fields — gains `"candidate"` |
| `POSTRETRO_ROOT_MODULE_EXPORTS` (`luau_prelude.rs:271`; insert beside `"brain"` at `:290`) | Gains `"candidate"` |
| `luau_require.rs:446` | Test-only exact-key assertion (`#[cfg(test)]`, `assert_exact_string_keys` on the root module); gains `"candidate"` or the test goes red |
| `crates/script-compiler/src/light_membership.rs:496,502` | Both `copy_lua_fields` call sites |
| `sdk/lib/index.ts:18-19` | Re-exports `{ brain, state }`; gains `candidate` |

`CANDIDATE_INPUTS` needs its own drift-test pair mirroring the Task 1 names:
`candidate_input_typedefs_match_the_foundation_table` (order-sensitive
`Vec<String>` equality, per AC 13) and `candidate_sdk_helpers_cover_every_candidate_input`
(sets and counts, deliberately not order-sensitive), counting occurrences of
the literal `"@candidate.` in `sdk/lib/brain.ts` alone against
`CANDIDATE_INPUTS.len()`, and set-comparing the `candidate` table's keys out
of `sdk/lib/brain.luau`. New doc-comment prose in either file must not contain
that literal, and must not contain the string `BrainInputs` either: the brain
drift test finds its block by `skip_while(|line| !line.contains("BrainInputs"))`
(`surface.rs:593`) and would latch onto the wrong one. Task 2 therefore also
writes the `CandidateInputs` blocks into `sdk_lib.d.ts` and `sdk_lib.luau` in
this phase, exactly as Task 1 does for `BrainInputs` — including the top-level
`candidate` declaration lines, not just the interface blocks.

`luau_virtual_module_types_and_require_overloads_are_generated`
(`surface.rs:164`; the `POSTRETRO_ROOT_MODULE_EXPORTS` loop is at `:202`)
iterates `POSTRETRO_ROOT_MODULE_EXPORTS` and asserts the generated Luau contains
`"candidate:"`. Task 2's own `declare candidate: CandidateInputs` line satisfies
it within this phase: `generate_luau` concatenates the `sdk_lib.luau` block
ahead of `virtual_module.luau` (`typedef/luau.rs:252`) and the check is a
substring test over the whole output — `declare brain: BrainInputs`
(`sdk_lib.luau:1091`) is what satisfies `brain:` today. So there is no red
window, and the test does **not** guard the `PostretroModule` entry. Task 5's
`virtual_module.luau:120` edit is what makes `require("postretro").candidate`
type-check, and only review catches its absence. Task 5 keeps only
`virtual_module.luau` and the regeneration.

Task 2 and Task 4 both extend `BehaviorGraphDescriptor::validate`: all errors,
filter bind included, run first, and the lints run only on the success path, so
a rejected descriptor never also logs a warning. Both runtimes must agree.

**Verification.** Extend the alloc probe:
`refresh_and_guard_eval_perform_zero_heap_allocations` (`brain_scope.rs:482`)
gets a candidate-scope twin arming the probe **after** bind, over per-candidate
refresh and eval across a multi-candidate scan. Bind compiles a program and
allocates, which is why the existing probe's own comment puts binding outside
the armed window; the twin follows it. That test is AC 5's zero-alloc verifier. AC 5's other half — that the
filter compiles on the `Arc` staleness edge and not per tick — is a `sync` claim
no alloc measurement reaches; its verifier is a twin of
`sync_leaves_an_unchanged_brain_bound_without_rebinding` (`brain_programs.rs:527`).
Twin its *observable*, not a pointer comparison: `sync` rebinds by
`self.entries.insert(entity, entry)` (`:147`), which replaces a `HashMap` value
in place, so the bound program's address survives a rebind and a pointer
assertion passes even for a rebind-every-tick implementation. The existing test
uses a warn-once latch for exactly that reason — a filter with one unbindable
leaf warns on bind, and a rebind re-warns. Assert the latch stays empty across
the second `sync`.

Task 2 also owns the runnable tests for AC 1 (a co-op pawn
dies, the enemy releases it and engages a live pawn beside it, its graph
authoring both interrupt and filter), AC 3, AC 14, AC 15, AC 16's filter half, the candidate
half of AC 17, and AC 7's runnable half. AC 1's fixture must drive the death
latch itself: `HealthComponent.death_handled` is set by `sweep_deaths`
(`health.rs:77`), whose one non-test caller is `impact_effects.rs:360` —
`run_ai_tick` never runs it, so no AI-test tick path latches a death.

### Task 3: Leash bounds acquisition

`BrainComponent::leash_range` (`Option<f32>`, `Some` only for lowered legacy
brains) bounds retention but not acquisition. It is consulted in `ai/mod.rs` for
the retained candidate and for the replacement search on a leash-escape tick,
while the ordinary acquisition scan applies no range limit. A brain tuned
`leashRange < detectionRange` therefore acquires a pawn its own leash
immediately rejects — the whole oscillation. Give the leash the same authority
over fresh acquisition it already has over retention.

**Why the floor keeps a leash at all.** Lowering already spells `leashRange` as
an authored guard edge: `alert → idle` when `targetDistance > leashRange`
(`behavior_lowering.rs:69-183`). So the cheaper fix looks obvious — emit
`candidateFilter: le(candidate.distance, leashRange)` alongside it, bound legacy
acquisition through the filter Task 2 adds, and delete most of this task.
That fails on retention, not acquisition. The floor clears an out-of-leash
retained target on *every* tick (`ai/mod.rs:447-468`; only the replacement
search is strided), whereas the filter is consulted inside `select_target`,
which on the retained path runs only when acquisition is due. A per-scan filter
structurally cannot reproduce an off-stride immediate clear. Retention is what
keeps a leash on the floor, and acquisition follows it there rather than being
spelled a second time in the graph. That is what "symmetric" buys: one number,
one owner, both arms.

Move the rule into `targeting.rs` so the oversized tick module only passes a
value. `nearest_target_candidate` and the selection entry point take the brain's
optional leash and reject any candidate beyond it, and `ai/mod.rs` passes
`brain.leash_range` at its four `select_target` call sites: the early
non-engaged scan (`:437`), the leash-escape replacement (`:455`), and both
acquisition-due branches (`:471`, `:488`). The reuse at `:484` is not a fifth
site — it consumes `:437`'s result rather than scanning again. The leash does
**not** go inside `target_candidate` (`targeting.rs:27`), which stays a raw
distance read because `ai/mod.rs:434` calls it directly to price the think
stride. Leashing that read makes `current_distance` `None` for an out-of-leash
retained target, and `acquisition_due` reads `None` as due every tick. A brain
with no leash — every authored graph — keeps today's unbounded behavior exactly.

**What neither the leash nor the filter may reach: the think stride.**
`ai/mod.rs` derives `current_distance` from the retained candidate when a
retained id resolves and from the early non-engaged scan otherwise (`:439-441`),
then feeds it to `acquisition_due`, whose `None` arm means *due every tick*.
Applying a relevance rule to either source turns a far-band strided enemy into a
scan-every-tick enemy — the exact inversion of what the stride is for. The
stride is cost machinery; the leash and the filter are relevance rules. Keeping
them apart takes three distinct paths through the chokepoint:

| Path | Leash applies | Filter applies |
|---|---|---|
| Stride distance — what `acquisition_due` reads | no | no |
| Retained-target eligibility | yes | no (AC 14) |
| Ranking-scan selection | yes | yes |

So the scan returns two values in one pass, and Task 3 is what builds that
shape. `nearest_target_candidate` is a `filter_map` + `min_by`
(`targeting.rs:52-60`) returning one `Option<TargetCandidate>`, and the leash
cannot simply join that `filter_map` — dropping a candidate there would also
drop it from the distance the stride reads. It becomes a single fold over the
same iterator carrying two accumulators: the nearest candidate over every
offered pawn, unleashed and unfiltered, and the nearest *eligible* candidate.
Both accumulators honor the existing `exclude` parameter — it drops a pawn from
the offer set rather than judging its relevance, so it is not one of the three
paths above. `select_target` propagates both. Two values is a requirement, not a
convenience — re-running the scan to get the second is the regression
`ai/mod.rs:478-483` records as already fixed, and the early scan's result is
reused verbatim as the selection on the non-engaged acquisition branch (`:484`).
Any shape that splits the pass reintroduces the double scan. Task 2 adds the
filter to the eligible accumulator two phases later and changes nothing else
about it.

Building the fold here rather than with the filter is deliberate. The leash is
the only relevance rule with coverage already in the tree — three
inverted-tuning fixtures, AC 8, and AC 16's legacy half — so the stride
separation is exercised by real tests from Phase 1. A filter-first order would
land the same machinery motivated by a rule whose fixtures do not exist until
Task 2 authors graphs to create them.

Task 3 owns AC 16's legacy half. That state lasts exactly one tick — the floor
clears an out-of-leash retained target on the tick it sees it, and the brain
rescans from non-engaged afterwards — so the fixture asserts one tick, not a
window. Seed an engaged legacy brain with `think_stride_counter` at 0, its
retained pawn at far-band distance outside the leash, and a second pawn inside
it. Correct wiring: `current_distance` is the retained pawn's real distance, the
far band's divisor makes tick 1 not due, the strided replacement never runs, and
`acquired_target` is `None` after the tick. Inverted wiring: `current_distance`
is `None`, tick 1 is due, and the replacement search returns the second pawn.
Derive the divisor from `think_stride_for_distance` (`engine_floor.rs:35`,
`pub(crate)`) rather than hardcoding it — `STRIDE_FAR` is private and stays so.

The retained-target lookup stays a read, yielding the candidate and its distance
unconditionally, so an out-of-leash retained target still prices the stride.
Eligibility is then the chokepoint's answer rather than the caller's:
`targeting.rs` exposes `retained_is_outside_leash(candidate, leash) -> bool`,
which `ai/mod.rs:447-448` spells inline today and now calls instead. The caller
still gets the local boolean its cheap immediate clear consumes at `:449`, while
the comparison itself has exactly one spelling — that is what "one spelling"
means here, not that the boolean disappears. The name deliberately avoids
`retained_outside_leash`, already a `bool` *parameter* of `select_target`
(`targeting.rs:101`) documented at `:92-94`; two things of that name in one
module would shadow inside `select_target`'s body. Task 3 pins `select_target`'s
leash parameter and its two-value return; Task 2 extends the same signature two
phases later with the bound filter and the shared candidate scope. It changes
twice because it must — `CandidateScope` does not exist until Task 2 creates it,
so Task 3 cannot pre-thread a parameter of that type the way it could a bare
`Option<f32>`. AC 4 bounds the total edit surface across the plan, not per
phase. The six direct `select_target` call sites in `ai_tests.rs:728-806` are
compile-broken by the signature change and updated here; their assertions are
unchanged. The post-hoc `.filter(...)` on the replacement search (`:462-465`)
vanishes, since the scan now applies the leash itself. The leash-escape branch
keeps its cheap immediate clear and its strided replacement, so the four call
sites stay four.

**The ordering rule.** Add it to `AiDescriptor::validate`
(`crates/foundation/src/data_descriptors/types/combat.rs`) after the six-field
range loop (`:302-317`) and before the `attackDamage` check (`:318-326`), so
both operands are known finite when the error reports. Reject a descriptor whose
`leashRange` is below its `detectionRange`, naming both values and both paths —
`components.ai.leashRange` and `components.ai.detectionRange`, the style the
adjacent range loop's format string already establishes. Both runtimes funnel
through that validator, so one edit covers QuickJS and Luau.

Rust fixtures construct `AiDescriptor` literally and bypass validation, which is
why the floor rule must stand on its own. Three inverted-tuning fixtures pass
unmodified and become the non-oscillation regression rather than being deleted:

| Fixture | Location | Tuning |
|---|---|---|
| `detection_sets_agent_destination_and_leash_clears_it` | `ai_tests.rs:820` | detection 18 / leash 8 |
| `retained_target_outside_leash_drops_instead_of_switching_to_out_of_leash_replacement` | `:1082` | detection 40 / leash 10 |
| `retained_target_outside_leash_clears_stale_destination_off_stride` | `:1119` | detection 40 / leash 10 |

A fourth, `distant_enemy_strides_detection_but_attack_still_fires`
(`ai_tests.rs:1583`), sets `detection_range = 40.0` while inheriting `tuning()`'s
`leash_range: 26.0` and places the pawn at 35, between the two radii. Sub-case 1
still passes, but a leash-bounded scan rejects that pawn outright and the
assertion stops exercising the stride it is named for. Retune its `leash_range`
above 40. This is the one existing AI test whose tuning Task 3 changes.

The three fixtures pass unmodified and stand as the no-regression floor;
neither asserts AC 8's shape. Task 3 adds AC 8's fixture — a seeded-at-rest
enemy with a pawn between the two radii, asserting no destination request and
no state change across the window — and AC 9's two validation tests, one per
runtime, each asserting the error names both field values. Their home is the
existing dual-runtime `crates/scripting-core/src/data_descriptors/tests/ai.rs`.

### Task 4: Graph disengagement lints

Add two structural diagnostics for authored behavior graphs in a new sibling
module, `crates/foundation/src/data_descriptors/types/behavior_lints.rs`, beside
`behavior.rs` (824 lines — do not extend it). Both are pure functions of the
descriptor. `BehaviorGraphDescriptor::validate` calls the entry point after its
existing structural checks succeed.

A state is **engaging** when its motion verb is the chase verb or it declares
any action — the same predicate the evaluator uses, restated over the descriptor
rather than over a resolved state index.

| Lint | Condition | Warning |
|---|---|---|
| `LevelWidePursuer` | At least one engaging state, and no engaging state declares a state-local transition to a non-engaging state | Pursues without limit; names the engaging states |
| `NoHasTargetInterrupt` | At least one engaging state, and no interrupt whose guard tree reads the has-target input | Target loss will be handled through distance guards, where the no-target sentinel makes `gt`/`ge` read true |

`LevelWidePursuer` reads state-local `transitions` only, never the graph-level
`interrupts` vector. The two lints answer different questions: lint two is about
losing a target, lint one about never letting go of one it still has. Counting
interrupts in lint one would collapse them — any graph with a has-target
interrupt would pass lint one, so lint one would fire only where lint two
already does, and a graph that chases a live pawn across the level forever would
go unwarned. Both lints report their findings against every engaging state, in
`states` key order (the descriptor's `BTreeMap`, so name-sorted and
deterministic).

Detect the read by reusing `IrNode::dispatch_input_names`
(`crates/foundation/src/ir/mod.rs:185`) rather than hand-rolling a second tree
walk, matching an input leaf whose name equals `BRAIN_HAS_TARGET_INPUT`
(`"@brain.hasTarget"`) — `dispatch_input_names` yields wire strings, not SDK
property names. It is already colocated with the closed opcode set, so adding an
opcode must update it, and its `Vec<String>` allocation is fine at validation
time.

The entry point is `behavior_lints::inspect(&BehaviorGraphDescriptor) ->
Vec<BehaviorLint>`, where `BehaviorLint { kind: BehaviorLintKind, states:
Vec<String> }` carries its kind and the offending state names, so "one finding
naming those states" is checkable. Declare the module in
`data_descriptors/types/mod.rs` and re-export `BehaviorLint` and
`BehaviorLintKind` alongside the other descriptor types, so AC 10 and AC 11 can
assert on them from outside the module. It **returns** findings rather than logging
them; `validate` is a thin caller that logs them via `log::warn!` — foundation
already depends on `log`. Findings are not errors, so they do not join the
`Result`. AC 10 and AC 11 call `inspect` directly. This follows the existing
bind-failure path, which chose an observable latch over raw logging for the same
reason: a warning nothing can assert on is a warning nothing protects. No
log-capture harness is needed, and none exists reachable from foundation.
Messages carry the state names and the `components.behavior` path prefix, and
fire once per descriptor validation — once per parse, not per spawn.

`the_wire_shape_round_trips_through_camel_case_json` (`behavior.rs:743`) trips
both lints. It stays untuned: it exercises the serde wire shape, not graph
semantics, and its findings are unasserted and expected. The shared `fn graph()`
helper (`behavior.rs:498`) used by most sibling tests declares no engaging
state, since `fn state()` (`:488`) hardcodes `MotionVerb::Hold` with no action,
so it trips neither lint and those tests are unaffected. The lowered legacy
graph does not route through `validate` (`lower_ai_descriptor`,
`behavior_lowering.rs:69-183`, never calls it), so legacy spawns produce no
findings and the once-per-parse contract holds. Confirm the shipped reference
enemy trips neither, and add a fixture graph for each lint.

### Task 5: Reference enemy, docs, and typedefs

Author the new vocabulary into the reference enemy
(`sdk/behaviors/reference/entities.{ts,luau}`, identical spellings): a
target-death stand-down interrupt declared immediately after the existing
has-target interrupt, and a candidate filter excluding dead pawns and bounding
acquisition by distance. The TS file imports its vocabulary — `import { brain,
defineEntity, runtime } from "postretro"` (`entities.ts:7`) gains `candidate`;
the Luau twin reads globals and needs no import line.

Four existing sites are affected:

| Test | Location | Effect |
|---|---|---|
| `the_shipped_reference_enemy_graph_is_identical_in_both_authorings` | `crates/scripting-core/src/data_descriptors/tests/behavior.rs:533` | Pins interrupt targets as `vec!["idle"]`; becomes `vec!["idle", "idle"]` |
| `the_reference_oracle_matches_the_shipped_authored_graph` | `ai_tests.rs:3954` | Mirrors the shipped graph in Rust; gains the same interrupt and filter |
| `the_authored_reference_graph_is_behavior_identical_to_the_legacy_block` | `ai_tests.rs:4016` | Traces authored against lowered legacy over 200 ticks |
| `shipped_reference_behavior_graph` | `ai_tests.rs:3897` | Evaluates `entities.luau` in a hand-built Lua state setting only `runtime`, `brain`, and `defineEntity`. Authoring `candidate.*` makes it panic on a nil global; it needs the `candidate` table from `brain.luau` set as a fourth |

That last one is the reference enemy's only consumer that does not go through a
real prelude, so Task 2's six-site allowlist table does not reach it.

The third constrains the authoring. It traces
`BrainComponent::from_graph(&reference_behavior_graph())` against
`from_descriptor(&reference_ai_descriptor())` and asserts identical traces, and
`BrainTrace` (`:3966`) carries an `acquired` field — target selection is part of
what "identical" covers, not just state and animation. `reference_player_x`
(`:3976`) parks the pawn at 30 for ticks 0-9 and at 80 from tick 170.

**Pin the filter's radius to 50.** After Task 3 the floor bounds legacy
acquisition by `leashRange`, not `detectionRange`, and
`reference_ai_descriptor()` (`ai_tests.rs:3770`) is detection 16 / leash 50. At
tick 0 the legacy twin therefore acquires the pawn at 30 (inside its leash) and
holds it in `idle` on the graph's detection guard — `acquired` is true. A filter
pinned to `detectionRange` would refuse that pawn and read false, diverging on
ticks 0-9. A filter is nonetheless required: without one the authored graph, which
carries no leash, would acquire the pawn at 80 that the legacy floor rejects.
Fifty is the value that makes both ends agree, and the authoring comment says
so. `reference_player_x` never kills the pawn, so the filter's death clause and
the target-death interrupt are inert across the trace; only the distance clause
can diverge.

The reference enemy's comments are the de-facto authoring documentation and must
teach four things:

- Target-side facts read zero with no target and are only meaningful under
  `hasTarget`.
- The death latch, not a health comparison, is the death test — and why.
- Candidacy is per-graph eligibility; disengagement is per-state policy.
- The filter's acquisition radius is pinned to the legacy descriptor's
  `leashRange` — the floor's acquisition bound for the lowered twin — so the
  authored and lowered traces agree.

Extend `docs/scripting-reference.md` with the floor's acquisition contract: what
the engine offers as candidates, what it never decides (aliveness), the legacy
leash's dual acquisition/retention role, that an authored graph owns its own
disengagement, the target-side facts' no-target zero readings and why
`@brain.hasTarget` is the sole presence test, why the death latch and not
`le(targetHealth, 0)` is the death test, and that a filter over
`candidate.distance` **is** the authored acquisition radius — the reason no
descriptor field spells one, and the answer to the question the seed research
left open. Anchors to extend: `### @brain.* guard inputs` (line 620, a
hand-maintained table with no drift test guarding it, stale from Phase 2 until
this task), `### The no-target trap` (648), `### The level-wide pursuer` (702),
and `## components.behavior` (429).

Typedef work splits by owner. `primitives/mod.rs` does not own the `brain`
interface — `BrainInputs` and the `brain` declaration are hand-written into the
two templates Task 1 names.

| Target | Change |
|---|---|
| `crates/postretro/src/scripting/primitives/mod.rs` | `components.ai` `detectionRange` and `leashRange` descriptions (lines 346 and 348; `:347` is `attackRange`, untouched) state the ordering constraint and the leash's widened role |
| Same file | Register the `candidateFilter` field on `BehaviorGraphDescriptor` as type `"IrNode"`, mirroring the `when` field (`:387`) — `common.rs` maps it to `RuntimeValue`. Typedef surface only |
| `virtual_module.luau:120` | Gains `candidate: CandidateInputs,` on `PostretroModule` beside `brain: BrainInputs,` — the one typedef template Task 1 does not touch |

The current `leashRange` description says "Distance from its origin past which
the brain disengages", but the implementation measures target distance from the
enemy's current position (`ai/mod.rs:447-448`). The rewrite corrects that
phrasing, not just extends it.

Then regenerate both artifacts and commit them:
`cargo run -p postretro --bin gen-script-types`.
`committed_sdk_types_match_current_registry`
(`crates/postretro/src/scripting/typedef/tests/committed.rs:8-37`) is what goes
red if they drift. Re-run the lint fixtures against the rewritten reference
enemy — Task 4's clean-trip confirmation predates this rewrite.

## Sequencing

| Phase | Tasks | Why |
|---|---|---|
| 1 (concurrent) | Task 3, Task 4 | Disjoint files: `targeting.rs`, `ai/mod.rs`, and `combat.rs` versus a new `behavior_lints.rs` and `behavior.rs`'s `validate` call site. Task 3 leads because it builds the two-value scan against the only relevance rule with fixtures already in the tree |
| 2 | Task 1 | Shares `ai/mod.rs` and `ai_tests.rs` with Task 3 — different regions, but its `BrainFacts` fold consumes the selected target Task 3's block produces |
| 3 | Task 2 | Consumes Task 1's settled input table; adds the filter to the eligible accumulator Task 3 built; extends the same evaluator side-table |
| 4 | Task 5 | Documents the contract the first four settle; regenerates typedefs once |

## Durable decisions, captured

Settled on promotion, so a task agent finds them in `context/lib/` rather than
here:

| Decision | Landed in |
|---|---|
| The ownership split: the engine offers candidates, the graph decides which are worth engaging; perceivable vs. worth-engaging is where the line falls | `entity_model.md` §7c |
| Candidacy is per-graph eligibility, read-only, per offered candidate, never against the retained target, and never a rank | `entity_model.md` §7c |
| Target selection holds no aliveness policy; the death latch and not a health comparison is the authored death signal | `entity_model.md` §7c |
| The leash is symmetric and stays engine-side because retention clears every tick; an authored graph bounds acquisition with a distance clause instead | `entity_model.md` §7c |
| The think stride is cost machinery and shares no data path with relevance rules | `entity_model.md` §7c |
| The candidate scope as a second binding scope, resolved against a different entity, and the only per-pair evaluation context | `scripting.md` §11 |
| Both fixed fact tables are append-only — position is the runtime read handle | `scripting.md` §11 |
| The per-entity state seam is same-entity by construction, which is what blocks candidate-scoped `@state.*` | `scripting.md` §11 |

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau |
|---|---|---|---|---|
| Target health fact | `BRAIN_TARGET_HEALTH_INPUT` | `"@brain.targetHealth"` | `brain.targetHealth` | `brain.targetHealth` |
| Target max health fact | `BRAIN_TARGET_MAX_HEALTH_INPUT` | `"@brain.targetMaxHealth"` | `brain.targetMaxHealth` | `brain.targetMaxHealth` |
| Target death latch fact | `BRAIN_TARGET_DIED_INPUT` | `"@brain.targetDied"` | `brain.targetDied` | `brain.targetDied` |
| Has-target fact (existing; listed because Task 4's lint matches on it) | `BRAIN_HAS_TARGET_INPUT` | `"@brain.hasTarget"` | `brain.hasTarget` | `brain.hasTarget` |
| Candidate filter field | `BehaviorGraphDescriptor::candidate_filter: Option<IrNode>` | `"candidateFilter"` | `candidateFilter?: RuntimeValue` | `candidateFilter: RuntimeValue?` |
| Filter error path | — | `"components.behavior.candidateFilter"` | — | — |
| Ordering error path | — | `"components.ai.leashRange"`, `"components.ai.detectionRange"` | — | — |
| Candidate facts table | `CANDIDATE_INPUTS` | — | `candidate.*` prelude object | `candidate.*` |
| Candidate distance | `CANDIDATE_DISTANCE_INPUT` | `"@candidate.distance"` | `candidate.distance` | `candidate.distance` |
| Candidate health | `CANDIDATE_HEALTH_INPUT` | `"@candidate.health"` | `candidate.health` | `candidate.health` |
| Candidate max health | `CANDIDATE_MAX_HEALTH_INPUT` | `"@candidate.maxHealth"` | `candidate.maxHealth` | `candidate.maxHealth` |
| Candidate death latch | `CANDIDATE_DIED_INPUT` | `"@candidate.died"` | `candidate.died` | `candidate.died` |
| Candidate input prefix | `CANDIDATE_INPUT_PREFIX` | `"@candidate."` | — | — |
| Candidate inputs type | — | — | `CandidateInputs` | `CandidateInputs` |

No FGD column: all of it is descriptor-owned tuning, never map-overridable.
`RuntimeValue` is the number|boolean
union, so the boolean-only constraint on `candidateFilter` is runtime-enforced
(AC 6) and deliberately not expressed in the typedef.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| The engine floor holds no aliveness policy for target selection; the attack gate's aliveness check is the floor's only health read and stays an attack gate | Task 2 (by not adding one) | Any future "obvious fix" adding a health test to candidacy or retention | AC 7, 4 |
| Target-side facts read their type's zero with no target; `@brain.hasTarget` is the sole authoritative presence test, and the distance sentinel remains the lone exception | Task 1 | Any new target-side fact choosing a non-zero no-target reading | AC 2, 17 |
| The death latch, not a health comparison, is the authored death signal: unambiguous untargeted, where every target-side fact reads its type's zero, and it carries the sweep's non-finite arm. `targetDied` turns true on the tick after death commits | Task 1 | Docs and reference authoring must not teach `le(targetHealth, 0)` as the death test. Both facts are already uniform across observers — the compute pass evaluates every brain before the apply pass lands any damage — so no iteration-order argument may be offered for the latch | AC 1, 12, 13 |
| Candidacy is per-graph eligibility; disengagement is per-state policy. The filter never runs against the retained target | Task 2 | The shared candidate lookup also resolves the retained target — the filter must sit in the ranking scan only | AC 1, 3, 14 |
| The floor decides what is *perceivable* — which entities the scan is offered, and later the `visible` predicate — while the graph decides which offered candidates are worth engaging. The filter is strictly downstream of `visible` and can only narrow the offer set | Task 2 | A perception spec exposing a `@candidate.visible` fact and moving line-of-sight policy into the filter; any filter arm that widens what the scan considers | Design constraint — not observable until a resolver exists |
| The filter answers eligibility, never order: it produces a boolean, and ranking stays nearest-with-hysteresis | Task 2 | A threat or priority spec widening the filter to a score rather than adding its own seam | AC 3, 6 |
| Both fixed input tables are append-only — a name's index is its runtime read handle | Task 1, Task 2 | An insertion or reorder silently re-points every bound program | AC 13 (consistency only) + review gate: no test catches a coordinated reorder of table and typedef |
| An unauthored graph is bit-identical to today; legacy lowering emits no new edge and no filter; legacy acquisition changes only by gaining the leash bound | Task 2, Task 3 | Any default filter, or a lowering that emits a target-death edge | AC 4, 7, 8 |
| Bound programs stay derived data in the evaluator side-table, rebuilt via the `Arc` pointer-identity staleness test; the filter joins them rather than riding the component | Predecessor, extended by Task 2 | A filter program stored on `BrainComponent` would reintroduce serde and equality coupling | AC 5 |
| Acquisition-path evaluation is zero-alloc per tick; the filter adds a constant factor to an existing traversal, never a new walk | Task 2 | Candidate-scope refresh must not clone or collect — the fact array is fixed-size and written by index | AC 5 |
| `refresh` takes `&mut self`, which is what makes "grows at bind, written by index at refresh" a compile-time fact. Simultaneous access to a filter and its scope comes from splitting the side-table by field, not from interior mutability | Task 2 | Wrapping the fixed array in a `RefCell` to dodge a borrow error would demote the guarantee to a convention, and the alloc probe would not catch it — `RefCell` does not allocate | Review gate — AC 5's probe explicitly cannot catch this |
| The floor's leash is symmetric: a target beyond it is neither acquired nor retained. Retention is why the leash stays floor-owned — the floor clears every tick, while a filter is consulted only when acquisition is due | Task 3 | Any new acquisition path bypassing the selection chokepoint; the ordering validator alone cannot enforce it, since Rust fixtures bypass validation; a future spec moving the bound into lowering would silently lose the off-stride clear | AC 8 |
| A brain with no leash keeps unbounded acquisition, with disengagement owned by its guards | Task 3 | Any default value substituted for the absent leash | AC 4 |
| The think stride is cost machinery and shares no data path with relevance rules. Its distance comes from an unfiltered, unleashed read on both sources — the retained candidate and the early scan — so neither leash nor filter can silently reprice it. The scan returns the stride's distance and the eligible selection from one pass | Task 3, extended by Task 2 | Deriving `current_distance` from a filtered or leashed value — `acquisition_due` reads a `None` distance as *due every tick*, inverting the stride. Splitting the scan into two passes reintroduces the double scan `ai/mod.rs:478-483` records as already fixed. Putting the leash inside `target_candidate` inverts the stride and no legacy fixture catches it | AC 16, 4 |
| Acquisition-range authority stays engine-side and legacy-only: no descriptor field spells disengagement range. An authored graph bounds acquisition with a distance filter instead | Predecessor (pinned), extended by Task 2 | Task 3 must thread the existing component field, never introduce an authored one; a future perception spec must reach for the filter before a new descriptor range | AC 4, 13, 15 |
| Engagement radius stays combat-slot spread only — never acquisition, retention, or damage | Predecessor (pinned) | Any task tempted to reuse it as an acquisition radius | AC 4 |
| Graph diagnostics are warnings, never errors: a relentless pursuer is a legitimate authored design | Task 4 | Escalation to a validation error would reject shipping content | AC 10, 11 |

## Script syntax examples

```ts
// Proposed design
const ACQUIRE_RANGE = 16;

behavior: {
  initial: "idle",
  // Per-GRAPH eligibility, asked once per candidate on every ranking scan.
  // Not a guard: guards run against a target that has already been chosen.
  //
  // Two clauses: not dead, and within this graph's acquisition radius. The
  // radius lives HERE — the floor spells no acquisition range for an authored
  // graph, so a distance clause is how a graph gets one. No `and` opcode:
  // conjunction is `select(a, b, false)`, negation is `select(a, false, true)`,
  // and "not-a and b" is `select(a, false, b)`. This is the third form.
  candidateFilter: runtime.select(
    candidate.died,
    false,
    runtime.le(candidate.distance, ACQUIRE_RANGE),
  ),
  interrupts: [
    // Target loss first, as always — it outranks every range guard.
    { to: "idle", when: runtime.select(brain.hasTarget, false, true) },
    // Then target death. `brain.targetDied` is false with no target, so this
    // edge is inert untargeted and the interrupt above stays the one that
    // handles loss. Do NOT spell this `le(brain.targetHealth, 0)`: with no
    // target that reads zero and fires for the wrong reason, and it misses the
    // sweep's non-finite arm. The latch carries both.
    { to: "idle", when: brain.targetDied },
  ],
  // ... states as before
}
```

## Accepted trade-offs

- Warning and error messages name state names and the `components.behavior` path
  but not the owning entity's canonical name, which the descriptor validators do
  not have in hand. Threading it would widen every descriptor type's validator
  signature. A third path exists and is untried: annotate at the parser call
  site, which does know the name, rather than inside the validators. Accepted
  as-is; reach for that option first if authors report the diagnostics as hard
  to place.

## Where the deferred dimensions land

Each waits for a real consumer. *Where* each lands is decided here so a
successor spec inherits it instead of re-deriving it.

- **Line of sight splits by conservatism, not by layer.** The choice is not
  floor-gate *or* published fact — it is both, cut where the one-right-answer
  test cuts. A conservative Cell→Cell broad-phase
  (`research/cell-visibility-substrate.md`) removes only what *no* policy could
  perceive, so it gates the offer set as correctness. The exact eye-to-target
  BVH raycast is where taste starts — a blind grunt, a psychic boss, and an
  enemy that hunts by sound are all valid designs — so it is published as
  `@candidate.visible` and never gates the floor. Gating the offer set on exact
  LOS would bake in one point on that spectrum, structurally the same defect as
  the hardcoded aliveness rule this plan removes. The cost objection answers
  itself through the IR: `IrNode::dispatch_input_names`
  (`crates/foundation/src/ir/mod.rs:185`) reveals at **bind time** whether a
  filter reads that leaf, so a graph that never mentions it pays for no raycasts
  and perception is priced per graph by what its author asked for. What is
  genuinely open is the resolver's own shape, which waits on a consumer.
- **Target ranking lands as a second IR expression, not a widened predicate.**
  The seed research assumes the alternative — widening `visible` from `bool` to
  a weight — but that puts *preference* inside the *perception* predicate, which
  the perceivable/worth-engaging invariant forbids. Ranking is pure taste:
  nearest, weakest, most-recently-damaged are all valid designs. So it belongs
  beside eligibility, as a per-graph score expression over the same
  `@candidate.*` namespace this plan builds, where namespace, scope, binding,
  and staleness are already in place. The filter stays boolean and the score
  stays separate: folding them by encoding ineligibility as a sentinel score is
  the exact shape of `BRAIN_NO_TARGET_DISTANCE`, which shipped as a real defect.
  *Where* is settled; *when* waits on a consumer.
- **Mod-authored candidate state waits on a write path, not on a consumer.**
  A filter reading `@state.*` on the candidate was cut from this plan. The
  blocker is the write side, not the read side: entity-scoped `setState` exists
  in exactly one place — inside an impact policy, targeting only
  `@impact.target` (`impact_policy.rs:396` rejects every other target), and
  `EntityStateComponent` has one write site in the tree. So the only way to mark
  a pawn today is to damage it, and writer and reader are the same entity. The
  shipped example, `state("staggered")`, works precisely because of that: an
  impact staggers an enemy and that enemy's own guards read the flag. Candidate
  filtering inverts it — entity A is marked, entity B's filter reads it — and
  nothing in the tree does that. A consumer therefore needs one of two things
  first: a way to set entity state outside an impact policy, or candidate-scoped
  reach into `defineStore` slots, which is where persistent per-player stats
  (faction standing, mission flags) actually live and which guard IR cannot read
  at all today. Pick that seam before adding the leaves; building the
  `EntityStateComponent` arm now would guess it.
- **Threat and last-attacker land on the candidate scope, not a new mechanism.**
  The candidate scope is refreshed per *(enemy, candidate)*, so it is the
  design's only per-pair evaluation context. Any enemy × candidate relation
  reducing to a number or a boolean is a `@candidate.*` fact; no per-pair
  storage on the brain is needed to express one. The memory already exists:
  `HealthComponent` carries a bounded per-source contributor ledger
  (`crates/entities/src/components/health.rs:151-159`) with `accumulated_damage`,
  `hit_count`, and `last_attacker`, maintained by the damage chokepoint. The
  enemy is the victim when a player shoots it, so its own health component
  already records who hurt it and how much. `@candidate.damageDealtToMe` is a
  scan of that ledger for entries whose `last_attacker` is this candidate;
  `@candidate.isLastAttacker` is the boolean collapse of the same. What
  genuinely constrains a threat spec is the ledger's *shape*, not the seam: it is
  keyed by `source_id` with the attacker as a field, and its overflow bucket
  carries no source id, so an attacker evicted from the exact ledger is
  indistinguishable from one that never fired. A threat spec must decide whether
  that fidelity is enough before adding storage.
