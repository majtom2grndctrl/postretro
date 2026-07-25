# Behavior State Graph

## Goal

Replace the engine-closed four-state enemy FSM with an authored **behavior state graph**: mods declare states, per-state motion/action/animation, and IR transition guards; the engine owns the evaluator, target selection, steering, damage, and determinism. This is the "Richer enemy/character behavior descriptors" roadmap bullet (Epic 10) — the foundation that turns future behaviors (stagger, patrol, flee, multi-attack pacing) into authored content instead of `LogicalState` enum widenings, and the substrate a later planner/utility selector plugs into (FEAR-style intelligence is a transition-selection policy over this graph, not a different graph).

## Scope

### In scope

- `components.behavior` descriptor block: named states (`animation`, `motion` verb, optional `action`, ordered `transitions`), a top-level ordered `interrupts` list (any-state transitions, evaluated first), `initial` state, and an `attack` tuning block (`damage`, `range`, `cooldownMs`) consumed by the `attack` action verb.
- Transition guards as behavior IR (`RuntimeValue` → `IrNode`), bound once at spawn over a new **BrainScope** and evaluated every tick — first-true-wins in declaration order, interrupts before state-local transitions. No latent actions: nothing a state is doing (animation, attack cooldown) ever blocks guard evaluation. Commitment windows are authored guards over `@brain.timeInStateMs`, not an engine mechanism.
- BrainScope input vocabulary (fixed table + prefixes): `@brain.hasTarget` (Bool), `@brain.targetDistance` (Number; no-target sentinel `1.0e9`), `@brain.timeInStateMs`, `@brain.attackCooldownMs`, `@brain.acquisitionDue` (Bool — true on think-stride ticks), `@brain.health`, `@brain.maxHealth`, plus `@state.<name>` per-entity state leaves (E16 keystone routing, `EntityScope` precedent).
- Closed motion-verb vocabulary v1: `chaseTarget` (combat-slot steering, today's Chase), `hold` (clear destination, stand), `freeze` (touch nothing — terminal presentation). Closed action-verb vocabulary v1: `attack` (cooldown-gated contact damage from the `attack` block, today's melee).
- Optional per-state `onEnter` named-event address, fired through the existing post-tick event drain.
- Legacy compatibility: `components.ai` is retained and **lowers at spawn to a generated graph** that reproduces `evaluate_transition` exactly (including acquisition-gated detection/leash edges via `@brain.acquisitionDue`). One brain representation, one evaluator code path. Authoring both `ai` and `behavior` is a parse error.
- Engine-owned floor unchanged: target selection/retention/hysteresis, think-stride, aggro gate (`updateEnemyState`; closed gate forces `initial` + clear steering), no-target forces `initial` + clear steering, combat-slot resolution, facing/locomotion arbitration, damage chokepoint, host-only evaluation (client skip gate untouched), E16 death/despawn ownership.
- Behavior-preserving split of `scripting/systems/ai.rs` (969 lines) before extension.
- Reference enemy migrates to an authored explicit graph; pose-fixture enemy stays on legacy `ai` to keep the lowering path exercised.

### Out of scope

- Planner / utility transition selection — v1 is ordered guards; the selection step is pinned as a pure, replaceable seam (see Rough sketch) so a planner is a later drop-in.
- Parallel layers (separate locomotion/action graphs), hierarchical sub-states, squad blackboard — named extensions; the descriptor envelope must not preclude them (a graph is one layer of a future set) but none ships now.
- Graph-authored writes (`onEnter` `setState`, guard side effects) — guards are read-only; per-entity state is written by impact policies and reactions, read by guards.
- New perception inputs (LOS, sound, alert propagation) — the roadmap's line-of-sight bullet and `research/enemy-aggro-model.md`; they land as new BrainScope inputs later, no graph-shape change.
- `and`/`or`/`not` IR opcodes — additive substrate follow-up, not this plan. Lowering builds `select` trees Rust-side until the opcodes land; authored guards benefit from them once available.
- Multi-attack (`E10--enemy-multi-attack`) and stagger (`E10--enemy-stagger`) — both re-target onto this graph after it lands (stagger = impact policy writing `@state.*` + an authored interrupt; multi-attack enriches the action vocabulary). Their drafts are not edited here.
- Wire/replication changes — graph state is host-only sim state; clients keep consuming replicated animation state.
- Removing the vestigial `death` state map entry from legacy `ai` descriptors — parsed and lowered as today; cleanup is separate. `deathDespawnMs` is carried forward as an optional field on `BehaviorGraphDescriptor` (default 2000 ms).

