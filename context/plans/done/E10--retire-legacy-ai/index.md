# Retire Legacy AI

## Goal

Make `components.behavior` the only enemy-brain authoring surface. Remove the
fixed four-state `components.ai` preset, its lowering path, and its hidden
engine leash so every acquisition and disengagement policy is visible in the
authored graph.

## Scope

### In scope

- Remove the legacy descriptor, generated types, lowering, and engine leash.
- Reject stale `components.ai` blocks in both script runtimes with a migration
  error.
- Move the pose fixture and development artifacts to direct behavior graphs.
- Preserve behavior-graph targeting, replication, spawning, and diagnostics.
- Update tests, SDK artifacts, and author documentation.

### Out of scope

- New candidate facts, target scoring, faction alignment, or threat tracking.
- New perception rules or a replacement global disengagement radius.
- Compatibility loading for descriptors or serialized data authored with
  `components.ai`.

## Acceptance criteria

- [ ] `components.behavior` is the sole descriptor path that materializes an
      enemy brain and navigation agent. No public SDK type or documentation
      advertises `components.ai`, `AiDescriptor`, `AiStateNames`, or
      `leashRange`.
- [ ] TS/JS rejects every own `components.ai` property, including `null` and
      `undefined`. Luau rejects every non-nil `components.ai` value; `ai = nil`
      removes the table key and is indistinguishable from omission. Both errors
      direct authors to `components.behavior`. Neither runtime silently drops a
      representable legacy value.
- [ ] Target selection has no engine-owned range policy. Candidate filters
      narrow fresh acquisition only; retained-target stand-down is controlled
      by graph transitions or interrupts. While a target is retained, price
      stride from its raw distance without a new scan. Otherwise price stride
      from the unfiltered nearest offered candidate. These rules apply to
      think-stride pricing only. Candidate filtering and graph guards do not
      filter, clamp, or otherwise alter that raw stride distance. `BrainFacts`,
      including `@brain.targetDistance` and all target-side facts, remain bound
      to the selected eligible target, or report no target.
- [ ] The shipped TS and Luau reference descriptors include equivalent direct
      behavior-graph pose fixtures. Each has 50 m candidate eligibility, a 16 m
      idle-to-engaged transition, 2 m attack range, target-loss and target-death
      interrupts, then an authored `targetDistance > 50` stand-down interrupt
      to idle. Both pass descriptor and mesh-animation validation through
      production loading paths.
- [ ] Enemy spawning, host/client classification, remote presentation,
      diagnostics, and telemetry continue to operate for behavior-authored
      brains.
- [ ] Generated TS/Luau declarations and full-registry snapshots contain the
      behavior surface and omit retired legacy names. The complete workspace
      quality gate passes.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| Behavior graph is the only brain representation from descriptor parse through spawn and client presentation. | Task 2 | Task 3 fixtures, Task 4 SDK/docs | AC 1, 4, 5 |
| Stale legacy source fails before manifest commit; it never becomes a descriptor with no brain. | Task 2 | Task 3 parser tests | AC 2 |
| Engine targeting offers and prices candidates but does not impose acquisition or retention distance policy. Think stride uses a retained target's raw distance without a scan, or the unfiltered nearest offered candidate when none is retained. Candidate filtering and graph guards do not alter that stride distance. `BrainFacts` stay bound to the selected eligible target. Graph candidate filtering controls acquisition; graph state policy controls stand-down. | Task 2 | Task 3 AI regressions, Task 1 fixture graph | AC 3, 4 |
| No wire-format or replication-version change is introduced: live Brain + Agent state remains the network classifier. | Task 2 | Task 3 replication fixtures | AC 5 |

## Tasks

### Task 1: Migrate reference content

