// Data-context descriptors: authored behavior statecharts.
// See: context/lib/scripting.md §11 (typed command buffer) ·
//      context/lib/entity_model.md §7c (enemy brain ownership).

use std::borrow::Cow;
use std::fmt;
use std::sync::{Arc, OnceLock, Weak};

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

    /// Whether this motion steers the body along a path: every verb but
    /// `hold` and `freeze`, which plant it. A state an activity plays under a
    /// steering motion is rate-scaled ([`any_rate_scaled_animation`]).
    /// Exhaustive so that a new verb decides here.
    pub const fn steers(self) -> bool {
        match self {
            MotionVerb::ChaseTarget
            | MotionVerb::MoveToAnchor
            | MotionVerb::MoveToLastKnown
            | MotionVerb::Patrol
            | MotionVerb::Strafe
            | MotionVerb::Reposition => true,
            MotionVerb::Hold | MotionVerb::Freeze => false,
        }
    }
}

/// What an activity does besides moving. An attack name resolves against the
/// root graph's `attacks` map, including from nested layers.
///
/// Authored as `{ attack: NAME, at?: EVENT }`; unknown keys reject. Stance
/// lowering may add `atDefault: true` beside `at`; authors never write it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "RawActionVerb", into = "RawActionVerb")]
pub enum ActionVerb {
    /// Fire the attack `name`. `at` names a clip event: the attack fires on
    /// the tick the activity's clip reaches it, once per entry, instead of on
    /// entry. Only an activity's own action may carry it, and it implies the
    /// attack's turn mark (`entity_model.md` §7c, Clip-timed attacks).
    ///
    /// `at_default` marks an `at` that stance lowering filled in (a release's
    /// omitted `at` defaults to `hit`): a clip without the event binds
    /// silently and fires as without `at` (Stances and attacks, Release exit).
    Attack {
        name: String,
        at: Option<String>,
        at_default: bool,
    },
}

impl ActionVerb {
    pub fn all() -> [ActionVerb; 1] {
        [ActionVerb::attack("attack")]
    }

    /// Fire `name` on entry: no clip event.
    pub fn attack(name: &str) -> ActionVerb {
        ActionVerb::Attack {
            name: name.to_string(),
            at: None,
            at_default: false,
        }
    }

