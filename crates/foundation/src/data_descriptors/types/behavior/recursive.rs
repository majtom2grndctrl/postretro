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

use super::{ActionVerb, AttackParams, MotionVerb, PatrolDescriptor};
use crate::brain::bind_brain_guard;
use crate::candidate::bind_candidate_filter;
use crate::data_descriptors::DescriptorError;
use crate::data_descriptors::types::behavior_lints;
use crate::ir::{IrNode, IrType};

/// Maximum number of nested behavior envelopes, including the root envelope.
/// This gives the evaluator a fixed-capacity active path.
pub const MAX_BEHAVIOR_NESTING_DEPTH: usize = 8;

/// The source-keyed adjacency row shared by every behavior envelope.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GuardedRow {
    pub when: IrNode,
    pub to: String,
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
    #[serde(flatten)]
    pub envelope: BehaviorGraphEnvelope,
    #[serde(default)]
    pub candidate_filter: Option<IrNode>,
    #[serde(default)]
    pub patrol: Option<PatrolDescriptor>,
    #[serde(default)]
    pub attacks: BTreeMap<String, AttackParams>,
    #[serde(default)]
    pub engagement_radius: Option<f32>,
    pub move_speed: f32,
}

/// `flatten` and `deny_unknown_fields` cannot be combined in serde. Use a
/// strict wire-shaped helper so retired `states` / `interrupts` spellings are
/// errors rather than silently ignored.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RawBehaviorGraphDescriptor {
    initial: String,
    #[serde(deserialize_with = "deserialize_activities")]
    activities: BTreeMap<String, BehaviorActivityDescriptor>,
    transitions: BTreeMap<String, Vec<GuardedRow>>,
    #[serde(default)]
    candidate_filter: Option<IrNode>,
    #[serde(default)]
    patrol: Option<PatrolDescriptor>,
    #[serde(default)]
    attacks: BTreeMap<String, AttackParams>,
    #[serde(default)]
    engagement_radius: Option<f32>,
    move_speed: f32,
}

