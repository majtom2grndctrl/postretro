// Entity-side descriptor validators. These construct entity-resident
// descriptor types and therefore stay above the future foundation layer.
// See: context/lib/scripting.md §12 (Crate Architecture)

use super::super::{CrossingCondition, CrossingDescriptor, DescriptorError};

/// Build a [`CrossingDescriptor`] from the raw fields gathered by either FFI
/// path. Shared so JS and Luau enforce identical rules.
pub fn build_crossing(
    slot: String,
    below: Option<f32>,
    above: Option<f32>,
    max: Option<f32>,
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
    Ok(CrossingDescriptor {
        slot,
        condition,
        max,
        fire,
    })
}