## Acceptance criteria

- [ ] Full existing AI integration test suite passes unchanged for legacy `components.ai` descriptors: reference-fixture transition ticks, damage cadence, animation switches, facing, stride, aggro-gate, and combat-slot behavior are identical. Pure-core unit tests port to the lowered graph rather than being deleted.
- [ ] A `components.behavior` descriptor parses and validates identically in QuickJS and Luau, with pathed errors in both for: unknown `initial`, a transition `to` naming no declared state, duplicate state names, empty `states`, and a guard that fails bind validation (unknown input name, type mismatch) — the error names the state and transition index.
- [ ] Authoring both `components.ai` and `components.behavior` on one descriptor is a parse error in both runtimes.
- [ ] Guards are evaluated every tick: a transition whose guard becomes true fires that tick even mid one-shot attack clip and mid attack cooldown. A guard over `@brain.timeInStateMs` implements a commitment window: the exit fires on the first tick the window elapses and never before.
- [ ] An `interrupts` entry fires from any state and wins over a simultaneously-true state-local transition; among interrupts, declaration order wins. Sim determinism tests stay green.
- [ ] An impact policy writing a per-entity state field causes a `@state.<name>` guard to fire on the next AI tick (the stagger shape, demonstrated end to end).
- [ ] The reference enemy authored as an explicit graph is behavior-identical on a fixture to the same enemy authored via legacy `components.ai`: same transition ticks, damage cadence, and animation requests.
- [ ] A state's unknown animation name warns once at spawn and keeps the prior animation at tick time (existing invariant, now walked over authored graph states).
- [ ] `updateEnemyState` with `aggro: false` forces an authored-graph enemy to its `initial` state with steering cleared; re-arming resumes normal evaluation.
- [ ] `BrainComponent` serde round-trips without serializing bound guard programs; programs rebind from the retained graph, and component equality is unaffected by them (dash precedent).
- [ ] Per-tick guard evaluation allocates zero heap (alloc-probe assertion, matching the substrate invariant).
- [ ] SDK typedef drift tests pass with the new `behavior` types in both committed fixtures (`postretro.d.ts`, `postretro.d.luau`); the agent diagnostics overlay shows the authored state name.

## Tasks

### Task 1: Split `ai.rs`

Behavior-preserving split of `crates/postretro/src/scripting/systems/ai.rs` (969 lines) into a `systems/ai/` module directory: the pure transition core (`evaluate_transition`, `TransitionResult`, `SteeringIntent`, stride/hysteresis helpers), target selection (`select_target`, `TargetPawn`, candidate helpers), and the tick orchestration (snapshot/compute/apply passes, steering application, damage, animation, facing). `pub(crate)` surface (`run_ai_tick_with_navigation_and_impact`, `ENEMY_ATTACK_EVENT`, test-visible helpers) re-exported so `sim/mod.rs` and `ai_tests.rs` change imports only. No behavior change; full suite green.

### Task 2: Foundation descriptor + parsing

