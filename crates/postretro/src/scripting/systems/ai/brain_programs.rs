// Evaluator-owned side-table of bound transition guards, keyed by entity.
// See: context/lib/scripting.md §11 (IR substrate — bind once, evaluate per tick)

// Bound programs are DERIVED data. A `BrainComponent` retains its behavior
// graph; the `BoundProgram`s compiled from that graph live here, beside the
// evaluator, and never on the component:
//
// - `BoundProgram<BrainScope>` cannot cross the crate boundary — `BrainScope`
//   names entity components and therefore lives in the binary, while
//   `BrainComponent` lives in `postretro-entities`. The layering gives the same
//   invariant the dash programs get from `#[serde(skip)]` plus a custom
//   `PartialEq`, with none of that machinery: programs are simply not reachable
//   from the component, so they cannot be serialized and cannot affect equality.
// - One `BrainScope` is shared by every entry. Interning happens inside `bind`,
//   so the `@state.*` slot table converges on the union of the names across all
//   bound graphs only if every bind goes through the same scope instance.
//
// [`BrainPrograms::sync`] is the single lifecycle hook: it binds graphs it has
// not seen, rebinds an entity whose retained graph changed, and drops entries
// for entities that no longer carry a brain. Reconciling against the registry
// this way covers spawn, despawn, and a wholesale deserialize with one call,
// instead of threading the table through every seam that can attach or remove a
// brain.

use std::collections::{HashMap, HashSet};

use postretro_entities::components::brain::BrainComponent;
use postretro_entities::{ComponentKind, ComponentValue, EntityId, EntityRegistry};
use postretro_foundation::{
    BakedIr, BehaviorGraphDescriptor, BoundProgram, CURRENT_IR_VERSION, IrType,
    TransitionDescriptor, bind,
};

use super::brain_scope::BrainScope;

/// One entity's bound guards, laid out to match its graph so the evaluator can
/// index straight from the descriptor it is walking.
///
/// A `None` slot is a DISABLED edge: its guard failed to bind, so the evaluator
/// treats it as permanently false rather than aborting the brain. Authored
/// guards are bind-validated at parse and lowered legacy guards are engine
/// generated, so a `None` here means one of those two contracts broke — hence
/// the warn at bind time.
pub(crate) struct BrainEntityPrograms {
    /// The graph these programs were bound from. Retained so `sync` can tell a
    /// still-valid entry from one whose entity was re-seeded with a different
    /// brain (the deserialize/reload case).
    graph: BehaviorGraphDescriptor,
    /// Parallel to `graph.interrupts`.
    interrupts: Vec<Option<BoundProgram<BrainScope>>>,
    /// Indexed by resolved state index (`graph.states` in `BTreeMap` order);
    /// each inner vec is parallel to that state's `transitions`.
    states: Vec<Vec<Option<BoundProgram<BrainScope>>>>,
}

impl BrainEntityPrograms {
    /// The bound any-state guards, in declaration order.
    pub(crate) fn interrupts(&self) -> &[Option<BoundProgram<BrainScope>>] {
        &self.interrupts
    }

    /// The bound state-local guards for the state at `state_index`, in
    /// declaration order. Total: an index outside the resolved state list yields
    /// an empty slice.
    pub(crate) fn transitions(&self, state_index: usize) -> &[Option<BoundProgram<BrainScope>>] {
        self.states
            .get(state_index)
            .map(Vec::as_slice)
            .unwrap_or_default()
    }
}

/// The evaluator's per-entity guard programs plus the shared binding scope.
pub(crate) struct BrainPrograms {
    scope: BrainScope,
    entries: HashMap<EntityId, BrainEntityPrograms>,
}

impl BrainPrograms {
    pub(crate) fn new() -> Self {
        Self {
            scope: BrainScope::for_validation(),
            entries: HashMap::new(),
        }
    }

    /// The shared scope, for reading bound programs after a refresh.
    pub(crate) fn scope(&self) -> &BrainScope {
        &self.scope
    }

    /// The shared scope, for repopulating its snapshots before evaluating one
    /// entity's guards.
    pub(crate) fn scope_mut(&mut self) -> &mut BrainScope {
        &mut self.scope
    }

    /// This entity's bound guards, or `None` when it carries no brain.
    pub(crate) fn get(&self, entity: EntityId) -> Option<&BrainEntityPrograms> {
        self.entries.get(&entity)
    }

