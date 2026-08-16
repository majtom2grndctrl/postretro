// Parse-time advisory diagnostics for authored behavior graphs.
// See: context/lib/entity_model.md §7c (brain lifecycle) · context/lib/scripting.md §11 (guard IR)

use crate::brain::BRAIN_HAS_TARGET_INPUT;
use crate::data_descriptors::types::behavior::{
    BehaviorGraphDescriptor, BehaviorStateDescriptor, MotionVerb,
};

/// An advisory finding about a graph's ability to stand down after engagement.
///
/// These findings are deliberately not descriptor validation errors: a graph
/// that pursues forever may be an intentional authored behavior.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BehaviorLint {
    pub kind: BehaviorLintKind,
    /// Engaging states relevant to this finding, in `states` map-key order.
    pub states: Vec<String>,
}

/// The closed vocabulary of behavior-graph advisory findings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BehaviorLintKind {
    /// No engaging state has a state-local transition to a non-engaging state.
    LevelWidePursuer,
    /// No graph interrupt reads the `@brain.hasTarget` fact.
    NoHasTargetInterrupt,
}

/// Inspect an authored graph for likely missing disengagement behavior.
///
/// This is a descriptor-time analysis only. Callers run it after structural
/// validation, so state-local targets are known to resolve; interrupts do not
/// satisfy the `LevelWidePursuer` escape condition because they are a separate
/// authoring mechanism from state-local transitions.
pub fn inspect(graph: &BehaviorGraphDescriptor) -> Vec<BehaviorLint> {
    let engaging_states = graph
        .states
        .iter()
        .filter(|(_, state)| is_engaging(state))
        .map(|(name, _)| name.clone())
        .collect::<Vec<_>>();
    if engaging_states.is_empty() {
        return Vec::new();
    }

    let has_state_local_disengagement = graph.states.iter().any(|(_, state)| {
        is_engaging(state)
            && state.transitions.iter().any(|transition| {
                graph
                    .states
                    .get(&transition.to)
                    .is_some_and(|target| !is_engaging(target))
            })
    });
    let has_target_interrupt = graph.interrupts.iter().any(|interrupt| {
        interrupt
            .when
            .dispatch_input_names()
            .iter()
            .any(|name| name == BRAIN_HAS_TARGET_INPUT)
    });

    let mut lints = Vec::new();
    if !has_state_local_disengagement {
        lints.push(BehaviorLint {
            kind: BehaviorLintKind::LevelWidePursuer,
            states: engaging_states.clone(),
        });
    }
    if !has_target_interrupt {
        lints.push(BehaviorLint {
            kind: BehaviorLintKind::NoHasTargetInterrupt,
            states: engaging_states,
        });
    }
    lints
}

fn is_engaging(state: &BehaviorStateDescriptor) -> bool {
    state.motion == MotionVerb::ChaseTarget || state.action.is_some()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::data_descriptors::types::behavior::{ActionVerb, TransitionDescriptor};
    use crate::ir::{IrNode, IrValue};

    fn always() -> IrNode {
        IrNode::Const {
            value: IrValue::Bool(true),
        }
    }

    fn has_target() -> IrNode {
        IrNode::Select {
            cond: Box::new(IrNode::Input {
                name: BRAIN_HAS_TARGET_INPUT.to_string(),
                owner: None,
            }),
            a: Box::new(always()),
            b: Box::new(IrNode::Const {
                value: IrValue::Bool(false),
            }),
        }
    }

    fn state(motion: MotionVerb, action: Option<ActionVerb>) -> BehaviorStateDescriptor {
        BehaviorStateDescriptor {
            animation: "state".to_string(),
            motion,
            action,
            transitions: Vec::new(),
            on_enter: None,
        }
    }

    fn graph(states: BTreeMap<String, BehaviorStateDescriptor>) -> BehaviorGraphDescriptor {
        BehaviorGraphDescriptor {
            initial: "idle".to_string(),
            states,
            interrupts: Vec::new(),
            candidate_filter: None,
            patrol: None,
            attack: None,
            engagement_radius: None,
            move_speed: 3.0,
        }
    }

    #[test]
    fn reports_level_wide_pursuer_in_btree_state_order() {
        let mut graph = graph(BTreeMap::from([
            ("zeta".to_string(), state(MotionVerb::Hold, None)),
            (
                "attack".to_string(),
                state(MotionVerb::Hold, Some(ActionVerb::Attack)),
            ),
            ("chase".to_string(), state(MotionVerb::ChaseTarget, None)),
        ]));
        graph.states.get_mut("attack").unwrap().transitions = vec![TransitionDescriptor {
            to: "chase".to_string(),
            when: always(),
        }];
        graph.states.get_mut("chase").unwrap().transitions = vec![TransitionDescriptor {
            to: "attack".to_string(),
            when: always(),
        }];
        graph.interrupts = vec![TransitionDescriptor {
            to: "zeta".to_string(),
            when: has_target(),
        }];

        assert_eq!(
            inspect(&graph),
            vec![BehaviorLint {
                kind: BehaviorLintKind::LevelWidePursuer,
                states: vec!["attack".to_string(), "chase".to_string()],
            }],
            "an interrupt never substitutes for a state-local disengagement edge"
        );
    }

    #[test]
    fn reports_missing_has_target_interrupt() {
        let mut graph = graph(BTreeMap::from([
            ("idle".to_string(), state(MotionVerb::Hold, None)),
            ("chase".to_string(), state(MotionVerb::ChaseTarget, None)),
        ]));
        graph.states.get_mut("chase").unwrap().transitions = vec![TransitionDescriptor {
            to: "idle".to_string(),
            when: always(),
        }];

        assert_eq!(
            inspect(&graph),
            vec![BehaviorLint {
                kind: BehaviorLintKind::NoHasTargetInterrupt,
                states: vec!["chase".to_string()],
            }]
        );
    }

    #[test]
    fn no_lint_is_reported_when_both_disengagement_paths_are_authored() {
        let mut graph = graph(BTreeMap::from([
            ("idle".to_string(), state(MotionVerb::Hold, None)),
            ("chase".to_string(), state(MotionVerb::ChaseTarget, None)),
        ]));
        graph.states.get_mut("chase").unwrap().transitions = vec![TransitionDescriptor {
            to: "idle".to_string(),
            when: always(),
        }];
        graph.interrupts = vec![TransitionDescriptor {
            to: "idle".to_string(),
            when: has_target(),
        }];

        assert!(inspect(&graph).is_empty());
    }
}
