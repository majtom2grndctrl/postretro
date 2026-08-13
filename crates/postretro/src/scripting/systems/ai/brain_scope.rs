// Runtime `BindingScope` for behavior-graph transition guards: the live brain
// namespace guards bind against at spawn and read every tick.
// See: context/lib/scripting.md §11 (IR substrate — pluggable scope abstraction)

// The scope pairs the fixed `@brain.*` facts the engine computes each tick with
// the `@state.*` per-entity leaves impact policies write, so an authored guard
// reads both through one namespace. It lives in the binary, beside the AI tick,
// because it names entity components — `postretro-foundation`, which owns the
// name table and the declaration-time twin (`BrainValidationScope`), cannot.
// Both scopes resolve names through `resolve_brain_input`, so they cannot
// disagree about which names exist or what they project to.
//
// Read-only: guards consume state, they never write it. `resolve_output`
// returns `None` for every name; `write` is unreachable.

use std::cell::RefCell;

use postretro_entities::components::health::HealthComponent;
use postretro_entities::{EntityId, EntityRegistry, EntityStateComponent};
use postretro_foundation::{
    BRAIN_INPUTS, BRAIN_NO_TARGET_DISTANCE, BindingScope, BrainInputRef, IrType, IrValue,
    ResolvedInput, ResolvedOutput, resolve_brain_input,
};

/// The engine-computed facts for one enemy's guard evaluation this tick.
///
/// These are the values the AI tick already derives before evaluating a brain —
/// target selection, the think stride, and the brain's own timers — handed to
/// [`BrainScope::refresh`] rather than re-derived inside it. Health is not here
/// because it is read straight from the registry during refresh.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct BrainFacts {
    /// Selected target identity and distance, or `None` when this enemy has no
    /// target.
    ///
    /// One binding feeds every target-side input: `@brain.hasTarget` is its
    /// presence, `@brain.targetDistance` its distance (or
    /// [`BRAIN_NO_TARGET_DISTANCE`]), and the target health facts resolve its
    /// entity. They therefore cannot disagree about whether a target exists.
    pub target: Option<(EntityId, f32)>,
    /// Milliseconds since the brain entered its current graph state.
    pub time_in_state_ms: f32,
    /// Milliseconds remaining on the attack cooldown.
    pub attack_cooldown_ms: f32,
    /// `true` on the think-stride ticks where acquisition is re-evaluated.
    pub acquisition_due: bool,
    /// XZ distance from the enemy's current position to its spawn-time home
    /// anchor. Computed by the AI tick every tick, independently of target
    /// selection and acquisition stride.
    pub distance_from_anchor: f32,
    /// Whether the selected target's faction differs from the evaluating
    /// enemy's. `false` without a selected target, following the target-side
    /// fact convention.
    pub target_hostile: bool,
    /// Whether the selected target is routeable by the nav floor's pathfinder.
    /// `false` without a target; maps without a navmesh report `true` so the
    /// chase motion keeps its direct-destination behavior.
    pub target_reachable: bool,
}

/// A resolved read handle: an index into one of the scope's two snapshots.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BrainInputHandle {
    /// Index into [`BRAIN_INPUTS`] and the fixed-slot snapshot.
    Fixed(usize),
    /// Index into the interned `@state.*` snapshot.
    State(usize),
}

/// The live brain namespace an authored guard binds against once at spawn and
/// reads on every tick.
///
/// One instance is shared by every enemy: [`BrainScope::refresh`] repopulates
/// both snapshots for the enemy about to be evaluated (the `MovementScope`
/// precedent). The `@state.*` snapshot therefore holds the union of every field
/// name any bound guard mentions, and each refresh fills all of them from the
/// current enemy — an entity that never had a field reads it as `0.0`, matching
/// `EntityStateComponent`'s emergent-field contract.
#[derive(Debug)]
pub(crate) struct BrainScope {
    /// Slot `i` holds the value for `BRAIN_INPUTS[i]`.
    fixed: [IrValue; BRAIN_INPUTS.len()],
    /// Interned `@state.*` field names, appended at bind. Parallel to
    /// `state_values`; a name's position is its read handle for the life of
    /// every program bound against this scope.
    state_names: RefCell<Vec<String>>,
    /// The per-enemy snapshot of those fields. Grown only at bind, written by
    /// index at refresh — nothing here allocates once binding has settled.
    state_values: RefCell<Vec<f32>>,
}

