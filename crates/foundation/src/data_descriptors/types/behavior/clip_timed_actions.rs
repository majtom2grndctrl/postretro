//! Load-time checks for `action.at`, an attack fired at a clip event, and the
//! turn mark it implies.
//! See: context/lib/entity_model.md §7c (Clip-timed attacks).

use super::{ActionVerb, BehaviorActivityDescriptor};
use crate::clip_events::is_clip_event_name;
use crate::data_descriptors::DescriptorError;

/// `at` must name an event a clip could carry, so a typo rejects at load
/// instead of never matching.
pub(super) fn validate_at_name(action: &ActionVerb, path: &str) -> Result<(), DescriptorError> {
    let ActionVerb::Attack { at, .. } = action;
    match at.as_deref() {
        Some(event) if !is_clip_event_name(event) => Err(DescriptorError::InvalidShape {
            reason: format!(
                "`{path}.at` (\"{event}\") must name a clip event matching `[A-Za-z_][A-Za-z0-9_]*`"
            ),
        }),
        _ => Ok(()),
    }
}

/// A selector row selects no animation, so it has no clip to time an attack
/// by, and its implied mark would span its owner's whole life.
pub(super) fn reject_row_at(action: &ActionVerb, path: &str) -> Result<(), DescriptorError> {
    let ActionVerb::Attack { at, .. } = action;
    if at.is_some() {
        return Err(DescriptorError::InvalidShape {
            reason: format!(
                "`{path}.at` belongs to an activity's own action; a selector row selects no animation"
            ),
        });
    }
    Ok(())
}

