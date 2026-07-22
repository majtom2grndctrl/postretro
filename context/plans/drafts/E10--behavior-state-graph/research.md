# Behavior State Graph — Research Notes

Code-grounding digest for the spec. Line numbers as of drafting; treat as ephemeral.

## Current FSM (v0) facts

- Tick body: `run_ai_tick_with_navigation_and_impact(registry, warned, tick_dt, nav_graph, collision_world, on_impact) -> Vec<&'static str>` — `ai.rs:503` (969 lines total). Sole caller: `sim::simulate_tick` at `sim/mod.rs:293`. Returns `ENEMY_ATTACK_EVENT = "enemyAttack"` pushes; app drains via `fire_named_event`.
- Pure core: `evaluate_transition(player_pos, agent_pos, tuning, current, evaluate_acquisition) -> TransitionResult` (`ai.rs:250`); `TransitionResult { next_state, steering }`, `SteeringIntent { Chase, Clear, Hold }`. Edges:
  - Idle: gated (`evaluate_acquisition`) `dist <= detection_range` → Attack if `dist <= attack_range` else Alert, Chase.
  - Alert: ungated `dist <= attack_range` → Attack/Chase; gated `dist > leash_range` → Idle/Clear; else Alert/Chase.
  - Attack: ungated `dist > attack_range` → Alert/Chase; else Attack/Chase.
  - Death: terminal, Hold — **not HP-reachable**. Zero HP does not force Death (tests `ai_tests.rs:1476-1582`). Removal is E16 authored deferred despawn; AI pass-1 skips inert/pending-despawn/downed entities. `death_despawn_ms` / `death_despawn_remaining_ms` are vestigial in the live tick.
- Stride: bands 12/30 → divisors 1/4/12; `acquisition_due` (`ai.rs:374`); counter increments every tick; only detection/leash edges are gated — attack edges + cooldown run every tick. This forces the lowered graph to conjoin `@brain.acquisitionDue` on exactly those two edges.
- Targeting: `select_target(registry, from, retained_target, retained_outside_leash, visible: Option<&dyn Fn(EntityId) -> bool>) -> Option<TargetPawn>` (`ai.rs:403`); hysteresis `TARGET_SWITCH_HYSTERESIS_DISTANCE = 1.0`; `visible` always `None` today (the LOS seam).
- Steering: `set_destination` / `clear_destination` / `path_state` (`agent_steering.rs:232/250/272`); Chase passes `combat_slot.unwrap_or(target.position)`; combat slots via `select_combat_positions_batch`, hold ticks 8.
- Attack: `state == Attack && cooldown <= 0 && selected_target_alive` → `apply_damage_with_context(.., DamagePayload { amount }, DamageContext { source_id: "enemy.attack", attacker, producer: InTick })`, cooldown reset, event push.
- Animation: `switch_animation_state` when `state_changed || moving != latch`; Alert-at-rest maps to the idle animation (`animation_for_locomotion`, `ai.rs:127`); `UnknownState`/`NotAnimated` warn once, keep prior; `restart_animation_clip` on repeated in-state attack swings.
- Facing: only when `aggro_armed && state ∈ {Alert, Attack}`; slew at `FACING_TURN_RATE = MAX_TURN_RATE * 2`.
- Aggro gate: `aggro_armed` (serde default true, spawn-seeded from `enabled_on_spawn` KVP) — closed forces Idle, clears target/steering; toggled only by `updateEnemyState` reaction (`reactions/enemy_state.rs`).
- Host authority: connected clients `continue` before `simulate_tick` (`main.rs:1956-2010`) — graph evaluation is host-only with no new work.
- Descriptor: `AiDescriptor` (`foundation/data_descriptors/types/combat.rs:250`, camelCase, all fields required, `validate()` finiteness/positivity); twin parsers both funnel `serde_json::from_value` + `validate()` (`scripting-core/data_descriptors/js/entity.rs:103`, `scripting-core/data_descriptors/lua/entity.rs:136`) so they cannot diverge. Attach site: `data_archetype.rs:479-499` (`attach_brain` → aggro seed → `attach_agent(move_speed)` → `validate_brain_animation_states`).

## IR substrate facts

- `IrNode` (`foundation/ir/mod.rs:97`): 15 ops, `#[serde(tag = "op", snake_case)]`; values Number(f32)/Bool. `BakedIr { version, output?, root }`, `CURRENT_IR_VERSION = 1`; additive variants do not bump the version. `load_baked_ir` warns-and-`None` on unsupported version.
- `BindingScope` (`foundation/ir/scope.rs:61`): `resolve_input/resolve_output/read/write` with typed handles; bind once (`bind() -> BoundProgram<S>`, type-checks, resolves names), eval per tick (`eval_value`, allocation-free, total: div0→0, non-finite→0, missing→type-zero).
- Scope precedents:
  - `MovementScope` (`foundation/movement/scope.rs`): fixed 6-entry input table, index handles, `for_validation()`, `refresh()` per tick.
  - `EntityScope` (`scripting-core/ir/scopes.rs:240`): `@state.` prefix (`ENTITY_STATE_INPUT_PREFIX`) → interned name→index handles over `EntityStateComponent`, per-fire snapshot; `@impact.*` via `DispatchScope`; bare names → store.
