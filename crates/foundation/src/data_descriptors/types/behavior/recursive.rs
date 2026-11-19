//! Recursive behavior-statechart descriptors.
//!
//! The root graph and every nested graph layer share `BehaviorGraphEnvelope`.
//! Root-only tuning intentionally lives on `BehaviorGraphDescriptor`, never on
//! the envelope, so a nested layer cannot accidentally acquire navigation or
//! target-selection policy.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::de::{MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};

use super::activity_motion::validate_reposition;
use super::attack_turn_marks::validate_attack_turn_marks;
use super::clip_timed_actions::{imply_attack_turn, reject_row_at, validate_at_name};
use super::stance_defaults::MAX_RELEASE_REPEAT;
use super::{
    ActionVerb, AttackParams, FacingPolicy, MotionVerb, PatrolDescriptor, RepositionParams,
};
use crate::brain::bind_brain_guard;
use crate::candidate::bind_candidate_filter;
use crate::data_descriptors::types::behavior_lints;
use crate::data_descriptors::{DescriptorError, validate_sound_key};
use crate::ir::{IrNode, IrType};

/// Maximum number of nested behavior envelopes, including the root envelope.
/// This gives the evaluator a fixed-capacity active path.
pub const MAX_BEHAVIOR_NESTING_DEPTH: usize = 8;

/// The source-keyed adjacency row shared by every behavior envelope.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GuardedRow {
    /// The row's guard. An authored row may omit it; the row is then always
    /// enabled, so it deserializes as the constant `true`.
    #[serde(default = "always_enabled")]
    pub when: IrNode,
    pub to: String,
    /// A stance hold's release row, written only by lowering: the `[min, max]`
    /// milliseconds its hidden release time is drawn from on each entry of the
    /// activity whose row this is. The row passes only once that activity's
    /// pose has lasted the drawn time, and its `when` passes.
    #[serde(
        default,
        rename = "releaseAfter",
        skip_serializing_if = "Option::is_none"
    )]
    pub release_after: Option<[f32; 2]>,
    /// A row to an attack, written only by lowering: the attack, by name, a
    /// stance's rest strikes through this row. The row passes only once that
    /// attack's turn reads held, the target is within its queue distance
    /// (`queueRange`, else `maxRange`) when it declares one, its own
    /// cooldown has elapsed, and its `when` passes;
    /// while all but the turn pass, the resting enemy requests the turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queue: Option<String>,
    /// A stance release's own row, written only by lowering: the row stays
    /// closed until the entry's moment resolves (fired, whiffed or
    /// cancelled), or with no moment until the activity's clip has played
    /// one pass, and opens on the evaluation after that. Its `when` (an `ms`
    /// guard, say) may only hold it longer.
    #[serde(
        default,
        rename = "afterMoment",
        skip_serializing_if = "std::ops::Not::not"
    )]
    pub after_moment: bool,
}

/// The guard an authored row without `when` reads.
fn always_enabled() -> IrNode {
    IrNode::Const {
        value: crate::ir::IrValue::Bool(true),
    }
}

/// A recursive graph envelope with no graph-wide gameplay fields.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BehaviorGraphEnvelope {
    pub initial: String,
    #[serde(deserialize_with = "deserialize_activities")]
    pub activities: BTreeMap<String, BehaviorActivityDescriptor>,
    pub transitions: BTreeMap<String, Vec<GuardedRow>>,
}

/// A leaf (animation plus optional motion/action sugar) or a composite
/// (layers plus optional locomotion animation).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BehaviorActivityDescriptor {
    #[serde(default)]
    pub animation: Option<String>,
    #[serde(default)]
    pub motion: Option<MotionVerb>,
    #[serde(default)]
    pub action: Option<ActionVerb>,
    #[serde(default)]
    pub on_enter: Option<String>,
    /// Presentation-only sound key played at the enemy when this leaf is
    /// entered, whether or not it authors `on_enter`.
    #[serde(default)]
    pub sound: Option<String>,
    /// Which heading the enemy faces while this activity is active, covering
    /// the motion its own selector layer supplies. Omitted, the motion verb's
    /// default applies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub facing: Option<FacingPolicy>,
    /// Bounds for this activity's own `reposition` motion; valid only when
    /// `motion` is `reposition`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reposition: Option<RepositionParams>,
    /// Marks the activity for one of the graph's attacks: a turn for it is
    /// requested on entry and held while any activity marked for it is on
    /// the active path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attack_turn: Option<String>,
    /// Multiplier on the archetype's `moveSpeed` while this is the innermost
    /// active activity that sets one; an activity without one inherits its
    /// nearest ancestor's, and the whole path without one moves at 1. Scales
    /// the agent's own steered travel only, never knockback.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speed: Option<f32>,
    /// A stance's pose, written only by lowering. Activities of one level
    /// sharing a pose act as one activity: moving between them keeps that
    /// level's scope-relative facts, raises no entry event, and restarts no
    /// clip (`entity_model.md` §7c, Stances and attacks).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pose: Option<String>,
    /// A stance release's or recover's flinch route, written only by
    /// lowering: the sibling rest an impact flinch during this activity ends
    /// in, once its reaction clip plays out (`entity_model.md` §7c, Stances
    /// and attacks, Release exit).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stagger_to: Option<String>,
    /// A stance release's pass count (its `repeat`), written only by
    /// lowering: its clip plays this many passes back to back, its attack
    /// strikes once per pass at its moment, and its own rows stay closed until
    /// the last pass has played (`entity_model.md` §7c, Stances and attacks,
    /// Release exit). Absent, one pass.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub passes: Option<u32>,
    #[serde(default)]
    pub layers: BTreeMap<String, BehaviorLayerDescriptor>,
}

/// A stateless selector list or another recursive envelope.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum BehaviorLayerDescriptor {
    Selector(Vec<BehaviorSelectorEntry>),
    Graph(BehaviorGraphEnvelope),
}

/// A selector item. A bare motion is the final fallback in a `move` selector.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum BehaviorSelectorEntry {
    Row(BehaviorSelectorRow),
    Motion(MotionVerb),
}

/// A conditional selector row. `when` is optional only for the internal
/// single-entry `action:` sugar shape, which is unconditional by construction.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BehaviorSelectorRow {
    #[serde(default)]
    pub when: Option<IrNode>,
    #[serde(default)]
    pub motion: Option<MotionVerb>,
    #[serde(default)]
    pub action: Option<ActionVerb>,
}

/// The root envelope plus root-only behavior policy and tuning.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BehaviorGraphDescriptor {
    #[serde(default)]
    pub knockback: crate::KnockbackResponse,
    #[serde(flatten)]
    pub envelope: BehaviorGraphEnvelope,
    #[serde(default)]
    pub candidate_filter: Option<IrNode>,
    /// Engine-owned target-retaliation tuning. Authors choose scalar feel
    /// parameters here; they never provide a ranking expression.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retaliation: Option<RetaliationDescriptor>,
    #[serde(default)]
    pub patrol: Option<PatrolDescriptor>,
    #[serde(default)]
    pub attacks: BTreeMap<String, AttackParams>,
    #[serde(default)]
    pub engagement_radius: Option<f32>,
    pub move_speed: f32,
    /// Root fields the AI tick scheduler reads.
    #[serde(flatten)]
    pub ai: BehaviorAiTuning,
}

/// Behavior-root fields the AI tick scheduler reads, on the wire beside the
/// other root fields. Each serializes only when authored off its default, so
/// a graph that sets none keeps its bytes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BehaviorAiTuning {
    /// Keeps the archetype's NPCs out of the dormant tier: they run idle or
    /// engaged whether or not a player perceives them. A placement's
    /// `always_active` key overrides it.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub always_active: bool,
}

/// Scalar tuning for the engine-owned retaliation preference. The candidate
/// threshold and rank formula remain engine code; this descriptor exposes only
/// the authored feel axes.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RetaliationDescriptor {
    /// How long a damager remains eligible to accrue retaliation preference.
    /// A zero or sub-tick window explicitly disables the term.
    #[serde(default = "default_retaliation_window_ms")]
    pub window_ms: f32,
    /// Multiplier applied to recent accumulated damage when ranking two
    /// over-tolerance provokers.
    #[serde(default = "default_retaliation_damage_weight")]
    pub damage_weight: f32,
    /// Per-millisecond penalty applied to the age of a provoking hit when
    /// ranking two over-tolerance provokers.
    #[serde(default = "default_retaliation_recency_weight")]
    pub recency_weight: f32,
}

impl Default for RetaliationDescriptor {
    fn default() -> Self {
        Self {
            window_ms: default_retaliation_window_ms(),
            damage_weight: default_retaliation_damage_weight(),
            recency_weight: default_retaliation_recency_weight(),
        }
    }
}

fn default_retaliation_window_ms() -> f32 {
    1_500.0
}

fn default_retaliation_damage_weight() -> f32 {
    1.0
}

fn default_retaliation_recency_weight() -> f32 {
    0.001
}

/// `flatten` and `deny_unknown_fields` cannot be combined in serde. Use a
/// strict wire-shaped helper so retired `states` / `interrupts` spellings are
/// errors rather than silently ignored.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RawBehaviorGraphDescriptor {
    #[serde(default)]
    knockback: crate::KnockbackResponse,
    initial: String,
    #[serde(deserialize_with = "deserialize_activities")]
    activities: BTreeMap<String, BehaviorActivityDescriptor>,
    transitions: BTreeMap<String, Vec<GuardedRow>>,
    #[serde(default)]
    candidate_filter: Option<IrNode>,
    #[serde(default)]
    retaliation: Option<RetaliationDescriptor>,
    #[serde(default)]
    patrol: Option<PatrolDescriptor>,
    #[serde(default)]
    attacks: BTreeMap<String, AttackParams>,
    #[serde(default)]
    engagement_radius: Option<f32>,
    move_speed: f32,
    #[serde(default)]
    always_active: bool,
}

impl<'de> Deserialize<'de> for BehaviorGraphDescriptor {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = RawBehaviorGraphDescriptor::deserialize(deserializer)?;
        Ok(Self {
            knockback: raw.knockback,
            envelope: BehaviorGraphEnvelope {
                initial: raw.initial,
                activities: raw.activities,
                transitions: raw.transitions,
            },
            candidate_filter: raw.candidate_filter,
            retaliation: raw.retaliation,
            patrol: raw.patrol,
            attacks: raw.attacks,
            engagement_radius: raw.engagement_radius,
            move_speed: raw.move_speed,
            ai: BehaviorAiTuning {
                always_active: raw.always_active,
            },
        })
    }
}

