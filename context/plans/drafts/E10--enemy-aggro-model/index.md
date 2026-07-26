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
  block, compiled once per bound brain and evaluated per candidate on every
  ranking scan, including the per-tick non-engaged one, over a
  candidate-facts namespace: engine facts about the candidate, plus the
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
- Changing what legacy lowering emits. No new edges, no filter; hostage
  behavior is preserved. Legacy *acquisition* does change — it gains the
  leash bound (Task 3).
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
- No network replication changes.

## Acceptance criteria

- [ ] A graph can author an interrupt that stands the brain down when its
      selected target's death latch has fired; an enemy engaged with a co-op
      pawn that dies releases it on the tick after the death sweep commits, and
      engages a live pawn standing beside it, even inside the switch-hysteresis
      margin. A pawn killed by another enemy's attack inside the same AI tick
      reads `targetDied` false that tick and true the next, uniformly for
      every observer. The graph under test authors the filter as well as
      the interrupt: standing down clears the target, and the next scan is
      offered the corpse with no retained target to seed hysteresis, so
      without the filter the corpse is simply re-acquired. This is an
      integration criterion over
      Tasks 1 and 2, not a Task 1 one.
- [ ] With no selected target, every target-side fact reads its type's zero, and
      the boolean death fact reads false. A graph with no has-target interrupt is exactly
      what lint two warns about.
- [ ] A graph can author a candidate filter that excludes dead pawns; an enemy
      whose only nearby pawn is dead acquires nothing and stays at rest, while
      the same enemy with a live pawn in range engages normally.
- [ ] A candidate filter reads a mod-authored `@state.*` field on the candidate
      and acts on it: a pawn some impact policy marked untargetable is skipped by
      the acquisition scan while an unmarked pawn beside it is acquired, with no
      engine component naming that field (review gate, not a runnable
      assertion). A candidate whose state component
      never had that field written reads `0.0`.
- [ ] A graph with no candidate filter and a brain with no leash select
      targets exactly as today. No existing AI test changes its asserted
      behavior. Within the target-selection surface — `targeting.rs` and
      `ai/mod.rs`'s selection block — the mechanical edits are exactly: the
      signatures and doc comments of `select_target`,
      `nearest_target_candidate`, and `target_candidate`, their call sites
      (including the six direct ones in `ai_tests.rs:728-806`), and `BrainFacts`
      construction at the one tick site that builds it. This bounds that
      surface only; every task adds its own new types and fields elsewhere.
      Review gate, not a runnable test: `engagementRadius` is still read for
      combat-slot spread alone.
- [ ] A candidate filter is compiled once per bound brain, inside
      `sync`, on the same `Arc` pointer-identity staleness edge that rebinds
      the graph's guards — never on a tick where the `Arc` is unchanged, and
      never per candidate — and evaluated per candidate
      with no heap allocation on the acquisition path, the interned `@state.*`
      snapshot included — it grows at bind and is written by index at refresh
      (alloc-probe assertion, matching the substrate invariant).
- [ ] A candidate filter that fails to bind is a validation error in both
      QuickJS and Luau, with the authored path in the message; a filter that
      produces a non-boolean is rejected the same way.
- [ ] Target selection remains aliveness-free — no task adds one — a review
      gate rather than a runnable assertion.
      `selected_target_alive` keeps gating damage application only: an enemy
      whose nearest pawn is dead and whose graph authors no filter still
      selects it and is still blocked from damaging it. The runnable half
      extends `no_attack_or_event_when_player_already_dead` (`ai_tests.rs:1336`)
      with an assertion that the enemy's acquired target is still the dead
      pawn.
- [ ] A legacy enemy tuned with a leash smaller than its detection range no
      longer oscillates: seeded at rest with a pawn between the two radii, it
      stays at rest indefinitely, requests no destination, and changes state on
      no tick.
- [ ] Authoring `components.ai` with a leash range below its detection range is
      a validation error in both runtimes, naming both field values.
- [ ] A behavior graph whose engaging states offer no edge to a non-engaging
      state validates successfully and returns one finding naming those states,
      asserted on directly rather than through captured log output.
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
      prelude, and the filter field in both generated typedef artifacts
      (`sdk/types/postretro.d.ts`, `sdk/types/postretro.d.luau`).
      `brain_input_typedefs_match_the_foundation_table` is order-sensitive
      (`Vec<String>` equality) and its `CANDIDATE_INPUTS` twin must be too;
      `brain_sdk_helpers_cover_every_brain_input` compares sets and counts and
      is deliberately not. Byte-exact equality of the two committed artifacts
      is a third test, `committed_sdk_types_match_current_registry`. Review
      gates, not runnable tests: the `leashRange` phrasing correction and the
      scripting-reference topics below. The `components.ai` `leashRange` description states
      measurement from the enemy's current position rather than its origin, and
      both range descriptions state the `leashRange >= detectionRange` ordering
      rule. The scripting reference documents the floor's
      acquisition contract, the target-side facts' no-target readings, that an
      authored graph owns its own disengagement, that a distance filter is the
      authored acquisition radius, and that `le(targetHealth, 0)` is not the
      death test.
