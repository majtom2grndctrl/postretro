//! Load-time checks for `attackTurn` marks: a mark names one of the graph's
//! attacks, and marks on one active path never disagree.
//! See: context/lib/entity_model.md §7c (Attack budget, Turns).

use std::collections::BTreeMap;

use super::{
    ActionVerb, AttackParams, BehaviorActivityDescriptor, BehaviorGraphEnvelope,
    BehaviorLayerDescriptor, BehaviorSelectorEntry,
};
use crate::data_descriptors::DescriptorError;

/// The nearest enclosing mark: its attack and the authored path naming it.
struct Mark<'a> {
    attack: &'a str,
    path: String,
}

/// Walk every envelope, carrying the nearest mark down each path. A mark
/// nested in a mark for another attack, or an action under a mark that fires
/// another attack, rejects: one turn is held across a marked chain, so the
/// chain may fire only the attack it holds the turn for.
pub(super) fn validate_attack_turn_marks(
    envelope: &BehaviorGraphEnvelope,
    path: &str,
    attacks: &BTreeMap<String, AttackParams>,
) -> Result<(), DescriptorError> {
    walk_envelope(envelope, path, None, attacks)
}

fn walk_envelope(
    envelope: &BehaviorGraphEnvelope,
    path: &str,
    enclosing: Option<&Mark<'_>>,
    attacks: &BTreeMap<String, AttackParams>,
) -> Result<(), DescriptorError> {
    for (name, activity) in &envelope.activities {
        walk_activity(
            activity,
            &format!("{path}.activities.{name}"),
            enclosing,
            attacks,
        )?;
    }
    Ok(())
}

fn walk_activity(
    activity: &BehaviorActivityDescriptor,
    path: &str,
    enclosing: Option<&Mark<'_>>,
    attacks: &BTreeMap<String, AttackParams>,
) -> Result<(), DescriptorError> {
    let own_mark;
    let mark = match activity.attack_turn.as_deref() {
        Some(attack) => {
            let mark_path = format!("{path}.attackTurn");
            validate_marked_attack(attack, &mark_path, attacks)?;
            if let Some(outer) = enclosing
                && outer.attack != attack
            {
                return Err(DescriptorError::InvalidShape {
                    reason: format!(
                        "`{mark_path}` marks \"{attack}\" inside `{}`, which marks \"{}\"; a mark cannot nest inside a mark for another attack",
                        outer.path, outer.attack
                    ),
                });
            }
            own_mark = Mark {
                attack,
                path: mark_path,
            };
            Some(&own_mark)
        }
        None => enclosing,
    };
    let Some(mark) = mark else {
        // Unmarked subtrees may still hold marks deeper down.
        return walk_layers(activity, path, None, attacks);
    };

    if let Some(action) = activity.action.as_ref() {
        reject_foreign_action(action, &format!("{path}.action"), mark)?;
    }
    walk_layers(activity, path, Some(mark), attacks)
}

fn walk_layers(
    activity: &BehaviorActivityDescriptor,
    path: &str,
    mark: Option<&Mark<'_>>,
    attacks: &BTreeMap<String, AttackParams>,
) -> Result<(), DescriptorError> {
    for (layer_name, layer) in &activity.layers {
        let layer_path = format!("{path}.layers.{layer_name}");
        match layer {
            BehaviorLayerDescriptor::Graph(envelope) => {
                walk_envelope(envelope, &layer_path, mark, attacks)?;
            }
            BehaviorLayerDescriptor::Selector(entries) => {
                let Some(mark) = mark else { continue };
                for (index, entry) in entries.iter().enumerate() {
                    if let BehaviorSelectorEntry::Row(row) = entry
                        && let Some(action) = row.action.as_ref()
                    {
                        reject_foreign_action(
                            action,
                            &format!("{layer_path}[{index}].action"),
                            mark,
                        )?;
                    }
                }
            }
        }
    }
    Ok(())
}

fn reject_foreign_action(
    action: &ActionVerb,
    path: &str,
    mark: &Mark<'_>,
) -> Result<(), DescriptorError> {
    let ActionVerb::Attack(fired) = action;
    if fired != mark.attack {
        return Err(DescriptorError::InvalidShape {
            reason: format!(
                "`{path}.attack` fires \"{fired}\" under `{}`, which marks \"{}\"; an action under a mark must fire the marked attack",
                mark.path, mark.attack
            ),
        });
    }
    Ok(())
}

