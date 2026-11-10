//! Per-activity engaged-motion tuning: authored facing and `reposition`
//! bounds, plus their load-time validation.
//! See: context/lib/entity_model.md §7c (Engaged motion and facing).

use serde::{Deserialize, Serialize};

use super::{BehaviorActivityDescriptor, MotionVerb};
use crate::data_descriptors::DescriptorError;

/// Which heading an enemy faces while an activity is active. Omitted on an
/// activity, its motion verb's default applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FacingPolicy {
    /// Turn toward the selected target's aim point.
    Target,
    /// Turn toward steering travel; hold the heading while steering is stopped.
    Travel,
}

impl FacingPolicy {
    pub const ALL: [FacingPolicy; 2] = [FacingPolicy::Target, FacingPolicy::Travel];
}

/// `reposition` arc when the activity authors none: either side, any bearing.
pub const DEFAULT_REPOSITION_ARC_DEG: [f32; 2] = [0.0, 180.0];

/// The combat ring steps in 45° slots, so an arc must contain one of these.
const RING_STEP_DEG: f32 = 45.0;

/// Bounds a `reposition` activity fixes at entry. `min_travel_m` stays `None`
/// when omitted: its default, one slot spacing, depends on the standoff the
/// runtime resolves.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepositionParams {
    /// `[min, max]` degrees around the target, measured from the bearing from
    /// the target to the enemy at entry. Either side qualifies.
    #[serde(rename = "arc", default = "default_reposition_arc")]
    pub arc_deg: [f32; 2],
    /// Metres the claimed slot must lie from the enemy's position at entry.
    #[serde(rename = "minTravel", default, skip_serializing_if = "Option::is_none")]
    pub min_travel_m: Option<f32>,
}

impl Default for RepositionParams {
    fn default() -> Self {
        Self {
            arc_deg: DEFAULT_REPOSITION_ARC_DEG,
            min_travel_m: None,
        }
    }
}

fn default_reposition_arc() -> [f32; 2] {
    DEFAULT_REPOSITION_ARC_DEG
}