- [ ] The filter is never consulted for the retained-target lookup: an enemy
      already engaged with a pawn its filter excludes is not dropped by the
      filter. It is displaced only by ordinary switch hysteresis, by the floor's
      leash, or by a guard standing the brain down.
- [ ] An authored graph whose filter is `le(candidate.distance, R)` acquires a
      pawn at `R - ε` and does not acquire one at `R + ε`, with no
      `leashRange` in play.
- [ ] A relevance rule never reprices the think stride. A far-band enemy whose
      only nearby pawn its filter excludes, and a legacy far-band enemy whose
      retained target sits outside its leash, both keep scanning on the stride's
      cadence instead of every tick. Asserted on scan cadence directly, because
      every other criterion here — and all three legacy leash fixtures — passes
      under the stride-inverting wiring.

## Tasks

### Task 1: Target-side brain facts

Extend the fixed guard-input table with three facts about the selected target:
its current health, its maximum health, and whether the engine's one-shot death
latch (`HealthComponent.death_handled`) has fired for it. Add them in
`crates/foundation/src/brain.rs` as `@brain.`-prefixed constants appended to the
end of `BRAIN_INPUTS` — that table's order **is** the runtime read handle, so
append, never insert. Two Numbers and one Bool. The constants are
`BRAIN_TARGET_HEALTH_INPUT` (`"@brain.targetHealth"`),
`BRAIN_TARGET_MAX_HEALTH_INPUT` (`"@brain.targetMaxHealth"`), and
`BRAIN_TARGET_DIED_INPUT` (`"@brain.targetDied"`), in that order. With no
selected target each
reads its declared type's zero: no new sentinel constant is introduced, and the
doc comments must say plainly that `@brain.hasTarget` is the only authoritative
presence test and that the existing `BRAIN_NO_TARGET_DISTANCE` remains the lone
exception to the zero convention. The Bool latch fact, not a health comparison,
is the death signal, for three reasons. It is unambiguous with no target. It
carries the death sweep's full definition (which includes non-finite health)
rather than an author's re-derivation of it. And it is the only one of the two
that is order-independent: `run_death_sweep` runs last in the tick, while enemy
attacks apply damage synchronously inside the AI tick
(`ai/mod.rs:796`), so a pawn killed by another enemy reads `targetHealth` as `0`
for enemies iterated after its killer and `targetDied` as `false` for all of
them. The health read is coupled to iteration order for that one tick; the latch
is uniform. Authors must therefore not conjoin `le(targetHealth, 0)` to get a
same-tick test — it would import iteration order into an authored guard.
Same-tick suppression is already the floor's job: `selected_target_alive` reads
health directly and is current within the tick, so an enemy cannot damage a pawn
that died earlier in the same AI tick. The contract the docs state is that
`targetDied` becomes true on the tick after death is committed, uniformly for
every observer. Populate the facts in
`crates/postretro/src/scripting/systems/ai/brain_scope.rs`: `BrainFacts` folds
distance and identity into one binding — `target: Option<(EntityId, f32)>` —
preserving the existing "cannot disagree" guarantee, and its doc comment
updates to say so. `refresh` reads that entity's `HealthComponent` the same way
it already reads the evaluating enemy's — absent component reads as zeros,
absent target likewise. The tick site in
`ai/mod.rs` already computes the selected target immediately before building
`BrainFacts`, so it builds the folded `target` binding at that one call site;
nothing else moves. `BrainValidationScope` needs no change beyond the
table growing, since it resolves through `resolve_brain_input`. Extend the SDK's
hand-maintained `brain` prelude object in both runtimes with the three new
pre-wrapped input leaves. Two runtime drift tests guard two different surfaces
here: `brain_sdk_helpers_cover_every_brain_input`
(`crates/scripting-core/src/data_descriptors/tests/behavior.rs:372`), which
asserts an exact count match over the hand-maintained `sdk/lib/brain.{ts,luau}`
preludes — the count is over occurrences of the literal `"@brain.` in
`sdk/lib/brain.ts`, so new doc-comment prose in that file must not contain that
literal or the count inflates and the test fails — and
`brain_input_typedefs_match_the_foundation_table`
(`crates/postretro/src/scripting/typedef/tests/surface.rs:584`), over the
emitted `BrainInputs` typedef block. The `BrainInputs` block itself is
hand-written text in two typedef template files —
`crates/scripting-core/src/typedef/templates/sdk_lib.d.ts`
(`export interface BrainInputs {`, line 842) and `sdk_lib.luau`
(`export type BrainInputs = {`, line 1073) — and Task 1 must edit both, or
`brain_input_typedefs_match_the_foundation_table` fails from Phase 1 through
Phase 4. `virtual_module.luau` needs no Task 1 edit; it names the type without
declaring its fields. `BRAIN_INPUTS`' hard-coded array length literal (`[(&str, IrType); 7]`)
grows to 10. `BrainFacts` is also constructed by `engaged_facts()` in
`brain_scope.rs` and at five sites in `brain_programs.rs`; `expected_fixed_value`
(`brain_scope.rs:344`) has a deliberate no-wildcard match that will not compile
until three arms are added. All are test-side and surface as compile failures.

### Task 2: Per-graph candidate filter

