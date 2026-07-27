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
use std::sync::Arc;

use postretro_entities::{ComponentKind, ComponentValue, EntityId, EntityRegistry};
use postretro_foundation::{
    BakedIr, BehaviorGraphDescriptor, BoundProgram, CURRENT_IR_VERSION, IrType,
    TransitionDescriptor, bind,
};

use super::brain_scope::BrainScope;
use super::candidate_scope::CandidateScope;

/// One entity's bound guards, laid out to match its graph so the evaluator can
/// index straight from the descriptor it is walking.
///
/// A `None` slot is a DISABLED edge: its guard failed to bind, so the evaluator
/// treats it as permanently false rather than aborting the brain. Authored
/// guards are bind-validated at parse and lowered legacy guards are engine
/// generated, so a `None` here means one of those two contracts broke — hence
/// the warn at bind time.
pub(crate) struct BrainEntityPrograms {
    /// The graph these programs were bound from — the same shared handle the
    /// brain carries. Retained so `sync` can tell a still-valid entry from one
    /// whose entity was re-seeded with a different brain (the deserialize/reload
    /// case), and held as an `Arc` so that test is a pointer compare rather than
    /// a structural walk of every guard tree on every entity every tick.
    graph: Arc<BehaviorGraphDescriptor>,
    /// Parallel to `graph.interrupts`.
    interrupts: Vec<Option<BoundProgram<BrainScope>>>,
    /// Indexed by resolved state index (`graph.states` in `BTreeMap` order);
    /// each inner vec is parallel to that state's `transitions`.
    states: Vec<Vec<Option<BoundProgram<BrainScope>>>>,
    /// Optional per-graph acquisition eligibility program. It uses the shared
    /// candidate scope rather than the guard scope and remains evaluator data.
    candidate_filter: Option<BoundProgram<CandidateScope>>,
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

    pub(crate) fn candidate_filter(&self) -> Option<&BoundProgram<CandidateScope>> {
        self.candidate_filter.as_ref()
    }
}

/// The evaluator's per-entity guard programs plus the shared binding scope.
pub(crate) struct BrainPrograms {
    scope: BrainScope,
    candidate_scope: CandidateScope,
    entries: HashMap<EntityId, BrainEntityPrograms>,
    /// The entities `sync` saw on its current pass. A field rather than a local
    /// purely so its capacity survives: `sync` runs every AI tick, and a local
    /// set would allocate on every one of them to detect a condition that only
    /// changes at spawn, despawn, or re-seed.
    live: HashSet<EntityId>,
}

impl BrainPrograms {
    pub(crate) fn new() -> Self {
        Self {
            scope: BrainScope::for_validation(),
            candidate_scope: CandidateScope::for_validation(),
            entries: HashMap::new(),
            live: HashSet::new(),
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

    /// Borrow the optional filter and reusable refresh scope independently.
    /// Acquisition holds the former over a scan while mutating the latter for
    /// each offered candidate; no interior mutability is needed.
    pub(crate) fn candidate_filter_context(
        &mut self,
        entity: EntityId,
    ) -> (Option<&BoundProgram<CandidateScope>>, &mut CandidateScope) {
        let (entries, candidate_scope) = (&self.entries, &mut self.candidate_scope);
        let filter = entries
            .get(&entity)
            .and_then(BrainEntityPrograms::candidate_filter);
        (filter, candidate_scope)
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
    /// reports once per distinct broken contract and leaves that edge disabled.
    ///
    /// Steady state — nothing spawned, despawned, or re-seeded — allocates
    /// nothing. That is deliberate: `sync` runs every AI tick but only ever has
    /// work on those three events, so it walks the registry iterator directly
    /// (`registry` is an independent borrow from `&mut self`) and reuses the
    /// `live` set's capacity instead of building a fresh `Vec` and `HashSet`.
    pub(crate) fn sync(&mut self, registry: &EntityRegistry, warned: &mut HashSet<String>) {
        self.live.clear();
        for (entity, value) in registry.iter_with_kind(ComponentKind::Brain) {
            let ComponentValue::Brain(brain) = value else {
                continue;
            };
            self.live.insert(entity);
            // Pointer identity, NOT structural equality: a graph is immutable
            // once attached, so sharing the same allocation is proof the bound
            // programs still describe this brain. Comparing contents would walk
            // every authored state and guard tree for every enemy every tick to
            // detect a change that only happens at spawn or after a
            // deserialize (both of which land a different allocation).
            let bound = self
                .entries
                .get(&entity)
                .is_some_and(|entry| Arc::ptr_eq(&entry.graph, &brain.graph));
            if !bound {
                let entry = bind_graph(
                    &self.scope,
                    &self.candidate_scope,
                    Arc::clone(&brain.graph),
                    warned,
                );
                self.entries.insert(entity, entry);
            }
        }

        let live = &self.live;
        self.entries.retain(|entity, _| live.contains(entity));
    }
}

/// Where a guard sits inside its graph.
///
/// Carried as a value rather than a formatted `String` because binding
/// SUCCEEDS for all but a broken graph: building the path at the call site
/// allocated one string per guard per spawn — a monster-closet reveal of thirty
/// enemies over a twenty-guard graph paid six hundred allocations in one tick —
/// and every one of them was dropped unread. [`Display`] defers the cost to the
/// failure branch, which runs at most once per distinct problem.
enum GuardPath<'a> {
    Interrupt { index: usize },
    StateTransition { state: &'a str, index: usize },
}

impl std::fmt::Display for GuardPath<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Interrupt { index } => write!(f, "interrupts[{index}]"),
            Self::StateTransition { state, index } => {
                write!(f, "states.{state}.transitions[{index}]")
            }
        }
    }
}