fn validate_marked_attack(
    attack: &str,
    path: &str,
    attacks: &BTreeMap<String, AttackParams>,
) -> Result<(), DescriptorError> {
    if attacks.contains_key(attack) {
        return Ok(());
    }
    Err(DescriptorError::InvalidShape {
        reason: format!(
            "`{path}` names \"{attack}\", which is not declared in `components.behavior.attacks`"
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::super::BehaviorGraphDescriptor;

    fn graph(activities: serde_json::Value) -> BehaviorGraphDescriptor {
        serde_json::from_value(serde_json::json!({
            "initial": "engage",
            "moveSpeed": 3.0,
            "attacks": {
                "jab": { "damage": 4, "maxRange": 2, "cooldownMs": 500 },
                "slam": { "damage": 9, "maxRange": 3, "cooldownMs": 1500 }
            },
            "activities": activities,
            "transitions": {}
        }))
        .expect("fixture deserializes")
    }

    fn error(activities: serde_json::Value) -> String {
        graph(activities)
            .validate()
            .expect_err("disagreeing marks reject")
            .to_string()
    }

    #[test]
    fn a_marked_chain_firing_its_own_attack_validates() {
        let graph = graph(serde_json::json!({
            "engage": { "attackTurn": "slam", "layers": { "offense": {
                "initial": "windup",
                "activities": {
                    "windup": { "animation": "windup" },
                    "commit": { "animation": "slam", "action": { "attack": "slam" }, "attackTurn": "slam" }
                },
                "transitions": {}
            } } }
        }))
        .validate()
        .expect("a chain marked for one attack validates");
        let engage = &graph.envelope.activities["engage"];
        assert_eq!(engage.attack_turn.as_deref(), Some("slam"));
    }

    #[test]
    fn an_undeclared_marked_attack_rejects_with_its_path() {
        let error = error(serde_json::json!({
            "engage": { "animation": "idle", "attackTurn": "kick" }
        }));
        assert!(
            error.contains("components.behavior.activities.engage.attackTurn"),
            "{error}"
        );
        assert!(error.contains("\"kick\""), "{error}");
    }

    #[test]
    fn a_mark_nested_inside_a_mark_for_another_attack_rejects() {
        let error = error(serde_json::json!({
            "engage": { "attackTurn": "slam", "layers": { "offense": {
                "initial": "jab",
                "activities": { "jab": { "animation": "jab", "attackTurn": "jab" } },
                "transitions": {}
            } } }
        }));
        assert!(
            error.contains(
                "components.behavior.activities.engage.layers.offense.activities.jab.attackTurn"
            ),
            "{error}"
        );
        assert!(
            error.contains("components.behavior.activities.engage.attackTurn"),
            "{error}"
        );
    }

    #[test]
    fn an_action_under_a_mark_firing_another_attack_rejects() {
        for (activities, action_path) in [
            (
                serde_json::json!({
                    "engage": { "animation": "jab", "attackTurn": "slam", "action": { "attack": "jab" } }
                }),
                "components.behavior.activities.engage.action.attack",
            ),
            (
                serde_json::json!({
                    "engage": { "attackTurn": "slam", "layers": {
                        "offense": [{ "when": { "op": "const", "value": true }, "action": { "attack": "jab" } }]
                    } }
                }),
                "components.behavior.activities.engage.layers.offense[0].action.attack",
            ),
            (
                serde_json::json!({
                    "engage": { "attackTurn": "slam", "layers": { "offense": {
                        "initial": "hit",
                        "activities": { "hit": { "animation": "jab", "action": { "attack": "jab" } } },
                        "transitions": {}
                    } } }
                }),
                "components.behavior.activities.engage.layers.offense.activities.hit.action.attack",
            ),
        ] {
            let error = error(activities);
            assert!(error.contains(action_path), "{error}");
            assert!(
                error.contains("components.behavior.activities.engage.attackTurn"),
                "{error}"
            );
        }
    }

    #[test]
    fn unmarked_siblings_may_fire_other_attacks() {
        graph(serde_json::json!({
            "engage": { "layers": { "offense": {
                "initial": "slam",
                "activities": {
                    "slam": { "animation": "slam", "action": { "attack": "slam" }, "attackTurn": "slam" },
                    "jab": { "animation": "jab", "action": { "attack": "jab" }, "attackTurn": "jab" },
                    "free": { "animation": "jab", "action": { "attack": "jab" } }
                },
                "transitions": {}
            } } }
        }))
        .validate()
        .expect("sibling marks for different attacks never share a path");
    }
}
