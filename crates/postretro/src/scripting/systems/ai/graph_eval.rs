// Recursive behavior-statechart evaluation and the verb resolvers used by the
// AI tick. The transition ordering contract lives entirely in
// `select_transition`.

use postretro_entities::components::brain::BrainComponent;
use postretro_foundation::{
    ActionVerb, BehaviorActivityDescriptor, BehaviorGraphDescriptor, BehaviorLayerDescriptor,
    BehaviorSelectorEntry, IrValue, MotionVerb, eval_value,
};

use super::brain_programs::{BoundLayer, BrainEntityPrograms};
use super::brain_scope::{BrainScope, ScopeRelativeValues};
use super::engine_floor::SteeringIntent;

type SelectorLayer<'a> = (
    &'a [BehaviorSelectorEntry],
    &'a [Option<postretro_foundation::BoundProgram<BrainScope>>],
);

/// Evaluate each active envelope outer-to-inner. At a level, `"*"` rows run
/// before the active activity's own rows. A winning edge seats its target and
/// complete initial descent immediately, then ends this pass so an entered
/// node cannot evaluate its own row until the following tick.
pub(super) fn select_transition_path(
    bound: &BrainEntityPrograms,
    scope: &mut BrainScope,
    brain: &mut BrainComponent,
) -> bool {
    let mut envelope_index = 0;
    for depth in 0..brain.active_depth() {
        let Some(activity_index) = brain.active_activity_index(depth) else {
            return false;
        };
        let Some(envelope) = brain.envelope_at_depth(depth) else {
            return false;
        };
        let Some(bound_envelope) = bound.envelope(envelope_index) else {
            return false;
        };
        let Some((current_name, _)) = envelope.activities.iter().nth(activity_index) else {
            return false;
        };
        let Some(bound_activity) = bound_envelope.activities.get(activity_index) else {
            return false;
        };
        scope.repoint_scope_relative(ScopeRelativeValues {
            time_in_activity_ms: brain.activity_timer(depth).unwrap_or(0.0),
            attacks_fired_in_activity: brain.activity_attack_count(depth).unwrap_or(0),
        });

        let wildcard = envelope
            .transitions
            .get("*")
            .map(Vec::as_slice)
            .unwrap_or_default();
        let local = envelope
            .transitions
            .get(current_name)
            .map(Vec::as_slice)
            .unwrap_or_default();
        if let Some(target) =
            first_enabled_target(wildcard, &bound_envelope.wildcard, current_name, scope)
                .or_else(|| first_enabled_target(local, &bound_activity.rows, current_name, scope))
            && let Some(target_index) = envelope.activities.keys().position(|name| name == target)
        {
            return brain.enter_activity_at(depth, target_index);
        }

        // AC18 makes this unambiguous: a composite has at most one stateful
        // layer, while any number of selectors remain orthogonal/stateless.
        let Some(child) = bound_activity.layers.iter().find_map(|layer| match layer {
            BoundLayer::Graph(index) => Some(*index),
            BoundLayer::Selector(_) => None,
        }) else {
            return false;
        };
        envelope_index = child;
    }
    false
}

fn first_enabled_target<'a>(
    rows: &'a [postretro_foundation::GuardedRow],
    programs: &'a [Option<postretro_foundation::BoundProgram<BrainScope>>],
    current_name: &str,
    scope: &BrainScope,
) -> Option<&'a str> {
    rows.iter()
        .zip(programs)
        .filter(|(row, _)| row.to != current_name)
        .find(|(_, program)| {
            program
                .as_ref()
                .is_some_and(|program| eval_value(program, scope) == IrValue::Bool(true))
        })
        .map(|(row, _)| row.to.as_str())
}

/// Resolve a move selector (first matching row, then trailing fallback) or the
/// active leaf's direct motion. Selectors are per-tick policy; they do not
/// enter a state or mutate the path.
pub(super) fn motion_for_path(
    bound: &BrainEntityPrograms,
    scope: &mut BrainScope,
    brain: &BrainComponent,
) -> Option<MotionVerb> {
    for depth in (0..brain.active_depth()).rev() {
        let (_, activity) = brain.activity_at_depth(depth)?;
        if let Some(motion) = activity.motion {
            return Some(motion);
        }
        if let Some(motion) = selector_motion(bound, scope, brain, depth, activity) {
            return Some(motion);
        }
    }
    None
}