Add an optional IR predicate to the behavior block that decides which offered
candidates an enemy will acquire. Descriptor side, in `postretro-foundation`:
`BehaviorGraphDescriptor` gains an optional raw `IrNode` field (camelCase wire
key per the boundary inventory), following the same descriptor-partition rule as
transition guards — the bound program is derived data the evaluator owns, never
a descriptor field. `BehaviorGraphDescriptor` carries `deny_unknown_fields`, so
the new key needs `#[serde(default)]` and old serialized graphs must still
deserialize. That is the only direction that matters here: `deny_unknown_fields`
means a reader predating this change rejects a graph carrying `candidateFilter`,
which is why the key is additive content rather than replicated state. It needs
its own input namespace and its own
declaration-time binding scope, both in a new module, `crates/foundation/src/candidate.rs`, beside
`brain.rs` rather than inside it, and both shaped exactly like the brain pair. Two halves, as
there: a fixed table of `@candidate.`-prefixed engine facts — the candidate's XZ
distance from the enemy, its current health, its max health, and its death-latch
boolean — whose order **is** the runtime read handle, so it grows by appending
and never by insertion; Those four are `CANDIDATE_DISTANCE_INPUT`
(`"@candidate.distance"`), `CANDIDATE_HEALTH_INPUT` (`"@candidate.health"`),
`CANDIDATE_MAX_HEALTH_INPUT` (`"@candidate.maxHealth"`), and
`CANDIDATE_DIED_INPUT` (`"@candidate.died"`) — note `died`, not `targetDied`,
since the scope already names the entity — under `CANDIDATE_INPUT_PREFIX =
"@candidate."`. The descriptor field is `candidate_filter: Option<IrNode>`,
wire key `candidateFilter`; `BehaviorGraphDescriptor` is `rename_all =
"camelCase"`, so no explicit serde rename is needed. and the `@state.*`
per-entity leaves, resolved against
the candidate. One resolve function mirroring `resolve_brain_input` answers
both arms, a validation scope resolves through it, and a bind helper mirrors the
existing guard-bind helper. The shared `@state.` prefix is deliberate: a leaf
names a field, and the scope names whose field it is — `state("revivable")`
reads the enemy's in a guard and the candidate's in a filter, which is the
composition seam of `scripting.md` §11 reaching the acquisition path.
`BehaviorGraphDescriptor::validate` binds the filter when present and rejects an
unbindable or non-boolean one with the authored path
`components.behavior.candidateFilter`, exactly as it already does per
transition. Both runtimes emit that same string. Runtime side, in the binary: a candidate `BindingScope` beside
the brain scope, the same two-snapshot shape — a fixed array written by index,
plus an interned `@state.*` name vector that grows only at bind. Its refresh
signature is `refresh(&mut self, registry, candidate: EntityId, from: Vec3)`,
mirroring `BrainScope::refresh`, with `from` the enemy position
`nearest_target_candidate` already holds; a candidate with no
`EntityStateComponent`, or without the named field, reads `0.0`, the same
emergent-field contract the brain scope honors. Binding rides the existing
evaluator side-table:
`BrainPrograms` holds one shared candidate scope alongside its brain scope, and
`BrainEntityPrograms` gains one optional bound filter program compiled during
the same `sync` pass that binds the graph's guards, keyed off the same `Arc`
pointer-identity staleness test. Evaluation goes in `targeting.rs`, inside the
ranking scan's per-pawn `filter_map` and **not** in the shared candidate lookup
that also resolves the retained target — filtering retention there would make
the engine decide disengagement again, which is the graph's job per Task 1.
Plumbing: the scan and the selection entry point take the enemy's bound filter
and the shared candidate scope as two parameters — never `&BrainPrograms`, which
would hand `targeting.rs` guard programs it has no business reading. `ai/mod.rs`
passes them at its existing selection call sites (the early non-engaged scan,
the leash-escape replacement, and both acquisition-due branches). The filter is
a relevance rule, so it obeys the stride separation Task 3 states in full: it
narrows what becomes the selected target and never the distance the stride
reads. Task 3 pins the scan's two-value return shape that keeps those apart, and
Task 2 is what makes that shape necessary — Phase 2 introduces the first
relevance rule the early scan carries.

Getting both out of the side-table at once needs one accessor, because the
filter is a shared borrow and the scope a mutable one, and the scan holds the
filter across repeated per-candidate refreshes. Split them by field:

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
this needs no interior mutability and costs nothing at runtime. It is also the
discipline the evaluator already follows: `select_transition` takes the graph,
the entry, and the scope as separate parameters rather than the table itself,
precisely so the caller owns the split.

`CandidateScope::refresh` therefore stays `&mut self`. That is not incidental —
the `&mut` is what makes "grows only at bind, written by index at refresh" a
compile-time fact rather than a convention. A `RefCell` on the fixed array would
buy the same call pattern and give that up, and the alloc probe would not
notice, since `RefCell` does not allocate. The existing `RefCell`s on the state
half are a *bind*-phase need — `BindingScope::resolve_input` takes `&self`, and
interning happens inside it, during bind — and are not a precedent for the
refresh path.