/// The author-facing spelling of an IR result type. Matched exhaustively so a
/// third `IrType` cannot silently keep reporting one of the first two.
fn ir_type_label(ir_type: IrType) -> &'static str {
    match ir_type {
        IrType::Number => "a number",
        IrType::Bool => "a boolean",
    }
}

/// Bind every guard in `graph` against the shared scope, warning once per
/// distinct failure and leaving those edges disabled.
fn bind_graph(
    scope: &BrainScope,
    candidate_scope: &CandidateScope,
    graph: Arc<BehaviorGraphDescriptor>,
    warned: &mut HashSet<String>,
) -> BrainEntityPrograms {
    let interrupts = graph
        .interrupts
        .iter()
        .enumerate()
        .map(|(index, transition)| {
            bind_guard(
                scope,
                &graph,
                transition,
                GuardPath::Interrupt { index },
                warned,
            )
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
                        &graph,
                        transition,
                        GuardPath::StateTransition { state: name, index },
                        warned,
                    )
                })
                .collect()
        })
        .collect();
    let candidate_filter = graph
        .candidate_filter
        .as_ref()
        .and_then(|filter| bind_candidate_filter_program(candidate_scope, &graph, filter, warned));
    BrainEntityPrograms {
        graph,
        interrupts,
        states,
        candidate_filter,
    }
}

fn bind_candidate_filter_program(
    scope: &CandidateScope,
    graph: &BehaviorGraphDescriptor,
    filter: &postretro_foundation::IrNode,
    warned: &mut HashSet<String>,
) -> Option<BoundProgram<CandidateScope>> {
    let baked = BakedIr {
        version: CURRENT_IR_VERSION,
        output: None,
        root: filter.clone(),
    };
    let reason = match bind(&baked, scope) {
        Ok(program) if program.root_type == IrType::Bool => return Some(program),
        Ok(program) => format!(
            "its root produces {}, not a boolean",
            ir_type_label(program.root_type)
        ),
        Err(error) => error.to_string(),
    };
    let initial = &graph.initial;
    if warned.insert(format!("candidate-filter:{initial}:{reason}")) {
        log::warn!(
            "[AI] behavior candidate filter could not be bound ({reason}); it is disabled for \
             the rest of the run. The graph starts in `{initial}`. Warned once per distinct \
             graph and reason."
        );
    }
    None
}

