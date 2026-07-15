// Entity-side descriptor validators. These construct entity-resident
// descriptor types and therefore stay above the foundation layer.
// See: context/lib/scripting.md §12 (Crate Architecture)

use super::super::{CrossingCondition, CrossingDescriptor, DescriptorError};
use postretro_foundation::ir::IrNode;

/// Build a [`CrossingDescriptor`] from the raw fields gathered by either FFI
/// path. Shared so JS and Luau enforce identical rules.
pub fn build_crossing(
    slot: String,
    below: Option<f32>,
    above: Option<f32>,
    max: Option<f32>,
    edge: Option<String>,
    fire: Vec<String>,
) -> Result<CrossingDescriptor, DescriptorError> {
    if slot.is_empty() {
        return Err(DescriptorError::InvalidShape {
            reason: "crossing entry `slot` must be a non-empty string".to_string(),
        });
    }
    let max = max.unwrap_or(1.0);
    if !max.is_finite() || max <= 0.0 {
        return Err(DescriptorError::InvalidShape {
            reason: format!("crossing entry `max` must be a finite value > 0.0, got {max}"),
        });
    }
    let condition = match (below, above) {
        (Some(below), None) => {
            if !below.is_finite() {
                return Err(DescriptorError::InvalidShape {
                    reason: format!("crossing entry `below` must be finite, got {below}"),
                });
            }
            CrossingCondition::Below {
                threshold: below / max,
            }
        }
        (None, Some(above)) => {
            if !above.is_finite() {
                return Err(DescriptorError::InvalidShape {
                    reason: format!("crossing entry `above` must be finite, got {above}"),
                });
            }
            CrossingCondition::Above {
                threshold: above / max,
            }
        }
        (None, None) => return Err(DescriptorError::CrossingCondition { count: 0 }),
        (Some(_), Some(_)) => return Err(DescriptorError::CrossingCondition { count: 2 }),
    };
    let edge = normalize_crossing_edge(edge);
    Ok(CrossingDescriptor {
        slot: Some(slot),
        condition,
        max,
        edge,
        fire,
    })
}

/// Build a predicate-form crossing descriptor. Its raw IR is bound against the
/// live store scope by scripting-core at install time, so this entities-layer
/// builder deliberately performs no scope or type validation.
pub fn build_predicate_crossing(
    predicate: IrNode,
    edge: Option<String>,
    fire: Vec<String>,
) -> CrossingDescriptor {
    CrossingDescriptor {
        slot: None,
        condition: CrossingCondition::Ir(predicate),
        // Predicate crossings do not normalize a single numeric slot. Keep the
        // existing field at its neutral threshold-form default so descriptor
        // consumers retain a stable shape.
        max: 1.0,
        edge: normalize_crossing_edge(edge),
        fire,
    }
}

fn normalize_crossing_edge(edge: Option<String>) -> Option<String> {
    match edge.as_deref() {
        None => None,
        Some("both") => edge,
        Some(unknown) => {
            log::warn!(
                "[Scripting] onStateCrossing: unknown edge `{unknown}`; using shipped single-edge behavior"
            );
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_edge_degrades_to_absent_for_both_crossing_forms() {
        let threshold = build_crossing(
            "test.value".to_string(),
            None,
            Some(1.0),
            None,
            Some("future".to_string()),
            vec!["fire".to_string()],
        )
        .unwrap();
        let predicate = build_predicate_crossing(
            IrNode::Input {
                name: "test.flag".to_string(),
            },
            Some("future".to_string()),
            vec!["fire".to_string()],
        );

        assert_eq!(threshold.edge, None);
        assert_eq!(predicate.edge, None);
    }
}
