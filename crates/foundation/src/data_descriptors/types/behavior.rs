// Data-context descriptors: authored behavior statecharts.
// See: context/lib/scripting.md §11 (typed command buffer) ·
//      context/lib/entity_model.md §7c (enemy brain ownership).

use std::fmt;

use serde::de::{MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};

/// What an activity does with the enemy's movement. Closed vocabulary: the
/// engine owns steering, the author selects one of its modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MotionVerb {
    ChaseTarget,
    MoveToAnchor,
    Patrol,
    Hold,
    Freeze,
}

impl MotionVerb {
    pub const ALL: [MotionVerb; 5] = [
        MotionVerb::ChaseTarget,
        MotionVerb::MoveToAnchor,
        MotionVerb::Patrol,
        MotionVerb::Hold,
        MotionVerb::Freeze,
    ];
}

/// What an activity does besides moving. An attack name resolves against the
/// root graph's `attacks` map, including from nested layers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ActionVerb {
    Attack(String),
}

impl ActionVerb {
    pub fn all() -> [ActionVerb; 1] {
        [ActionVerb::Attack("attack".to_string())]
    }
}

/// Tuning consumed by a named [`ActionVerb::Attack`] entry.
///
/// Contact attacks retain their inline combat stats. Weapon attacks instead
/// name a canonical weapon descriptor and leave those stats absent until the
/// runtime resolves them into derived evaluator state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AttackParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weapon: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub damage: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_range: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cooldown_ms: Option<f32>,
    #[serde(default)]
    pub engagement_radius: Option<f32>,
    #[serde(default)]
    pub standoff_distance: Option<f32>,
}

/// How an authored patrol route moves when it reaches an endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PatrolMode {
    Loop,
    PingPong,
}

impl PatrolMode {
    pub const ALL: [PatrolMode; 2] = [PatrolMode::Loop, PatrolMode::PingPong];
}

/// Anchor-relative XZ positions followed by [`MotionVerb::Patrol`] motion.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PatrolDescriptor {
    #[serde(deserialize_with = "deserialize_patrol_points")]
    pub points: Vec<[f32; 2]>,
    pub mode: PatrolMode,
}

/// Luau represents an empty sequence as `{}`. Keep that one parity shim for
/// patrol points; a non-empty map remains an authoring error.
fn deserialize_patrol_points<'de, D>(deserializer: D) -> Result<Vec<[f32; 2]>, D::Error>
where
    D: Deserializer<'de>,
{
    struct PatrolPointsVisitor;

    impl<'de> Visitor<'de> for PatrolPointsVisitor {
        type Value = Vec<[f32; 2]>;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("an ordered sequence of [x, z] patrol points")
        }

        fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
        where
            A: SeqAccess<'de>,
        {
            let mut points = Vec::new();
            while let Some(point) = sequence.next_element()? {
                points.push(point);
            }
            Ok(points)
        }

        fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
        where
            A: MapAccess<'de>,
        {
            if map
                .next_entry::<serde::de::IgnoredAny, serde::de::IgnoredAny>()?
                .is_none()
            {
                Ok(Vec::new())
            } else {
                Err(serde::de::Error::custom(
                    "patrol points must be an ordered sequence, not an object",
                ))
            }
        }
    }

    deserializer.deserialize_any(PatrolPointsVisitor)
}

mod recursive;

pub use recursive::*;