- Adopter pattern (dash, `M14--movement-dash-runtime-values`): `NumberOrIr`/`BoolOrIr` untagged unions; bound programs in `DashPrograms` on the component, `#[serde(skip)]`, always-true `PartialEq`; bind at `from_descriptor` against `for_validation()`; bind failure → warn once + native fallback. Declaration-time mirror in `data_descriptors/validate/foundation.rs`.
- Per-tick boolean-guard precedent: impact group gates — `group.when: Option<BoundProgram<EntityScope>>`, evaluated `matches!(eval_value(when, &scope), IrValue::Bool(true))` (`impact_policy.rs:219`). Absent guard ⇒ eligible. This is the transition-guard shape.
- Per-entity state: `EntityStateComponent { values: HashMap<String, f32> }` (`entities/components/entity_state.rs`), schemaless, unset reads 0, write via `registry.entity_state_mut`.
- Author surface: `runtime` namespace (`sdk/lib/runtime.{ts,luau}`) — `constant/read/add/sub/mul/div/clamp/lerp/lt/le/gt/ge/eq/ne/select`; **no and/or/not** — boolean composition is `select`-only by substrate rule. `read` accepts string or StateRef. Alloc-free eval asserted by `alloc_probe.rs`.

## Evaluator lifecycle

```mermaid
sequenceDiagram
    participant Parse as Descriptor parse (load)
    participant Spawn as Spawn (attach site)
    participant Tick as AI tick (host, per enemy)
    participant Drain as Post-tick event drain
    Parse->>Parse: structural validation + bind-check vs BrainScope::for_validation()
    Spawn->>Spawn: legacy ai → lower_ai_descriptor → graph
    Spawn->>Spawn: bind guards → BoundProgram vec (#[serde(skip)])
    Spawn->>Spawn: validate animation names over graph states (warn once)
    Tick->>Tick: pass 1 snapshot (skip despawn-pending/downed)
    Tick->>Tick: engine floor: select_target, stride++, aggro gate
    Tick->>Tick: scope.refresh(brain, target, state snapshot)
    Tick->>Tick: select_transition: interrupts then state transitions, first true
    Tick->>Tick: apply MotionVerb → SteeringIntent; attack action; animation request
    Tick->>Tick: state change → reset time_in_state_ms, queue on_enter
    Tick->>Drain: owned event names (enemyAttack + authored on_enter)
    Drain->>Drain: fire_named_event per name
```

Reads for every arrow exist today except: `select_transition` (new, Task 5), scope refresh (new, Task 3), `on_enter` queue (new, Task 5 — widens the `Vec<&'static str>` return to `Vec<Cow<'static, str>>` for the one caller at `sim/mod.rs:293`).

## Alternatives considered

- **Behavior tree instead of state graph.** Rejected for v1: every roadmapped behavior (stagger, patrol, flee, multi-attack pacing) is expressible as states + every-tick guards; BT selector semantics arrive later, if ever, as a transition-selection policy over the same states (the FEAR precedent — its FSM had ~3 states, intelligence lived in the planner choosing transitions). The Unreal "can't interrupt" complaint traces to latent BT tasks, not trees per se; this design bans latent actions outright.
- **Generalize `components.ai` in place** (add optional graph fields). Rejected: mixed shapes in one block make the mutual-exclusion and lowering story murkier than a sibling block; `ai` stays as sugar and its parse path is untouched.
- **Guards gated by stride engine-side** (evaluate transitions only on acquisition ticks). Rejected: breaks behavior-identity (attack-range edges are ungated today) and buries the perception-rate axis; publishing `@brain.acquisitionDue` keeps stride authorable and reproduces v0 exactly.
- **`targetDistance` type-zero when no target.** Rejected: 0 reads as "in melee range" and misfires every proximity guard; a large finite sentinel (1.0e9) makes no-target fail closeness checks and pass leash checks naturally, and stays inside eval totality rules (finite).
- **Graph-authored writes on enter.** Deferred: keeps guards pure/read-only in v1; write paths already exist (impact policies, reactions) and compose through `@state.*`.

## Oversized-file flags

`ai.rs` 969 (split is Task 1), `ai_tests.rs` 3045 (ports in Task 6), `agent_steering.rs` 1088 (not extended — read-only consumer), `data_archetype.rs` 2609 (touched at one attach block; extension is ~20 lines — flagged but not split here, its seams are a separate cleanup).