/// Reject `reposition` properties that could never select a slot, or that sit
/// on an activity whose own motion is not `reposition`. A selector-supplied
/// `reposition` uses the defaults, so properties belong to the activity that
/// names the verb itself.
pub(super) fn validate_reposition(
    activity: &BehaviorActivityDescriptor,
    path: &str,
) -> Result<(), DescriptorError> {
    let Some(params) = activity.reposition else {
        return Ok(());
    };
    if activity.motion != Some(MotionVerb::Reposition) {
        return Err(DescriptorError::InvalidShape {
            reason: format!(
                "`{path}.reposition` requires `{path}.motion` to be \"reposition\"; only an activity that names the verb itself may tune it"
            ),
        });
    }

    let arc_path = format!("{path}.reposition.arc");
    let [min, max] = params.arc_deg;
    if !min.is_finite() || !max.is_finite() {
        return Err(DescriptorError::InvalidShape {
            reason: format!("`{arc_path}` must contain finite degrees, got [{min}, {max}]"),
        });
    }
    if min < 0.0 || max > 180.0 {
        return Err(DescriptorError::InvalidShape {
            reason: format!("`{arc_path}` must lie within 0 to 180 degrees, got [{min}, {max}]"),
        });
    }
    if min > max {
        return Err(DescriptorError::InvalidShape {
            reason: format!("`{arc_path}` minimum must not exceed its maximum, got [{min}, {max}]"),
        });
    }
    let contains_ring_step = (0..=4).any(|step| {
        let bearing = RING_STEP_DEG * step as f32;
        min <= bearing && bearing <= max
    });
    if !contains_ring_step {
        return Err(DescriptorError::InvalidShape {
            reason: format!(
                "`{arc_path}` [{min}, {max}] contains no multiple of 45 degrees; the combat ring steps in 45-degree slots, so it could never select one"
            ),
        });
    }

    if let Some(min_travel) = params.min_travel_m
        && (!min_travel.is_finite() || min_travel < 0.0)
    {
        return Err(DescriptorError::InvalidShape {
            reason: format!(
                "`{path}.reposition.minTravel` must be a finite value >= 0.0, got {min_travel}"
            ),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    fn reposition_leaf(motion: MotionVerb, params: RepositionParams) -> BehaviorActivityDescriptor {
        BehaviorActivityDescriptor {
            animation: Some("run".to_string()),
            motion: Some(motion),
            action: None,
            on_enter: None,
            sound: None,
            facing: None,
            reposition: Some(params),
            attack_turn: None,
            layers: BTreeMap::new(),
        }
    }

    fn arc(min: f32, max: f32) -> RepositionParams {
        RepositionParams {
            arc_deg: [min, max],
            min_travel_m: None,
        }
    }

    #[test]
    fn reposition_params_default_when_omitted_and_keep_min_travel_unresolved() {
        let params: RepositionParams =
            serde_json::from_value(serde_json::json!({})).expect("empty block deserializes");
        assert_eq!(params, RepositionParams::default());
        assert_eq!(params.arc_deg, [0.0, 180.0]);
        assert_eq!(params.min_travel_m, None);

        let params: RepositionParams =
            serde_json::from_value(serde_json::json!({ "arc": [60, 120], "minTravel": 8 }))
                .expect("authored block deserializes");
        assert_eq!(params.arc_deg, [60.0, 120.0]);
        assert_eq!(params.min_travel_m, Some(8.0));

        assert!(
            serde_json::from_value::<RepositionParams>(serde_json::json!({ "arcDeg": [0, 90] }))
                .is_err(),
            "unknown reposition keys reject"
        );
    }

    #[test]
    fn reposition_validation_accepts_arcs_holding_a_ring_step() {
        for (min, max) in [
            (0.0, 180.0),
            (45.0, 135.0),
            (90.0, 90.0),
            (180.0, 180.0),
            (30.0, 50.0),
        ] {
            validate_reposition(&reposition_leaf(MotionVerb::Reposition, arc(min, max)), "a")
                .unwrap_or_else(|error| panic!("[{min}, {max}] must validate: {error}"));
        }
    }

    #[test]
    fn reposition_validation_rejects_each_bad_bound_with_its_path() {
        for (params, field, needle) in [
            (arc(-1.0, 90.0), "reposition.arc", "within 0 to 180"),
            (arc(0.0, 181.0), "reposition.arc", "within 0 to 180"),
            (arc(120.0, 60.0), "reposition.arc", "must not exceed"),
            (arc(f32::NAN, 90.0), "reposition.arc", "finite"),
            (arc(0.0, f32::INFINITY), "reposition.arc", "finite"),
            (arc(50.0, 80.0), "reposition.arc", "no multiple of 45"),
            (
                RepositionParams {
                    min_travel_m: Some(-0.5),
                    ..RepositionParams::default()
                },
                "reposition.minTravel",
                ">= 0.0",
            ),
            (
                RepositionParams {
                    min_travel_m: Some(f32::INFINITY),
                    ..RepositionParams::default()
                },
                "reposition.minTravel",
                ">= 0.0",
            ),
        ] {
            let error = validate_reposition(
                &reposition_leaf(MotionVerb::Reposition, params),
                "components.behavior.activities.move",
            )
            .expect_err("bad bound rejects")
            .to_string();
            assert!(
                error.contains(&format!("components.behavior.activities.move.{field}")),
                "{error}"
            );
            assert!(error.contains(needle), "{error}");
        }
    }

    #[test]
    fn reposition_properties_reject_on_any_other_motion() {
        for motion in MotionVerb::ALL
            .into_iter()
            .filter(|motion| *motion != MotionVerb::Reposition)
        {
            let error = validate_reposition(
                &reposition_leaf(motion, RepositionParams::default()),
                "components.behavior.activities.wait",
            )
            .expect_err("reposition properties need the reposition verb")
            .to_string();
            assert!(
                error.contains("components.behavior.activities.wait.reposition"),
                "{motion:?}: {error}"
            );
        }
    }
}