One shared scope means one interned `@state.*` vector, and interning converges
on the union of the names across every bound filter — the shape `BrainScope`
already has, per `intern_state_field`'s own doc comment — which never shrinks
for the life of a `BrainPrograms`. Per-candidate refresh writes that whole
union, so a filter reading one field still pays every bound field's
`EntityStateComponent` lookup. State the bound rather than assume it: the offer
set is player pawns, so per-enemy cost is party size times union size, both
small. It is not free the way the guard scope's union is — brain refresh runs
once per enemy per tick, candidate refresh once per *(enemy, candidate)* pair,
so the same union costs a factor more here. If it stops being small the answer
is a per-entry scope, not interior mutability.

`None` means no filter: nothing is evaluated and the scan is byte-for-byte
today's. Lowering emits no filter, so
legacy brains are unchanged. Extend the SDK with a `candidate` prelude object in
both runtimes — pre-wrapped leaves for the fixed table. `candidate` is a third
top-level export of the **existing** `sdk/lib/brain.{ts,luau}` module, not a new
SDK module: that module already owns the guard-input vocabulary, and `state` is
literally shared between the two scopes, so splitting them across two files
would fork one vocabulary. Unlike a new key on the `brain` object, a new
top-level export is gated by an explicit Luau allowlist mirrored in five places:
`BRAIN_LUAU_FIELDS` (`crates/scripting-core/src/luau_prelude.rs:222`) — which is
`&["brain", "state"]`, the module's *exports*, not the `brain` object's fields —
gains `"candidate"`, and so do the root export inventory
(`POSTRETRO_ROOT_MODULE_EXPORTS`, `luau_prelude.rs:290`), `luau_require.rs:446`,
and both `copy_lua_fields` call sites in
`crates/script-compiler/src/light_membership.rs:496,502`. A sixth site is
`sdk/lib/index.ts:18-19`, which re-exports `{ brain, state }` and gains
`candidate` beside them. The existing `state(name)` builder serves both
scopes unchanged: it emits an `@state.` leaf and the scope it binds against
decides whose field that is. `CANDIDATE_INPUTS` needs its own parallel pair of
drift tests, mirroring the two Task 1 names — and Task 2 therefore also writes
the `CandidateInputs` blocks into `sdk_lib.d.ts` and `sdk_lib.luau` in this same
phase, exactly as Task 1 does for `BrainInputs`. Landing the drift test without
the template blocks leaves it red from Phase 2 to Phase 4. Task 5 keeps only
`virtual_module.luau` and the regeneration. Task 2 and Task 4 both extend
`BehaviorGraphDescriptor::validate`: all errors — filter bind included — run
first, and the lints run only on the success path, so a rejected descriptor
never also logs a warning. Both runtimes must agree. Extend the alloc probe:
`refresh_and_guard_eval_perform_zero_heap_allocations` (`brain_scope.rs:482`)
gets a candidate-scope twin that arms the probe **after** bind, over
per-candidate refresh and eval across a multi-candidate scan. Arming over bind
fails by construction — `intern_state_field` pushes a `String` and grows two
`Vec`s, which is why the existing probe's own comment puts binding outside the
armed window. That test is AC 6's zero-alloc verifier. AC 6's other half — that
the filter compiles on the `Arc` staleness edge and not per tick — is a `sync`
claim no alloc measurement reaches; its verifier is a twin of
`sync_leaves_an_unchanged_brain_bound_without_rebinding`
(`brain_programs.rs:527`), asserting filter-program pointer identity across two
`sync` calls with an unchanged `Arc`.

### Task 3: Leash bounds acquisition

Give the floor's leash the same authority over fresh acquisition it already has
over retention. Today `BrainComponent::leash_range` (`Option<f32>`, `Some` only
for lowered legacy brains) is consulted in `ai/mod.rs` for the retained
candidate and for the replacement search on a leash-escape tick, but the
ordinary acquisition scan applies no range limit — which is the whole
oscillation. Move the rule into `targeting.rs` so the oversized tick module only
passes a value: the ranking scan and the selection entry point take the brain's
optional leash and reject any candidate beyond it, and `ai/mod.rs` passes
`brain.leash_range` at the same four call sites Task 2 touches. Symmetric means
one spelling, so the leash lives inside the chokepoint and answers for **both**
arms — retained eligibility and the ranking scan. It does **not** go inside
`target_candidate` or `nearest_target_candidate`: those are raw distance reads,
and `ai/mod.rs:434` calls `target_candidate` directly to price the think stride.
Leashing the read makes `current_distance` `None` for an out-of-leash retained
target, and `acquisition_due` reads `None` as due every tick — the stride
inversion the next paragraph exists to prevent, which no existing fixture
catches. No caller-side spelling survives; where each one goes is pinned below.
A brain with
no leash — every authored graph — keeps today's unbounded behavior exactly.

One thing neither the leash nor the filter may reach: the think stride.
`ai/mod.rs` derives `current_distance` from the retained candidate when a
retained id resolves, and from the early non-engaged scan otherwise
(`ai/mod.rs:439-441`), then feeds it to `acquisition_due`, whose `None` arm
means *due every tick*. Applying a relevance rule to either source turns a
far-band strided enemy into a full-scan-every-tick enemy — the exact inversion
of what the stride is for. The stride is cost machinery; the leash and the
filter are relevance rules; they must not share a data path. Keeping them apart
takes three distinct paths through the chokepoint, and this plan pins all three:

| Path | Leash applies | Filter applies |
|---|---|---|
| Stride distance — what `acquisition_due` reads | no | no |
| Retained-target eligibility | yes | no (AC 15) |
| Ranking-scan selection | yes | yes |

So the scan returns two values in one pass: the unfiltered, unleashed nearest
distance, and the best eligible candidate. Two values is a requirement, not a
convenience — re-running the scan to get the second is the regression the
comment at `ai/mod.rs:478-483` records as already fixed, and the early scan's
result is reused verbatim as the selection on the non-engaged acquisition
branch (`:484`). Any shape that splits the pass reintroduces the double scan.

The retained-target lookup stays a read: it yields the candidate and its
distance unconditionally, so an out-of-leash retained target still prices the
stride. Eligibility is then the chokepoint's answer rather than the caller's:
`targeting.rs` exposes `retained_outside_leash(candidate, leash) -> bool` and
`ai/mod.rs:449` calls it, so the caller still gets the local boolean its cheap
immediate clear needs while the comparison itself has exactly one spelling. That
is what "one spelling" means here — not that the boolean disappears. The
resulting `select_target` signature gains the optional leash and the two filter
parameters and returns both values; pin it once in Phase 2, since Task 2 changes
the same signature one phase earlier. The post-hoc `.filter(...)` on the
replacement search (`ai/mod.rs:462-465`) does vanish, since the scan now applies
the leash itself. The leash-escape branch keeps its cheap immediate clear and
its strided replacement, so the four call sites stay four.

Separately, add the ordering rule to `AiDescriptor::validate`
(`crates/foundation/src/data_descriptors/types/combat.rs`): after the
six-field range loop (`:302-317`) and before the `attackDamage` check
(`:318-326`), so both operands are known finite when the ordering error
reports, reject a descriptor whose `leashRange` is below its `detectionRange`,
with a message naming both values in the established `components.ai.<field>`
style. Both runtimes funnel through that validator, so
one edit covers QuickJS and Luau. Rust fixtures construct `AiDescriptor`
literally and bypass validation, which is why the floor rule must stand on its
own; the three Rust test fixtures using inverted tuning —
`detection_sets_agent_destination_and_leash_clears_it` (`ai_tests.rs:820`,
detection 18 / leash 8),
`retained_target_outside_leash_drops_instead_of_switching_to_out_of_leash_replacement`
(`:1082`, detection 40 / leash 10), and
`retained_target_outside_leash_clears_stale_destination_off_stride` (`:1119`,
same tuning) — pass unmodified and become the non-oscillation regression
rather than being deleted.

### Task 4: Graph disengagement lints

