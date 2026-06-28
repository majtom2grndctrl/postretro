// Data-context descriptors: weapon/health/ai descriptors.
// See: context/lib/scripting.md

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::scripting::data_descriptors::DescriptorError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum FireMode {
    Semi,
    Auto,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum ResolutionMode {
    Hitscan,
}

/// Authored weapon component preset. This is descriptor-owned tuning data:
/// maps do not override these params, and the runtime materializes a separate
/// wieldable instance entity from the descriptor at player spawn.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WeaponDescriptor {
    pub(crate) damage: f32,
    pub(crate) range: f32,
    #[serde(rename = "fireRateMs")]
    pub(crate) cooldown_ms: f32,
    pub(crate) fire_mode: FireMode,
    pub(crate) resolution: ResolutionMode,
}

impl WeaponDescriptor {
    pub(crate) fn validate(self) -> Result<Self, DescriptorError> {
        if !self.damage.is_finite() || self.damage < 0.0 {
            return Err(DescriptorError::InvalidShape {
                reason: format!(
                    "`components.weapon.damage` must be a finite value >= 0.0, got {}",
                    self.damage
                ),
            });
        }
        if !self.range.is_finite() || self.range <= 0.0 {
            return Err(DescriptorError::InvalidShape {
                reason: format!(
                    "`components.weapon.range` must be a finite value > 0.0, got {}",
                    self.range
                ),
            });
        }
        if !self.cooldown_ms.is_finite() || self.cooldown_ms <= 0.0 {
            return Err(DescriptorError::InvalidShape {
                reason: format!(
                    "`components.weapon.fireRateMs` must be a finite value > 0.0, got {}",
                    self.cooldown_ms
                ),
            });
        }
        Ok(self)
    }
}

/// Authored health component preset attached to an [`EntityTypeDescriptor`].
/// `max` is the entity's hit-point ceiling; the optional `hitbox` makes the
/// entity hitscan-targetable (one world-aligned AABB, fixed per archetype).
/// Wire keys are camelCase. The data-archetype spawn path materializes this
/// into a [`crate::scripting::components::health::HealthComponent`] with `current == max`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct HealthDescriptor {
    pub(crate) max: f32,
    #[serde(default)]
    pub(crate) hitbox: Option<HitboxDescriptor>,
    /// Per-skeletal-zone damage multipliers, tag → factor (e.g. `"head" → 1.5`).
    /// A shot landing on a tagged zone scales the weapon's payload by this
    /// factor; an absent zone or an unlisted tag applies `1.0`. Each factor must
    /// be finite and `>= 0`. Defaults to empty (every zone applies `1.0`).
    #[serde(default, rename = "zoneMultipliers")]
    pub(crate) zone_multipliers: HashMap<String, f32>,
}

/// Authored hitbox sub-block: one world-aligned AABB. `half_extents` is the
/// box half-size on each axis; `offset` shifts the box center from the entity's
/// transform position (defaults to zero when absent).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct HitboxDescriptor {
    pub(crate) half_extents: [f32; 3],
    #[serde(default)]
    pub(crate) offset: Option<[f32; 3]>,
}

impl HealthDescriptor {
    /// Validate bounds serde cannot enforce (the `LightDescriptor::validate`
    /// precedent): `max` finite and `>= 1`; each `halfExtents` element finite and
    /// `> 0`; each `offset` element finite.
    pub(crate) fn validate(self) -> Result<Self, DescriptorError> {
        if !self.max.is_finite() || self.max < 1.0 {
            return Err(DescriptorError::InvalidShape {
                reason: format!(
                    "`components.health.max` must be a finite value >= 1.0, got {}",
                    self.max
                ),
            });
        }
        if let Some(hitbox) = self.hitbox.as_ref() {
            for (axis, value) in ["x", "y", "z"].iter().zip(hitbox.half_extents) {
                if !value.is_finite() || value <= 0.0 {
                    return Err(DescriptorError::InvalidShape {
                        reason: format!(
                            "`components.health.hitbox.halfExtents.{axis}` must be a finite value > 0.0, got {value}"
                        ),
                    });
                }
            }
            if let Some(offset) = hitbox.offset {
                for (axis, value) in ["x", "y", "z"].iter().zip(offset) {
                    if !value.is_finite() {
                        return Err(DescriptorError::InvalidShape {
                            reason: format!(
                                "`components.health.hitbox.offset.{axis}` must be a finite value, got {value}"
                            ),
                        });
                    }
                }
            }
        }
        for (tag, factor) in &self.zone_multipliers {
            if !factor.is_finite() || *factor < 0.0 {
                return Err(DescriptorError::InvalidShape {
                    reason: format!(
                        "`components.health.zoneMultipliers.{tag}` must be a finite value >= 0.0, got {factor}"
                    ),
                });
            }
        }
        Ok(self)
    }
}