/// Reject duplicate activity keys instead of silently accepting a final
/// writer.
///
/// Both script runtimes collapse duplicate object/table keys before the
/// descriptor bridge sees them, so this fires on the raw-JSON boundary. The
/// same visitor applies to the root and every nested graph envelope.
fn deserialize_activities<'de, D>(
    deserializer: D,
) -> Result<BTreeMap<String, BehaviorActivityDescriptor>, D::Error>
where
    D: Deserializer<'de>,
{
    struct ActivitiesVisitor;

    impl<'de> Visitor<'de> for ActivitiesVisitor {
        type Value = BTreeMap<String, BehaviorActivityDescriptor>;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("a map of activity name to behavior activity")
        }

        fn visit_map<M>(self, mut access: M) -> Result<Self::Value, M::Error>
        where
            M: MapAccess<'de>,
        {
            let mut activities = BTreeMap::new();
            while let Some((name, activity)) =
                access.next_entry::<String, BehaviorActivityDescriptor>()?
            {
                if activities.contains_key(&name) {
                    return Err(serde::de::Error::custom(format!(
                        "duplicate activity name `{name}`"
                    )));
                }
                activities.insert(name, activity);
            }
            Ok(activities)
        }
    }

    deserializer.deserialize_map(ActivitiesVisitor)
}

impl BehaviorGraphDescriptor {
    pub const DEFAULT_ENGAGEMENT_RADIUS: f32 = 2.0;

    pub fn engagement_radius(&self) -> f32 {
        self.engagement_radius
            .unwrap_or(Self::DEFAULT_ENGAGEMENT_RADIUS)
    }

    /// Resolve omitted tuning to the compatibility defaults. The default
    /// tolerance is `f32::MAX`, so these values leave unauthored graphs on the
    /// pre-retaliation distance ranking path.
    pub fn retaliation(&self) -> RetaliationDescriptor {
        self.retaliation.unwrap_or_default()
    }

    /// Root attack data stays authoritative even when an action appears under
    /// a nested graph layer.
    pub fn engagement_radius_for_action(&self, action: Option<&ActionVerb>) -> f32 {
        match action {
            Some(ActionVerb::Attack { name, .. }) => self
                .attacks
                .get(name)
                .and_then(|attack| attack.engagement_radius.or(attack.max_range))
                .unwrap_or_else(|| self.engagement_radius()),
            None => self.engagement_radius(),
        }
    }

    /// Combat-slot distance for the selected action. A per-attack standoff is
    /// explicit; otherwise retain the action-relative engagement-radius
    /// behavior so an attack override remains the positioning default.
    pub fn standoff_distance_for_action(&self, action: Option<&ActionVerb>) -> f32 {
        match action {
            Some(ActionVerb::Attack { name, .. }) => self
                .attacks
                .get(name)
                .and_then(|attack| attack.standoff_distance)
                .unwrap_or_else(|| self.engagement_radius_for_action(action)),
            None => self.engagement_radius_for_action(None),
        }
    }

    /// Shared validation used after both JS and Luau conversion paths.
    pub fn validate(mut self) -> Result<Self, DescriptorError> {
        self.knockback.validate("components.behavior.knockback")?;
        validate_positive("moveSpeed", self.move_speed)?;
        if let Some(radius) = self.engagement_radius {
            validate_positive("engagementRadius", radius)?;
        }
        if let Some(retaliation) = self.retaliation {
            validate_non_negative("retaliation.windowMs", retaliation.window_ms)?;
            validate_non_negative("retaliation.damageWeight", retaliation.damage_weight)?;
            validate_non_negative("retaliation.recencyWeight", retaliation.recency_weight)?;
        }
        validate_attacks(&self.attacks)?;
        validate_patrol(self.patrol.as_ref())?;

        if let Some(filter) = self.candidate_filter.as_ref() {
            let program =
                bind_candidate_filter(filter).map_err(|error| DescriptorError::InvalidShape {
                    reason: format!("`components.behavior.candidateFilter` is invalid: {error}"),
                })?;
            if program.root_type != IrType::Bool {
                return Err(DescriptorError::InvalidShape {
                    reason: format!(
                        "`components.behavior.candidateFilter` must produce a boolean, but its root produces {:?}",
                        program.root_type
                    ),
                });
            }
        }

        validate_envelope(
            &mut self.envelope,
            "components.behavior",
            1,
            &self.attacks,
            self.patrol.as_ref(),
        )?;
        validate_attack_turn_marks(&self.envelope, "components.behavior", &self.attacks)?;

        for lint in behavior_lints::inspect(&self) {
            match lint.kind {
                behavior_lints::BehaviorLintKind::UnreachableActivity => log::warn!(
                    "components.behavior: activities [{}] are unreachable at `{}`",
                    lint.activities.join(", "),
                    lint.envelope_path,
                ),
            }
        }
        Ok(self)
    }
}

/// The position-goal and action verbs an activity can contribute while that
/// activity is selected. Paths stay authored-path precise so a parent layer
/// can explain exactly which concurrent verb made the graph invalid.
struct ActivePathVerbs {
    position_goal_path: Option<String>,
    action_path: Option<String>,
}

fn validate_envelope(
    envelope: &mut BehaviorGraphEnvelope,
    path: &str,
    depth: usize,
    attacks: &BTreeMap<String, AttackParams>,
    patrol: Option<&PatrolDescriptor>,
) -> Result<Vec<ActivePathVerbs>, DescriptorError> {
    if depth > MAX_BEHAVIOR_NESTING_DEPTH {
        return Err(DescriptorError::InvalidShape {
            reason: format!(
                "`{path}` exceeds MAX_BEHAVIOR_NESTING_DEPTH ({MAX_BEHAVIOR_NESTING_DEPTH})"
            ),
        });
    }
    if envelope.activities.is_empty() {
        return Err(DescriptorError::InvalidShape {
            reason: format!("`{path}.activities` must declare at least one activity"),
        });
    }
    if !envelope.activities.contains_key(&envelope.initial) {
        return Err(DescriptorError::InvalidShape {
            reason: format!(
                "`{path}.initial` (\"{}\") does not name a declared activity",
                envelope.initial
            ),
        });
    }
    if envelope.activities.contains_key("*") {
        return Err(DescriptorError::InvalidShape {
            reason: format!("`{path}.activities.*` is reserved for the scope-all transitions key"),
        });
    }

    for (source, rows) in &envelope.transitions {
        if source != "*" && !envelope.activities.contains_key(source) {
            return Err(DescriptorError::InvalidShape {
                reason: format!("`{path}.transitions.{source}` does not name a declared activity"),
            });
        }
        for (index, row) in rows.iter().enumerate() {
            let row_path = format!("{path}.transitions.{source}[{index}]");
            if source == "*" && row.to == "*" {
                return Err(DescriptorError::InvalidShape {
                    reason: format!("`{row_path}.to` cannot target the `*` scope-all key"),
                });
            }
            if !envelope.activities.contains_key(&row.to) {
                return Err(DescriptorError::InvalidShape {
                    reason: format!(
                        "`{row_path}.to` (\"{}\") does not name a declared activity at this level",
                        row.to
                    ),
                });
            }
            if source != "*" && source == &row.to {
                return Err(DescriptorError::InvalidShape {
                    reason: format!(
                        "`{row_path}.to` (\"{}\") names the activity that declares it; a source-keyed transition cannot target itself",
                        row.to
                    ),
                });
            }
            validate_guard(&row.when, &format!("{row_path}.when"))?;
            validate_queue(source, row, attacks, &row_path)?;
        }
        validate_release_rows(source, rows, &format!("{path}.transitions.{source}"))?;
        validate_moment_rows(
            source,
            rows,
            envelope.activities.get(source),
            &format!("{path}.transitions.{source}"),
        )?;
    }
    for (name, activity) in &envelope.activities {
        validate_stagger_to(
            name,
            activity,
            envelope,
            &format!("{path}.activities.{name}"),
        )?;
        validate_passes(
            activity,
            envelope.transitions.get(name).map(Vec::as_slice),
            &format!("{path}.activities.{name}"),
        )?;
    }

    envelope
        .activities
        .iter_mut()
        .map(|(name, activity)| {
            validate_activity(
                activity,
                &format!("{path}.activities.{name}"),
                depth,
                attacks,
                patrol,
            )
        })
        .collect()
}

/// A hold's release row (`releaseAfter`): at most one per activity, so each
/// entry draws one unambiguous release time, never on the `"*"` rows, which
/// belong to no activity, and bounded by a finite, non-negative, ordered range.
fn validate_release_rows(
    source: &str,
    rows: &[GuardedRow],
    path: &str,
) -> Result<(), DescriptorError> {
    let mut release_row = None;
    for (index, row) in rows.iter().enumerate() {
        let Some([min, max]) = row.release_after else {
            continue;
        };
        let row_path = format!("{path}[{index}].releaseAfter");
        if source == "*" {
            return Err(DescriptorError::InvalidShape {
                reason: format!(
                    "`{row_path}` belongs to an activity's own rows; a `*` row has no hold to draw a release time for"
                ),
            });
        }
        if let Some(first) = release_row {
            return Err(DescriptorError::InvalidShape {
                reason: format!(
                    "`{row_path}` is a second release row; `{path}[{first}]` already releases this activity"
                ),
            });
        }
        release_row = Some(index);
        if !min.is_finite() || !max.is_finite() || min < 0.0 || max < 0.0 {
            return Err(DescriptorError::InvalidShape {
                reason: format!(
                    "`{row_path}` bounds must be finite values >= 0.0, got [{min}, {max}]"
                ),
            });
        }
        if min > max {
            return Err(DescriptorError::InvalidShape {
                reason: format!("`{row_path}` minimum {min} exceeds its maximum {max}"),
            });
        }
    }
    Ok(())
}

/// A release's moment-gated row (`afterMoment`) sits on a leaf's own rows:
/// the gate reads the clip that leaf draws, and the `"*"` rows belong to no
/// activity.
fn validate_moment_rows(
    source: &str,
    rows: &[GuardedRow],
    activity: Option<&BehaviorActivityDescriptor>,
    path: &str,
) -> Result<(), DescriptorError> {
    let Some(index) = rows.iter().position(|row| row.after_moment) else {
        return Ok(());
    };
    let row_path = format!("{path}[{index}].afterMoment");
    if source == "*" {
        return Err(DescriptorError::InvalidShape {
            reason: format!(
                "`{row_path}` belongs to a release's own rows; a `*` row has no moment to wait on"
            ),
        });
    }
    if activity.is_some_and(|activity| !activity.layers.is_empty()) {
        return Err(DescriptorError::InvalidShape {
            reason: format!(
                "`{row_path}` belongs to a leaf's rows; `{source}` is a composite and draws no clip of its own"
            ),
        });
    }
    Ok(())
}

