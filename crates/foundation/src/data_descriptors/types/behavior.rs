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
    MoveToLastKnown,
    Patrol,
    Hold,
    Freeze,
    /// Engaged: circle the target at standoff in one direction as a
    /// continuous orbit, aiming one 45° ring step ahead and re-aiming every
    /// tick; reverses when the way ahead is unusable. Takes no parameters.
    Strafe,
    /// Engaged: claim a combat slot at least one spacing from the entry slot,
    /// within the activity's `reposition` bounds.
    Reposition,
}

impl MotionVerb {
    pub const ALL: [MotionVerb; 8] = [
        MotionVerb::ChaseTarget,
        MotionVerb::MoveToAnchor,
        MotionVerb::MoveToLastKnown,
        MotionVerb::Patrol,
        MotionVerb::Hold,
        MotionVerb::Freeze,
        MotionVerb::Strafe,
        MotionVerb::Reposition,
    ];

    /// Fixed-world-position motion is non-engaged even when selected from a
    /// composite activity alongside independently-authored layers.
    pub const fn is_position_goal(self) -> bool {
        matches!(
            self,
            MotionVerb::MoveToAnchor | MotionVerb::MoveToLastKnown | MotionVerb::Patrol
        )
    }
}

/// What an activity does besides moving. An attack name resolves against the
/// root graph's `attacks` map, including from nested layers.
///
/// Authored as `{ attack: NAME, at?: EVENT }`; unknown keys reject.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "RawActionVerb", into = "RawActionVerb")]
pub enum ActionVerb {
    /// Fire the attack `name`. `at` names a clip event: the attack fires on
    /// the tick the activity's clip reaches it, once per entry, instead of on
    /// entry. Only an activity's own action may carry it, and it implies the
    /// attack's turn mark (`entity_model.md` §7c, Clip-timed attacks).
    Attack { name: String, at: Option<String> },
}

impl ActionVerb {
    pub fn all() -> [ActionVerb; 1] {
        [ActionVerb::Attack {
            name: "attack".to_string(),
            at: None,
        }]
    }
}

/// The authored object shape of [`ActionVerb`]. The enum keeps one variant
/// per verb, while authors write the attack's fields side by side.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawActionVerb {
    attack: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    at: Option<String>,
}

impl From<RawActionVerb> for ActionVerb {
    fn from(raw: RawActionVerb) -> Self {
        ActionVerb::Attack {
            name: raw.attack,
            at: raw.at,
        }
    }
}

impl From<ActionVerb> for RawActionVerb {
    fn from(action: ActionVerb) -> Self {
        let ActionVerb::Attack { name, at } = action;
        RawActionVerb { attack: name, at }
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
    /// Presentation-only sound key played at the enemy when this attack fires.
    /// A weapon attack plays it as well as the weapon's own sounds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sound: Option<String>,
    /// A contact attack's damage type. A weapon attack takes its weapon's,
    /// so naming one beside `weapon` rejects load.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub damage_type: Option<crate::AuthoredHandle>,
    /// The attack tokens this attack draws, each a handle or
    /// `token.with({ cost })`. `None` draws `defaultToken`; an empty list
    /// opts out of the attack budget.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::attack_budget::deserialize_draws"
    )]
    pub draws: Option<Vec<crate::attack_budget::TokenDraw>>,
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

mod activity_motion;
mod attack_turn_marks;
mod clip_timed_actions;
mod recursive;

pub use activity_motion::{
    COMBAT_RING_STEP_DEG, DEFAULT_REPOSITION_ARC_DEG, FacingPolicy, RepositionParams,
};
pub use recursive::*;

/// Whether an activity represents locomotion rather than an action-bearing
/// combat phase. An engaged verb that moves without acting (`chaseTarget`,
/// `strafe`, `reposition`) is locomotion: its animation is a travel cycle, so
/// it yields to the rest animation at a standstill.
pub fn is_locomotion_activity(activity: &BehaviorActivityDescriptor) -> bool {
    matches!(
        activity.motion,
        Some(MotionVerb::ChaseTarget | MotionVerb::Strafe | MotionVerb::Reposition)
    ) && activity.action.is_none()
        || activity.motion.is_some_and(MotionVerb::is_position_goal)
}

/// Resolve the graph's authored locomotion animation, including a composite
/// activity whose `move` layer selects locomotion per tick.
pub fn locomotion_animation(graph: &BehaviorGraphDescriptor) -> Option<&str> {
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

/// Visit the animation states a graph rate-scales by steered travel speed:
/// its [`locomotion_animation`], then every animation an activity at any depth
/// plays under its own `chaseTarget`, `strafe` or `reposition` motion, with
/// or without an action, so a moving attack strides at its speed. The host
/// tick and remote presentation both derive the set from the descriptor
/// graph, so they agree on which states scale without anything on the wire.
///
/// Stops at the first animation `visit` accepts and reports whether one did.
/// A state may be visited more than once. Allocates nothing.
pub fn any_rate_scaled_animation(
    graph: &BehaviorGraphDescriptor,
    mut visit: impl FnMut(&str) -> bool,
) -> bool {
    if locomotion_animation(graph).is_some_and(&mut visit) {
        return true;
    }
    envelope_any_rate_scaled_animation(&graph.envelope, &mut visit)
}

fn envelope_any_rate_scaled_animation(
    envelope: &BehaviorGraphEnvelope,
    visit: &mut dyn FnMut(&str) -> bool,
) -> bool {
    envelope.activities.values().any(|activity| {
        let steers = matches!(
            activity.motion,
            Some(MotionVerb::ChaseTarget | MotionVerb::Strafe | MotionVerb::Reposition)
        );
        if steers && activity.animation.as_deref().is_some_and(&mut *visit) {
            return true;
        }
        activity.layers.values().any(|layer| match layer {
            BehaviorLayerDescriptor::Graph(nested) => {
                envelope_any_rate_scaled_animation(nested, visit)
            }
            BehaviorLayerDescriptor::Selector(_) => false,
        })
    })
}

/// Resolve the animation authored on the graph's initial activity.
pub fn rest_animation(graph: &BehaviorGraphDescriptor) -> Option<&str> {
    graph
        .envelope
        .activities
        .get(&graph.envelope.initial)
        .and_then(|activity| activity.animation.as_deref())
}