impl BrainScope {
    /// A scope with no live values, for declaration-time and spawn-time binds.
    ///
    /// Bind consults only names and types, so guards type-check against this
    /// before any enemy is evaluated. The fixed slots are type-correct zeros
    /// derived from [`BRAIN_INPUTS`], so an accidental read before the first
    /// [`BrainScope::refresh`] is still total.
    pub(crate) fn for_validation() -> Self {
        let mut fixed = [IrValue::Number(0.0); BRAIN_INPUTS.len()];
        for (slot, (_, ir_type)) in fixed.iter_mut().zip(BRAIN_INPUTS.iter()) {
            *slot = ir_type.zero();
        }
        Self {
            fixed,
            state_names: RefCell::new(Vec::new()),
            state_values: RefCell::new(Vec::new()),
        }
    }

    /// Repopulate both snapshots for `entity` before its guards are evaluated.
    ///
    /// Allocation-free: the fixed slots are stack scalars written into an owned
    /// array, and the `@state.*` snapshot is written by index into a `Vec` that
    /// only ever grows at bind time. Guard eval must stay zero-alloc per tick
    /// (scripting.md §11), and refresh runs inside that same window.
    pub(crate) fn refresh(
        &mut self,
        registry: &EntityRegistry,
        entity: EntityId,
        facts: BrainFacts,
    ) {
        // A brain-bearing enemy without Health is not a modelling error (a prop
        // may carry a graph), so absent health reads as zero rather than
        // skipping the refresh and leaving stale values behind.
        let health = registry.get_component::<HealthComponent>(entity).ok();
        let target_health = facts
            .target
            .and_then(|(target, _)| registry.get_component::<HealthComponent>(target).ok());
        // Order is BRAIN_INPUTS order — the handle is the index.
        self.fixed = [
            IrValue::Bool(facts.target.is_some()),
            IrValue::Number(
                facts
                    .target
                    .map_or(BRAIN_NO_TARGET_DISTANCE, |(_, distance)| distance),
            ),
            IrValue::Number(facts.time_in_state_ms),
            IrValue::Number(facts.attack_cooldown_ms),
            IrValue::Bool(facts.acquisition_due),
            IrValue::Number(health.map_or(0.0, |health| health.current)),
            IrValue::Number(health.map_or(0.0, |health| health.max)),
            IrValue::Number(target_health.map_or(0.0, |health| health.current)),
            IrValue::Number(target_health.map_or(0.0, |health| health.max)),
            IrValue::Bool(target_health.is_some_and(|health| health.death_handled)),
            IrValue::Number(facts.distance_from_anchor),
            IrValue::Bool(facts.target_hostile),
            IrValue::Bool(facts.target_reachable),
        ];

        let state = registry.get_component::<EntityStateComponent>(entity).ok();
        let names = self.state_names.borrow();
        let mut values = self.state_values.borrow_mut();
        for (slot, name) in values.iter_mut().zip(names.iter()) {
            *slot = state.map_or(0.0, |state| state.get(name));
        }
    }

    /// Intern one `@state.*` field name, returning its stable snapshot index.
    ///
    /// This is the only place either snapshot grows, and it runs at bind alone —
    /// a repeated name reuses its existing slot, so the snapshot converges on
    /// the union of the names across every bound descriptor.
    fn intern_state_field(&self, name: &str) -> usize {
        let mut names = self.state_names.borrow_mut();
        if let Some(index) = names.iter().position(|bound| bound == name) {
            return index;
        }
        let index = names.len();
        names.push(name.to_string());
        self.state_values.borrow_mut().push(0.0);
        index
    }
}