/// An activity whose own action fires `at` a clip event holds that attack's
/// turn from entry, as if marked `attackTurn` for it: otherwise its turn would
/// be requested only at the moment, after the swing has played. Writing the
/// mark here, before mark validation, lets the implied mark join the ordinary
/// mark-agreement rules. An explicit mark for the same attack is the same
/// mark, so a validated descriptor re-parses unchanged.
pub(super) fn imply_attack_turn(
    activity: &mut BehaviorActivityDescriptor,
    path: &str,
) -> Result<(), DescriptorError> {
    let Some(ActionVerb::Attack { name, at: Some(_) }) = activity.action.as_ref() else {
        return Ok(());
    };
    match activity.attack_turn.as_deref() {
        None => activity.attack_turn = Some(name.clone()),
        Some(mark) if mark == name => {}
        Some(mark) => {
            return Err(DescriptorError::InvalidShape {
                reason: format!(
                    "`{path}.attackTurn` marks \"{mark}\", but `{path}.action.at` fires \"{name}\" and so holds \"{name}\"'s turn; an activity's marks must agree"
                ),
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::BehaviorGraphDescriptor;
    use super::*;

    fn graph(activities: serde_json::Value) -> serde_json::Value {
        serde_json::json!({
            "initial": "stalk",
            "moveSpeed": 3.0,
            "attacks": {
                "jab": { "damage": 4, "maxRange": 2, "cooldownMs": 500 },
                "slam": { "damage": 9, "maxRange": 3, "cooldownMs": 1500 }
            },
            "activities": activities,
            "transitions": {}
        })
    }

    fn load(activities: serde_json::Value) -> Result<BehaviorGraphDescriptor, String> {
        serde_json::from_value::<BehaviorGraphDescriptor>(graph(activities))
            .map_err(|error| error.to_string())?
            .validate()
            .map_err(|error| error.to_string())
    }

    fn rejection(activities: serde_json::Value) -> String {
        load(activities).expect_err("the descriptor rejects")
    }

    fn jab_at_hit() -> serde_json::Value {
        serde_json::json!({
            "stalk": { "animation": "walk" },
            "jab": { "animation": "attack_jab", "action": { "attack": "jab", "at": "hit" } }
        })
    }

    #[test]
    fn action_at_parses_and_round_trips_through_its_authored_shape() {
        let graph = load(jab_at_hit()).expect("`at` on an activity's own action loads");
        let jab = &graph.envelope.activities["jab"];
        assert_eq!(
            jab.action,
            Some(ActionVerb::Attack {
                name: "jab".to_string(),
                at: Some("hit".to_string()),
            })
        );

        let json = serde_json::to_value(&graph).expect("the descriptor serializes");
        assert_eq!(
            json["activities"]["jab"]["action"],
            serde_json::json!({ "attack": "jab", "at": "hit" })
        );
        assert_eq!(
            json["activities"]["stalk"]["action"],
            serde_json::Value::Null,
            "an activity without an action serializes none"
        );
        let reparsed = serde_json::from_value::<BehaviorGraphDescriptor>(json)
            .expect("the serialized descriptor parses")
            .validate()
            .expect("the serialized descriptor, its implied mark now explicit, loads");
        assert_eq!(reparsed, graph);
    }

    #[test]
    fn action_without_at_serializes_as_the_bare_attack_shape() {
        let action = ActionVerb::Attack {
            name: "jab".to_string(),
            at: None,
        };
        assert_eq!(
            serde_json::to_value(&action).unwrap(),
            serde_json::json!({ "attack": "jab" })
        );
    }

    #[test]
    fn action_with_an_unknown_key_rejects() {
        let error = serde_json::from_value::<ActionVerb>(
            serde_json::json!({ "attack": "jab", "when": "hit" }),
        )
        .expect_err("unknown action keys reject");
        assert!(error.to_string().contains("when"), "{error}");
    }

    // Fire 19: the implied mark is the activity's own `attackTurn` after load.
    #[test]
    fn action_at_implies_its_attacks_turn_mark_after_load() {
        let graph = load(jab_at_hit()).expect("loads");
        assert_eq!(
            graph.envelope.activities["jab"].attack_turn.as_deref(),
            Some("jab")
        );
        assert_eq!(
            graph.envelope.activities["stalk"].attack_turn, None,
            "an activity without `at` gains no mark"
        );

        let untimed = load(serde_json::json!({
            "stalk": { "animation": "walk", "action": { "attack": "jab" } }
        }))
        .expect("loads");
        assert_eq!(
            untimed.envelope.activities["stalk"].attack_turn, None,
            "an action without `at` implies no mark"
        );
    }

    #[test]
    fn action_at_with_an_explicit_mark_for_the_same_attack_loads() {
        let graph = load(serde_json::json!({
            "stalk": {
                "animation": "attack_jab",
                "action": { "attack": "jab", "at": "hit" },
                "attackTurn": "jab"
            }
        }))
        .expect("an explicit mark naming the `at` attack agrees");
        assert_eq!(
            graph.envelope.activities["stalk"].attack_turn.as_deref(),
            Some("jab")
        );
    }

    #[test]
    fn action_at_with_an_explicit_mark_for_another_attack_rejects() {
        let error = rejection(serde_json::json!({
            "stalk": {
                "animation": "attack_jab",
                "action": { "attack": "jab", "at": "hit" },
                "attackTurn": "slam"
            }
        }));
        assert!(
            error.contains("components.behavior.activities.stalk.attackTurn"),
            "{error}"
        );
        assert!(
            error.contains("components.behavior.activities.stalk.action.at"),
            "{error}"
        );
        assert!(
            error.contains("\"slam\"") && error.contains("\"jab\""),
            "{error}"
        );
    }

    #[test]
    fn action_at_under_an_enclosing_mark_for_another_attack_rejects() {
        let error = rejection(serde_json::json!({
            "stalk": { "attackTurn": "slam", "layers": { "offense": {
                "initial": "jab",
                "activities": {
                    "jab": { "animation": "attack_jab", "action": { "attack": "jab", "at": "hit" } }
                },
                "transitions": {}
            } } }
        }));
        assert!(
            error.contains("components.behavior.activities.stalk.attackTurn"),
            "{error}"
        );
        assert!(
            error.contains(
                "components.behavior.activities.stalk.layers.offense.activities.jab.action.at"
            ),
            "{error}"
        );
    }

    #[test]
    fn action_at_on_a_selector_row_rejects() {
        for (layer, row_path) in [
            (
                "offense",
                "components.behavior.activities.stalk.layers.offense[0].action.at",
            ),
            (
                "aux",
                "components.behavior.activities.stalk.layers.aux[0].action.at",
            ),
        ] {
            let error = rejection(serde_json::json!({
                "stalk": { "animation": "walk", "layers": {
                    layer: [{
                        "when": { "op": "const", "value": true },
                        "action": { "attack": "jab", "at": "hit" }
                    }]
                } }
            }));
            assert!(error.contains(row_path), "{layer}: {error}");
            assert!(error.contains("selector row"), "{layer}: {error}");
        }
    }

    #[test]
    fn action_at_with_an_invalid_event_name_rejects() {
        for event in ["", "2hit", "hit-1", "hit point"] {
            let error = rejection(serde_json::json!({
                "stalk": { "animation": "attack_jab", "action": { "attack": "jab", "at": event } }
            }));
            assert!(
                error.contains("components.behavior.activities.stalk.action.at"),
                "{event:?}: {error}"
            );
        }
    }

    // `at` on an activity whose path selects no animation is unreachable as a
    // separate check: only a leaf authors an action, a leaf must author an
    // animation, and a leaf's own animation is the one its path selects.
    #[test]
    fn action_at_on_a_leaf_without_an_animation_rejects() {
        let error = rejection(serde_json::json!({
            "stalk": { "action": { "attack": "jab", "at": "hit" } }
        }));
        assert!(
            error.contains("components.behavior.activities.stalk.animation"),
            "{error}"
        );
    }
}