pub(super) fn action_for_path<'a>(
    bound: &'a BrainEntityPrograms,
    scope: &mut BrainScope,
    brain: &'a BrainComponent,
) -> Option<&'a ActionVerb> {
    action_for_path_from_depth(bound, scope, brain, 0)
}

fn action_for_path_from_depth<'a>(
    bound: &'a BrainEntityPrograms,
    scope: &mut BrainScope,
    brain: &'a BrainComponent,
    start_depth: usize,
) -> Option<&'a ActionVerb> {
    for depth in (start_depth.min(brain.active_depth())..brain.active_depth()).rev() {
        let (_, activity) = brain.activity_at_depth(depth)?;
        if let Some(action) = activity.action.as_ref()
            && !matches!(
                activity.motion,
                Some(MotionVerb::MoveToAnchor | MotionVerb::Patrol)
            )
        {
            return Some(action);
        }
        if let Some(action) = selector_action(bound, scope, brain, depth, activity) {
            return Some(action);
        }
    }
    None
}

pub(super) fn engages_path(
    bound: &BrainEntityPrograms,
    scope: &mut BrainScope,
    brain: &BrainComponent,
) -> bool {
    matches!(
        motion_for_path(bound, scope, brain),
        Some(MotionVerb::ChaseTarget)
    ) || action_for_path(bound, scope, brain).is_some()
}

/// Target retention follows the active path's engagement capability, not the
/// verb a selector resolves this tick. An in-range selector may intentionally
/// hold through an actionless committed phase and still need target facts next
/// tick.
pub(super) fn engages_active(brain: &BrainComponent) -> bool {
    (0..brain.active_depth()).any(|depth| {
        brain
            .activity_at_depth(depth)
            .is_some_and(|(_, activity)| activity_can_engage(activity))
    })
}

/// Retention cannot depend only on a selector's current row: before fact
/// refresh no current resolution exists, and an in-range hold still needs the
/// target on the next tick. A `move`/`offense` selector that can chase or attack
/// is therefore an engaged activity. Other selector names are not AI consumers.
fn activity_can_engage(activity: &BehaviorActivityDescriptor) -> bool {
    if matches!(
        activity.motion,
        Some(MotionVerb::MoveToAnchor | MotionVerb::Patrol)
    ) {
        return false;
    }

    matches!(activity.motion, Some(MotionVerb::ChaseTarget))
        || activity.action.is_some()
        || activity
            .layers
            .iter()
            .any(|(name, layer)| selector_can_engage(name, layer))
}

fn selector_can_engage(name: &str, layer: &BehaviorLayerDescriptor) -> bool {
    match (name, layer) {
        ("move", BehaviorLayerDescriptor::Selector(entries)) => {
            entries.iter().any(|entry| match entry {
                BehaviorSelectorEntry::Row(row) => {
                    matches!(row.motion, Some(MotionVerb::ChaseTarget))
                }
                BehaviorSelectorEntry::Motion(MotionVerb::ChaseTarget) => true,
                BehaviorSelectorEntry::Motion(_) => false,
            })
        }
        ("offense", BehaviorLayerDescriptor::Selector(entries)) => entries
            .iter()
            .any(|entry| matches!(entry, BehaviorSelectorEntry::Row(row) if row.action.is_some())),
        _ => false,
    }
}

fn selector_motion(
    bound: &BrainEntityPrograms,
    scope: &mut BrainScope,
    brain: &BrainComponent,
    depth: usize,
    activity: &BehaviorActivityDescriptor,
) -> Option<MotionVerb> {
    let (entries, programs) = selector_layer(bound, brain, depth, activity, "move")?;
    scope.repoint_scope_relative(ScopeRelativeValues {
        time_in_activity_ms: brain.activity_timer(depth).unwrap_or(0.0),
        attacks_fired_in_activity: brain.activity_attack_count(depth).unwrap_or(0),
    });
    for (entry, program) in entries.iter().zip(programs) {
        match entry {
            BehaviorSelectorEntry::Row(row)
                if program
                    .as_ref()
                    .is_some_and(|program| eval_value(program, scope) == IrValue::Bool(true)) =>
            {
                return row.motion;
            }
            BehaviorSelectorEntry::Motion(motion) => return Some(*motion),
            BehaviorSelectorEntry::Row(_) => {}
        }
    }
    None
}