impl BindingScope for BrainScope {
    type InputHandle = BrainInputHandle;
    // Read-only scope: no output is ever resolved, so this is never constructed.
    type OutputHandle = BrainInputHandle;

    fn resolve_input(&self, name: &str) -> Option<ResolvedInput<BrainInputHandle>> {
        match resolve_brain_input(name)? {
            BrainInputRef::Fixed { index, ir_type } => Some(ResolvedInput {
                handle: BrainInputHandle::Fixed(index),
                ir_type,
            }),
            BrainInputRef::State(field) => Some(ResolvedInput {
                handle: BrainInputHandle::State(self.intern_state_field(field)),
                ir_type: IrType::Number,
            }),
        }
    }

    fn resolve_output(&self, _name: &str) -> Option<ResolvedOutput<BrainInputHandle>> {
        // Guards are read-only. Per-entity state is written by impact policies
        // and reactions through their own scopes, never by a transition guard.
        None
    }

    fn read(&self, handle: &BrainInputHandle) -> IrValue {
        // Total: the handle came from a successful `resolve_input`, so its index
        // is in bounds and its slot holds a value of the resolved type.
        match handle {
            BrainInputHandle::Fixed(index) => self.fixed[*index],
            BrainInputHandle::State(index) => IrValue::Number(self.state_values.borrow()[*index]),
        }
    }