    /// Reconcile the table with the registry's live brains.
    ///
    /// Binds an entity seen for the first time, rebinds one whose retained graph
    /// no longer matches what its programs were bound from, and drops entries
    /// whose entity no longer carries a brain. Binding is the only growth point
    /// for the shared scope's `@state.*` slot table, and it happens here — never
    /// mid-evaluation.
    ///
    /// `warned` is the run-long warn-once latch: a guard that fails to bind
    /// reports once per authored path and leaves that edge disabled.
    pub(crate) fn sync(&mut self, registry: &EntityRegistry, warned: &mut HashSet<String>) {
        let mut live: HashSet<EntityId> = HashSet::new();
        let brains: Vec<(EntityId, &BrainComponent)> = registry
            .iter_with_kind(ComponentKind::Brain)
            .filter_map(|(entity, value)| match value {
                ComponentValue::Brain(brain) => Some((entity, brain)),
                _ => None,
            })
            .collect();

        for (entity, brain) in brains {
            live.insert(entity);
            let bound = self
                .entries
                .get(&entity)
                .is_some_and(|entry| entry.graph == brain.graph);
            if !bound {
                let entry = bind_graph(&self.scope, &brain.graph, warned);
                self.entries.insert(entity, entry);
            }
        }

        self.entries.retain(|entity, _| live.contains(entity));
    }
}

/// Bind every guard in `graph` against the shared scope, warning once per
/// authored path for the ones that fail and leaving those edges disabled.
fn bind_graph(
    scope: &BrainScope,
    graph: &BehaviorGraphDescriptor,
    warned: &mut HashSet<String>,
) -> BrainEntityPrograms {
    let interrupts = graph
        .interrupts
        .iter()
        .enumerate()
        .map(|(index, transition)| {
            bind_guard(scope, transition, &format!("interrupts[{index}]"), warned)
        })
        .collect();
    let states = graph
        .states
        .iter()
        .map(|(name, state)| {
            state
                .transitions
                .iter()
                .enumerate()
                .map(|(index, transition)| {
                    bind_guard(
                        scope,
                        transition,
                        &format!("states.{name}.transitions[{index}]"),
                        warned,
                    )
                })
                .collect()
        })
        .collect();
    BrainEntityPrograms {
        graph: graph.clone(),
        interrupts,
        states,
    }
}

fn bind_guard(
    scope: &BrainScope,
    transition: &TransitionDescriptor,
    path: &str,
    warned: &mut HashSet<String>,
) -> Option<BoundProgram<BrainScope>> {
    let baked = BakedIr {
        version: CURRENT_IR_VERSION,
        output: None,
        root: transition.when.clone(),
    };
    let reason = match bind(&baked, scope) {
        Ok(program) if program.root_type == IrType::Bool => return Some(program),
        Ok(_) => "its root produces a number, not a boolean".to_string(),
        Err(error) => error.to_string(),
    };
    if warned.insert(format!("brain-guard:{path}")) {
        log::warn!(
            "[AI] behavior guard `{path}` could not be bound ({reason}); \
             the transition to `{to}` is disabled for the rest of the run. \
             Warned once per guard.",
            to = transition.to,
        );
    }
    None
}

#[cfg(test)]
mod tests {
    use super::super::brain_scope::BrainFacts;
    use super::super::transition::{SteeringIntent, TransitionResult, evaluate_transition};
    use super::*;
    use glam::Vec3;
    use postretro_entities::Transform;
    use postretro_entities::components::brain::{
        AiTuning, LogicalState, attach_brain, attach_brain_graph, graph_state_index,
    };
    use postretro_entities::data_descriptors::{
        AiDescriptor, AiStateNames, BehaviorStateDescriptor, LEGACY_ALERT_STATE,
        LEGACY_ATTACK_STATE, LEGACY_DEATH_STATE, LEGACY_IDLE_STATE, MotionVerb,
        lower_ai_descriptor,
    };
    use postretro_foundation::{IrNode, IrValue, eval_value};
    use std::collections::BTreeMap;

    fn sample_descriptor() -> AiDescriptor {
        AiDescriptor {
            detection_range: 18.0,
            attack_range: 2.2,
            leash_range: 26.0,
            attack_damage: 8.0,
            attack_cooldown_ms: 1200.0,
            move_speed: 3.5,
            death_despawn_ms: 1500.0,
            states: AiStateNames {
                idle: "idle".into(),
                alert: "walk".into(),
                attack: "attack".into(),
                death: "die".into(),
            },
        }
    }