Add two structural diagnostics for authored behavior graphs, in a new sibling
module, `crates/foundation/src/data_descriptors/types/behavior_lints.rs`,
beside `crates/foundation/src/data_descriptors/types/behavior.rs` (824
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
no-target sentinel makes `gt`/`ge` read true. Detect the read by reusing
`IrNode::dispatch_input_names` (`crates/foundation/src/ir/mod.rs:186`) rather
than hand-rolling a second tree walk for an input node naming the has-target
constant — it is already colocated with the closed opcode set, so adding an
opcode must update it, and its `Vec<String>` allocation is fine at validation
time. The lint entry point **returns** its findings rather than logging them,
and `validate` logs what it returns via `log::warn!` — foundation already
depends on `log`. The entry point is
`behavior_lints::inspect(&BehaviorGraphDescriptor) -> Vec<BehaviorLint>` in
`crates/foundation/src/data_descriptors/types/behavior_lints.rs`; a
`BehaviorLint` carries its kind and the offending state names, so "one finding
naming those states" is checkable. AC 11 and AC 12 call `inspect` directly;
`validate` is a thin caller that logs what it returns. Findings are not
errors, so they do not join the `Result`.
This follows the existing bind-failure path, which chose an observable latch
over raw logging for the same reason: a warning nothing can assert on is a
warning nothing protects. Tests assert on the returned findings; no log-capture
harness is needed, and none exists reachable from foundation. Messages carry the
offending state names and the `components.behavior` path prefix, and fire once
per descriptor validation, which is once per parse and not per spawn.
Existing in-tree fixture graphs will start warning —
`the_wire_shape_round_trips_through_camel_case_json`
(`crates/foundation/src/data_descriptors/types/behavior.rs:743`) trips both
lints, and so does the shared `fn graph()` helper
(`crates/foundation/src/data_descriptors/types/behavior.rs:498`) used by most
sibling tests in that module. That fixture stays untuned — it exercises the serde wire shape, not
graph semantics — and its findings are unasserted and expected; same
disposition for `fn graph()`: untuned, findings unasserted and expected. The lowered legacy graph does not route through `validate`
(`lower_ai_descriptor`, `behavior_lowering.rs:69-182`, never calls it), so
legacy spawns produce no findings and the once-per-parse contract holds.
Confirm the shipped reference enemy trips neither, and add
a fixture graph for each.

### Task 5: Reference enemy, docs, and typedefs

Author the new vocabulary into the reference enemy
(`sdk/behaviors/reference/entities.{ts,luau}`, identical spellings): a
target-death stand-down interrupt declared immediately after the existing
has-target interrupt, and a candidate filter that excludes dead pawns and bounds
acquisition by distance. Two existing parity tests break on the added interrupt
and must be updated in this task:
`the_shipped_reference_enemy_graph_is_identical_in_both_authorings`
(`crates/scripting-core/src/data_descriptors/tests/behavior.rs:533`), which
pins interrupt targets as `vec!["idle"]` and becomes `vec!["idle", "idle"]`;
and `the_reference_oracle_matches_the_shipped_authored_graph`
(`ai_tests.rs:3953`), which mirrors the shipped graph in Rust and must gain the
same interrupt and filter. Its comments are the de-facto authoring documentation
and must teach four things — that target-side facts read zero with no target and
are only meaningful under `hasTarget`; that the death latch, not a health
comparison, is the death test and why; that candidacy is per-graph eligibility
while disengagement is per-state policy; and that `state("field")` names the
enemy's field in a guard and the candidate's in a filter, because the scope, not
the leaf, decides the entity. Extend `docs/scripting-reference.md` with the
floor's acquisition contract: what the engine offers as candidates, what it
never decides (aliveness), the legacy leash's dual acquisition/retention role,
that an authored graph owns its own disengagement, the target-side facts'
no-target zero readings and why `@brain.hasTarget` is the sole presence test,
why the death latch, not `le(targetHealth, 0)`, is the death test, and that a
filter over `candidate.distance` **is** the authored acquisition radius — the
reason no descriptor field spells one, and the answer to the question the seed
research left open. The existing anchors to extend are `### @brain.* guard
inputs` (line 620, a hand-maintained input table with no drift test guarding
it — it goes stale from Phase 1 until this task), `### The no-target trap`
(648), `### The level-wide pursuer` (702), and `## components.behavior` (429).
`primitives/mod.rs` does not own the `brain` interface —
`BrainInputs` and the `brain` declaration are hand-written into the two
template files Task 1 names. The typedef work splits accordingly: in
`crates/postretro/src/scripting/primitives/mod.rs`, update the `components.ai`
`detectionRange` and `leashRange` field descriptions (lines 346-348) to state
the ordering constraint and the leash's widened role. The current
`leashRange` description says "Distance from its origin past which the brain
disengages", but the implementation measures target distance from the
enemy's current position (`ai/mod.rs:447-448`); the rewrite must correct that
phrasing, not just extend it. Register the `candidateFilter` field on
`BehaviorGraphDescriptor`; the `CandidateInputs` interface and the `candidate`
declaration go into `sdk_lib.d.ts` and `sdk_lib.luau`, and `virtual_module.luau`
gains a `candidate: CandidateInputs,` member on `PostretroModule` alongside its
existing `brain: BrainInputs,` (`virtual_module.luau:120`) — this is the one
typedef template Task 1 does not touch. Then regenerate both
artifacts via `cargo run -p postretro --bin gen-script-types` and commit them;
`committed_sdk_types_match_current_registry`
(`crates/postretro/src/scripting/typedef/tests/committed.rs:8-37`) is what goes
red if they drift. Re-run the lint fixtures against the rewritten reference enemy — Task
4's clean-trip confirmation predates this rewrite.

## Sequencing

**Phase 1 (concurrent):** Task 1, Task 4 — disjoint (guard-input table and
brain scope versus descriptor lint module).
**Phase 2 (sequential):** Task 2 — consumes Task 1's settled input table and
extends the same evaluator side-table.
**Phase 3 (sequential):** Task 3 — shares `targeting.rs` and the same four
selection call sites with Task 2.
**Phase 4 (sequential):** Task 5 — documents the contract the first four tasks
settle, and regenerates typedefs once.

## Promotion prerequisites

`context_style_guide.md` §194 gates `drafts/` → `ready/` on capturing durable
decisions in `context/lib/`. Two are durable and neither is a task:

- `entity_model.md` §7c currently gives the engine "target selection and
  retention" outright. That stops being true: the engine offers candidates and
  the graph filters them. Amend the ownership split.
- `scripting.md` §11's adopter list describes the brain scope as the enemy
  behavior graph's only binding scope. Add the candidate scope as a second one
  over the same `@state.*` seam, resolved against a different entity.

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau |
|---|---|---|---|---|
| Target health fact | `BRAIN_TARGET_HEALTH_INPUT` | `"@brain.targetHealth"` | `brain.targetHealth` | `brain.targetHealth` |
| Target max health fact | `BRAIN_TARGET_MAX_HEALTH_INPUT` | `"@brain.targetMaxHealth"` | `brain.targetMaxHealth` | `brain.targetMaxHealth` |
| Target death latch fact | `BRAIN_TARGET_DIED_INPUT` | `"@brain.targetDied"` | `brain.targetDied` | `brain.targetDied` |
| Candidate filter field | `BehaviorGraphDescriptor::candidate_filter: Option<IrNode>` | `"candidateFilter"` | `candidateFilter?: RuntimeValue` | `candidateFilter: RuntimeValue?` |
| Filter error path | — | `"components.behavior.candidateFilter"` | — | — |
| Candidate facts table | `CANDIDATE_INPUTS` | — | `candidate.*` prelude object | `candidate.*` |
| Candidate distance | `CANDIDATE_DISTANCE_INPUT` | `"@candidate.distance"` | `candidate.distance` | `candidate.distance` |
| Candidate health | `CANDIDATE_HEALTH_INPUT` | `"@candidate.health"` | `candidate.health` | `candidate.health` |
| Candidate max health | `CANDIDATE_MAX_HEALTH_INPUT` | `"@candidate.maxHealth"` | `candidate.maxHealth` | `candidate.maxHealth` |
| Candidate death latch | `CANDIDATE_DIED_INPUT` | `"@candidate.died"` | `candidate.died` | `candidate.died` |
| Candidate state leaf | `ENTITY_STATE_INPUT_PREFIX` (reused) | `"@state.<field>"` | `state("field")` | `state("field")` |
| Candidate input prefix | `CANDIDATE_INPUT_PREFIX` | `"@candidate."` | — | — |
| Candidate inputs type | — | — | `CandidateInputs` | `CandidateInputs` |

No FGD column: all of it is descriptor-owned tuning, never map-overridable. The
state leaf reuses the guard spelling on purpose — the scope binds it, so no
second name exists for the same field. `RuntimeValue` is the number|boolean
union, so the boolean-only constraint on `candidateFilter` is runtime-enforced
(AC 7) and deliberately not expressed in the typedef.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| The engine floor holds no aliveness policy for target selection; the attack gate's aliveness check is the floor's only health read and stays a damage gate | Task 2 (by not adding one) | Any future "obvious fix" adding a health test to candidacy or retention | AC 8, 5 |
| Target-side facts read their type's zero with no target; `@brain.hasTarget` is the sole authoritative presence test, and the distance sentinel remains the lone exception | Task 1 | Any new target-side fact choosing a non-zero no-target reading | AC 2 |
| The death latch, not a health comparison, is the authored death signal — it is unambiguous untargeted, carries the sweep's non-finite arm, and is the only one of the two that every observer reads alike. `targetDied` turns true on the tick after death commits, uniformly | Task 1 | Docs and reference authoring must not teach `le(targetHealth, 0)` as the death test, nor as a same-tick conjunct — the health read is coupled to AI iteration order for one tick after a kill | AC 1, 13, 14 |
| Candidacy is per-graph eligibility; disengagement is per-state policy. The filter never runs against the retained target | Task 2 | The shared candidate lookup also resolves the retained target — the filter must sit in the ranking scan only | AC 1, 3, 15 |
| The floor decides what is *perceivable* — which entities the scan is offered, and later the `visible` predicate — while the graph decides which offered candidates are worth engaging. The filter is strictly downstream of `visible` and can only narrow the offer set | Task 2 | A perception spec exposing a `@candidate.visible` fact and moving line-of-sight policy into the filter; any filter arm that widens what the scan considers | Design constraint — not observable until a resolver exists |
| The filter answers eligibility, never order: it produces a boolean, and ranking stays nearest-with-hysteresis | Task 2 | A threat or priority spec widening the filter to a score rather than adding its own seam | AC 3, 7 |
| `@state.*` names a field; the binding scope names whose. One spelling reaches the enemy from a guard and the candidate from a filter, so the `scripting.md` §11 composition seam holds on the acquisition path | Task 2 | A second, candidate-specific spelling for the same fields would fork the seam and strand mod-authored properties outside candidacy | AC 4 |
| Both fixed input tables are append-only — a name's index is its runtime read handle | Task 1, Task 2 | An insertion or reorder silently re-points every bound program | AC 14 |
| An unauthored graph is bit-identical to today; legacy lowering emits no new edge and no filter; legacy acquisition changes only by gaining the leash bound | Task 2, Task 3 | Any default filter, or a lowering that emits a target-death edge | AC 5, 8, 9 |
| Bound programs stay derived data in the evaluator side-table, rebuilt via the `Arc` pointer-identity staleness test; the filter joins them rather than riding the component | Predecessor, extended by Task 2 | A filter program stored on `BrainComponent` would reintroduce serde and equality coupling | AC 6 |
| Acquisition-path evaluation is zero-alloc per tick; the filter adds a constant factor to an existing traversal, never a new walk | Task 2 | Candidate-scope refresh must not intern, clone, or collect — the `@state.*` snapshot grows at bind alone | AC 6 |
| `refresh` takes `&mut self`, which is what makes "grows at bind, written by index at refresh" a compile-time fact. Simultaneous access to a filter and its scope comes from splitting the side-table by field, not from interior mutability | Task 2 | Wrapping the fixed array in a `RefCell` to dodge a borrow error would demote the guarantee to a convention, and the alloc probe would not catch it — `RefCell` does not allocate | AC 6 |
| The floor's leash is symmetric: a target beyond it is neither acquired nor retained | Task 3 | Any new acquisition path bypassing the selection chokepoint; the ordering validator alone cannot enforce it, since Rust fixtures bypass validation | AC 9 |
| A brain with no leash keeps unbounded acquisition, with disengagement owned by its guards | Task 3 | Any default value substituted for the absent leash | AC 5 |
| The think stride is cost machinery and shares no data path with relevance rules. Its distance comes from an unfiltered, unleashed read on both of its sources — the retained candidate and the early scan — so neither leash nor filter can silently reprice it. The scan returns the stride's distance and the eligible selection from one pass | Task 3, constrained by Task 2 | Deriving `current_distance` from a filtered or leashed value — `acquisition_due` reads a `None` distance as *due every tick*, inverting the stride. Splitting the scan into two passes to separate the two values reintroduces the double scan `ai/mod.rs:478-483` records as already fixed. Putting the leash inside `target_candidate` inverts the stride and no legacy fixture catches it | AC 17, 5 |
| Acquisition-range authority stays engine-side and legacy-only: no descriptor field spells disengagement range. An authored graph bounds acquisition with a distance filter instead | Predecessor (pinned), extended by Task 2 | Task 3 must thread the existing component field, never introduce an authored one; a future perception spec must reach for the filter before a new descriptor range | AC 5, 14, 16 |
| Engagement radius stays combat-slot spread only — never acquisition, retention, or damage | Predecessor (pinned) | Any task tempted to reuse it as an acquisition radius | AC 5 |
| Graph diagnostics are warnings, never errors: a relentless pursuer is a legitimate authored design | Task 4 | Escalation to a validation error would reject shipping content | AC 11, 12 |

## Script syntax examples

```ts
// Proposed design
const ACQUIRE_RANGE = 16;

behavior: {
  initial: "idle",
  // Per-GRAPH eligibility, asked once per candidate on every ranking scan.
  // Not a guard: guards run against a target that has already been chosen.
  //
  // Three clauses: not dead, within this graph's acquisition radius, and not
  // flagged untargetable by whatever impact policy owns that field. The radius
  // lives HERE — the floor spells no acquisition range for an authored graph,
  // so a distance clause is how a graph gets one. `state()` reads the
  // CANDIDATE's field in this position and the enemy's inside a guard: the
  // scope decides the entity, not the leaf. No `and` opcode: conjunction is
  // `select(a, b, false)`, negation is `select(a, false, true)`, and "not-a
  // and b" is `select(a, false, b)`. The outer node here is the third form;
  // the inner is the first.
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
    // target that reads zero and fires for the wrong reason, and for one tick
    // after a kill it reads zero only for enemies iterated after the killer.
    // The latch is the same for everyone; the health read is not.
    { to: "idle", when: brain.targetDied },
  ],
  // ... states as before
}
```

## Accepted trade-offs

- Warning and error messages name state names and the `components.behavior`
  path but not the owning entity's canonical name, which the descriptor
  validators do not have in hand. Threading it would widen every descriptor
  type's validator signature. A third path exists and is untried: annotate at
  the parser call site, which does know the name, rather than inside the
  validators. Accepted as-is for this plan; that is the option to reach for
  first if authors report the diagnostics as hard to place.

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
  LOS would bake in one point on that spectrum, which is structurally the same
  defect as the hardcoded aliveness rule this plan removes. The cost objection
  answers itself through the IR: `IrNode::dispatch_input_names`
  (`crates/foundation/src/ir/mod.rs:186`) reveals at **bind time** whether a
  filter reads that leaf, so a graph that never mentions it pays for no raycasts
  and perception is priced per graph by what its author actually asked for. What
  is genuinely open is only the resolver's own shape, which waits on a consumer.
- **Target ranking lands as a second IR expression, not a widened predicate.**
  The seed research assumes the alternative — widening `visible` from `bool` to
  a weight — but that puts *preference* inside the *perception* predicate, which
  the perceivable/worth-engaging invariant above forbids. Ranking is pure taste:
  nearest, weakest, most-recently-damaged are all valid designs. So it belongs
  beside eligibility, as a per-graph score expression over the same
  `@candidate.*` namespace this plan builds — where namespace, scope, binding,
  and staleness are already in place. The filter stays boolean and the score
  stays separate: folding them by encoding ineligibility as a sentinel score is
  the exact shape of `BRAIN_NO_TARGET_DISTANCE`, which shipped as a real defect.
  *Where* is settled; *when* still waits on a consumer.
- **Threat and last-attacker land on the candidate scope, not a new mechanism.**
  The candidate scope is refreshed per *(enemy, candidate)*, so it is the
  design's only per-pair evaluation context. Any enemy × candidate relation that
  reduces to a number or a boolean is a `@candidate.*` fact — no per-pair
  storage on the brain is needed to express one. The memory already exists too:
  `HealthComponent` carries a bounded per-source contributor ledger
  (`crates/entities/src/components/health.rs:151-159`) with `accumulated_damage`,
  `hit_count`, and `last_attacker`, maintained by the damage chokepoint. The
  enemy is the victim when a player shoots it, so its own health component
  already records who hurt it and how much. `@candidate.damageDealtToMe` is a
  scan of that bounded ledger for entries whose `last_attacker` is this
  candidate; `@candidate.isLastAttacker` is the boolean collapse of the same.
  What genuinely constrains a threat spec is the ledger's *shape*, not the seam:
  it is keyed by `source_id` with the attacker as a field, and its overflow
  bucket carries no source id, so an attacker evicted from the exact ledger is
  indistinguishable from one that never fired. A threat spec must decide whether
  that fidelity is enough before adding storage.