fn selector_action<'a>(
    bound: &'a BrainEntityPrograms,
    scope: &mut BrainScope,
    brain: &'a BrainComponent,
    depth: usize,
    activity: &'a BehaviorActivityDescriptor,
) -> Option<&'a ActionVerb> {
    let (entries, programs) = selector_layer(bound, brain, depth, activity, "offense")?;
    scope.repoint_scope_relative(ScopeRelativeValues {
        time_in_activity_ms: brain.activity_timer(depth).unwrap_or(0.0),
        attacks_fired_in_activity: brain.activity_attack_count(depth).unwrap_or(0),
    });
    entries
        .iter()
        .zip(programs)
        .find_map(|(entry, program)| match entry {
            BehaviorSelectorEntry::Row(row)
                if row.when.is_none()
                    || program.as_ref().is_some_and(|program| {
                        eval_value(program, scope) == IrValue::Bool(true)
                    }) =>
            {
                row.action.as_ref()
            }
            BehaviorSelectorEntry::Row(_) | BehaviorSelectorEntry::Motion(_) => None,
        })
}

fn selector_layer<'a>(
    bound: &'a BrainEntityPrograms,
    brain: &BrainComponent,
    depth: usize,
    activity: &'a BehaviorActivityDescriptor,
    name: &str,
) -> Option<SelectorLayer<'a>> {
    let mut envelope_index = 0;
    for prior_depth in 0..depth {
        let active_index = brain.active_activity_index(prior_depth)?;
        let bound_activity = bound
            .envelope(envelope_index)?
            .activities
            .get(active_index)?;
        envelope_index = bound_activity.layers.iter().find_map(|layer| match layer {
            BoundLayer::Graph(index) => Some(*index),
            BoundLayer::Selector(_) => None,
        })?;
    }
    let active_index = brain.active_activity_index(depth)?;
    let bound_activity = bound
        .envelope(envelope_index)?
        .activities
        .get(active_index)?;
    activity
        .layers
        .iter()
        .zip(&bound_activity.layers)
        .find_map(|((layer_name, layer), program)| (layer_name == name).then_some((layer, program)))
        .and_then(|(layer, program)| match (layer, program) {
            (BehaviorLayerDescriptor::Selector(entries), BoundLayer::Selector(programs)) => {
                Some((entries.as_slice(), programs.as_slice()))
            }
            _ => None,
        })
}

/// Resolve the one mesh animation state driven by the active path. A nested
/// phase's clip wins even when that phase has no action (windup/recover), then
/// the active composite supplies locomotion, and a root leaf supplies the
/// degenerate one-level fallback.
pub(super) fn animation_for_path(brain: &BrainComponent, moving: bool) -> Option<&str> {
    if brain.active_depth() > 1 {
        let (_, leaf) = brain.activity_at_depth(brain.active_depth() - 1)?;
        if leaf.animation.is_some() {
            return leaf.animation.as_deref();
        }
    }
    for depth in (0..brain.active_depth()).rev() {
        let (_, activity) = brain.activity_at_depth(depth)?;
        if !activity.layers.is_empty() && activity.animation.is_some() {
            return activity.animation.as_deref();
        }
    }
    let (_, leaf) = brain.activity_at_depth(brain.active_depth().checked_sub(1)?)?;
    if !moving && is_locomotion_activity(leaf) {
        return rest_animation(&brain.graph);
    }
    leaf.animation.as_deref()
}

fn is_locomotion_activity(activity: &BehaviorActivityDescriptor) -> bool {
    matches!(activity.motion, Some(MotionVerb::ChaseTarget)) && activity.action.is_none()
        || matches!(
            activity.motion,
            Some(MotionVerb::MoveToAnchor | MotionVerb::Patrol)
        )
}