`BehaviorGraphDescriptor` in `postretro-foundation` (`data_descriptors/types/`): `initial: String`, `states: BTreeMap<String, BehaviorStateDescriptor>` (`animation: String`, `motion: MotionVerb`, `action: Option<ActionVerb>`, `transitions: Vec<TransitionDescriptor>`, `on_enter: Option<String>`), `interrupts: Vec<TransitionDescriptor>`, `attack: Option<AttackParams>`, `move_speed: f32`, `death_despawn_ms: Option<f32>` (default 2000 — matches the legacy `AiDescriptor` default; absent or `null` uses the default; validated finite and > 0 when present). `move_speed` feeds `attach_agent` at the attach site. `TransitionDescriptor { to: String, when: IrNode }` — raw `IrNode` per the descriptor-partition rule; every referenced type is foundation-resident so the descriptor stays in foundation. `MotionVerb`/`ActionVerb` are closed serde enums per the boundary inventory. Structural validation at parse: names resolve, no duplicates, numeric `attack` fields finite/positive (damage ≥ 0). Add the `behavior` key to `EntityTypeDescriptor` (`postretro-entities`, `entities/src/data_descriptors/types/entity.rs`) and both twin parsers (`scripting-core/src/data_descriptors/js/entity.rs`, `scripting-core/src/data_descriptors/lua/entity.rs` — both funnel through `serde_json`, so divergence is impossible), including the `ai`+`behavior` mutual-exclusion error. Extend the typedef generator (`scripting/primitives/mod.rs` descriptor fields) so `behavior` and its types emit into both typedefs; guard fields emit as `RuntimeValue`. The typedef generator's `rust_to_ts` (`scripting-core/src/typedef/common.rs`) needs an explicit `"IrNode" => "RuntimeValue"` mapping entry (both runtimes share `common.rs` for type mapping — `rust_to_luau` mirrors `rust_to_ts` in the same file). `MotionVerb` and `ActionVerb` emit as string literal union types (`"chaseTarget" | "hold" | "freeze"`, `"attack"`) — the `FireMode` precedent, not plain `string`. State-name cross-references (`initial`, `transitions[].to`) should use `const`-generic inference on the `behavior` block so the IDE constrains them to authored state keys — the `defineStore<const S>` precedent; `defineEntity` gains a `const` generic or the behavior sub-block uses a typed helper. This is best-effort TS ergonomics: if the inference proves awkward, plain `string` with runtime validation is acceptable. Luau has no equivalent compile-time constraint — state-name cross-references rely on the runtime structural validation (names resolve, no duplicates) that both runtimes already perform. SDK sugar for guard inputs: a `brain` export (frozen object of pre-wrapped `runtime.read("@brain.*")` IR input leaves, generated from `BRAIN_INPUTS` — `brain.targetDistance`, `brain.hasTarget`, etc.) and a `state(name)` function (wraps `runtime.read("@state." + name)`) ship from `"postretro"` in both runtimes. No new primitives — pure SDK-side prelude helpers, same pattern as `getGameState()`. The `brain` object and `state` function are hand-maintained SDK prelude helpers (`sdk/lib/`) with a compile-time sync obligation against `BRAIN_INPUTS` — not emitted by the typedef generator, since they are authored JS/Luau, not generated type declarations. Typedef fixture files are regenerated and committed in Task 6; drift tests are expected red from Task 2 through Task 5.

### Task 3: BrainScope

A `BindingScope` implementation in the binary crate (`crates/postretro/src/scripting/systems/ai/`) beside the graph evaluator — AI domain logic, not component storage, so it stays out of the entities chokepoint (dev guide: "AI lives in the binary"). It names `EntityStateComponent` for `@state.*` routing, which the binary can import; it cannot sit in foundation for the same reason. Fixed input table (`MovementScope` precedent — name→index handles, order load-bearing): `@brain.hasTarget` Bool, `@brain.targetDistance` Number (sentinel `1.0e9` when no target), `@brain.timeInStateMs`, `@brain.attackCooldownMs`, `@brain.acquisitionDue` Bool, `@brain.health`, `@brain.maxHealth`. `@brain.health` and `@brain.maxHealth` read from `HealthComponent` during `refresh`, before guard evaluation. The `BRAIN_INPUTS` constant table (name-to-type pairs) lives in foundation alongside the descriptor types; BrainScope in the binary imports it. `@state.`-prefixed names route to `EntityStateComponent` via interned name→index handles with per-tick snapshot refresh (`EntityScope` routing precedent, read path only; impact policies write `EntityStateComponent` through the E16 keystone routing substrate — no new write plumbing needed). No outputs (`resolve_output` → `None` — guards are read-only). Ships `for_validation()` (type-correct zeros, for declaration-time bind checks) and `refresh(...)` repopulating the fixed slots and state snapshot allocation-free each evaluated entity. Declaration-time validation: a **validation-only `BindingScope`** implementation lives in foundation (not the binary), constructed from `BRAIN_INPUTS` — it resolves `@brain.*` names to their declared types and accepts any `@state.*` prefix as Number. Both runtimes bind every authored guard against this scope at descriptor validation and reject with pathed `BindError` context — the dash `bind_dash_node` mirror. The runtime `BrainScope` in the binary is a superset (it also refreshes live values), but declaration-time validation never imports it. The `@state.*` snapshot Vec is grown at bind time (one slot per unique state name across all guards) and refresh writes by index — no per-tick growth. BrainScope is a single reusable instance (the `MovementScope` precedent): one lives beside the evaluator, `refresh` repopulates it for each evaluated entity; the `@state.*` slot table is the union of all names across all bound descriptors, grown at each entity type's spawn-time bind (not per-tick). Typedef drift tests are expected red until Task 6 regenerates the fixtures.

