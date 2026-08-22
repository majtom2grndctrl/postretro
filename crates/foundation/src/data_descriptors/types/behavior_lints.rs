// Parse-time advisory diagnostics for authored behavior statecharts.

use crate::data_descriptors::types::behavior::{
    BehaviorGraphDescriptor, BehaviorLayerDescriptor, unreachable_activities,
};

/// An advisory finding about an envelope's declared activities.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BehaviorLint {
    pub kind: BehaviorLintKind,
    /// The wire-cased envelope path containing the reported activities.
    pub envelope_path: String,
    /// Activity names in deterministic map-key order.
    pub activities: Vec<String>,
}

/// The closed vocabulary of behavior-statechart advisory findings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BehaviorLintKind {
    /// Activities have no incoming row and are not the envelope's initial one.
    UnreachableActivity,
}

/// Inspect every recursive envelope for activities that cannot be entered from
/// the envelope's declared `initial` or any of its transition rows.
///
/// This deliberately is not a transitive graph-reachability proof. An author
/// may be staging an unreachable source row, but any activity referenced by a
/// row is still declared reachable in the lint's simple authoring sense.
pub fn inspect(graph: &BehaviorGraphDescriptor) -> Vec<BehaviorLint> {
    let mut lints = Vec::new();
    inspect_envelope(&graph.envelope, "components.behavior", &mut lints);
    lints
}

fn inspect_envelope(
    envelope: &crate::data_descriptors::types::behavior::BehaviorGraphEnvelope,
    path: &str,
    lints: &mut Vec<BehaviorLint>,
) {
    let activities = unreachable_activities(envelope);
    if !activities.is_empty() {
        lints.push(BehaviorLint {
            kind: BehaviorLintKind::UnreachableActivity,
            envelope_path: path.to_string(),
            activities,
        });
    }
    for (activity_name, activity) in &envelope.activities {
        for (layer_name, layer) in &activity.layers {
            if let BehaviorLayerDescriptor::Graph(nested) = layer {
                inspect_envelope(
                    nested,
                    &format!("{path}.activities.{activity_name}.layers.{layer_name}"),
                    lints,
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::data_descriptors::types::behavior::{
        BehaviorActivityDescriptor, BehaviorGraphEnvelope, GuardedRow,
    };
    use crate::ir::{IrNode, IrValue};

    fn leaf() -> BehaviorActivityDescriptor {
        BehaviorActivityDescriptor {
            animation: Some("idle".to_string()),
            motion: None,
            action: None,
            on_enter: None,
            layers: BTreeMap::new(),
        }
    }

    fn graph() -> BehaviorGraphDescriptor {
        BehaviorGraphDescriptor {
            envelope: BehaviorGraphEnvelope {
                initial: "initial".to_string(),
                activities: BTreeMap::from([
                    ("initial".to_string(), leaf()),
                    ("reachable".to_string(), leaf()),
                    ("orphan".to_string(), leaf()),
                ]),
                transitions: BTreeMap::from([(
                    "initial".to_string(),
                    vec![GuardedRow {
                        when: IrNode::Const {
                            value: IrValue::Bool(true),
                        },
                        to: "reachable".to_string(),
                    }],
                )]),
            },
            candidate_filter: None,
            patrol: None,
            attacks: BTreeMap::new(),
            engagement_radius: None,
            move_speed: 3.0,
        }
    }

    #[test]
    fn reports_an_activity_without_initial_or_incoming_row() {
        assert_eq!(
            inspect(&graph()),
            vec![BehaviorLint {
                kind: BehaviorLintKind::UnreachableActivity,
                envelope_path: "components.behavior".to_string(),
                activities: vec!["orphan".to_string()],
            }]
        );
    }
}