Replace the sole intentional legacy consumer, the pose fixture, with equivalent
direct behavior graphs in the TS and Luau reference descriptors. Add and
register the Luau pose fixture. Each graph has 50 m candidate eligibility, a
16 m idle-to-engaged transition, 2 m attack range, target-loss and target-death
interrupts, then an authored `targetDistance > 50` stand-down interrupt to
idle. Preserve that interrupt order. Preserve attack tuning, movement, and
mesh-state names. Update generated
development-script artifacts by their real source/ownership, and remove wording
that treats the reference enemy's 50 m policy as legacy-lowering parity. Add or
extend production-path TS/Luau reference-descriptor coverage so graph shape and
mesh-animation validation are exercised without `components.ai`.

### Task 2: Remove the legacy descriptor and runtime path

Delete the fixed AI descriptor and its lowering module. Remove the `ai`
descriptor slot, legacy-only component exclusivity rule, legacy brain
constructors, and the optional brain leash. Simplify targeting and the AI tick
to remove every engine-owned range value that clears retention or filters
acquisition; preserve graph-authored candidate filters, including distance
predicates, for fresh acquisition only. Preserve retained-target raw-distance
stride pricing without a scan and unfiltered-nearest-offered-candidate stride
pricing otherwise. These are think-stride inputs only: candidate filtering and
graph guards must not filter, clamp, or otherwise alter their raw distance, and
`BrainFacts` must remain bound to the selected eligible target. Detect the JS
legacy key as an own key without prototype-chain lookup. Migrate every
compiled production and test fixture/call site that imports the retired
descriptor, lowering, or legacy brain constructor. Make archetype attachment,
host/client enemy classification, and remote locomotion consume behavior graphs
only. Both script parsers must explicitly reject a present `components.ai` key
rather than ignore it. Do not change replication formats or telemetry semantics.

### Task 3: Replace legacy tests and fixtures

Delete lowering/parity-only coverage. Replace legacy-only assertions with direct
graph behavioral regressions for acquisition stride, candidate eligibility,
retention/hysteresis, target loss/death, attack timing, aggro gating,
navigation/combat slots, host/client suppression, replication, and presentation.
Add a direct-graph regression with a far retained target and a nearer offered
candidate. Assert retained-target stride cadence and no off-stride retargeting.
Add a no-retained-target direct-graph stride regression with a near offered pawn
rejected by the candidate filter and a farther eligible pawn. Assert cadence
uses the near raw offered distance while `BrainFacts` and graph guards describe
the farther selected target. Assert an inherited JS `ai` property is ignored
while own `null` and `undefined` properties reject. Add one-tick
stand-down regressions from both chase and attack; each must reach idle through
the `targetDistance > 50` interrupt, ordered after target-loss and target-death
interrupts.
Add JS and Luau parser-rejection tests and a behavior graph's Brain + Agent
spawn tests.

### Task 4: Remove SDK and documentation surface

Remove legacy primitive registrations and typedef mappings. Regenerate both
committed SDK declarations and full-registry fixtures; replace the positive
legacy-type guard with a behavior-present/legacy-absent guard. Update the
scripting reference and entity-model context to describe one behavior-graph
surface with graph-owned acquisition and disengagement policy. Do not update
frozen historical plans.

## Sequencing

**Phase 1 (sequential):** Task 1 — direct reference content must be ready before the legacy surface disappears.

**Phase 2 (sequential):** Task 2 — atomic removal of shared types plus migration
of compile-time consumers.

**Phase 3 (concurrent):** Task 3 and Task 4 — direct graph test strengthening
and generated SDK/documentation cleanup after Task 2.

## Rough sketch

`components.behavior` already expresses legacy intent: a candidate distance
filter bounds new acquisition and a target-distance transition or interrupt
stands down a retained target. Target loss and target death remain graph
interrupts. The old death state and despawn delay are retired with the preset;
death remains health/impact-policy owned.

The parser rejection is intentionally retained after the descriptor type is
gone. It is the migration boundary for pre-release content and prevents an
unknown component from being accepted as an inert entity.

The implementation deletes logic from several oversized runtime modules rather
than extending them. Large fixture modules may change only to migrate their
test setup; no split is required for deletions or test-only mechanical updates.