pub(super) fn steering_for(motion: MotionVerb) -> SteeringIntent {
    match motion {
        MotionVerb::ChaseTarget => SteeringIntent::Chase,
        MotionVerb::MoveToAnchor | MotionVerb::Patrol | MotionVerb::Hold => SteeringIntent::Clear,
        MotionVerb::Freeze => SteeringIntent::Hold,
    }
}

pub(crate) fn locomotion_animation(graph: &BehaviorGraphDescriptor) -> Option<&str> {
    graph
        .envelope
        .activities
        .values()
        .find(|activity| {
            activity.animation.is_some()
                && (is_locomotion_activity(activity)
                    || matches!(
                        activity.layers.get("move"),
                        Some(BehaviorLayerDescriptor::Selector(_))
                    ))
        })
        .and_then(|activity| activity.animation.as_deref())
}

pub(crate) fn rest_animation(graph: &BehaviorGraphDescriptor) -> Option<&str> {
    graph
        .envelope
        .activities
        .get(&graph.envelope.initial)
        .and_then(|activity| activity.animation.as_deref())
}

#[cfg(test)]
mod statechart_tests {
    use std::collections::{BTreeMap, HashSet};
    use std::sync::Arc;

    use postretro_entities::components::brain::BrainComponent;
    use postretro_foundation::{
        ActionVerb, AttackParams, BRAIN_TIME_IN_ACTIVITY_MS_INPUT, BehaviorGraphEnvelope,
        BehaviorSelectorRow, GuardedRow, IrNode, IrValue,
    };

    use super::*;
    use crate::alloc_probe::AllocSnapshot;
    use crate::scripting_systems::ai::brain_programs::bind_graph;
    use crate::scripting_systems::ai::candidate_scope::CandidateScope;

    fn activity(
        animation: &str,
        motion: Option<MotionVerb>,
        action: Option<ActionVerb>,
        layers: BTreeMap<String, BehaviorLayerDescriptor>,
    ) -> BehaviorActivityDescriptor {
        BehaviorActivityDescriptor {
            animation: Some(animation.to_string()),
            motion,
            action,
            on_enter: None,
            layers,
        }
    }

    fn constant(value: bool) -> IrNode {
        IrNode::Const {
            value: IrValue::Bool(value),
        }
    }

    fn elapsed_at_least(milliseconds: f32) -> IrNode {
        IrNode::Ge {
            a: Box::new(IrNode::Input {
                name: BRAIN_TIME_IN_ACTIVITY_MS_INPUT.to_string(),
                owner: None,
            }),
            b: Box::new(IrNode::Const {
                value: IrValue::Number(milliseconds),
            }),
        }
    }

    fn nested_fixture(outer_escape: bool) -> BehaviorGraphDescriptor {
        let offense = BehaviorGraphEnvelope {
            initial: "windup".to_string(),
            activities: BTreeMap::from([
                (
                    "windup".to_string(),
                    activity("windup", None, None, BTreeMap::new()),
                ),
                (
                    "commit".to_string(),
                    activity(
                        "commit",
                        None,
                        Some(ActionVerb::Attack("slam".to_string())),
                        BTreeMap::new(),
                    ),
                ),
                (
                    "recover".to_string(),
                    activity("recover", None, None, BTreeMap::new()),
                ),
            ]),
            transitions: BTreeMap::from([
                (
                    "windup".to_string(),
                    vec![GuardedRow {
                        to: "commit".to_string(),
                        when: elapsed_at_least(10.0),
                    }],
                ),
                (
                    "commit".to_string(),
                    vec![GuardedRow {
                        to: "recover".to_string(),
                        when: elapsed_at_least(10.0),
                    }],
                ),
            ]),
        };
        let move_selector = BehaviorLayerDescriptor::Selector(vec![
            BehaviorSelectorEntry::Row(BehaviorSelectorRow {
                when: Some(IrNode::Not {
                    x: Box::new(constant(false)),
                }),
                motion: Some(MotionVerb::ChaseTarget),
                action: None,
            }),
            BehaviorSelectorEntry::Motion(MotionVerb::Hold),
        ]);
        BehaviorGraphDescriptor {
            envelope: BehaviorGraphEnvelope {
                initial: "engage".to_string(),
                activities: BTreeMap::from([
                    (
                        "engage".to_string(),
                        activity(
                            "locomotion",
                            None,
                            None,
                            BTreeMap::from([
                                ("move".to_string(), move_selector),
                                (
                                    "offense".to_string(),
                                    BehaviorLayerDescriptor::Graph(offense),
                                ),
                            ]),
                        ),
                    ),
                    (
                        "rest".to_string(),
                        activity("idle", Some(MotionVerb::Hold), None, BTreeMap::new()),
                    ),
                ]),
                transitions: outer_escape
                    .then(|| {
                        BTreeMap::from([(
                            "*".to_string(),
                            vec![GuardedRow {
                                to: "rest".to_string(),
                                when: constant(true),
                            }],
                        )])
                    })
                    .unwrap_or_default(),
            },
            candidate_filter: None,
            patrol: None,
            attacks: BTreeMap::from([(
                "slam".to_string(),
                AttackParams {
                    weapon: None,
                    damage: Some(8.0),
                    max_range: Some(2.0),
                    cooldown_ms: Some(100.0),
                    engagement_radius: None,
                    standoff_distance: None,
                },
            )]),
            engagement_radius: None,
            move_speed: 3.5,
        }
    }

