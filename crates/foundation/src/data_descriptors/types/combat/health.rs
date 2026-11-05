// Health descriptor: hit points, hitbox and hit-zone multipliers authored on an
// entity's `components.health` block.
// See: context/lib/entity_model.md §Components

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::data_descriptors::DescriptorError;

/// Authored health component preset attached to an entity type descriptor.
/// `max` is the entity's hit-point ceiling; the optional `hitbox` makes the
/// entity a hitscan and swept-projectile target (one world-aligned AABB, fixed
/// per archetype).
/// Wire keys are camelCase. Runtime data-archetype spawn materializes this into
/// a health component with `current == max`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthDescriptor {
    pub max: f32,
    #[serde(default)]
    pub hitbox: Option<HitboxDescriptor>,
    /// Per-skeletal-zone damage multipliers, tag → factor (e.g. `"head" → 1.5`).
    /// A shot landing on a tagged zone scales the weapon's payload by this
    /// factor; an absent zone or an unlisted tag applies `1.0`. Each factor must
    /// be finite and `>= 0`. Defaults to empty (every zone applies `1.0`).
    #[serde(default, rename = "zoneMultipliers")]
    pub zone_multipliers: HashMap<String, f32>,
}

/// Authored hitbox sub-block: one world-aligned AABB. `half_extents` is the
/// box half-size on each axis; `offset` (zero when absent) shifts the box
/// center: a descriptor with `behavior` measures it from the feet; anything
/// else from the entity's transform position, which for a `movement` pawn is
/// its capsule centre.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HitboxDescriptor {
    pub half_extents: [f32; 3],
    #[serde(default)]
    pub offset: Option<[f32; 3]>,
}

impl HealthDescriptor {
    /// Validate bounds serde cannot enforce (the `LightDescriptor::validate`
    /// precedent): `max` finite and `>= 1`; each `halfExtents` element finite and
    /// `> 0`; each `offset` element finite.
    pub fn validate(self) -> Result<Self, DescriptorError> {
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