/// The closed `components.ai.states` block: the four logical-state → animation-
/// state name mappings. `#[serde(deny_unknown_fields)]` makes an UNRECOGNIZED
/// key a parse error (the closed-set requirement), and every field is required
/// (no `#[serde(default)]`), so a MISSING key is also a parse error. Both
/// outcomes funnel through serde, so the QuickJS and Luau parse twins (which
/// both deserialize via `serde_json`) cannot diverge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AiStateNames {
    pub(crate) idle: String,
    pub(crate) alert: String,
    pub(crate) attack: String,
    pub(crate) death: String,
}

/// Authored AI brain component preset attached to an [`EntityTypeDescriptor`].
/// Descriptor-owned tuning (entity_model.md §4): maps never override these. The
/// data-archetype spawn path materializes this into a
/// [`crate::scripting::components::brain::BrainComponent`] (logical state + timers +
/// resolved [`crate::scripting::components::brain::AiTuning`]).
///
/// Wire keys are camelCase (boundary inventory): `detectionRange`,
/// `attackRange`, `leashRange`, `attackDamage`, `attackCooldownMs`, `moveSpeed`,
/// `deathDespawnMs`, and the closed `states` block. The
/// logical-state → animation-state mapping cannot be validated at parse (the ai
/// block cannot see the mesh block — cross-component); it is validated at SPAWN
/// (`components::brain::validate_brain_animation_states`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AiDescriptor {
    pub(crate) detection_range: f32,
    pub(crate) attack_range: f32,
    pub(crate) leash_range: f32,
    pub(crate) attack_damage: f32,
    pub(crate) attack_cooldown_ms: f32,
    pub(crate) move_speed: f32,
    pub(crate) death_despawn_ms: f32,
    pub(crate) states: AiStateNames,
}

impl AiDescriptor {
    /// The shared parse-time validator both runtimes funnel through, so QuickJS
    /// and Luau cannot diverge. Bounds serde cannot enforce
    /// (`LightDescriptor::validate` / `HealthDescriptor::validate` precedent):
    ///
    /// - every range field (`detectionRange`, `attackRange`, `leashRange`,
    ///   `attackCooldownMs`, `moveSpeed`, `deathDespawnMs`) must be finite and
    ///   strictly positive;
    /// - `attackDamage` must be finite and non-negative (a negative
    ///   `attackDamage` would HEAL the player through `apply_damage`'s
    ///   subtraction).
    ///
    /// The closed `states` key set is enforced upstream by
    /// `#[serde(deny_unknown_fields)]` on [`AiStateNames`]; the logical-state →
    /// animation-state name mapping is validated at spawn (cross-component).
    pub(crate) fn validate(self) -> Result<Self, DescriptorError> {
        for (field, value) in [
            ("detectionRange", self.detection_range),
            ("attackRange", self.attack_range),
            ("leashRange", self.leash_range),
            ("attackCooldownMs", self.attack_cooldown_ms),
            ("moveSpeed", self.move_speed),
            ("deathDespawnMs", self.death_despawn_ms),
        ] {
            if !value.is_finite() || value <= 0.0 {
                return Err(DescriptorError::InvalidShape {
                    reason: format!(
                        "`components.ai.{field}` must be a finite value > 0.0, got {value}"
                    ),
                });
            }
        }
        for (field, value) in [("attackDamage", self.attack_damage)] {
            if !value.is_finite() || value < 0.0 {
                return Err(DescriptorError::InvalidShape {
                    reason: format!(
                        "`components.ai.{field}` must be a finite value >= 0.0, got {value}"
                    ),
                });
            }
        }
        Ok(self)
    }
}