    /// Exercise the production descriptor parser before binding the synthetic
    /// statechart. The direct `outer_escape` variant above remains useful for
    /// the one focused ordering branch below; this parsed fixture carries the
    /// complete windup → commit → recover path used by the end-to-end checks.
    fn parsed_nested_fixture() -> BehaviorGraphDescriptor {
        const SOURCE: &str = r#"({ components: { behavior: {
            initial: "engage",
            moveSpeed: 3.5,
            attacks: { slam: { damage: 8, maxRange: 2, cooldownMs: 100 } },
            activities: {
                engage: {
                    animation: "locomotion",
                    layers: {
                        move: [
                            { when: { op: "not", x: { op: "const", value: false } }, motion: "chaseTarget" },
                            "hold"
                        ],
                        offense: {
                            initial: "windup",
                            activities: {
                                windup: { animation: "windup" },
                                commit: { animation: "commit", action: { attack: "slam" } },
                                recover: { animation: "recover" }
                            },
                            transitions: {
                                windup: [{ when: { op: "ge", a: { op: "input", name: "@brain.timeInActivityMs" }, b: { op: "const", value: 10 } }, to: "commit" }],
                                commit: [{ when: { op: "ge", a: { op: "input", name: "@brain.timeInActivityMs" }, b: { op: "const", value: 10 } }, to: "recover" }]
                            }
                        }
                    }
                },
                rest: { animation: "idle", motion: "hold" }
            },
            transitions: {}
        } } })"#;

        let runtime = rquickjs::Runtime::new().expect("synthetic JS runtime");
        let context = rquickjs::Context::full(&runtime).expect("synthetic JS context");
        context.with(|ctx| {
            let value = ctx
                .eval(SOURCE)
                .expect("synthetic nested behavior source evaluates");
            postretro_scripting_core::data_descriptors::entity_descriptor_from_js(&ctx, value)
                .expect("synthetic nested behavior parses")
                .behavior
                .expect("synthetic entity carries a behavior graph")
        })
    }

    fn bound(graph: &BehaviorGraphDescriptor) -> (BrainEntityPrograms, BrainScope) {
        let scope = BrainScope::for_validation();
        let candidate_scope = CandidateScope::for_validation();
        let programs = bind_graph(
            &scope,
            &candidate_scope,
            Arc::new(graph.clone()),
            &mut HashSet::new(),
        );
        (programs, scope)
    }

    #[test]
    fn parsed_nested_fixture_covers_entry_timers_order_selectors_and_animation_without_tick_allocations()
     {
        let graph = parsed_nested_fixture();
        let (programs, mut scope) = bound(&graph);
        let mut brain = BrainComponent::from_graph(&graph);

        assert_eq!(
            brain.active_path_names(),
            ["engage", "windup"],
            "AC7: initial composite entry fully descends"
        );
        assert_eq!(
            brain.activity_timer(0),
            Some(0.0),
            "AC9: parent timer resets before entry work"
        );
        assert_eq!(
            brain.activity_timer(1),
            Some(0.0),
            "AC9: descendant timer resets before entry work"
        );
        assert_eq!(
            animation_for_path(&brain, true),
            Some("windup"),
            "AC12: a non-action offense windup drives the replicated animation"
        );
        assert_eq!(
            brain.take_entry_pending(),
            Some(0),
            "AC7: initial descent has one entry edge"
        );
        assert_eq!(
            motion_for_path(&programs, &mut scope, &brain),
            Some(MotionVerb::ChaseTarget),
            "AC17: move selector evaluates `not` and wins before fallback"
        );

        let snapshot = AllocSnapshot::arm();
        assert!(
            !select_transition_path(&programs, &mut scope, &mut brain),
            "AC7: windup holds before its elapsed-time window"
        );
        assert_eq!(
            snapshot.allocs_since(),
            0,
            "AC13: nested transition evaluation allocates nothing"
        );

        brain.time_in_activity_ms[0] = 20.0;
        brain.time_in_activity_ms[1] = 10.0;
        let snapshot = AllocSnapshot::arm();
        assert!(
            select_transition_path(&programs, &mut scope, &mut brain),
            "AC8: child guard sees its own elapsed timer"
        );
        assert_eq!(
            snapshot.allocs_since(),
            0,
            "AC13: per-level timer re-pointing allocates nothing"
        );
        assert_eq!(
            brain.active_path_names(),
            ["engage", "commit"],
            "AC7: one transition seats commit, not recover, this tick"
        );
        assert_eq!(
            brain.activity_timer(0),
            Some(20.0),
            "AC8: parent timer survives inner transition"
        );
        assert_eq!(
            brain.activity_timer(1),
            Some(0.0),
            "AC9: entered child timer resets atomically"
        );
        assert_eq!(
            brain.take_entry_pending(),
            Some(1),
            "AC7: commit produces exactly one attack-entry edge"
        );
        assert_eq!(
            action_for_path(&programs, &mut scope, &brain),
            Some(&ActionVerb::Attack("slam".to_string())),
            "AC7/AC17: the active offense leaf supplies the one edge-fired action"
        );
        let snapshot = AllocSnapshot::arm();
        assert_eq!(
            animation_for_path(&brain, true),
            Some("commit"),
            "AC12: offense leaf collapses to one animation name"
        );
        assert_eq!(
            snapshot.allocs_since(),
            0,
            "AC13: animation collapse borrows the graph without allocating"
        );

        assert!(
            !select_transition_path(&programs, &mut scope, &mut brain),
            "AC7: an entered phase is first evaluated on the following call and holds at zero"
        );
        brain.time_in_activity_ms[1] = 10.0;
        assert!(select_transition_path(&programs, &mut scope, &mut brain));
        assert_eq!(
            brain.active_path_names(),
            ["engage", "recover"],
            "AC7: commit exits only after its own window"
        );
        assert_eq!(
            animation_for_path(&brain, true),
            Some("recover"),
            "AC12: a non-action offense recovery drives the replicated animation"
        );

        let escape_graph = nested_fixture(true);
        let (escape_programs, mut escape_scope) = bound(&escape_graph);
        let mut escaping = BrainComponent::from_graph(&escape_graph);
        assert!(select_transition_path(
            &escape_programs,
            &mut escape_scope,
            &mut escaping
        ));
        assert_eq!(
            escaping.active_path_names(),
            ["rest"],
            "AC10: outer wildcard preempts the inner phase"
        );
    }

    #[test]
    fn arbitrary_selector_layers_are_not_ai_motion_or_action_consumers() {
        let custom = activity(
            "idle",
            None,
            None,
            BTreeMap::from([(
                "presentation".to_string(),
                BehaviorLayerDescriptor::Selector(vec![BehaviorSelectorEntry::Row(
                    BehaviorSelectorRow {
                        when: None,
                        motion: Some(MotionVerb::ChaseTarget),
                        action: Some(ActionVerb::Attack("slam".to_string())),
                    },
                )]),
            )]),
        );

        assert!(
            !activity_can_engage(&custom),
            "only selectors named move/offense feed AI engagement semantics"
        );
    }
}
