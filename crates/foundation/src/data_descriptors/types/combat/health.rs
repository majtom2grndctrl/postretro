// Health descriptor: hit points, hitbox and hit-zone multipliers authored on an
// entity's `components.health` block.
// See: context/lib/entity_model.md §Components

use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};

use crate::data_descriptors::{DescriptorError, NumberOrIr};
use crate::defense::{
    HandleRef, LAYER_CAPACITY_RANGE, LAYER_DELAY_MS_RANGE, LAYER_RATE_RANGE, MULTIPLIER_RANGE,
};

/// Authored health component preset attached to an entity type descriptor.
/// `max` is the entity's hit-point ceiling; the optional `hitbox` makes the
/// entity a hitscan and swept-projectile target (one world-aligned AABB, fixed
/// per archetype).
/// Wire keys are camelCase. Runtime data-archetype spawn materializes this into
/// a health component with `current == max`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
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
    /// Defense layers in drain order: each drains before health on every hit.
    #[serde(default)]
    pub layers: Vec<DefenseLayerEntry>,
    /// Health's own resistance per damage type, keyed by type name: scales
    /// that type on health only. A missing key is ×1.
    #[serde(default)]
    pub resist: BTreeMap<String, NumberOrIr>,
}

/// A handle reference as authored, kept raw so a bad one is reported with
/// its key path instead of a bare serde message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AuthoredHandle(pub serde_json::Value);

impl AuthoredHandle {
    pub fn from_handle(handle: &HandleRef) -> Self {
        Self(serde_json::to_value(handle).expect("handle refs serialize"))
    }

    /// The handle this value carries, or a diagnostic naming `path`.
    pub fn handle(&self, path: &str) -> Result<HandleRef, String> {
        HandleRef::from_json(&self.0).map_err(|error| format!("`{path}`: {error}"))
    }
}

/// One defense layer on a descriptor: the layer kind it is an instance of,
/// its capacity and recharge, and its own resistances. Each parameter is a
/// literal or a Number expression.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DefenseLayerEntry {
    pub layer: AuthoredHandle,
    pub capacity: NumberOrIr,
    #[serde(default)]
    pub recharge_delay_ms: Option<NumberOrIr>,
    #[serde(default)]
    pub recharge_rate: Option<NumberOrIr>,
    /// This layer's resistance per damage type, keyed by type name: scales
    /// that type on this layer only.
    #[serde(default)]
    pub resist: BTreeMap<String, NumberOrIr>,
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
        let mut kinds: Vec<String> = Vec::with_capacity(self.layers.len());
        for (index, entry) in self.layers.iter().enumerate() {
            let path = format!("components.health.layers[{index}]");
            let handle = entry
                .layer
                .handle(&format!("{path}.layer"))
                .map_err(|reason| DescriptorError::InvalidShape { reason })?;
            let name = handle
                .expect_layer()
                .map_err(|error| DescriptorError::InvalidShape {
                    reason: format!("`{path}.layer`: {error}"),
                })?;
            if kinds.iter().any(|kind| kind == name) {
                return Err(DescriptorError::InvalidShape {
                    reason: format!(
                        "`{path}.layer`: layer `{name}` is listed twice in one `layers`"
                    ),
                });
            }
            kinds.push(name.to_string());
            check_parameter(
                &format!("{path}.capacity"),
                &entry.capacity,
                LAYER_CAPACITY_RANGE,
            )?;
            if let Some(delay) = &entry.recharge_delay_ms {
                check_parameter(
                    &format!("{path}.rechargeDelayMs"),
                    delay,
                    LAYER_DELAY_MS_RANGE,
                )?;
            }
            if let Some(rate) = &entry.recharge_rate {
                check_parameter(&format!("{path}.rechargeRate"), rate, LAYER_RATE_RANGE)?;
            }
            for (key, value) in &entry.resist {
                check_parameter(&format!("{path}.resist.{key}"), value, MULTIPLIER_RANGE)?;
            }
        }
        for (key, value) in &self.resist {
            check_parameter(
                &format!("components.health.resist.{key}"),
                value,
                MULTIPLIER_RANGE,
            )?;
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

/// A literal must sit inside its range; an expression must fit the
/// depth-32 / 256-node cap. Expression results clamp at evaluation instead.
fn check_parameter(
    path: &str,
    value: &NumberOrIr,
    range: (f32, f32),
) -> Result<(), DescriptorError> {
    match value {
        NumberOrIr::Literal(literal) => {
            if literal.is_finite() && (range.0..=range.1).contains(literal) {
                Ok(())
            } else {
                Err(DescriptorError::InvalidShape {
                    reason: format!(
                        "`{path}` must be a finite value in [{}, {}], got {literal}",
                        range.0, range.1
                    ),
                })
            }
        }
        NumberOrIr::Ir(node) => {
            crate::weapon_activation::check_activation_ir_caps(node).map_err(|error| {
                DescriptorError::InvalidShape {
                    reason: format!("`{path}` expression {error}"),
                }
            })
        }
    }
}