    /// The single-state graph used for lifecycle assertions; its one guard is
    /// deliberately trivial so the tests read as lifecycle, not evaluation.
    fn minimal_graph(animation: &str) -> BehaviorGraphDescriptor {
        BehaviorGraphDescriptor {
            initial: "rest".to_string(),
            states: BTreeMap::from([(
                "rest".to_string(),
                BehaviorStateDescriptor {
                    animation: animation.to_string(),
                    motion: MotionVerb::Hold,
                    action: None,
                    transitions: vec![TransitionDescriptor {
                        to: "rest".to_string(),
                        when: IrNode::Const {
                            value: IrValue::Bool(false),
                        },
                    }],
                    on_enter: None,
                },
            )]),
            interrupts: Vec::new(),
            attack: None,
            move_speed: 3.0,
            death_despawn_ms: None,
        }
    }

    /// The v0 steering intent each lowered state's motion verb stands in for.
    /// An exhaustive `match` so a widened motion vocabulary is a compile error
    /// here rather than a silently-passing identity check.
    fn steering_for(motion: MotionVerb) -> SteeringIntent {
        match motion {
            MotionVerb::ChaseTarget => SteeringIntent::Chase,
            MotionVerb::Hold => SteeringIntent::Clear,
            MotionVerb::Freeze => SteeringIntent::Hold,
        }
    }