fn bind_guard(
    scope: &BrainScope,
    graph: &BehaviorGraphDescriptor,
    transition: &TransitionDescriptor,
    path: GuardPath<'_>,
    warned: &mut HashSet<String>,
) -> Option<BoundProgram<BrainScope>> {
    let baked = BakedIr {
        version: CURRENT_IR_VERSION,
        output: None,
        root: transition.when.clone(),
    };
    let reason = match bind(&baked, scope) {
        Ok(program) if program.root_type == IrType::Bool => return Some(program),
        Ok(program) => format!(
            "its root produces {}, not a boolean",
            ir_type_label(program.root_type)
        ),
        Err(error) => error.to_string(),
    };
    // The latch key describes the whole PROBLEM, not just its coordinate. A path
    // is intra-graph — `interrupts[0]` is about the most collision-prone string
    // this vocabulary produces — and `AiRuntime` lives for the whole app and is
    // never rebuilt on level load, so keying on the path alone let one
    // archetype's broken guard permanently swallow a different archetype's
    // different broken guard. Authored graphs are bind-validated at parse, so a
    // failure here means a contract broke; masking a second, unrelated break is
    // precisely the diagnosability this warn exists to preserve.
    let to = &transition.to;
    let initial = &graph.initial;
    if warned.insert(format!("brain-guard:{initial}:{path}:{to}:{reason}")) {
        let states: Vec<&str> = graph.states.keys().map(String::as_str).collect();
        log::warn!(
            "[AI] behavior guard `{path}` could not be bound ({reason}); the transition to \
             `{to}` is disabled for the rest of the run. The graph starts in `{initial}` and \
             declares {states:?}. Warned once per distinct graph, path, target, and reason.",
        );
    }
    None
}