### Task 4: Brain generalization + spawn bind

`BrainComponent` (existing, `entities/components/brain.rs`) gains graph fields: retained `BehaviorGraphDescriptor`, current state (stable index into a resolved state list), and `time_in_state_ms: f32`. Bound guard programs (`Vec` parallel to interrupts + per-state transitions) live in a **side-table owned by the evaluator** in the binary crate (`systems/ai/`), not on `BrainComponent` — the `DashPrograms` pattern does not apply here because `BrainScope` is in the binary while `BrainComponent` is in entities, and `BoundProgram<BrainScope>` cannot cross that crate boundary. The evaluator binds programs at spawn and removes them at despawn; bind failure warns once and disables the affected transition (fall-back-to-native posture). Programs are derived data: never serialized, rebuilt from the retained graph on spawn/deserialize, and invisible to `BrainComponent` equality and serde — the layering achieves the same invariant as `DashPrograms` without the `#[serde(skip)]` / custom-`PartialEq` machinery. Engine substates stay as-is: `attack_cooldown_remaining_ms`, `think_stride_counter`, `aggro_armed`, `acquired_target`, `combat_slot`, `combat_slot_hold_ticks`, `death_despawn_remaining_ms` (timer; for legacy `ai` descriptors, seeded from `AiDescriptor.death_despawn_ms` via the lowered graph; for authored `behavior` descriptors, seeded from `BehaviorGraphDescriptor.death_despawn_ms` which defaults to 2000 ms when absent). The attach site (`builtins/data_archetype.rs` ai-block, line ~479) becomes: legacy `AiDescriptor` → `lower_ai_descriptor(&AiDescriptor) -> BehaviorGraphDescriptor` (foundation fn, Task 2's types; guards built as `IrNode` trees Rust-side reproducing `evaluate_transition` edge-for-edge: detection/leash edges conjoined with `@brain.acquisitionDue` via `select` (`select(acquisitionDue, innerGuard, false)` — evaluates `innerGuard` only when `acquisitionDue` is true, otherwise short-circuits to false; this encodes logical AND using the existing opcode set); Idle→Attack further conjoins `le(dist, detection_range)` and `le(dist, attack_range)` (see Invariants table row 5 for the full three-way conjunction shape)) → one `BrainComponent` shape; authored `behavior` attaches directly. Spawn-time animation validation walks the graph's states instead of `LogicalState::ALL`. `attach_agent` keeps consuming `move_speed` (legacy) or a `behavior.moveSpeed` field mirrored into the same parameter. Additional call sites that key on `descriptor.ai.is_some()` must widen to also check `descriptor.behavior.is_some()`: `descriptor_materializes_ai_enemy()`, `ai_capsule_center_from_feet_offset()`, and the mesh `origin_offset` branch — all in `builtins/data_archetype.rs`. Typedef drift tests are expected red until Task 6 regenerates the fixtures.

### Task 5: Graph evaluator in the tick

Replace the `evaluate_transition` call path with graph evaluation inside the existing snapshot/compute/apply tick: per evaluated enemy, `refresh` the BrainScope, look up the entity's bound programs from the evaluator's side-table (Task 4), evaluate `interrupts` then current-state `transitions` in order, first `IrValue::Bool(true)` wins (the impact `group.when` eval shape); the selection step is one pure function `select_transition(graph, bound, scope) -> Option<StateIndex>` — the pinned pluggable seam. Self-targeting interrupts (`interrupt.to == current_state`) are skipped — they only fire as actual state transitions. Apply the target state's `MotionVerb` as today's `SteeringIntent` mapping (`chaseTarget`→Chase with combat-slot preference, `hold`→Clear, `freeze`→Hold), run the `attack` action (cooldown-gated `apply_damage_with_context`, `ENEMY_ATTACK_EVENT`, target-alive check — today's rules), request the state's animation through the existing switch/warn path with the locomotion latch (`chaseTarget` states with no action verb substitute the graph `initial` state's animation at rest (agent has no active navigation destination or has reached its combat slot — the existing locomotion-latch condition); states with an action verb keep their own animation — Alert gets substitution, Attack does not). Authored graphs should use a rest-appropriate animation for `initial`, since it serves as the at-rest fallback. Reset `time_in_state_ms` and fire `on_enter` on state change. Plumbing: the tick's event return widens from `Vec<&'static str>` to `Vec<Cow<'static, str>>` to carry authored `on_enter` addresses without per-attack allocation for the static `ENEMY_ATTACK_EVENT`. Authored `on_enter` addresses produce `Cow::Owned` (one clone per state-entry fire, not per-tick); the zero-alloc invariant (AC 11) covers guard evaluation, not event emission. `TickEvents.death` already uses `Vec<String>` — the `ai` field uses `Cow` only to avoid cloning the static `ENEMY_ATTACK_EVENT` on every attack tick. The return flows through `TickEvents.ai` (`sim/mod.rs`) into the post-tick drain in `main.rs` (~line 2167); both the `TickEvents` field, the `main.rs` accumulator, and the drain loop's `fire_named_event` call widen. Aggro gate closed forces `initial` + Clear before evaluation, as today's forced Idle. `acquisitionDue` is computed from the initial candidate's distance; `targetDistance` reflects the final selected target — both set by the engine floor before graph evaluation. Stride, target selection, hysteresis, combat slots, facing, and the E16 pass-1 skips (deferred despawn, downed) are untouched. Death is not a graph transition: the E16 death sweep (stage 8) latches HP-zero entities, and the pass-1 skip prevents graph evaluation on subsequent ticks. The lowered legacy graph includes a `death` state with `freeze` motion that the despawn timer occupies, but graph evaluation never enters it. Typedef drift tests are expected red until Task 6 regenerates the fixtures.

### Task 6: Reference graph + tests + diagnostics

Author the reference enemy (`sdk/behaviors/reference/entities.{ts,luau}`) as an explicit `components.behavior` graph reproducing the current idle/alert/attack tuning; keep the pose-fixture enemy on legacy `components.ai`. Port the pure-core unit tests in `ai_tests.rs` to lowered-graph equivalents; add fixture assertions for behavior-identity (legacy vs authored graph), interrupt priority, commitment window, `@state` guard end-to-end via an impact policy — author a test-only impact policy that writes a `@state.*` field on the fixture enemy, then assert the brain transitions on the next AI tick, serde round-trip/rebind, and the alloc-probe eval assertion. Update the agent diagnostics overlay to label agents with the authored state name. Regenerate and commit both typedef fixtures.

## Sequencing

**Phase 1 (sequential):** Task 1 — the split unblocks every `ai.rs` edit.
**Phase 2 (concurrent):** Task 2, Task 3 — independent (descriptor/parsers vs. scope).
**Phase 3 (sequential):** Task 4 — consumes Task 2's descriptor types and Task 3's scope.
**Phase 4 (sequential):** Task 5 — consumes Task 4's component shape.
**Phase 5 (sequential):** Task 6 — consumes the settled evaluator.

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| Behavior block | `BehaviorGraphDescriptor` | `"behavior"`, camelCase fields | `behavior:` | `behavior =` | none (descriptor-owned tuning, never map-overridable) |
| State entry | `BehaviorStateDescriptor` | `states.<name>.{animation,motion,action,transitions,onEnter}` | same | same | n/a |
| Motion verb | `MotionVerb::{ChaseTarget, Hold, Freeze}` | `"chaseTarget"` / `"hold"` / `"freeze"` | `"chaseTarget" \| "hold" \| "freeze"` (string literal union) | same | n/a |
| Action verb | `ActionVerb::Attack` | `"attack"` | `"attack"` (string literal union) | same | n/a |
| Transition | `TransitionDescriptor { to, when: IrNode }` | `{to, when}`; `when` is the op-tagged IR object | `when: RuntimeValue` | same | n/a |
| Attack params | `AttackParams` | `attack.{damage,range,cooldownMs}` | same | same | n/a |
| Death despawn | `BehaviorGraphDescriptor::death_despawn_ms: Option<f32>` | `"deathDespawnMs"` (default 2000) | `deathDespawnMs?` | `deathDespawnMs?` | n/a |
| Move speed | `BehaviorGraphDescriptor::move_speed: f32` | `"moveSpeed"` | `moveSpeed: number` | `moveSpeed: number` | n/a |
| Brain inputs | `BRAIN_INPUTS` fixed table | — | `brain.targetDistance` etc. (typed object, pre-wrapped IR input leaves) | `brain.targetDistance` | n/a |
| Per-entity state leaf | `ENTITY_STATE_INPUT_PREFIX` | — | `state("<name>")` (function, wraps `@state.<name>` IR input leaf) | `state("<name>")` | n/a |

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| Guards evaluated every tick; no state/action ever blocks evaluation (interruptible-by-default) | Task 5 (evaluator) | Threatened by any future latent action verb; commitment is authored `timeInStateMs` guards only | AC 4 |
| Interrupts before state-local transitions; declaration order; first-true-wins; deterministic | Task 5 | Planner seam must replace `select_transition` whole, not reorder inside it | AC 5 |
| Self-targeting interrupts skipped during evaluation; an interrupt cannot re-enter its own state | Task 5 (evaluator) | Any future guard side-effect must not rely on self-interrupt re-entry | AC 4 |
| One brain representation: legacy `ai` lowers at spawn; single evaluator code path | Task 4 (lowering), Task 5 | Any new `LogicalState`-style special case reintroduces the fork | AC 1, 7 |
| Lowered legacy graph is behavior-identical to v0 `evaluate_transition` | Task 4 (guard construction), Task 5 | Acquisition-gated edges must conjoin `@brain.acquisitionDue`; Idle→Attack must conjoin `le(dist, detection_range)` and `le(dist, attack_range)` under the `acquisitionDue` gate — `select(acquisitionDue, select(le(dist, detection), le(dist, attack), false), false)`; stride/hysteresis stay engine-side | AC 1, 7 |
| Bound programs are derived data: stored in evaluator side-table (not on `BrainComponent`), never serialized, rebuilt from retained graph at spawn/deserialize | Task 4 | Evaluator side-table lifecycle (spawn/despawn/reload); `BrainComponent` serde is unaffected since programs are external | AC 10 |
| Animation subordinate to logical state: unknown name warns, keeps prior, never aborts | Preserved (v0) | Task 5 animation request path; Task 4 spawn validation walk | AC 8 |
| Host-only graph evaluation; no wire change | Preserved (client `simulate_tick` skip) | Task 5 must add no client-side eval | AC 1 (suite includes net tests) |
| Guard eval is pure, total, zero-alloc per tick | Epic 14 substrate | Task 3 `refresh` and `@state` snapshot must not allocate per-eval | AC 11 |

## Script syntax examples

```ts
// Proposed design
import { defineEntity, brain, state, runtime } from "postretro";

export const grunt = defineEntity({
  canonicalName: "grunt",
  components: {
    health: { max: 40 },
    mesh: { /* model, animation states */ },
    behavior: {
      initial: "idle",
      attack: { damage: 8, range: 2, cooldownMs: 1200 },
      interrupts: [
        { to: "flinch", when: runtime.ge(state("staggered"), 1) },
      ],
      states: {
        idle: {
          animation: "idle", motion: "hold",
          transitions: [{ to: "chase", when: runtime.le(brain.targetDistance, 16) }],
        },
        chase: {
          animation: "walk", motion: "chaseTarget",
          transitions: [
            { to: "attack", when: runtime.le(brain.targetDistance, 2) },
            { to: "idle", when: runtime.gt(brain.targetDistance, 50) },
          ],
        },
        attack: {
          animation: "attack", motion: "chaseTarget", action: "attack",
          transitions: [{ to: "chase", when: runtime.gt(brain.targetDistance, 2) }],
        },
        flinch: {
          animation: "pain", motion: "hold", onEnter: "gruntFlinched",
          transitions: [{
            to: "chase",
            when: runtime.ge(brain.timeInStateMs, 400),
          }],
        },
      },
    },
  },
});
```

## Resolved questions

- **`behavior.moveSpeed`**: v1 mirrors legacy — `moveSpeed` on the behavior block feeds `attach_agent`. Revisit if the agent surface grows its own descriptor.
- **`and`/`or`/`not` opcodes**: confirmed as a desired substrate follow-up. Lowering builds `select` trees Rust-side until the opcodes land; authored guards use them once available. Graph evaluator is opcode-agnostic — no graph-side work needed when they arrive.
- **Sibling drafts**: `E10--enemy-stagger` and `E10--enemy-multi-attack` re-targeted onto the behavior graph substrate (stagger = impact policy + authored interrupt; multi-attack = action-vocabulary growth).