impl<'de> Deserialize<'de> for BehaviorGraphDescriptor {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = RawBehaviorGraphDescriptor::deserialize(deserializer)?;
        Ok(Self {
            envelope: BehaviorGraphEnvelope {
                initial: raw.initial,
                activities: raw.activities,
                transitions: raw.transitions,
            },
            candidate_filter: raw.candidate_filter,
            patrol: raw.patrol,
            attacks: raw.attacks,
            engagement_radius: raw.engagement_radius,
            move_speed: raw.move_speed,
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

    /// Root attack data stays authoritative even when an action appears under
    /// a nested graph layer.
    pub fn engagement_radius_for_action(&self, action: Option<&ActionVerb>) -> f32 {
        match action {
            Some(ActionVerb::Attack(name)) => self
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
            Some(ActionVerb::Attack(name)) => self
                .attacks
                .get(name)
                .and_then(|attack| attack.standoff_distance)
                .unwrap_or_else(|| self.engagement_radius_for_action(action)),
            None => self.engagement_radius_for_action(None),
        }
    }

    /// Shared validation used after both JS and Luau conversion paths.
    pub fn validate(mut self) -> Result<Self, DescriptorError> {
        validate_positive("moveSpeed", self.move_speed)?;
        if let Some(radius) = self.engagement_radius {
            validate_positive("engagementRadius", radius)?;
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

fn validate_envelope(
    envelope: &mut BehaviorGraphEnvelope,
    path: &str,
    depth: usize,
    attacks: &BTreeMap<String, AttackParams>,
    patrol: Option<&PatrolDescriptor>,
) -> Result<(), DescriptorError> {
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
        }
    }

    for (name, activity) in &mut envelope.activities {
        validate_activity(
            activity,
            &format!("{path}.activities.{name}"),
            depth,
            attacks,
            patrol,
        )?;
    }
    Ok(())
}

fn validate_activity(
    activity: &mut BehaviorActivityDescriptor,
    path: &str,
    depth: usize,
    attacks: &BTreeMap<String, AttackParams>,
    patrol: Option<&PatrolDescriptor>,
) -> Result<(), DescriptorError> {
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
        for (name, layer) in &mut activity.layers {
            validate_layer(
                layer,
                &format!("{path}.layers.{name}"),
                name,
                depth,
                attacks,
                patrol,
            )?;
        }
        return Ok(());
    }

    if activity.animation.as_ref().is_none_or(String::is_empty) {
        return Err(DescriptorError::InvalidShape {
            reason: format!("`{path}.animation` must be a non-empty string for a leaf activity"),
        });
    }
    if let Some(motion) = activity.motion {
        validate_motion(motion, &format!("{path}.motion"), patrol)?;
        if matches!(motion, MotionVerb::MoveToAnchor | MotionVerb::Patrol)
            && activity.action.is_some()
        {
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
    Ok(())
}

fn validate_layer(
    layer: &mut BehaviorLayerDescriptor,
    path: &str,
    layer_name: &str,
    depth: usize,
    attacks: &BTreeMap<String, AttackParams>,
    patrol: Option<&PatrolDescriptor>,
) -> Result<(), DescriptorError> {
    match layer {
        BehaviorLayerDescriptor::Graph(envelope) => {
            if layer_name == "move" {
                return Err(DescriptorError::InvalidShape {
                    reason: format!("`{path}` must be a move selector list, not a nested graph"),
                });
            }
            validate_envelope(envelope, path, depth + 1, attacks, patrol)
        }
        BehaviorLayerDescriptor::Selector(entries) => {
            if layer_name == "move" {
                validate_move_selector(entries, path, patrol)
            } else if layer_name == "offense" {
                validate_offense_selector(entries, path, attacks)
            } else {
                validate_unconsumed_selector(entries, path, attacks, patrol)
            }
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
                    validate_action(action, &format!("{path}[{index}].action"), attacks)?;
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
        validate_action(
            row.action.as_ref().expect("checked above"),
            &format!("{path}[{index}].action"),
            attacks,
        )?;
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
    match action {
        ActionVerb::Attack(name) if attacks.contains_key(name) => Ok(()),
        ActionVerb::Attack(name) if attacks.is_empty() => Err(DescriptorError::InvalidShape {
            reason: format!(
                "`{path}.attack` names \"{name}\", so `components.behavior.attacks` must declare at least one entry"
            ),
        }),
        ActionVerb::Attack(name) => Err(DescriptorError::InvalidShape {
            reason: format!(
                "`{path}.attack` names \"{name}\", which is not declared in `components.behavior.attacks`"
            ),
        }),
    }
}

fn validate_motion(
    motion: MotionVerb,
    path: &str,
    patrol: Option<&PatrolDescriptor>,
) -> Result<(), DescriptorError> {
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
            ("cooldownMs", attack.cooldown_ms),
        ];
        if attack.weapon.is_some() {
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

    #[test]
    fn standoff_distance_defaults_to_per_attack_engagement_radius_override() {
        let graph = BehaviorGraphDescriptor {
            envelope: BehaviorGraphEnvelope {
                initial: "idle".to_string(),
                activities: BTreeMap::new(),
                transitions: BTreeMap::new(),
            },
            candidate_filter: None,
            patrol: None,
            attacks: BTreeMap::from([(
                "slam".to_string(),
                AttackParams {
                    weapon: None,
                    damage: Some(1.0),
                    max_range: Some(4.0),
                    cooldown_ms: Some(1.0),
                    engagement_radius: Some(3.0),
                    standoff_distance: None,
                },
            )]),
            engagement_radius: Some(2.0),
            move_speed: 1.0,
        };
        let action = ActionVerb::Attack("slam".to_string());

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
    fn validate_attacks_rejects_non_positive_standoff_distance() {
        let attacks = BTreeMap::from([(
            "slam".to_string(),
            AttackParams {
                weapon: None,
                damage: Some(1.0),
                max_range: Some(4.0),
                cooldown_ms: Some(1.0),
                engagement_radius: None,
                standoff_distance: Some(0.0),
            },
        )]);

        let error = validate_attacks(&attacks).expect_err("zero standoff must reject");
        assert!(
            error.to_string().contains("attacks.slam.standoffDistance"),
            "{error}"
        );
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
                    weapon: Some("enemy_rifle".to_string()),
                    damage: Some(8.0),
                    max_range: None,
                    cooldown_ms: None,
                    engagement_radius: None,
                    standoff_distance: None,
                },
            ),
            (
                "maxRange",
                AttackParams {
                    weapon: Some("enemy_rifle".to_string()),
                    damage: None,
                    max_range: Some(12.0),
                    cooldown_ms: None,
                    engagement_radius: None,
                    standoff_distance: None,
                },
            ),
            (
                "cooldownMs",
                AttackParams {
                    weapon: Some("enemy_rifle".to_string()),
                    damage: None,
                    max_range: None,
                    cooldown_ms: Some(250.0),
                    engagement_radius: None,
                    standoff_distance: None,
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
}