/// A flinch route (`staggerTo`) is a leaf's, and names another activity at
/// its own level.
fn validate_stagger_to(
    name: &str,
    activity: &BehaviorActivityDescriptor,
    envelope: &BehaviorGraphEnvelope,
    path: &str,
) -> Result<(), DescriptorError> {
    let Some(target) = activity.stagger_to.as_deref() else {
        return Ok(());
    };
    let field = format!("{path}.staggerTo");
    if !activity.layers.is_empty() {
        return Err(DescriptorError::InvalidShape {
            reason: format!("`{field}` belongs to a leaf activity"),
        });
    }
    if target == name || !envelope.activities.contains_key(target) {
        return Err(DescriptorError::InvalidShape {
            reason: format!(
                "`{field}` (\"{target}\") must name another activity declared at this level"
            ),
        });
    }
    Ok(())
}

/// A release's pass count (`passes`) is 1 to [`MAX_RELEASE_REPEAT`], on a
/// leaf whose own action fires at a clip event (each pass strikes at its
/// moment) and whose own rows wait on it (`afterMoment`): a stance release.
fn validate_passes(
    activity: &BehaviorActivityDescriptor,
    rows: Option<&[GuardedRow]>,
    path: &str,
) -> Result<(), DescriptorError> {
    let Some(passes) = activity.passes else {
        return Ok(());
    };
    let field = format!("{path}.passes");
    if !(1..=MAX_RELEASE_REPEAT).contains(&passes) {
        return Err(DescriptorError::InvalidShape {
            reason: format!("`{field}` must be 1 to {MAX_RELEASE_REPEAT}, got {passes}"),
        });
    }
    if !matches!(
        activity.action,
        Some(ActionVerb::Attack { at: Some(_), .. })
    ) {
        return Err(DescriptorError::InvalidShape {
            reason: format!(
                "`{field}` belongs to a leaf whose own action fires `at` a clip event: each pass strikes at its moment"
            ),
        });
    }
    if !rows.is_some_and(|rows| rows.iter().any(|row| row.after_moment)) {
        return Err(DescriptorError::InvalidShape {
            reason: format!(
                "`{field}` belongs to a stance release, whose own rows wait on its moment (`afterMoment`)"
            ),
        });
    }
    Ok(())
}

/// A queued row (`queue`) names an attack the graph declares, and sits on an
/// activity's own rows: a strike leaves only out of its stance's rest, never
/// from the `"*"` rows, which belong to no activity.
fn validate_queue(
    source: &str,
    row: &GuardedRow,
    attacks: &BTreeMap<String, AttackParams>,
    row_path: &str,
) -> Result<(), DescriptorError> {
    let Some(attack) = row.queue.as_deref() else {
        return Ok(());
    };
    let queue_path = format!("{row_path}.queue");
    if source == "*" {
        return Err(DescriptorError::InvalidShape {
            reason: format!(
                "`{queue_path}` belongs to a rest's own rows; a strike never leaves from a `*` row"
            ),
        });
    }
    if !attacks.contains_key(attack) {
        return Err(DescriptorError::InvalidShape {
            reason: format!(
                "`{queue_path}` (\"{attack}\") does not name an attack in the behavior's `attacks`"
            ),
        });
    }
    Ok(())
}

fn validate_activity(
    activity: &mut BehaviorActivityDescriptor,
    path: &str,
    depth: usize,
    attacks: &BTreeMap<String, AttackParams>,
    patrol: Option<&PatrolDescriptor>,
) -> Result<ActivePathVerbs, DescriptorError> {
    if let Some(sound) = activity.sound.as_deref() {
        validate_sound_key(&format!("{path}.sound"), sound)?;
    }
    if let Some(speed) = activity.speed
        && (!speed.is_finite() || speed <= 0.0)
    {
        return Err(DescriptorError::InvalidShape {
            reason: format!("`{path}.speed` must be a finite value > 0.0, got {speed}"),
        });
    }
    if activity.pose.as_ref().is_some_and(String::is_empty) {
        return Err(DescriptorError::InvalidShape {
            reason: format!("`{path}.pose` must be a non-empty string when supplied"),
        });
    }
    validate_reposition(activity, path)?;
    if !activity.layers.is_empty() {
        if activity.motion.is_some() {
            return Err(DescriptorError::InvalidShape {
                reason: format!("`{path}.motion` belongs to a leaf; composites author `layers`"),
            });
        }
        if activity.action.is_some() {
            return Err(DescriptorError::InvalidShape {
                reason: format!(
                    "`{path}.action` belongs to a leaf; composites author an `offense` layer"
                ),
            });
        }
        if activity.on_enter.is_some() {
            return Err(DescriptorError::InvalidShape {
                reason: format!("`{path}.onEnter` belongs to a leaf activity"),
            });
        }
        if activity.sound.is_some() {
            return Err(DescriptorError::InvalidShape {
                reason: format!("`{path}.sound` belongs to a leaf activity"),
            });
        }
        if activity.animation.as_ref().is_some_and(String::is_empty) {
            return Err(DescriptorError::InvalidShape {
                reason: format!("`{path}.animation` must be a non-empty string when supplied"),
            });
        }
        let stateful_layers = activity
            .layers
            .values()
            .filter(|layer| matches!(layer, BehaviorLayerDescriptor::Graph(_)))
            .count();
        if stateful_layers > 1 {
            return Err(DescriptorError::InvalidShape {
                reason: format!(
                    "`{path}.layers` may contain at most one nested-graph (stateful) layer; \
                     selector layers remain unlimited"
                ),
            });
        }
        let mut nested_paths = Vec::new();
        for (name, layer) in &mut activity.layers {
            if let Some(paths) = validate_layer(
                layer,
                &format!("{path}.layers.{name}"),
                name,
                depth,
                attacks,
                patrol,
            )? {
                nested_paths = paths;
            }
        }

        let position_goal_path = first_local_position_goal_path(activity, path);
        let action_path = first_local_action_path(activity, path);
        reject_concurrent_position_goal_and_action(
            position_goal_path.as_deref(),
            action_path.as_deref(),
        )?;

        // Selector layers are always active with this composite. A nested graph
        // supplies only one of its activities, though, so compare the local
        // layers with each possible child path rather than merging sibling
        // activities into one impossible simultaneous state.
        for child in &nested_paths {
            reject_concurrent_position_goal_and_action(
                position_goal_path.as_deref(),
                child.action_path.as_deref(),
            )?;
            reject_concurrent_position_goal_and_action(
                child.position_goal_path.as_deref(),
                action_path.as_deref(),
            )?;
        }

        return Ok(ActivePathVerbs {
            position_goal_path: position_goal_path.or_else(|| {
                nested_paths
                    .iter()
                    .find_map(|path| path.position_goal_path.clone())
            }),
            action_path: action_path.or_else(|| {
                nested_paths
                    .iter()
                    .find_map(|path| path.action_path.clone())
            }),
        });
    }

    if activity.animation.as_ref().is_none_or(String::is_empty) {
        return Err(DescriptorError::InvalidShape {
            reason: format!("`{path}.animation` must be a non-empty string for a leaf activity"),
        });
    }
    if let Some(motion) = activity.motion {
        if motion == MotionVerb::Leap {
            validate_leap(activity, path)?;
        } else {
            validate_motion(motion, &format!("{path}.motion"), patrol)?;
        }
        if motion.is_position_goal() && activity.action.is_some() {
            return Err(DescriptorError::InvalidShape {
                reason: format!(
                    "`{path}.action` must be omitted when `{path}.motion` is a position-goal verb; position-goal activities are non-engaged"
                ),
            });
        }
    }
    if let Some(action) = activity.action.as_ref() {
        validate_action(action, &format!("{path}.action"), attacks)?;
    }
    imply_attack_turn(activity, path)?;
    Ok(ActivePathVerbs {
        position_goal_path: activity
            .motion
            .is_some_and(MotionVerb::is_position_goal)
            .then(|| format!("{path}.motion")),
        action_path: activity.action.as_ref().map(|_| format!("{path}.action")),
    })
}

fn reject_concurrent_position_goal_and_action(
    position_goal_path: Option<&str>,
    action_path: Option<&str>,
) -> Result<(), DescriptorError> {
    if let (Some(position_goal_path), Some(action_path)) = (position_goal_path, action_path) {
        return Err(DescriptorError::InvalidShape {
            reason: format!(
                "`{action_path}` must be omitted because `{position_goal_path}` is a position-goal verb; position-goal activities are non-engaged"
            ),
        });
    }
    Ok(())
}

fn first_local_position_goal_path(
    activity: &BehaviorActivityDescriptor,
    path: &str,
) -> Option<String> {
    if activity.motion.is_some_and(MotionVerb::is_position_goal) {
        return Some(format!("{path}.motion"));
    }

    if let Some(BehaviorLayerDescriptor::Selector(entries)) = activity.layers.get("move")
        && let Some(position_goal_path) =
            entries
                .iter()
                .enumerate()
                .find_map(|(index, entry)| match entry {
                    BehaviorSelectorEntry::Motion(motion) if motion.is_position_goal() => {
                        Some(format!("{path}.layers.move[{index}]"))
                    }
                    BehaviorSelectorEntry::Row(row)
                        if row.motion.is_some_and(MotionVerb::is_position_goal) =>
                    {
                        Some(format!("{path}.layers.move[{index}].motion"))
                    }
                    BehaviorSelectorEntry::Motion(_) | BehaviorSelectorEntry::Row(_) => None,
                })
    {
        return Some(position_goal_path);
    }

    None
}

fn first_local_action_path(activity: &BehaviorActivityDescriptor, path: &str) -> Option<String> {
    if activity.action.is_some() {
        return Some(format!("{path}.action"));
    }

    if let Some(BehaviorLayerDescriptor::Selector(entries)) = activity.layers.get("offense")
        && let Some(index) = entries.iter().position(
            |entry| matches!(entry, BehaviorSelectorEntry::Row(row) if row.action.is_some()),
        )
    {
        return Some(format!("{path}.layers.offense[{index}].action"));
    }

    None
}