    /// Fire `name` at clip event `event`, as authored.
    pub fn attack_at(name: &str, event: &str) -> ActionVerb {
        ActionVerb::Attack {
            name: name.to_string(),
            at: Some(event.to_string()),
            at_default: false,
        }
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
    #[serde(default, rename = "atDefault", skip_serializing_if = "is_false")]
    at_default: bool,
}

fn is_false(value: &bool) -> bool {
    !*value
}

impl From<RawActionVerb> for ActionVerb {
    fn from(raw: RawActionVerb) -> Self {
        ActionVerb::Attack {
            name: raw.attack,
            at: raw.at,
            at_default: raw.at_default,
        }
    }
}

impl From<ActionVerb> for RawActionVerb {
    fn from(action: ActionVerb) -> Self {
        let ActionVerb::Attack {
            name,
            at,
            at_default,
        } = action;
        RawActionVerb {
            attack: name,
            at,
            at_default,
        }
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
    /// A contact attack's queue distance: how far out a row into it waits in
    /// line and requests its turn. At least `max_range`, which stays the hit
    /// distance; omitted, the queue distance is `max_range`. It moves no
    /// combat slot: `engagement_radius` and `standoff_distance` still default
    /// from `max_range`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queue_range: Option<f32>,
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

impl AttackParams {
    /// How far out a queued row into this attack waits in line and requests
    /// its turn: `queue_range`, else `max_range`. `None` for a weapon attack,
    /// which queues at any distance.
    pub fn queue_distance(&self) -> Option<f32> {
        self.queue_range.or(self.max_range)
    }
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
pub mod stance_defaults;

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
/// plays under its own steering motion ([`MotionVerb::steers`]: every verb but
/// `hold` and `freeze`), position goals included, with or without an action,
/// so a patrol's walk and a moving attack's stride both match their speed. A
/// motion a composite's `move` selector supplies adds nothing beyond the
/// locomotion state. The host tick and remote presentation both derive the set
/// from the descriptor graph, through [`RateScaledStates::of`], so they agree
/// on which states scale without anything on the wire.
///
/// Stops at the first animation `visit` accepts and reports whether one did.
/// A state may be visited more than once. Allocates nothing, but walks the
/// whole graph: per-tick callers read a [`RateScaledStates`] derived once.
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
        let steers = activity.motion.is_some_and(MotionVerb::steers);
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

/// A graph's rate-scaled animation states ([`any_rate_scaled_animation`]),
/// collected once, each named once, in visit order. The one set the host's
/// playback-rate pass and a remote client's walk reference both read, so a
/// per-tick check walks a handful of names instead of the graph.
///
/// Matching a presented state against it, directional variants included, is
/// the reader's: `entities`' `TravelVariant::of_state`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RateScaledStates(Arc<[Box<str>]>);

impl RateScaledStates {
    /// Collect `graph`'s rate-scaled states. Walks the graph and allocates:
    /// call it when a graph is attached or materialized, not per tick.
    pub fn of(graph: &BehaviorGraphDescriptor) -> Self {
        let mut states: Vec<Box<str>> = Vec::new();
        any_rate_scaled_animation(graph, |state| {
            if !states.iter().any(|known| &**known == state) {
                states.push(state.into());
            }
            false
        });
        Self(states.into())
    }

    /// The states, in visit order.
    pub fn iter(&self) -> impl Iterator<Item = &str> {
        self.0.iter().map(|state| &**state)
    }
}

/// The animation states a graph's repeating stance releases play (an activity
/// with `passes` above 1, at any depth), each named once, in visit order. A
/// repeating release's looping state starts at clip time 0 on entry instead
/// of the enemy's drawn phase, so its first pass continues from the held tell
/// and strikes on that pass. The host and a remote client both derive the set
/// from the descriptor graph when the mesh is materialized, so nothing crosses
/// the wire. Walks the graph and allocates: call it at materialize, not per
/// tick.
pub fn repeating_release_states(graph: &BehaviorGraphDescriptor) -> Vec<&str> {
    fn walk<'a>(envelope: &'a BehaviorGraphEnvelope, states: &mut Vec<&'a str>) {
        for activity in envelope.activities.values() {
            if activity.passes.is_some_and(|passes| passes > 1)
                && let Some(state) = activity.animation.as_deref()
                && !states.contains(&state)
            {
                states.push(state);
            }
            for layer in activity.layers.values() {
                if let BehaviorLayerDescriptor::Graph(nested) = layer {
                    walk(nested, states);
                }
            }
        }
    }
    let mut states = Vec::new();
    walk(&graph.envelope, &mut states);
    states
}

/// A brain's [`RateScaledStates`], derived from its graph on first read and
/// kept for that graph, so the per-tick playback-rate pass reads a borrowed
/// set. Derived data: it is never serialized (a restored brain derives it on
/// its first read) and equality ignores it, since it is a pure function of the
/// graph that equality already compares. Clones share the derived set.
#[derive(Debug, Clone, Default)]
pub struct RateScaledStatesMemo {
    /// The graph the states were derived from, held weakly: the allocation
    /// stays reserved while this lives, so a pointer compare never mistakes a
    /// new graph at a reused address for the one derived.
    derived: OnceLock<(Weak<BehaviorGraphDescriptor>, RateScaledStates)>,
}

impl RateScaledStatesMemo {
    /// A memo already holding `graph`'s states.
    pub fn of(graph: &Arc<BehaviorGraphDescriptor>) -> Self {
        let memo = Self::default();
        let _ = memo
            .derived
            .set((Arc::downgrade(graph), RateScaledStates::of(graph)));
        memo
    }

    /// `graph`'s rate-scaled states, derived on the first read and borrowed
    /// after. A memo derived for another graph (a brain whose `graph` was
    /// swapped in place) derives anew on every read rather than serve stale
    /// states; production replaces the whole brain instead.
    pub fn get(&self, graph: &Arc<BehaviorGraphDescriptor>) -> Cow<'_, RateScaledStates> {
        let (held, states) = self
            .derived
            .get_or_init(|| (Arc::downgrade(graph), RateScaledStates::of(graph)));
        if std::ptr::eq(held.as_ptr(), Arc::as_ptr(graph)) {
            Cow::Borrowed(states)
        } else {
            Cow::Owned(RateScaledStates::of(graph))
        }
    }
}

impl PartialEq for RateScaledStatesMemo {
    /// Always equal: the memo is derived from the graph its owner compares.
    fn eq(&self, _other: &Self) -> bool {
        true
    }
}

/// Resolve the animation authored on the graph's initial activity.
pub fn rest_animation(graph: &BehaviorGraphDescriptor) -> Option<&str> {
    graph
        .envelope
        .activities
        .get(&graph.envelope.initial)
        .and_then(|activity| activity.animation.as_deref())
}