    /// The lowered-graph state name each legacy logical state lowers to.
    fn state_name_for(state: LogicalState) -> &'static str {
        match state {
            LogicalState::Idle => LEGACY_IDLE_STATE,
            LogicalState::Alert => LEGACY_ALERT_STATE,
            LogicalState::Attack => LEGACY_ATTACK_STATE,
            LogicalState::Death => LEGACY_DEATH_STATE,
        }
    }

    /// Walk the graph exactly as the ordered-guard contract specifies —
    /// self-targeting interrupts skipped, interrupts before the current state's
    /// transitions, declaration order, first true wins — and report the
    /// resulting `(state name, steering intent)`. Staying put is the current
    /// state with its own motion verb, matching the v0 core's same-state rows.
    fn step_graph(
        graph: &BehaviorGraphDescriptor,
        programs: &BrainEntityPrograms,
        scope: &BrainScope,
        current: &str,
    ) -> (String, SteeringIntent) {
        let state_index = graph_state_index(graph, current).expect("current state is declared");
        let mut edges = graph
            .interrupts
            .iter()
            .zip(programs.interrupts())
            .filter(|(interrupt, _)| interrupt.to != current)
            .chain(
                graph.states[current]
                    .transitions
                    .iter()
                    .zip(programs.transitions(state_index)),
            );
        let next = edges
            .find(|(_, program)| {
                program
                    .as_ref()
                    .is_some_and(|program| eval_value(program, scope) == IrValue::Bool(true))
            })
            .map(|(transition, _)| transition.to.clone())
            .unwrap_or_else(|| current.to_string());
        let motion = graph.states[&next].motion;
        (next, steering_for(motion))
    }

    fn registry_with_brain(graph: &BehaviorGraphDescriptor) -> (EntityRegistry, EntityId) {
        let mut registry = EntityRegistry::new();
        let entity = registry.spawn(Transform::default());
        attach_brain_graph(&mut registry, entity, graph).expect("fresh entity is live");
        (registry, entity)
    }

    #[test]
    fn sync_binds_every_guard_of_a_newly_spawned_brain() {
        let graph = lower_ai_descriptor(&sample_descriptor());
        let (registry, entity) = registry_with_brain(&graph);
        let mut programs = BrainPrograms::new();
        let mut warned = HashSet::new();

        programs.sync(&registry, &mut warned);

        let entry = programs.get(entity).expect("the spawned brain is bound");
        assert!(entry.interrupts().is_empty(), "the lowered graph has none");
        for (index, (name, state)) in graph.states.iter().enumerate() {
            let bound = entry.transitions(index);
            assert_eq!(
                bound.len(),
                state.transitions.len(),
                "`{name}` programs are parallel to its transitions"
            );
            assert!(
                bound.iter().all(Option::is_some),
                "every generated guard in `{name}` binds"
            );
        }
        assert!(warned.is_empty(), "a clean bind warns about nothing");
    }

    #[test]
    fn sync_drops_entries_for_despawned_brains() {
        let (mut registry, entity) = registry_with_brain(&minimal_graph("idle"));
        let mut programs = BrainPrograms::new();
        let mut warned = HashSet::new();
        programs.sync(&registry, &mut warned);
        assert!(programs.get(entity).is_some());

        registry.despawn(entity).expect("entity is live");
        programs.sync(&registry, &mut warned);

        assert!(
            programs.get(entity).is_none(),
            "a despawned entity's programs are released"
        );
    }

    #[test]
    fn sync_rebinds_when_a_deserialized_brain_carries_a_different_graph() {
        // A wholesale deserialize replaces the component under the same entity
        // id; the retained graph is what decides whether the cached programs
        // still describe this brain.
        let (mut registry, entity) = registry_with_brain(&minimal_graph("idle"));
        let mut programs = BrainPrograms::new();
        let mut warned = HashSet::new();
        programs.sync(&registry, &mut warned);
        assert_eq!(programs.get(entity).unwrap().transitions(0).len(), 1);

        let replacement = lower_ai_descriptor(&sample_descriptor());
        let restored = serde_json::from_value::<BrainComponent>(
            serde_json::to_value(BrainComponent::from_graph(&replacement)).unwrap(),
        )
        .unwrap();
        registry
            .set_component(entity, restored)
            .expect("entity is live");
        programs.sync(&registry, &mut warned);

        let entry = programs.get(entity).expect("the restored brain is bound");
        let idle_index = graph_state_index(&replacement, LEGACY_IDLE_STATE).unwrap();
        assert_eq!(
            entry.transitions(idle_index).len(),
            2,
            "the programs describe the graph the component now carries"
        );
    }

    #[test]
    fn sync_leaves_an_unchanged_brain_bound_without_rebinding() {
        // Binding is the only point where the shared scope's slot table grows,
        // so a repeated sync must not repeat it. A graph with one unbindable
        // guard makes that observable: a rebind would re-run the warn.
        let mut graph = minimal_graph("idle");
        graph.states.get_mut("rest").unwrap().transitions[0].when = IrNode::Input {
            name: "@brain.morale".to_string(),
        };
        let (registry, entity) = registry_with_brain(&graph);
        let mut programs = BrainPrograms::new();
        let mut warned = HashSet::new();
        programs.sync(&registry, &mut warned);
        warned.clear();

        programs.sync(&registry, &mut warned);

        assert!(
            warned.is_empty(),
            "an unchanged brain keeps the programs it already has"
        );
        assert!(programs.get(entity).unwrap().transitions(0)[0].is_none());
    }

    #[test]
    fn an_unbindable_guard_is_disabled_and_warns_once() {
        let mut graph = minimal_graph("idle");
        graph.states.get_mut("rest").unwrap().transitions[0].when = IrNode::Input {
            name: "@brain.morale".to_string(),
        };
        // A second entity with the same broken graph must not warn again.
        let (mut registry, entity) = registry_with_brain(&graph);
        let second = registry.spawn(Transform::default());
        attach_brain_graph(&mut registry, second, &graph).unwrap();

        let mut programs = BrainPrograms::new();
        let mut warned = HashSet::new();
        programs.sync(&registry, &mut warned);

        for bound in [entity, second] {
            assert!(
                programs.get(bound).unwrap().transitions(0)[0].is_none(),
                "the unbindable edge is disabled, not fatal"
            );
        }
        assert_eq!(
            warned.iter().collect::<Vec<_>>(),
            vec!["brain-guard:states.rest.transitions[0]"],
            "one warn per authored path, naming the state and index"
        );
    }

    #[test]
    fn a_number_producing_guard_is_disabled_rather_than_evaluated() {
        let mut graph = minimal_graph("idle");
        graph.states.get_mut("rest").unwrap().transitions[0].when = IrNode::Input {
            name: postretro_foundation::BRAIN_TARGET_DISTANCE_INPUT.to_string(),
        };
        let (registry, entity) = registry_with_brain(&graph);
        let mut programs = BrainPrograms::new();
        let mut warned = HashSet::new();

        programs.sync(&registry, &mut warned);

        assert!(programs.get(entity).unwrap().transitions(0)[0].is_none());
        assert_eq!(warned.len(), 1);
    }

    #[test]
    fn transitions_are_total_for_an_index_outside_the_resolved_state_list() {
        let (registry, entity) = registry_with_brain(&minimal_graph("idle"));
        let mut programs = BrainPrograms::new();
        programs.sync(&registry, &mut HashSet::new());
        assert!(programs.get(entity).unwrap().transitions(99).is_empty());
    }

    #[test]
    fn the_lowered_graph_reproduces_evaluate_transition_edge_for_edge() {
        // Drift guard: the expectation is `evaluate_transition` itself, not a
        // second table of the same edges. Every (state × distance × acquisition)
        // row the legacy core answers must be the row the lowered graph's
        // ordered guards answer.
        let ai = sample_descriptor();
        let tuning = AiTuning::from_descriptor(&ai);
        let graph = lower_ai_descriptor(&ai);
        let (registry, entity) = registry_with_brain(&graph);
        let mut programs = BrainPrograms::new();
        let mut warned = HashSet::new();
        programs.sync(&registry, &mut warned);
        assert!(warned.is_empty(), "every generated guard binds");

        // Sample each threshold and both sides of it, plus the ordering between
        // attack and detection range.
        let distances = [
            0.0,
            ai.attack_range - 0.1,
            ai.attack_range,
            ai.attack_range + 0.1,
            ai.detection_range - 0.1,
            ai.detection_range,
            ai.detection_range + 0.1,
            ai.leash_range,
            ai.leash_range + 0.1,
            120.0,
        ];
        for current in LogicalState::ALL {
            // `Death` is terminal in the legacy core and has no lowered edges;
            // the graph never enters it, so there is no row to compare.
            if current == LogicalState::Death {
                assert!(
                    graph.states[LEGACY_DEATH_STATE].transitions.is_empty(),
                    "death stays terminal in the lowered graph"
                );
                continue;
            }
            for distance in distances {
                for acquisition_due in [false, true] {
                    let expected: TransitionResult = evaluate_transition(
                        Vec3::new(distance, 0.0, 0.0),
                        Vec3::ZERO,
                        &tuning,
                        current,
                        acquisition_due,
                    );

                    programs.scope_mut().refresh(
                        &registry,
                        entity,
                        BrainFacts {
                            target_distance: Some(distance),
                            time_in_state_ms: 0.0,
                            attack_cooldown_ms: 0.0,
                            acquisition_due,
                        },
                    );
                    let entry = programs.get(entity).expect("the brain stays bound");
                    let (next, steering) =
                        step_graph(&graph, entry, programs.scope(), state_name_for(current));

                    assert_eq!(
                        (next.as_str(), steering),
                        (state_name_for(expected.next_state), expected.steering),
                        "state `{current:?}` at distance {distance} \
                         (acquisitionDue = {acquisition_due})"
                    );
                }
            }
        }
    }

    #[test]
    fn a_lowered_brain_with_no_target_stays_idle_under_the_distance_sentinel() {
        // The no-target sentinel must read false through every range guard, so
        // an enemy with nothing to chase never leaves `idle` on guard evidence
        // alone.
        let graph = lower_ai_descriptor(&sample_descriptor());
        let (registry, entity) = registry_with_brain(&graph);
        let mut programs = BrainPrograms::new();
        programs.sync(&registry, &mut HashSet::new());

        programs.scope_mut().refresh(
            &registry,
            entity,
            BrainFacts {
                target_distance: None,
                time_in_state_ms: 0.0,
                attack_cooldown_ms: 0.0,
                acquisition_due: true,
            },
        );
        let entry = programs.get(entity).unwrap();
        let (next, steering) = step_graph(&graph, entry, programs.scope(), LEGACY_IDLE_STATE);
        assert_eq!(
            (next.as_str(), steering),
            (LEGACY_IDLE_STATE, SteeringIntent::Clear)
        );
    }

    #[test]
    fn a_legacy_attached_brain_binds_through_the_same_lowering() {
        // `attach_brain` (the legacy `components.ai` seam) and the authored seam
        // land the same component shape, so the side-table cannot tell them
        // apart.
        let mut registry = EntityRegistry::new();
        let entity = registry.spawn(Transform::default());
        attach_brain(&mut registry, entity, &sample_descriptor()).unwrap();
        let mut programs = BrainPrograms::new();
        let mut warned = HashSet::new();

        programs.sync(&registry, &mut warned);

        let graph = lower_ai_descriptor(&sample_descriptor());
        let alert_index = graph_state_index(&graph, LEGACY_ALERT_STATE).unwrap();
        let attack_index = graph_state_index(&graph, LEGACY_ATTACK_STATE).unwrap();
        let entry = programs.get(entity).expect("legacy brains bind too");
        assert_eq!(entry.transitions(alert_index).len(), 2);
        assert_eq!(entry.transitions(attack_index).len(), 1);
        assert!(warned.is_empty());
    }
}