fn validate_layer(
    layer: &mut BehaviorLayerDescriptor,
    path: &str,
    layer_name: &str,
    depth: usize,
    attacks: &BTreeMap<String, AttackParams>,
    patrol: Option<&PatrolDescriptor>,
) -> Result<Option<Vec<ActivePathVerbs>>, DescriptorError> {
    match layer {
        BehaviorLayerDescriptor::Graph(envelope) => {
            if layer_name == "move" {
                return Err(DescriptorError::InvalidShape {
                    reason: format!("`{path}` must be a move selector list, not a nested graph"),
                });
            }
            validate_envelope(envelope, path, depth + 1, attacks, patrol).map(Some)
        }
        BehaviorLayerDescriptor::Selector(entries) => {
            if layer_name == "move" {
                validate_move_selector(entries, path, patrol)?;
            } else if layer_name == "offense" {
                validate_offense_selector(entries, path, attacks)?;
            } else {
                validate_unconsumed_selector(entries, path, attacks, patrol)?;
            }
            Ok(None)
        }
    }
}

/// Preserve selector layers owned by future or external consumers. The AI
/// evaluator only assigns meaning to `move` and `offense`, but every selector
/// still validates the shared guard and closed verb vocabulary at load time.
fn validate_unconsumed_selector(
    entries: &[BehaviorSelectorEntry],
    path: &str,
    attacks: &BTreeMap<String, AttackParams>,
    patrol: Option<&PatrolDescriptor>,
) -> Result<(), DescriptorError> {
    for (index, entry) in entries.iter().enumerate() {
        match entry {
            BehaviorSelectorEntry::Motion(motion) => {
                validate_motion(*motion, &format!("{path}[{index}]"), patrol)?;
            }
            BehaviorSelectorEntry::Row(row) => {
                if let Some(when) = row.when.as_ref() {
                    validate_guard(when, &format!("{path}[{index}].when"))?;
                }
                if let Some(motion) = row.motion {
                    validate_motion(motion, &format!("{path}[{index}].motion"), patrol)?;
                }
                if let Some(action) = row.action.as_ref() {
                    let action_path = format!("{path}[{index}].action");
                    reject_row_at(action, &action_path)?;
                    validate_action(action, &action_path, attacks)?;
                }
            }
        }
    }
    Ok(())
}

fn validate_move_selector(
    entries: &[BehaviorSelectorEntry],
    path: &str,
    patrol: Option<&PatrolDescriptor>,
) -> Result<(), DescriptorError> {
    let Some((fallback, rows)) = entries.split_last() else {
        return Err(DescriptorError::InvalidShape {
            reason: format!("`{path}` must end with a MotionVerb fallback"),
        });
    };
    let BehaviorSelectorEntry::Motion(motion) = fallback else {
        return Err(DescriptorError::InvalidShape {
            reason: format!("`{path}` must end with a MotionVerb fallback"),
        });
    };
    validate_motion(*motion, &format!("{path}[{}]", entries.len() - 1), patrol)?;
    for (index, entry) in rows.iter().enumerate() {
        let BehaviorSelectorEntry::Row(row) = entry else {
            return Err(DescriptorError::InvalidShape {
                reason: format!("`{path}[{index}]` must be a guarded move row before the fallback"),
            });
        };
        if row.when.is_none() || row.motion.is_none() || row.action.is_some() {
            return Err(DescriptorError::InvalidShape {
                reason: format!("`{path}[{index}]` must contain exactly `when` and `motion`"),
            });
        }
        validate_guard(
            row.when.as_ref().expect("checked above"),
            &format!("{path}[{index}].when"),
        )?;
        validate_motion(
            row.motion.expect("checked above"),
            &format!("{path}[{index}].motion"),
            patrol,
        )?;
    }
    Ok(())
}

fn validate_offense_selector(
    entries: &[BehaviorSelectorEntry],
    path: &str,
    attacks: &BTreeMap<String, AttackParams>,
) -> Result<(), DescriptorError> {
    for (index, entry) in entries.iter().enumerate() {
        let BehaviorSelectorEntry::Row(row) = entry else {
            return Err(DescriptorError::InvalidShape {
                reason: format!("`{path}[{index}]` must be a guarded offense row"),
            });
        };
        if row.action.is_none() || row.motion.is_some() {
            return Err(DescriptorError::InvalidShape {
                reason: format!("`{path}[{index}]` must contain `action` and no `motion`"),
            });
        }
        if let Some(when) = row.when.as_ref() {
            validate_guard(when, &format!("{path}[{index}].when"))?;
        }
        let action = row.action.as_ref().expect("checked above");
        let action_path = format!("{path}[{index}].action");
        reject_row_at(action, &action_path)?;
        validate_action(action, &action_path, attacks)?;
    }
    Ok(())
}

fn validate_guard(when: &IrNode, path: &str) -> Result<(), DescriptorError> {
    let program = bind_brain_guard(when).map_err(|error| DescriptorError::InvalidShape {
        reason: format!("`{path}` guard is invalid: {error}"),
    })?;
    if program.root_type != IrType::Bool {
        return Err(DescriptorError::InvalidShape {
            reason: format!(
                "`{path}` guard must produce a boolean, but its root produces {:?}",
                program.root_type
            ),
        });
    }
    Ok(())
}

fn validate_action(
    action: &ActionVerb,
    path: &str,
    attacks: &BTreeMap<String, AttackParams>,
) -> Result<(), DescriptorError> {
    let ActionVerb::Attack { name, .. } = action;
    if attacks.is_empty() {
        return Err(DescriptorError::InvalidShape {
            reason: format!(
                "`{path}.attack` names \"{name}\", so `components.behavior.attacks` must declare at least one entry"
            ),
        });
    }
    if !attacks.contains_key(name) {
        return Err(DescriptorError::InvalidShape {
            reason: format!(
                "`{path}.attack` names \"{name}\", which is not declared in `components.behavior.attacks`"
            ),
        });
    }
    validate_at_name(action, path)
}

/// A leaf's own `leap`: it paces itself to arrive by its own attack's clip
/// moment, so the leaf's action must fire `at` a clip event, and the moment
/// comes once: a repeating release (`passes` above 1) has no one moment to
/// arrive by.
fn validate_leap(activity: &BehaviorActivityDescriptor, path: &str) -> Result<(), DescriptorError> {
    if !matches!(
        activity.action,
        Some(ActionVerb::Attack { at: Some(_), .. })
    ) {
        return Err(DescriptorError::InvalidShape {
            reason: format!(
                "`{path}.motion` is \"leap\", which arrives at its attack's clip moment, so `{path}.action` must name an attack with `at`"
            ),
        });
    }
    if activity.passes.is_some_and(|passes| passes > 1) {
        return Err(DescriptorError::InvalidShape {
            reason: format!(
                "`{path}.motion` is \"leap\", which arrives at one clip moment, so `{path}.passes` must be 1"
            ),
        });
    }
    Ok(())
}

/// A selector's motion, or a leaf's other than `leap`. A selector cannot
/// select `leap`: it arrives at its own leaf's clip moment.
fn validate_motion(
    motion: MotionVerb,
    path: &str,
    patrol: Option<&PatrolDescriptor>,
) -> Result<(), DescriptorError> {
    if motion == MotionVerb::Leap {
        return Err(DescriptorError::InvalidShape {
            reason: format!(
                "`{path}` is \"leap\", which arrives at its own leaf's clip moment; give it to a leaf's `motion` beside an attack with `at`, not a selector"
            ),
        });
    }
    if motion == MotionVerb::Patrol {
        match patrol {
            Some(patrol) if !patrol.points.is_empty() => Ok(()),
            Some(_) => Err(DescriptorError::InvalidShape {
                reason: format!(
                    "`{path}` is \"patrol\", so `components.behavior.patrol.points` must declare at least one point"
                ),
            }),
            None => Err(DescriptorError::InvalidShape {
                reason: format!(
                    "`{path}` is \"patrol\", so `components.behavior.patrol` is required"
                ),
            }),
        }
    } else {
        Ok(())
    }
}

fn validate_attacks(attacks: &BTreeMap<String, AttackParams>) -> Result<(), DescriptorError> {
    for (name, attack) in attacks {
        let inline_stats = [
            ("damage", attack.damage),
            ("maxRange", attack.max_range),
            ("queueRange", attack.queue_range),
            ("cooldownMs", attack.cooldown_ms),
        ];
        if attack.weapon.is_some() {
            if attack.damage_type.is_some() {
                return Err(DescriptorError::InvalidShape {
                    reason: format!(
                        "`components.behavior.attacks.{name}.damageType` must be omitted when `components.behavior.attacks.{name}.weapon` is present; a weapon attack deals its weapon's type"
                    ),
                });
            }
            if let Some((field, _)) = inline_stats.iter().find(|(_, value)| value.is_some()) {
                return Err(DescriptorError::InvalidShape {
                    reason: format!(
                        "`components.behavior.attacks.{name}.{field}` must be omitted when `components.behavior.attacks.{name}.weapon` is present"
                    ),
                });
            }
        } else {
            let damage = attack.damage.ok_or_else(|| DescriptorError::InvalidShape {
                reason: format!(
                    "`components.behavior.attacks.{name}.damage` is required when `components.behavior.attacks.{name}.weapon` is absent"
                ),
            })?;
            let max_range = attack.max_range.ok_or_else(|| DescriptorError::InvalidShape {
                reason: format!(
                    "`components.behavior.attacks.{name}.maxRange` is required when `components.behavior.attacks.{name}.weapon` is absent"
                ),
            })?;
            let cooldown_ms = attack.cooldown_ms.ok_or_else(|| DescriptorError::InvalidShape {
                reason: format!(
                    "`components.behavior.attacks.{name}.cooldownMs` is required when `components.behavior.attacks.{name}.weapon` is absent"
                ),
            })?;

            if !damage.is_finite() || damage < 0.0 {
                return Err(DescriptorError::InvalidShape {
                    reason: format!(
                        "`components.behavior.attacks.{name}.damage` must be a finite value >= 0.0, got {damage}"
                    ),
                });
            }
            validate_positive(&format!("attacks.{name}.maxRange"), max_range)?;
            validate_positive(&format!("attacks.{name}.cooldownMs"), cooldown_ms)?;
            if let Some(queue_range) = attack.queue_range
                && (!queue_range.is_finite() || queue_range < max_range)
            {
                return Err(DescriptorError::InvalidShape {
                    reason: format!(
                        "`components.behavior.attacks.{name}.queueRange` must be a finite value >= `components.behavior.attacks.{name}.maxRange` ({max_range}), got {queue_range}; it is how far out the attack waits in line, and `maxRange` how far it hits"
                    ),
                });
            }

            if let Some(radius) = attack.engagement_radius {
                validate_positive(&format!("attacks.{name}.engagementRadius"), radius)?;
                if radius > max_range {
                    return Err(DescriptorError::InvalidShape {
                        reason: format!(
                            "`components.behavior.attacks.{name}.engagementRadius` must be <= `components.behavior.attacks.{name}.maxRange` ({max_range}), got {radius}"
                        ),
                    });
                }
            }
        }
        if attack.weapon.is_some()
            && let Some(radius) = attack.engagement_radius
        {
            validate_positive(&format!("attacks.{name}.engagementRadius"), radius)?;
        }
        if let Some(standoff_distance) = attack.standoff_distance {
            validate_positive(
                &format!("attacks.{name}.standoffDistance"),
                standoff_distance,
            )?;
        }
        if let Some(sound) = attack.sound.as_deref() {
            validate_sound_key(&format!("components.behavior.attacks.{name}.sound"), sound)?;
        }
        if let Some(draws) = &attack.draws {
            crate::attack_budget::check_draw_shapes(
                &format!("components.behavior.attacks.{name}.draws"),
                draws,
            )
            .map_err(|reason| DescriptorError::InvalidShape { reason })?;
        }
    }
    Ok(())
}