// Lowering/parity coverage is retired with the legacy descriptor. Task 3
// replaces this suite with direct-graph regressions.
#[cfg(any())]
mod tests {
    use super::super::brain_scope::BrainFacts;
    use super::super::engine_floor::SteeringIntent;
    use super::super::graph_eval::{select_transition, steering_for};
    use super::*;
    use crate::alloc_probe::AllocSnapshot;
    use postretro_entities::Transform;
    use postretro_entities::components::brain::{
        BrainComponent, attach_brain, attach_brain_graph, graph_state_index,
    };
    use postretro_entities::data_descriptors::{
        AiDescriptor, AiStateNames, BehaviorStateDescriptor, LEGACY_ALERT_STATE,
        LEGACY_ATTACK_STATE, LEGACY_DEATH_STATE, LEGACY_IDLE_STATE, MotionVerb,
        lower_ai_descriptor,
    };
    use postretro_foundation::{IrNode, IrValue};
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
            candidate_filter: None,
            attack: None,
            engagement_radius: None,
            move_speed: 3.0,
        }
    }

    /// The v0 four-state transition core, restated over plain scalars.
    ///
    /// This is the DIFFERENTIAL ORACLE the lowered graph's generated guards are
    /// tested against, and deliberately a second, independent implementation of
    /// the legacy rules rather than a table of the same edges — a table can be
    /// edited to agree with a broken lowering, a restatement of the rules
    /// cannot. It lives here, in the test module, because the v0 core itself is
    /// gone from the engine: nothing but this drift guard ever needed it.
    ///
    /// Closed transition set (`ai` is the descriptor being lowered):
    /// - NO TARGET, from ANY state: `idle` with steering cleared. The v0 tick
    ///   never reached the transition core without a pawn — its `else` arm
    ///   forced `Idle` + `Clear` outright — so that arm is restated here as the
    ///   `has_target == false` row. In the lowered graph this is the ONE edge
    ///   that is not a restatement of a v0 rule but a generated interrupt
    ///   (`!hasTarget → idle`), which is exactly why it has to be sampled: with
    ///   a target on every row it evaluates false everywhere and a
    ///   wrong-but-present interrupt would pass.
    /// - `idle` → `attack` when acquisition fires and the target is inside
    ///   detection AND attack range (the "newly alerted, already in contact"
    ///   branch, nested inside the detection check);
    /// - `idle` → `alert` when acquisition fires and the target is inside
    ///   detection range;
    /// - `alert` → `attack` whenever the target is inside attack range
    ///   (NOT acquisition-gated);
    /// - `alert` → `idle` when acquisition fires and the target is beyond leash;
    /// - `attack` → `alert` whenever the target leaves attack range
    ///   (NOT acquisition-gated);
    /// - `death` is terminal and touches no steering.
    fn v0_transition(
        ai: &AiDescriptor,
        current: &str,
        has_target: bool,
        distance: f32,
        acquisition_due: bool,
    ) -> (&'static str, SteeringIntent) {
        if !has_target {
            return (LEGACY_IDLE_STATE, SteeringIntent::Clear);
        }
        match current {
            LEGACY_IDLE_STATE => {
                if acquisition_due && distance <= ai.detection_range {
                    if distance <= ai.attack_range {
                        (LEGACY_ATTACK_STATE, SteeringIntent::Chase)
                    } else {
                        (LEGACY_ALERT_STATE, SteeringIntent::Chase)
                    }
                } else {
                    (LEGACY_IDLE_STATE, SteeringIntent::Clear)
                }
            }
            LEGACY_ALERT_STATE => {
                if distance <= ai.attack_range {
                    (LEGACY_ATTACK_STATE, SteeringIntent::Chase)
                } else if acquisition_due && distance > ai.leash_range {
                    (LEGACY_IDLE_STATE, SteeringIntent::Clear)
                } else {
                    (LEGACY_ALERT_STATE, SteeringIntent::Chase)
                }
            }
            LEGACY_ATTACK_STATE => {
                if distance > ai.attack_range {
                    (LEGACY_ALERT_STATE, SteeringIntent::Chase)
                } else {
                    (LEGACY_ATTACK_STATE, SteeringIntent::Chase)
                }
            }
            LEGACY_DEATH_STATE => (LEGACY_DEATH_STATE, SteeringIntent::Hold),
            other => panic!("`{other}` is not a lowered legacy state"),
        }
    }

    /// One step of the PRODUCTION selector and the PRODUCTION verb mapping,
    /// reported as `(state name, steering intent)`.
    ///
    /// Deliberately not a restatement of the ordered-guard walk: the point of
    /// the drift guard below is that the shipped `select_transition` and
    /// `steering_for` answer what the v0 oracle answers, which a test-local copy
    /// of either would quietly stop proving. Staying put is the current state
    /// with its own motion verb, matching the v0 core's same-state rows.
    fn step_graph(
        graph: &BehaviorGraphDescriptor,
        programs: &BrainEntityPrograms,
        scope: &BrainScope,
        current: &str,
    ) -> (String, SteeringIntent) {
        let current_index = graph_state_index(graph, current).expect("current state is declared");
        let next_index =
            select_transition(graph, programs, scope, current_index).unwrap_or(current_index);
        let (name, state) = graph
            .states
            .iter()
            .nth(next_index)
            .expect("the selected index is declared");
        (name.clone(), steering_for(state.motion))
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
        assert_eq!(
            entry.interrupts().len(),
            1,
            "the lowered graph's one any-state edge is the no-target stand-down"
        );
        assert!(
            entry.interrupts().iter().all(Option::is_some),
            "the generated stand-down guard binds"
        );
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
    fn sync_leaves_an_unchanged_candidate_filter_bound_without_rewarning() {
        // The observable is the warn-once latch, not a program address: a
        // HashMap replacement can retain an allocation address across a rebind.
        let mut graph = minimal_graph("idle");
        graph.candidate_filter = Some(IrNode::Input {
            name: "@candidate.morale".to_string(),
        });
        let (registry, entity) = registry_with_brain(&graph);
        let mut programs = BrainPrograms::new();
        let mut warned = HashSet::new();
        programs.sync(&registry, &mut warned);
        assert!(programs.get(entity).unwrap().candidate_filter().is_none());
        warned.clear();

        programs.sync(&registry, &mut warned);

        assert!(
            warned.is_empty(),
            "an unchanged graph must not bind and warn about its filter again"
        );
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
        assert_eq!(warned.len(), 1, "one warn per distinct broken guard");
        let key = warned.iter().next().unwrap();
        assert!(
            key.starts_with("brain-guard:") && key.contains("states.rest.transitions[0]"),
            "the warn key names the state and transition index: {key}"
        );
    }

    #[test]
    fn two_graphs_broken_at_the_same_path_each_report() {
        // The path is an INTRA-graph coordinate, and `interrupts[0]` /
        // `states.<name>.transitions[0]` collide readily across archetypes. A
        // latch keyed on the path alone would let the first broken graph silence
        // every later one for the rest of the run — the exact diagnosability loss
        // the warn exists to prevent.
        let mut first = minimal_graph("idle");
        first.states.get_mut("rest").unwrap().transitions[0].when = IrNode::Input {
            name: "@brain.morale".to_string(),
        };
        let mut second = minimal_graph("walk");
        second.states.get_mut("rest").unwrap().transitions[0].when = IrNode::Input {
            name: "@brain.nerve".to_string(),
        };

        let mut registry = EntityRegistry::new();
        for graph in [&first, &second] {
            let entity = registry.spawn(Transform::default());
            attach_brain_graph(&mut registry, entity, graph).unwrap();
        }
        let mut programs = BrainPrograms::new();
        let mut warned = HashSet::new();

        programs.sync(&registry, &mut warned);

        assert_eq!(
            warned.len(),
            2,
            "each broken graph reports its own failure: {warned:?}"
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
    fn a_steady_state_sync_performs_zero_heap_allocations() {
        // `sync` runs every AI tick, but the condition it detects only changes at
        // spawn, despawn, or re-seed. With none of those pending it must do no
        // work the allocator can see — it sits just outside the guarded per-tick
        // guard window, so nothing else would catch a regression here.
        let graph = lower_ai_descriptor(&sample_descriptor());
        let (registry, _) = registry_with_brain(&graph);
        let mut programs = BrainPrograms::new();
        let mut warned = HashSet::new();
        programs.sync(&registry, &mut warned);

        let snapshot = AllocSnapshot::arm();
        programs.sync(&registry, &mut warned);
        let allocs = snapshot.allocs_since();

        assert_eq!(
            allocs, 0,
            "a sync with nothing to reconcile must not allocate"
        );
    }

    #[test]
    fn transitions_are_total_for_an_index_outside_the_resolved_state_list() {
        let (registry, entity) = registry_with_brain(&minimal_graph("idle"));
        let mut programs = BrainPrograms::new();
        programs.sync(&registry, &mut HashSet::new());
        assert!(programs.get(entity).unwrap().transitions(99).is_empty());
    }

    #[test]
    fn the_lowered_graph_reproduces_the_v0_transition_core_edge_for_edge() {
        // Drift guard: every (state × has-target × distance × acquisition) row
        // the legacy core answers must be the row the lowered graph's ordered
        // guards answer. The expectation is an independent restatement of the
        // v0 rules (`v0_transition`), not a transcription of the lowered edges.
        //
        // The has-target dimension is what reaches the generated
        // `!hasTarget → idle` interrupt. Sampled from `attack` in particular:
        // that is the state whose only other exit is the attack-range edge, so
        // deleting the interrupt leaves it in `attack` on the sentinel. (From
        // `alert` the leash edge is itself true on the sentinel and reaches
        // `idle` anyway, so `alert` alone would not detect the loss.)
        let ai = sample_descriptor();
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
        for current in [
            LEGACY_IDLE_STATE,
            LEGACY_ALERT_STATE,
            LEGACY_ATTACK_STATE,
            LEGACY_DEATH_STATE,
        ] {
            if current == LEGACY_DEATH_STATE {
                // `death` is terminal in both: the graph never enters it and it
                // declares no edges, so `step_graph` can only report itself.
                assert!(
                    graph.states[LEGACY_DEATH_STATE].transitions.is_empty(),
                    "death stays terminal in the lowered graph"
                );
            }
            for has_target in [false, true] {
                for distance in distances {
                    for acquisition_due in [false, true] {
                        let expected =
                            v0_transition(&ai, current, has_target, distance, acquisition_due);

                        programs.scope_mut().refresh(
                            &registry,
                            entity,
                            BrainFacts {
                                // No target means no distance to read: the scope
                                // projects `hasTarget` false and the distance
                                // sentinel, which is what the interrupt sees.
                                target: has_target.then_some((entity, distance)),
                                time_in_state_ms: 0.0,
                                attack_cooldown_ms: 0.0,
                                acquisition_due,
                            },
                        );
                        let entry = programs.get(entity).expect("the brain stays bound");
                        let (next, steering) = step_graph(&graph, entry, programs.scope(), current);

                        assert_eq!(
                            (next.as_str(), steering),
                            expected,
                            "state `{current}` at distance {distance} \
                             (hasTarget = {has_target}, acquisitionDue = {acquisition_due})"
                        );
                    }
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
                target: None,
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
    fn a_round_tripped_brain_carries_no_programs_and_rebinds_from_its_retained_graph() {
        // Bound programs are DERIVED data: they live here, not on the
        // component, so serde cannot reach them and component equality cannot
        // see them. What crosses the wire is the retained graph, and that is
        // enough to rebuild guards that answer identically.
        let graph = lower_ai_descriptor(&sample_descriptor());
        let mut brain = BrainComponent::from_graph(&graph);
        brain.state_index = graph_state_index(&graph, LEGACY_ALERT_STATE).unwrap();
        brain.time_in_state_ms = 320.0;

        let json = serde_json::to_value(&brain).expect("brain serializes");
        let keys: Vec<&str> = json
            .as_object()
            .expect("brain is a JSON object")
            .keys()
            .map(String::as_str)
            .collect();
        assert!(
            !keys.iter().any(|key| key.contains("program")),
            "no bound-program field reaches the wire: {keys:?}"
        );
        let restored: BrainComponent = serde_json::from_value(json).expect("brain round-trips");
        assert_eq!(
            restored, brain,
            "component equality is unaffected by the programs bound from it"
        );

        // Two independent side-tables: one bound from the original brain, one
        // from the deserialized twin. Both must answer every sampled row the
        // same way, which is what "programs rebind from the retained graph"
        // means operationally.
        let facts = |entity: EntityId, distance: f32, acquisition_due: bool| BrainFacts {
            target: Some((entity, distance)),
            time_in_state_ms: 320.0,
            attack_cooldown_ms: 0.0,
            acquisition_due,
        };
        let mut answers = Vec::new();
        for brain in [brain.clone(), restored] {
            let mut registry = EntityRegistry::new();
            let entity = registry.spawn(Transform::default());
            registry
                .set_component(entity, brain)
                .expect("entity is live");
            let mut programs = BrainPrograms::new();
            let mut warned = HashSet::new();
            programs.sync(&registry, &mut warned);
            assert!(
                warned.is_empty(),
                "a rebind of a valid graph warns about nothing"
            );

            let mut rows = Vec::new();
            for distance in [1.0, 10.0, 40.0] {
                for acquisition_due in [false, true] {
                    programs.scope_mut().refresh(
                        &registry,
                        entity,
                        facts(entity, distance, acquisition_due),
                    );
                    let entry = programs.get(entity).expect("the brain is bound");
                    rows.push(step_graph(
                        &graph,
                        entry,
                        programs.scope(),
                        LEGACY_ALERT_STATE,
                    ));
                }
            }
            answers.push(rows);
        }
        assert_eq!(
            answers[0], answers[1],
            "programs rebuilt from the retained graph evaluate identically"
        );
    }

    #[test]
    fn the_per_tick_guard_window_performs_zero_heap_allocations() {
        // The substrate invariant (scripting.md §11) at the evaluator seam:
        // refreshing the scope for one enemy and walking its ordered guards
        // must not allocate. Binding and interning happen in `sync`, before the
        // probe is armed — which is exactly where allocation is allowed.
        let graph = lower_ai_descriptor(&sample_descriptor());
        let (registry, entity) = registry_with_brain(&graph);
        let mut programs = BrainPrograms::new();
        programs.sync(&registry, &mut HashSet::new());
        let idle_index = graph_state_index(&graph, LEGACY_IDLE_STATE).unwrap();
        let facts = BrainFacts {
            target: Some((entity, 1.0)),
            time_in_state_ms: 0.0,
            attack_cooldown_ms: 0.0,
            acquisition_due: true,
        };

        // Warm any one-time lazy state so the measured window is pure work.
        programs.scope_mut().refresh(&registry, entity, facts);
        let warm = select_transition(
            &graph,
            programs.get(entity).expect("bound"),
            programs.scope(),
            idle_index,
        );

        let snapshot = AllocSnapshot::arm();
        programs.scope_mut().refresh(&registry, entity, facts);
        let selected = select_transition(
            &graph,
            programs.get(entity).expect("bound"),
            programs.scope(),
            idle_index,
        );
        let allocs = snapshot.allocs_since();

        assert_eq!(selected, warm);
        assert_eq!(
            selected,
            graph_state_index(&graph, LEGACY_ATTACK_STATE),
            "the sampled row actually fires a transition, so guards really ran"
        );
        assert_eq!(
            allocs, 0,
            "scope refresh + ordered guard evaluation must perform zero heap allocations"
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