    fn write(&mut self, _handle: &BrainInputHandle, _value: IrValue) {
        unreachable!("BrainScope is read-only; resolve_output never grants a write handle")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::alloc_probe::AllocSnapshot;
    use postretro_entities::Transform;
    use postretro_foundation::{
        BRAIN_ACQUISITION_DUE_INPUT, BRAIN_ATTACK_COOLDOWN_MS_INPUT,
        BRAIN_DISTANCE_FROM_ANCHOR_INPUT, BRAIN_HAS_TARGET_INPUT, BRAIN_HEALTH_INPUT,
        BRAIN_MAX_HEALTH_INPUT, BRAIN_TARGET_DIED_INPUT, BRAIN_TARGET_DISTANCE_INPUT,
        BRAIN_TARGET_HEALTH_INPUT, BRAIN_TARGET_HOSTILE_INPUT, BRAIN_TARGET_MAX_HEALTH_INPUT,
        BRAIN_TARGET_REACHABLE_INPUT, BRAIN_TIME_IN_STATE_MS_INPUT, BakedIr, BindError,
        BoundProgram, BrainValidationScope, CURRENT_IR_VERSION, IrNode, bind, bind_brain_guard,
        eval_value,
    };

    const EPSILON: f32 = 1e-6;

    fn input(name: &str) -> IrNode {
        IrNode::Input {
            name: name.to_string(),
            owner: None,
        }
    }

    fn bind_read(name: &str, scope: &BrainScope) -> BoundProgram<BrainScope> {
        bind(
            &BakedIr {
                version: CURRENT_IR_VERSION,
                output: None,
                root: input(name),
            },
            scope,
        )
        .unwrap_or_else(|error| panic!("`{name}` must bind: {error}"))
    }

    fn assert_number(value: IrValue, expected: f32) {
        match value {
            IrValue::Number(actual) => assert!(
                (actual - expected).abs() <= EPSILON,
                "expected {expected}, got {actual}"
            ),
            other => panic!("expected a number, got {other:?}"),
        }
    }

    /// A registry holding one enemy with known health and one per-entity state
    /// field, plus a second enemy with different values so per-entity refresh
    /// is observable.
    fn seeded_registry() -> (EntityRegistry, EntityId, EntityId) {
        let mut registry = EntityRegistry::new();
        let first = registry.spawn(Transform::default());
        let second = registry.spawn(Transform::default());
        for (entity, current, max, staggered, death_handled) in [
            (first, 30.0, 40.0, 1.0, false),
            (second, 12.0, 75.0, 0.0, true),
        ] {
            registry
                .set_component(
                    entity,
                    HealthComponent {
                        max,
                        current,
                        hitbox: None,
                        death_handled,
                        pending_kill_credit: None,
                        zone_multipliers: Default::default(),
                        contributor_ledger: Default::default(),
                    },
                )
                .expect("test entity is live");
            registry
                .entity_state_mut(entity)
                .expect("spawn seeds entity state")
                .set("staggered", staggered);
        }
        (registry, first, second)
    }

    fn engaged_facts(target: EntityId) -> BrainFacts {
        BrainFacts {
            target: Some((target, 7.5)),
            time_in_state_ms: 250.0,
            attack_cooldown_ms: 400.0,
            acquisition_due: true,
            distance_from_anchor: 12.5,
            target_hostile: true,
            target_reachable: true,
        }
    }

    #[test]
    fn brain_scope_resolves_every_fixed_input_at_its_table_index() {
        let scope = BrainScope::for_validation();
        for (index, (name, ir_type)) in BRAIN_INPUTS.iter().enumerate() {
            let resolved = scope
                .resolve_input(name)
                .unwrap_or_else(|| panic!("`{name}` must resolve"));
            assert_eq!(
                resolved.handle,
                BrainInputHandle::Fixed(index),
                "`{name}` handle is its table index"
            );
            assert_eq!(
                resolved.ir_type, *ir_type,
                "`{name}` projects its declared type"
            );
        }
    }

    #[test]
    fn brain_scope_resolution_matches_the_validation_scope_on_every_name() {
        // Drift guard: the declaration-time scope in foundation decides whether
        // an authored guard is legal, and this scope decides what it reads at
        // tick time. Derive the cases from BRAIN_INPUTS so a new input is
        // covered without editing this test.
        let runtime = BrainScope::for_validation();
        let validation = BrainValidationScope;
        let state_and_unknown = [
            "@state.staggered",
            "@stateful.staggered",
            "@brain.morale",
            "staggered",
        ];
        let names = BRAIN_INPUTS
            .iter()
            .map(|(name, _)| *name)
            .chain(state_and_unknown);

        for name in names {
            let runtime_input = runtime.resolve_input(name);
            let validation_input = validation.resolve_input(name);
            assert_eq!(
                runtime_input.as_ref().map(|resolved| resolved.ir_type),
                validation_input.as_ref().map(|resolved| resolved.ir_type),
                "`{name}` must resolve identically in both scopes"
            );
            assert!(
                runtime.resolve_output(name).is_none() && validation.resolve_output(name).is_none(),
                "`{name}` must be read-only in both scopes"
            );
        }
    }

    #[test]
    fn for_validation_reads_type_correct_zeros_before_any_refresh() {
        let scope = BrainScope::for_validation();
        for (name, ir_type) in BRAIN_INPUTS {
            let program = bind_read(name, &scope);
            assert_eq!(
                eval_value(&program, &scope),
                ir_type.zero(),
                "`{name}` reads its type's zero before refresh"
            );
        }
        // A bound-but-never-refreshed state leaf is total too.
        let program = bind_read("@state.staggered", &scope);
        assert_number(eval_value(&program, &scope), 0.0);
    }

    /// The value `refresh` must project for a given fixed input name, computed
    /// from the same facts/health `refresh` consumes.
    ///
    /// No `_` arm: driving this from `BRAIN_INPUTS` iteration (below) rather
    /// than a hand-listed, positionally-zipped array means a `BRAIN_INPUTS`
    /// entry added without a matching arm here panics at test time instead of
    /// silently dropping out of a shorter hand-written list.
    fn expected_fixed_value(
        name: &str,
        facts: BrainFacts,
        health: &HealthComponent,
        target_health: &HealthComponent,
    ) -> IrValue {
        match name {
            BRAIN_HAS_TARGET_INPUT => IrValue::Bool(facts.target.is_some()),
            BRAIN_TARGET_DISTANCE_INPUT => IrValue::Number(
                facts
                    .target
                    .map_or(BRAIN_NO_TARGET_DISTANCE, |(_, distance)| distance),
            ),
            BRAIN_TIME_IN_STATE_MS_INPUT => IrValue::Number(facts.time_in_state_ms),
            BRAIN_ATTACK_COOLDOWN_MS_INPUT => IrValue::Number(facts.attack_cooldown_ms),
            BRAIN_ACQUISITION_DUE_INPUT => IrValue::Bool(facts.acquisition_due),
            BRAIN_HEALTH_INPUT => IrValue::Number(health.current),
            BRAIN_MAX_HEALTH_INPUT => IrValue::Number(health.max),
            BRAIN_TARGET_HEALTH_INPUT => IrValue::Number(target_health.current),
            BRAIN_TARGET_MAX_HEALTH_INPUT => IrValue::Number(target_health.max),
            BRAIN_TARGET_DIED_INPUT => IrValue::Bool(target_health.death_handled),
            BRAIN_DISTANCE_FROM_ANCHOR_INPUT => IrValue::Number(facts.distance_from_anchor),
            BRAIN_TARGET_HOSTILE_INPUT => IrValue::Bool(facts.target_hostile),
            BRAIN_TARGET_REACHABLE_INPUT => IrValue::Bool(facts.target_reachable),
            other => panic!(
                "`{other}` is in BRAIN_INPUTS but `expected_fixed_value` has no case for it \
                 — add one alongside the new `refresh` slot"
            ),
        }
    }

    #[test]
    fn refresh_projects_engine_facts_and_health_into_the_fixed_slots() {
        let (mut registry, enemy, target) = seeded_registry();
        let target_without_health = registry.spawn(Transform::default());
        let mut scope = BrainScope::for_validation();
        // Bind before refresh: programs bind once and observe every later
        // snapshot, which is what lets one scope serve every enemy.
        let programs: Vec<_> = BRAIN_INPUTS
            .iter()
            .map(|(name, _)| bind_read(name, &scope))
            .collect();

        let facts = engaged_facts(target);
        scope.refresh(&registry, enemy, facts);
        let health = registry
            .get_component::<HealthComponent>(enemy)
            .expect("seeded enemy has health");
        let target_health = registry
            .get_component::<HealthComponent>(target)
            .expect("seeded target has health");

        // Iterating BRAIN_INPUTS itself (rather than a separate hand-listed,
        // positionally-zipped array) is what makes this loop cover a newly
        // added input automatically — the previous fixed-length `expected`
        // array silently truncated a longer BRAIN_INPUTS via `zip`.
        for (program, (name, _)) in programs.iter().zip(BRAIN_INPUTS.iter()) {
            let want = expected_fixed_value(name, facts, health, target_health);
            match want {
                IrValue::Number(number) => assert_number(eval_value(program, &scope), number),
                IrValue::Bool(_) => {
                    assert_eq!(eval_value(program, &scope), want, "`{name}`")
                }
            }
        }

        let target_health_input = bind_read(BRAIN_TARGET_HEALTH_INPUT, &scope);
        let target_max_health_input = bind_read(BRAIN_TARGET_MAX_HEALTH_INPUT, &scope);
        let target_died_input = bind_read(BRAIN_TARGET_DIED_INPUT, &scope);
        let target_hostile_input = bind_read(BRAIN_TARGET_HOSTILE_INPUT, &scope);
        let target_reachable_input = bind_read(BRAIN_TARGET_REACHABLE_INPUT, &scope);

        scope.refresh(
            &registry,
            enemy,
            BrainFacts {
                target: None,
                target_hostile: false,
                target_reachable: false,
                ..facts
            },
        );
        assert_number(eval_value(&target_health_input, &scope), 0.0);
        assert_number(eval_value(&target_max_health_input, &scope), 0.0);
        assert_eq!(eval_value(&target_died_input, &scope), IrValue::Bool(false));
        assert_eq!(
            eval_value(&target_hostile_input, &scope),
            IrValue::Bool(false),
            "target hostility follows the target-side no-target convention"
        );
        assert_eq!(
            eval_value(&target_reachable_input, &scope),
            IrValue::Bool(false),
            "target reachability follows the target-side no-target convention"
        );

        scope.refresh(
            &registry,
            enemy,
            BrainFacts {
                target: Some((target_without_health, 7.5)),
                ..facts
            },
        );
        assert_number(eval_value(&target_health_input, &scope), 0.0);
        assert_number(eval_value(&target_max_health_input, &scope), 0.0);
        assert_eq!(eval_value(&target_died_input, &scope), IrValue::Bool(false));
        assert_eq!(
            eval_value(&target_hostile_input, &scope),
            IrValue::Bool(true),
            "the compute pass owns faction comparison and refresh preserves its result"
        );
        assert_eq!(
            eval_value(&target_reachable_input, &scope),
            IrValue::Bool(true),
            "the compute pass owns the nav verdict and refresh preserves its cached result"
        );
    }

    #[test]
    fn refresh_reports_the_no_target_sentinel_and_clears_has_target() {
        let (registry, enemy, target) = seeded_registry();
        let mut scope = BrainScope::for_validation();
        let has_target = bind_read(BRAIN_HAS_TARGET_INPUT, &scope);
        let distance = bind_read(BRAIN_TARGET_DISTANCE_INPUT, &scope);

        scope.refresh(
            &registry,
            enemy,
            BrainFacts {
                target: None,
                target_hostile: false,
                target_reachable: false,
                ..engaged_facts(target)
            },
        );

        assert_eq!(eval_value(&has_target, &scope), IrValue::Bool(false));
        assert_number(eval_value(&distance, &scope), BRAIN_NO_TARGET_DISTANCE);
        // The sentinel makes a bare range guard read false with no target, so
        // authors need no `hasTarget` conjunction.
        let in_range = bind(
            &BakedIr {
                version: CURRENT_IR_VERSION,
                output: None,
                root: IrNode::Le {
                    a: Box::new(input(BRAIN_TARGET_DISTANCE_INPUT)),
                    b: Box::new(IrNode::Const {
                        value: IrValue::Number(16.0),
                    }),
                },
            },
            &scope,
        )
        .expect("range guard binds");
        assert_eq!(eval_value(&in_range, &scope), IrValue::Bool(false));
    }

    #[test]
    fn state_leaves_intern_once_and_refresh_per_entity_by_index() {
        let (registry, first, second) = seeded_registry();
        let mut scope = BrainScope::for_validation();

        // Two guards over the same field share one interned slot; a distinct
        // field takes the next one.
        let staggered = bind_read("@state.staggered", &scope);
        let staggered_again = bind_read("@state.staggered", &scope);
        let unwritten = bind_read("@state.morale", &scope);
        assert_eq!(scope.state_names.borrow().len(), 2, "names intern once");
        assert_eq!(
            scope.state_values.borrow().len(),
            2,
            "snapshot stays parallel"
        );

        scope.refresh(&registry, first, engaged_facts(second));
        assert_number(eval_value(&staggered, &scope), 1.0);
        assert_number(eval_value(&staggered_again, &scope), 1.0);
        // A field this entity never had reads as zero, per EntityStateComponent.
        assert_number(eval_value(&unwritten, &scope), 0.0);

        // The same bound program follows the scope to the next enemy.
        scope.refresh(&registry, second, engaged_facts(first));
        assert_number(eval_value(&staggered, &scope), 0.0);
    }

    #[test]
    fn brain_scope_rejects_unknown_and_near_miss_input_names() {
        let scope = BrainScope::for_validation();
        for name in ["@brain.morale", "@stateful.staggered", "staggered"] {
            let error = bind(
                &BakedIr {
                    version: CURRENT_IR_VERSION,
                    output: None,
                    root: input(name),
                },
                &scope,
            )
            .expect_err("unknown names must not bind");
            assert_eq!(
                error,
                BindError::UnknownInput {
                    name: name.to_string()
                }
            );
        }
    }

    #[test]
    fn refresh_and_guard_eval_perform_zero_heap_allocations() {
        // The substrate invariant (scripting.md §11): the per-tick window —
        // snapshot refresh plus guard eval — must not allocate. Binding and
        // name interning happen before the probe is armed, which is exactly
        // where they are allowed to allocate.
        let (registry, first, second) = seeded_registry();
        let mut scope = BrainScope::for_validation();
        let guard = IrNode::Select {
            cond: Box::new(input(BRAIN_TARGET_REACHABLE_INPUT)),
            a: Box::new(IrNode::Select {
                cond: Box::new(input(BRAIN_TARGET_HOSTILE_INPUT)),
                a: Box::new(IrNode::Gt {
                    a: Box::new(input(BRAIN_DISTANCE_FROM_ANCHOR_INPUT)),
                    b: Box::new(IrNode::Const {
                        value: IrValue::Number(0.0),
                    }),
                }),
                b: Box::new(IrNode::Select {
                    cond: Box::new(IrNode::Ge {
                        a: Box::new(input("@state.staggered")),
                        b: Box::new(IrNode::Const {
                            value: IrValue::Number(1.0),
                        }),
                    }),
                    a: Box::new(IrNode::Le {
                        a: Box::new(input(BRAIN_TARGET_DISTANCE_INPUT)),
                        b: Box::new(IrNode::Const {
                            value: IrValue::Number(16.0),
                        }),
                    }),
                    b: Box::new(IrNode::Gt {
                        a: Box::new(input(BRAIN_HEALTH_INPUT)),
                        b: Box::new(input(BRAIN_MAX_HEALTH_INPUT)),
                    }),
                }),
            }),
            b: Box::new(IrNode::Const {
                value: IrValue::Bool(false),
            }),
        };
        let program = bind(
            &BakedIr {
                version: CURRENT_IR_VERSION,
                output: None,
                root: guard,
            },
            &scope,
        )
        .expect("mixed guard binds");

        // Warm any one-time lazy state so the measured window is pure work.
        let first_facts = BrainFacts {
            acquisition_due: false,
            ..engaged_facts(second)
        };
        let second_facts = BrainFacts {
            acquisition_due: false,
            ..engaged_facts(first)
        };
        scope.refresh(&registry, first, first_facts);
        let _ = eval_value(&program, &scope);

        let snapshot = AllocSnapshot::arm();
        scope.refresh(&registry, second, second_facts);
        let value = eval_value(&program, &scope);
        let allocs = snapshot.allocs_since();

        assert!(matches!(value, IrValue::Bool(_)), "value: {value:?}");
        assert_eq!(
            allocs, 0,
            "refresh + guard eval must perform zero heap allocations"
        );
    }

    #[test]
    fn a_guard_validated_at_declaration_binds_against_the_runtime_scope() {
        // The declaration-time gate and the tick-time scope must accept the
        // same programs: whatever `bind_brain_guard` admits must bind here too.
        let guard = IrNode::Select {
            cond: Box::new(input(BRAIN_HAS_TARGET_INPUT)),
            a: Box::new(IrNode::Le {
                a: Box::new(input(BRAIN_TARGET_DISTANCE_INPUT)),
                b: Box::new(input("@state.engageRange")),
            }),
            b: Box::new(IrNode::Const {
                value: IrValue::Bool(false),
            }),
        };
        bind_brain_guard(&guard).expect("guard passes declaration-time validation");

        let scope = BrainScope::for_validation();
        let program = bind(
            &BakedIr {
                version: CURRENT_IR_VERSION,
                output: None,
                root: guard,
            },
            &scope,
        )
        .expect("the same guard binds against the runtime scope");
        assert_eq!(program.root_type, IrType::Bool);
    }
}