fn validate_patrol(patrol: Option<&PatrolDescriptor>) -> Result<(), DescriptorError> {
    if let Some(patrol) = patrol {
        for (index, point) in patrol.points.iter().enumerate() {
            if !point[0].is_finite() || !point[1].is_finite() {
                return Err(DescriptorError::InvalidShape {
                    reason: format!(
                        "`components.behavior.patrol.points[{index}]` must contain finite x/z components, got {point:?}"
                    ),
                });
            }
        }
    }
    Ok(())
}

fn validate_positive(field: &str, value: f32) -> Result<(), DescriptorError> {
    if !value.is_finite() || value <= 0.0 {
        return Err(DescriptorError::InvalidShape {
            reason: format!(
                "`components.behavior.{field}` must be a finite value > 0.0, got {value}"
            ),
        });
    }
    Ok(())
}

fn validate_non_negative(field: &str, value: f32) -> Result<(), DescriptorError> {
    if !value.is_finite() || value < 0.0 {
        return Err(DescriptorError::InvalidShape {
            reason: format!(
                "`components.behavior.{field}` must be a finite value >= 0.0, got {value}"
            ),
        });
    }
    Ok(())
}

/// Activities declared in this envelope but not referenced by its `initial` or
/// by any incoming row. This is advisory, not a structural parse error.
pub(crate) fn unreachable_activities(envelope: &BehaviorGraphEnvelope) -> Vec<String> {
    let mut reachable = BTreeSet::from([envelope.initial.as_str()]);
    for rows in envelope.transitions.values() {
        for row in rows {
            reachable.insert(&row.to);
        }
    }
    envelope
        .activities
        .keys()
        .filter(|name| !reachable.contains(name.as_str()))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // S5: a row may omit `when`; it is then always enabled, and still
    // rejects a field it does not know.
    #[test]
    fn a_row_without_when_is_always_enabled_and_still_denies_unknown_fields() {
        let graph: BehaviorGraphDescriptor = serde_json::from_value(serde_json::json!({
            "initial": "idle",
            "activities": {
                "idle": { "animation": "idle" },
                "walk": { "animation": "walk" }
            },
            "transitions": { "idle": [{ "to": "walk" }] },
            "moveSpeed": 3.0
        }))
        .expect("a row without `when` deserializes");
        assert_eq!(
            graph.envelope.transitions["idle"][0].when,
            IrNode::Const {
                value: crate::ir::IrValue::Bool(true)
            }
        );
        graph.validate().expect("an always-enabled row validates");

        let unknown = serde_json::from_value::<BehaviorGraphDescriptor>(serde_json::json!({
            "initial": "idle",
            "activities": { "idle": { "animation": "idle" } },
            "transitions": { "idle": [{ "to": "idle", "wait": "slam" }] },
            "moveSpeed": 3.0
        }))
        .expect_err("an unknown row field rejects");
        assert!(unknown.to_string().contains("wait"), "{unknown}");
    }

    #[test]
    fn standoff_distance_defaults_to_per_attack_engagement_radius_override() {
        let graph = BehaviorGraphDescriptor {
            ai: Default::default(),
            knockback: Default::default(),
            envelope: BehaviorGraphEnvelope {
                initial: "idle".to_string(),
                activities: BTreeMap::new(),
                transitions: BTreeMap::new(),
            },
            candidate_filter: None,
            retaliation: None,
            patrol: None,
            attacks: BTreeMap::from([(
                "slam".to_string(),
                AttackParams {
                    sound: None,
                    weapon: None,
                    damage: Some(1.0),
                    max_range: Some(4.0),
                    cooldown_ms: Some(1.0),
                    engagement_radius: Some(3.0),
                    standoff_distance: None,
                    queue_range: None,
                    damage_type: None,
                    draws: None,
                },
            )]),
            engagement_radius: Some(2.0),
            move_speed: 1.0,
        };
        let action = ActionVerb::attack("slam");

        assert_eq!(graph.engagement_radius_for_action(Some(&action)), 3.0);
        assert_eq!(graph.standoff_distance_for_action(Some(&action)), 3.0);

        let mut explicit_standoff = graph.clone();
        explicit_standoff
            .attacks
            .get_mut("slam")
            .expect("fixture attack exists")
            .standoff_distance = Some(1.5);
        assert_eq!(
            explicit_standoff.standoff_distance_for_action(Some(&action)),
            1.5
        );
    }

    #[test]
    fn retaliation_descriptor_defaults_and_validates_finite_non_negative_scalars() {
        let mut graph: BehaviorGraphDescriptor = serde_json::from_value(serde_json::json!({
            "initial": "idle",
            "activities": { "idle": { "animation": "idle" } },
            "transitions": {},
            "moveSpeed": 3.0,
            "retaliation": {
                "windowMs": 750.0,
                "damageWeight": 2.0,
                "recencyWeight": 0.25
            }
        }))
        .expect("retaliation block deserializes");
        assert_eq!(
            graph.retaliation(),
            RetaliationDescriptor {
                window_ms: 750.0,
                damage_weight: 2.0,
                recency_weight: 0.25,
            }
        );

        graph.retaliation = Some(RetaliationDescriptor {
            window_ms: -1.0,
            ..RetaliationDescriptor::default()
        });
        let error = graph.validate().expect_err("negative window rejects");
        assert!(
            error.to_string().contains("retaliation.windowMs"),
            "{error}"
        );

        let defaults: BehaviorGraphDescriptor = serde_json::from_value(serde_json::json!({
            "initial": "idle",
            "activities": { "idle": { "animation": "idle" } },
            "transitions": {},
            "moveSpeed": 3.0
        }))
        .expect("omitted retaliation block retains defaults");
        assert_eq!(defaults.retaliation, None);
        assert_eq!(defaults.retaliation(), RetaliationDescriptor::default());
    }

    #[test]
    fn validate_attacks_rejects_non_positive_standoff_distance() {
        let attacks = BTreeMap::from([(
            "slam".to_string(),
            AttackParams {
                sound: None,
                weapon: None,
                damage: Some(1.0),
                max_range: Some(4.0),
                cooldown_ms: Some(1.0),
                engagement_radius: None,
                standoff_distance: Some(0.0),
                queue_range: None,
                damage_type: None,
                draws: None,
            },
        )]);

        let error = validate_attacks(&attacks).expect_err("zero standoff must reject");
        assert!(
            error.to_string().contains("attacks.slam.standoffDistance"),
            "{error}"
        );
    }

    #[test]
    fn always_active_is_an_optional_root_boolean_that_keeps_default_bytes() {
        let graph = |extra: serde_json::Value| {
            let mut json = serde_json::json!({
                "initial": "idle",
                "moveSpeed": 3.0,
                "activities": { "idle": { "animation": "idle", "motion": "hold" } },
                "transitions": {},
            });
            json.as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            serde_json::from_value::<BehaviorGraphDescriptor>(json)
        };
        let absent = graph(serde_json::json!({})).expect("absent `alwaysActive` parses");
        assert!(!absent.ai.always_active);
        assert!(
            serde_json::to_value(&absent)
                .unwrap()
                .get("alwaysActive")
                .is_none(),
            "an unauthored `alwaysActive` adds no bytes"
        );
        let on = graph(serde_json::json!({ "alwaysActive": true })).expect("`true` parses");
        assert!(on.ai.always_active);
        assert_eq!(
            serde_json::to_value(&on).unwrap()["alwaysActive"],
            serde_json::json!(true)
        );
        for bad in [serde_json::json!("yes"), serde_json::json!(1)] {
            assert!(
                graph(serde_json::json!({ "alwaysActive": bad })).is_err(),
                "`alwaysActive: {bad}` must be rejected"
            );
        }
    }

    #[test]
    fn validate_attacks_accepts_weapon_entries_and_preserves_contact_wire_shape() {
        let contact: AttackParams = serde_json::from_value(serde_json::json!({
            "damage": 8.0,
            "maxRange": 2.0,
            "cooldownMs": 1200.0,
            "engagementRadius": 1.5,
            "standoffDistance": 1.0,
        }))
        .expect("legacy contact attack must deserialize");
        validate_attacks(&BTreeMap::from([("claw".to_string(), contact.clone())]))
            .expect("legacy contact attack must remain valid");
        assert_eq!(
            serde_json::to_value(contact).expect("contact attack must serialize"),
            serde_json::json!({
                "damage": 8.0,
                "maxRange": 2.0,
                "cooldownMs": 1200.0,
                "engagementRadius": 1.5,
                "standoffDistance": 1.0,
            })
        );

        let weapon: AttackParams = serde_json::from_value(serde_json::json!({
            "weapon": "enemy_rifle",
            "engagementRadius": 12.0,
            "standoffDistance": 8.0,
        }))
        .expect("weapon attack must deserialize");
        validate_attacks(&BTreeMap::from([("shoot".to_string(), weapon.clone())]))
            .expect("weapon attack must be valid without inline contact stats");
        assert_eq!(
            serde_json::to_value(weapon).expect("weapon attack must serialize"),
            serde_json::json!({
                "weapon": "enemy_rifle",
                "engagementRadius": 12.0,
                "standoffDistance": 8.0,
            })
        );
    }

    #[test]
    fn validate_attacks_rejects_each_weapon_inline_stat_conflict_by_field_name() {
        for (field, attack) in [
            (
                "damage",
                AttackParams {
                    sound: None,
                    weapon: Some("enemy_rifle".to_string()),
                    damage: Some(8.0),
                    max_range: None,
                    cooldown_ms: None,
                    engagement_radius: None,
                    standoff_distance: None,
                    queue_range: None,
                    damage_type: None,
                    draws: None,
                },
            ),
            (
                "maxRange",
                AttackParams {
                    sound: None,
                    weapon: Some("enemy_rifle".to_string()),
                    damage: None,
                    max_range: Some(12.0),
                    cooldown_ms: None,
                    engagement_radius: None,
                    standoff_distance: None,
                    queue_range: None,
                    damage_type: None,
                    draws: None,
                },
            ),
            (
                "cooldownMs",
                AttackParams {
                    sound: None,
                    weapon: Some("enemy_rifle".to_string()),
                    damage: None,
                    max_range: None,
                    cooldown_ms: Some(250.0),
                    engagement_radius: None,
                    standoff_distance: None,
                    queue_range: None,
                    damage_type: None,
                    draws: None,
                },
            ),
        ] {
            let error = validate_attacks(&BTreeMap::from([("shoot".to_string(), attack)]))
                .expect_err("weapon attack must reject an inline contact stat");
            assert!(
                error
                    .to_string()
                    .contains(&format!("attacks.shoot.{field}")),
                "{error}"
            );
        }
    }

    #[test]
    fn nested_graph_and_selector_wire_shapes_deserialize() {
        let graph: BehaviorGraphDescriptor = serde_json::from_value(serde_json::json!({
            "initial": "idle", "moveSpeed": 3.0,
            "activities": {
                "idle": { "animation": "idle" },
                "engage": { "layers": {
                    "move": ["hold"],
                    "offense": {
                        "initial": "windup",
                        "activities": { "windup": { "animation": "windup" } },
                        "transitions": {}
                    }
                }}
            },
            "transitions": {}
        }))
        .unwrap();
        assert!(matches!(
            graph.envelope.activities["engage"].layers["offense"],
            BehaviorLayerDescriptor::Graph(_)
        ));
    }

    #[test]
    fn arbitrary_selector_layer_names_validate_but_remain_unconsumed() {
        let graph: BehaviorGraphDescriptor = serde_json::from_value(serde_json::json!({
            "initial": "engage", "moveSpeed": 3.0,
            "activities": {
                "engage": {
                    "animation": "idle",
                    "layers": {
                        "presentation": [
                            { "when": { "op": "const", "value": true } },
                            "hold"
                        ]
                    }
                }
            },
            "transitions": {}
        }))
        .unwrap();

        let graph = graph
            .validate()
            .expect("selector names outside move/offense are reserved for other consumers");
        assert!(matches!(
            graph.envelope.activities["engage"].layers["presentation"],
            BehaviorLayerDescriptor::Selector(_)
        ));
    }

    #[test]
    fn duplicate_activity_keys_in_raw_json_are_rejected_at_every_envelope() {
        // JS objects and Luau tables collapse duplicate keys before their
        // bridges run. Raw JSON is the boundary where last-writer-wins would
        // otherwise remain silent.
        let root = r#"{
            "initial": "idle",
            "moveSpeed": 3.0,
            "activities": {
                "idle": { "animation": "idle" },
                "idle": { "animation": "walk" }
            },
            "transitions": {}
        }"#;
        let error = serde_json::from_str::<BehaviorGraphDescriptor>(root).unwrap_err();
        assert!(
            error.to_string().contains("duplicate activity name `idle`"),
            "{error}"
        );

        let nested_envelope = r#"{
            "initial": "windup",
            "activities": {
                "windup": { "animation": "windup" },
                "windup": { "animation": "recover" }
            },
            "transitions": {}
        }"#;
        let error = serde_json::from_str::<BehaviorGraphEnvelope>(nested_envelope).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("duplicate activity name `windup`"),
            "{error}"
        );

        let nested_graph = r#"{
            "initial": "engage",
            "moveSpeed": 3.0,
            "activities": {
                "engage": { "layers": {
                    "offense": {
                        "initial": "windup",
                        "activities": {
                            "windup": { "animation": "windup" },
                            "windup": { "animation": "recover" }
                        },
                        "transitions": {}
                    }
                } }
            },
            "transitions": {}
        }"#;
        assert!(
            serde_json::from_str::<BehaviorGraphDescriptor>(nested_graph).is_err(),
            "a duplicate inside a nested graph layer must reject the whole descriptor"
        );
    }

    /// A root `engage` composite carrying `speed` over a nested `run` leaf,
    /// for the speed validation and rate-scaled-state rows.
    fn speed_graph(
        engage_speed: serde_json::Value,
        run_speed: serde_json::Value,
    ) -> serde_json::Value {
        serde_json::json!({
            "initial": "idle",
            "moveSpeed": 3.0,
            "activities": {
                "idle": { "animation": "idle", "motion": "hold" },
                "engage": {
                    "speed": engage_speed,
                    "layers": {
                        "offense": {
                            "initial": "run",
                            "activities": {
                                "run": { "animation": "run", "motion": "chaseTarget", "speed": run_speed }
                            },
                            "transitions": {}
                        }
                    }
                }
            },
            "transitions": { "idle": [] }
        })
    }

    // AC S26 (activity speed row): a non-positive or non-finite `speed`
    // rejects load with the activity's path, on a composite and a leaf alike.
    #[test]
    fn activity_speed_rejects_zero_negative_and_non_finite_with_its_path() {
        let accepted: BehaviorGraphDescriptor =
            serde_json::from_value(speed_graph(serde_json::json!(2.0), serde_json::json!(0.5)))
                .expect("positive speeds deserialize");
        let accepted = accepted.validate().expect("positive speeds load");
        let engage = &accepted.envelope.activities["engage"];
        assert_eq!(engage.speed, Some(2.0));

        for bad in [0.0, -1.0, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            for leaf in [false, true] {
                let mut graph = accepted.clone();
                let engage = graph.envelope.activities.get_mut("engage").unwrap();
                let expected_path = if leaf {
                    let BehaviorLayerDescriptor::Graph(offense) =
                        engage.layers.get_mut("offense").unwrap()
                    else {
                        panic!("offense is a nested graph");
                    };
                    offense.activities.get_mut("run").unwrap().speed = Some(bad);
                    "components.behavior.activities.engage.layers.offense.activities.run.speed"
                } else {
                    engage.speed = Some(bad);
                    "components.behavior.activities.engage.speed"
                };
                let error = graph
                    .validate()
                    .expect_err("a non-positive or non-finite speed rejects");
                assert!(
                    error.to_string().contains(&format!("`{expected_path}`")),
                    "speed {bad}: {error}"
                );
            }
        }
    }

    // The rate-scaled set: the graph's locomotion state, plus each animation
    // an activity at any depth plays under its own steering motion, action or
    // not, position goals included. Holds, freezes and selector-supplied
    // motion add nothing.
    #[test]
    fn rate_scaled_animations_add_states_played_under_an_own_steering_motion() {
        use super::super::{RateScaledStates, any_rate_scaled_animation};

        let graph: BehaviorGraphDescriptor = serde_json::from_value(serde_json::json!({
            "initial": "idle",
            "moveSpeed": 3.0,
            "attacks": { "bite": { "damage": 1.0, "maxRange": 2.0, "cooldownMs": 500.0 } },
            "patrol": { "mode": "loop", "points": [[0.0, 0.0], [6.0, 0.0]] },
            "activities": {
                "idle": { "animation": "idle", "motion": "hold" },
                "engage": {
                    "animation": "walk",
                    "layers": {
                        "move": ["chaseTarget"],
                        "offense": {
                            "initial": "close",
                            "activities": {
                                "close": { "animation": "jog" },
                                "charge": {
                                    "animation": "run",
                                    "motion": "chaseTarget",
                                    "action": { "attack": "bite" }
                                },
                                "circle": { "animation": "strafe_walk", "motion": "strafe" },
                                "planted": { "animation": "aim", "motion": "hold" },
                                "stunned": { "animation": "stun", "motion": "freeze" }
                            },
                            "transitions": {}
                        }
                    }
                },
                "wander": { "animation": "stroll", "motion": "patrol" },
                "search": {
                    "animation": "idle",
                    "layers": {
                        "body": {
                            "initial": "sweep",
                            "activities": {
                                "sweep": { "animation": "creep", "motion": "moveToLastKnown" }
                            },
                            "transitions": {}
                        }
                    }
                },
                "retreat": { "animation": "limp", "motion": "moveToAnchor" }
            },
            "transitions": { "idle": [] }
        }))
        .expect("graph deserializes");
        let mut scaled = Vec::new();
        assert!(!any_rate_scaled_animation(&graph, |state| {
            if !scaled.contains(&state.to_string()) {
                scaled.push(state.to_string());
            }
            false
        }));
        scaled.sort();
        assert_eq!(
            scaled,
            ["creep", "limp", "run", "strafe_walk", "stroll", "walk"]
        );
        assert!(any_rate_scaled_animation(&graph, |state| state == "run"));
        assert!(!any_rate_scaled_animation(&graph, |state| state == "jog"));

        // The collected set names each state once, the same states the
        // visitor reaches.
        let set = RateScaledStates::of(&graph);
        let mut collected: Vec<&str> = set.iter().collect();
        collected.sort_unstable();
        assert_eq!(collected, scaled);
        assert!(
            set.iter()
                .all(|state| !["idle", "jog", "aim", "stun"].contains(&state))
        );
    }

    // Every verb decides whether it steers: all but the two planted ones.
    #[test]
    fn every_motion_but_hold_and_freeze_steers() {
        for motion in MotionVerb::ALL {
            let planted = matches!(motion, MotionVerb::Hold | MotionVerb::Freeze);
            assert_eq!(motion.steers(), !planted, "{motion:?}");
        }
    }

    // The memo derives a graph's set once and lends it after; a memo derived
    // for another graph never serves that graph's states, and equality ignores
    // the memo.
    #[test]
    fn rate_scaled_states_memo_lends_its_graphs_set_and_rederives_for_another() {
        use super::super::{RateScaledStates, RateScaledStatesMemo};
        use std::borrow::Cow;
        use std::sync::Arc;

        let graph_playing = |animation: &str, motion: &str| -> Arc<BehaviorGraphDescriptor> {
            Arc::new(
                serde_json::from_value(serde_json::json!({
                    "initial": "idle",
                    "moveSpeed": 3.0,
                    "activities": {
                        "idle": { "animation": "idle", "motion": "hold" },
                        "go": { "animation": animation, "motion": motion }
                    },
                    "transitions": { "idle": [] }
                }))
                .expect("graph deserializes"),
            )
        };
        let walker = graph_playing("walk", "moveToAnchor");
        let runner = graph_playing("run", "chaseTarget");

        let memo = RateScaledStatesMemo::of(&walker);
        let lent = memo.get(&walker);
        assert!(matches!(lent, Cow::Borrowed(_)), "the derived set is lent");
        assert_eq!(*lent, RateScaledStates::of(&walker));
        assert_eq!(lent.iter().collect::<Vec<_>>(), ["walk"]);

        let other = memo.get(&runner);
        assert_eq!(other.iter().collect::<Vec<_>>(), ["run"]);

        let lazy = RateScaledStatesMemo::default();
        assert_eq!(lazy.get(&runner).iter().collect::<Vec<_>>(), ["run"]);
        assert!(matches!(lazy.get(&runner), Cow::Borrowed(_)));
        assert_eq!(lazy, memo, "equality ignores the derived set");
    }

    /// The lowered shape of one stance and one strike from it: a rest and a
    /// hold sharing a pose, the hold's rows break → turn-lost → release, its
    /// release row carrying `releaseAfter`.
    fn lowered_stance(release_after: serde_json::Value) -> serde_json::Value {
        serde_json::json!({
            "initial": "overhead",
            "moveSpeed": 3.0,
            "attacks": { "slam": { "damage": 14.0, "maxRange": 3.5, "cooldownMs": 1800.0 } },
            "activities": {
                "overhead": { "animation": "smash_windup", "motion": "hold", "pose": "overhead" },
                "slam_overhead": {
                    "animation": "smash_windup", "motion": "hold",
                    "pose": "overhead", "attackTurn": "slam"
                },
                "slam_release": {
                    "animation": "smash_strike", "motion": "hold", "attackTurn": "slam",
                    "action": { "attack": "slam" }
                },
                "flinch": { "animation": "flinch", "motion": "hold" }
            },
            "transitions": {
                "overhead": [{ "to": "slam_overhead" }],
                "slam_overhead": [
                    { "to": "flinch", "when": { "op": "const", "value": false } },
                    { "to": "overhead", "when": { "op": "const", "value": false } },
                    { "to": "slam_release", "releaseAfter": release_after }
                ],
                "slam_release": [{ "to": "overhead" }],
                "flinch": [{ "to": "overhead" }]
            }
        })
    }

    // Task 5: the runtime descriptor accepts the lowering-only `pose` and
    // `releaseAfter` fields lowering writes, and serializes neither when
    // absent, so descriptors without stances keep their shape.
    #[test]
    fn lowered_pose_and_release_rows_deserialize_validate_and_round_trip() {
        let graph: BehaviorGraphDescriptor =
            serde_json::from_value(lowered_stance(serde_json::json!([500.0, 1200.0])))
                .expect("the lowered stance deserializes");
        let graph = graph.validate().expect("the lowered stance validates");
        assert_eq!(
            graph.envelope.activities["slam_overhead"].pose.as_deref(),
            Some("overhead")
        );
        assert_eq!(
            graph.envelope.transitions["slam_overhead"][2].release_after,
            Some([500.0, 1200.0])
        );
        let round_trip = serde_json::to_value(&graph).expect("the graph serializes");
        assert_eq!(
            round_trip["transitions"]["slam_overhead"][2]["releaseAfter"],
            serde_json::json!([500.0, 1200.0])
        );
        assert!(
            round_trip["transitions"]["overhead"][0]
                .get("releaseAfter")
                .is_none()
        );
        assert!(round_trip["activities"]["flinch"].get("pose").is_none());

        for fixed in [
            serde_json::json!([0.0, 0.0]),
            serde_json::json!([800.0, 800.0]),
        ] {
            serde_json::from_value::<BehaviorGraphDescriptor>(lowered_stance(fixed.clone()))
                .unwrap()
                .validate()
                .unwrap_or_else(|error| panic!("{fixed} loads: {error}"));
        }
    }

    // A release range must be finite, non-negative and ordered; one release
    // row per activity keeps each entry's draw unambiguous; a `*` row has no
    // activity to draw for; and a pose names something.
    #[test]
    fn release_rows_and_poses_reject_malformed_shapes_with_their_path() {
        let release_path = "components.behavior.transitions.slam_overhead[2].releaseAfter";
        for bad in [
            serde_json::json!([900.0, 500.0]),
            serde_json::json!([-1.0, 500.0]),
            serde_json::json!([0.0, -1.0]),
        ] {
            let error =
                serde_json::from_value::<BehaviorGraphDescriptor>(lowered_stance(bad.clone()))
                    .unwrap()
                    .validate()
                    .expect_err("a malformed release range rejects");
            assert!(error.to_string().contains(release_path), "{bad}: {error}");
        }
        let mut graph: BehaviorGraphDescriptor =
            serde_json::from_value(lowered_stance(serde_json::json!([500.0, 1200.0]))).unwrap();
        graph.envelope.transitions.get_mut("slam_overhead").unwrap()[2].release_after =
            Some([f32::NAN, 1200.0]);
        let error = graph.validate().expect_err("a non-finite bound rejects");
        assert!(error.to_string().contains(release_path), "{error}");

        let mut graph: BehaviorGraphDescriptor =
            serde_json::from_value(lowered_stance(serde_json::json!([500.0, 1200.0]))).unwrap();
        graph.envelope.transitions.get_mut("slam_overhead").unwrap()[1].release_after =
            Some([100.0, 200.0]);
        let error = graph.validate().expect_err("a second release row rejects");
        assert!(
            error.to_string().contains(release_path) && error.to_string().contains("second"),
            "{error}"
        );

        let mut graph: BehaviorGraphDescriptor =
            serde_json::from_value(lowered_stance(serde_json::json!([500.0, 1200.0]))).unwrap();
        graph.envelope.transitions.insert(
            "*".to_string(),
            vec![GuardedRow {
                when: always_enabled(),
                to: "flinch".to_string(),
                release_after: Some([0.0, 1.0]),
                queue: None,
                after_moment: false,
            }],
        );
        let error = graph.validate().expect_err("a `*` release row rejects");
        assert!(
            error
                .to_string()
                .contains("components.behavior.transitions.*[0].releaseAfter"),
            "{error}"
        );

        let mut graph: BehaviorGraphDescriptor =
            serde_json::from_value(lowered_stance(serde_json::json!([500.0, 1200.0]))).unwrap();
        graph.envelope.activities.get_mut("overhead").unwrap().pose = Some(String::new());
        let error = graph.validate().expect_err("an empty pose rejects");
        assert!(
            error
                .to_string()
                .contains("components.behavior.activities.overhead.pose"),
            "{error}"
        );
    }

    // Task 5b: the runtime descriptor accepts the lowering-only `queue` on a
    // rest's row to an attack, serializes it only when set, and rejects one
    // naming no declared attack or sitting on a `*` row, with its path.
    #[test]
    fn queued_rows_name_a_declared_attack_on_an_activitys_own_rows() {
        let mut lowered = lowered_stance(serde_json::json!([500.0, 1200.0]));
        lowered["transitions"]["overhead"][0]["queue"] = serde_json::json!("slam");
        let graph: BehaviorGraphDescriptor =
            serde_json::from_value(lowered.clone()).expect("a queued row deserializes");
        let graph = graph.validate().expect("a queued row validates");
        assert_eq!(
            graph.envelope.transitions["overhead"][0].queue.as_deref(),
            Some("slam")
        );
        let round_trip = serde_json::to_value(&graph).expect("the graph serializes");
        assert_eq!(
            round_trip["transitions"]["overhead"][0]["queue"],
            serde_json::json!("slam")
        );
        assert!(
            round_trip["transitions"]["slam_overhead"][0]
                .get("queue")
                .is_none()
        );

        let mut undeclared = lowered.clone();
        undeclared["transitions"]["overhead"][0]["queue"] = serde_json::json!("sweep");
        let error = serde_json::from_value::<BehaviorGraphDescriptor>(undeclared)
            .unwrap()
            .validate()
            .expect_err("a queue naming no declared attack rejects");
        assert!(
            error
                .to_string()
                .contains("components.behavior.transitions.overhead[0].queue")
                && error.to_string().contains("sweep"),
            "{error}"
        );

        let mut graph: BehaviorGraphDescriptor = serde_json::from_value(lowered).unwrap();
        graph.envelope.transitions.insert(
            "*".to_string(),
            vec![GuardedRow {
                when: always_enabled(),
                to: "slam_overhead".to_string(),
                release_after: None,
                queue: Some("slam".to_string()),
                after_moment: false,
            }],
        );
        let error = graph.validate().expect_err("a `*` queued row rejects");
        assert!(
            error
                .to_string()
                .contains("components.behavior.transitions.*[0].queue"),
            "{error}"
        );
    }

    /// The lowered shape of a strike's release and recover: the release fires
    /// `slam` at a defaulted `hit` and its own rows wait on the moment (one
    /// holding for an `ms` guard as well); both route a flinch to `coil`.
    fn lowered_release() -> serde_json::Value {
        serde_json::json!({
            "initial": "coil",
            "moveSpeed": 3.0,
            "attacks": { "slam": { "damage": 14.0, "maxRange": 3.5, "cooldownMs": 1800.0 } },
            "activities": {
                "coil": { "animation": "swipe_windup", "motion": "hold", "pose": "coil" },
                "slam_release": {
                    "animation": "smash_strike", "motion": "hold", "attackTurn": "slam",
                    "action": { "attack": "slam", "at": "hit", "atDefault": true },
                    "staggerTo": "coil"
                },
                "slam_recover": {
                    "animation": "smash_strike", "motion": "hold", "attackTurn": "slam",
                    "staggerTo": "coil"
                }
            },
            "transitions": {
                "slam_release": [
                    { "to": "slam_recover", "afterMoment": true,
                      "when": { "op": "ge",
                        "a": { "op": "input", "name": "@brain.timeInActivityMs" },
                        "b": { "op": "const", "value": 900.0 } } }
                ],
                "slam_recover": [{ "to": "coil" }]
            }
        })
    }

    // Task 6: the runtime descriptor accepts the release exit's lowering-only
    // `afterMoment`, `atDefault` and `staggerTo`, and serializes none of them
    // when absent, so descriptors without stances keep their shape.
    #[test]
    fn lowered_release_exit_fields_deserialize_validate_and_round_trip() {
        let graph: BehaviorGraphDescriptor =
            serde_json::from_value(lowered_release()).expect("the lowered release deserializes");
        let graph = graph.validate().expect("the lowered release validates");
        let release = &graph.envelope.activities["slam_release"];
        assert_eq!(release.stagger_to.as_deref(), Some("coil"));
        assert_eq!(
            release.action,
            Some(ActionVerb::Attack {
                name: "slam".to_string(),
                at: Some("hit".to_string()),
                at_default: true,
            })
        );
        assert!(graph.envelope.transitions["slam_release"][0].after_moment);
        assert!(!graph.envelope.transitions["slam_recover"][0].after_moment);

        let json = serde_json::to_value(&graph).expect("the graph serializes");
        assert_eq!(
            json["transitions"]["slam_release"][0]["afterMoment"],
            serde_json::json!(true)
        );
        assert!(
            json["transitions"]["slam_recover"][0]
                .get("afterMoment")
                .is_none()
        );
        assert_eq!(json["activities"]["slam_recover"]["staggerTo"], "coil");
        assert!(json["activities"]["coil"].get("staggerTo").is_none());
        let reparsed: BehaviorGraphDescriptor =
            serde_json::from_value(json).expect("the serialized graph parses");
        assert_eq!(reparsed.validate().expect("and loads"), graph);
    }

    // A moment gate belongs to a leaf's own rows, and a flinch route names
    // another activity at its leaf's own level.
    #[test]
    fn moment_rows_and_stagger_routes_reject_malformed_shapes_with_their_path() {
        let reject = |edit: &dyn Fn(&mut serde_json::Value), path: &str| {
            let mut json = lowered_release();
            edit(&mut json);
            let error = serde_json::from_value::<BehaviorGraphDescriptor>(json)
                .unwrap()
                .validate()
                .expect_err("the malformed shape rejects")
                .to_string();
            assert!(error.contains(path), "{path}: {error}");
        };
        reject(
            &|json| {
                json["transitions"]["*"] =
                    serde_json::json!([{ "to": "coil", "afterMoment": true }])
            },
            "components.behavior.transitions.*[0].afterMoment",
        );
        reject(
            &|json| {
                json["activities"]["shell"] = serde_json::json!({ "layers": { "inner": {
                    "initial": "a",
                    "activities": { "a": { "animation": "idle" } },
                    "transitions": {}
                } } });
                json["transitions"]["shell"] =
                    serde_json::json!([{ "to": "coil", "afterMoment": true }]);
            },
            "components.behavior.transitions.shell[0].afterMoment",
        );
        for target in ["slam_release", "nowhere"] {
            reject(
                &|json| json["activities"]["slam_release"]["staggerTo"] = serde_json::json!(target),
                "components.behavior.activities.slam_release.staggerTo",
            );
        }
        reject(
            &|json| {
                json["activities"]["shell"] = serde_json::json!({
                    "staggerTo": "coil",
                    "layers": { "inner": {
                        "initial": "a",
                        "activities": { "a": { "animation": "idle" } },
                        "transitions": {}
                    } }
                });
            },
            "components.behavior.activities.shell.staggerTo",
        );
    }

    // A release's lowered pass count loads on a stance release,
    // round-trips, and rejects out of its range or off a timed release.
    #[test]
    fn release_passes_load_on_a_timed_release_and_reject_elsewhere_with_their_path() {
        let mut json = lowered_release();
        json["activities"]["slam_release"]["passes"] = serde_json::json!(3);
        let graph = serde_json::from_value::<BehaviorGraphDescriptor>(json)
            .unwrap()
            .validate()
            .expect("three passes on a timed release load");
        assert_eq!(graph.envelope.activities["slam_release"].passes, Some(3));
        let round_trip = serde_json::to_value(&graph).unwrap();
        assert_eq!(round_trip["activities"]["slam_release"]["passes"], 3);
        assert!(round_trip["activities"]["coil"].get("passes").is_none());

        let reject = |edit: &dyn Fn(&mut serde_json::Value), needle: &str| {
            let mut json = lowered_release();
            edit(&mut json);
            let error = serde_json::from_value::<BehaviorGraphDescriptor>(json)
                .unwrap()
                .validate()
                .expect_err("the malformed pass count rejects")
                .to_string();
            assert!(
                error.contains("components.behavior.activities.") && error.contains(needle),
                "{needle}: {error}"
            );
        };
        for passes in [0, 9] {
            reject(
                &|json| json["activities"]["slam_release"]["passes"] = serde_json::json!(passes),
                "slam_release.passes` must be 1 to 8",
            );
        }
        reject(
            &|json| {
                json["activities"]["slam_release"]["passes"] = serde_json::json!(2);
                json["activities"]["slam_release"]["action"] =
                    serde_json::json!({ "attack": "slam" });
            },
            "slam_release.passes` belongs to a leaf whose own action fires `at`",
        );
        reject(
            &|json| json["activities"]["slam_recover"]["passes"] = serde_json::json!(2),
            "slam_recover.passes` belongs to a leaf whose own action fires `at`",
        );
        reject(
            &|json| {
                json["activities"]["slam_release"]["passes"] = serde_json::json!(2);
                json["transitions"]["slam_release"][0]["afterMoment"] = serde_json::json!(false);
            },
            "slam_release.passes` belongs to a stance release",
        );
    }

    // `leap` loads on a leaf beside its own timed attack and round-trips; it
    // rejects without a clip moment to arrive by, with passes, and in a
    // selector, each naming its path.
    #[test]
    fn leap_loads_beside_a_timed_attack_and_rejects_elsewhere_with_its_path() {
        let mut json = lowered_release();
        json["activities"]["slam_release"]["motion"] = serde_json::json!("leap");
        let graph = serde_json::from_value::<BehaviorGraphDescriptor>(json)
            .unwrap()
            .validate()
            .expect("a leap beside a timed attack loads");
        assert_eq!(
            graph.envelope.activities["slam_release"].motion,
            Some(MotionVerb::Leap)
        );
        let round_trip = serde_json::to_value(&graph).unwrap();
        assert_eq!(round_trip["activities"]["slam_release"]["motion"], "leap");

        let reject = |edit: &dyn Fn(&mut serde_json::Value), needle: &str| {
            let mut json = lowered_release();
            json["activities"]["slam_release"]["motion"] = serde_json::json!("leap");
            edit(&mut json);
            let error = serde_json::from_value::<BehaviorGraphDescriptor>(json)
                .unwrap()
                .validate()
                .expect_err("the misplaced leap rejects")
                .to_string();
            assert!(error.contains(needle), "{needle}: {error}");
        };
        let untimed = "`components.behavior.activities.slam_release.motion` is \"leap\", which arrives at its attack's clip moment, so `components.behavior.activities.slam_release.action` must name an attack with `at`";
        reject(
            &|json| {
                json["activities"]["slam_release"]["action"] =
                    serde_json::json!({ "attack": "slam" });
            },
            untimed,
        );
        reject(
            &|json| {
                let release = json["activities"]["slam_release"].as_object_mut().unwrap();
                release.remove("action");
            },
            untimed,
        );
        reject(
            &|json| json["activities"]["slam_release"]["passes"] = serde_json::json!(2),
            "`components.behavior.activities.slam_release.motion` is \"leap\", which arrives at one clip moment, so `components.behavior.activities.slam_release.passes` must be 1",
        );
        reject(
            &|json| {
                json["activities"]["roam"] = serde_json::json!({
                    "layers": { "move": [
                        { "when": { "op": "const", "value": true }, "motion": "leap" },
                        "hold"
                    ] }
                });
            },
            "`components.behavior.activities.roam.layers.move[0].motion` is \"leap\", which arrives at its own leaf's clip moment",
        );
    }

    // `queueRange` is a contact attack's, at least its `maxRange`,
    // and a weapon attack rejects it as it rejects `maxRange`. Omitted, the
    // queue distance is `maxRange`; combat slots still derive from `maxRange`.
    #[test]
    fn queue_range_is_a_contact_attacks_at_least_its_max_range() {
        let attack = |fields: serde_json::Value| -> AttackParams {
            serde_json::from_value(fields).expect("the attack parses")
        };
        let leap = attack(serde_json::json!({
            "damage": 30.0, "maxRange": 3.0, "queueRange": 15.0, "cooldownMs": 4000.0
        }));
        validate_attacks(&BTreeMap::from([("leap".to_string(), leap.clone())]))
            .expect("a queue range past the hit distance loads");
        assert_eq!(leap.queue_distance(), Some(15.0));
        assert_eq!(
            serde_json::to_value(&leap).unwrap()["queueRange"],
            serde_json::json!(15.0)
        );
        let graph = BehaviorGraphDescriptor {
            attacks: BTreeMap::from([("leap".to_string(), leap)]),
            ..serde_json::from_value(lowered_release()).unwrap()
        };
        let action = ActionVerb::Attack {
            name: "leap".to_string(),
            at: None,
            at_default: false,
        };
        assert_eq!(graph.engagement_radius_for_action(Some(&action)), 3.0);
        assert_eq!(graph.standoff_distance_for_action(Some(&action)), 3.0);

        let jab =
            attack(serde_json::json!({ "damage": 5.0, "maxRange": 2.0, "cooldownMs": 900.0 }));
        assert_eq!(
            jab.queue_distance(),
            Some(2.0),
            "omitted, it queues at its reach"
        );
        assert!(
            serde_json::to_value(&jab)
                .unwrap()
                .get("queueRange")
                .is_none()
        );
        let equal = attack(serde_json::json!({
            "damage": 5.0, "maxRange": 2.0, "queueRange": 2.0, "cooldownMs": 900.0
        }));
        validate_attacks(&BTreeMap::from([("jab".to_string(), equal)]))
            .expect("a queue range equal to the reach loads");

        for (fields, needle) in [
            (
                serde_json::json!({ "damage": 5.0, "maxRange": 2.0, "queueRange": 1.5, "cooldownMs": 900.0 }),
                "`components.behavior.attacks.jab.queueRange` must be a finite value >= `components.behavior.attacks.jab.maxRange` (2)",
            ),
            (
                serde_json::json!({ "weapon": "rifle", "queueRange": 15.0 }),
                "`components.behavior.attacks.jab.queueRange` must be omitted when `components.behavior.attacks.jab.weapon` is present",
            ),
        ] {
            let error = validate_attacks(&BTreeMap::from([("jab".to_string(), attack(fields))]))
                .expect_err("the queue range rejects")
                .to_string();
            assert!(error.contains(needle), "{error}");
        }
    }

    // The repeating-release states come from activities with
    // `passes` above 1 only, each named once.
    #[test]
    fn repeating_release_states_name_only_a_repeating_releases_animation() {
        let mut json = lowered_release();
        let single = serde_json::from_value::<BehaviorGraphDescriptor>(json.clone()).unwrap();
        assert!(super::super::repeating_release_states(&single).is_empty());
        json["activities"]["slam_release"]["passes"] = serde_json::json!(3);
        let combo = serde_json::from_value::<BehaviorGraphDescriptor>(json).unwrap();
        assert_eq!(
            super::super::repeating_release_states(&combo),
            vec!["smash_strike"]
        );
    }
}
