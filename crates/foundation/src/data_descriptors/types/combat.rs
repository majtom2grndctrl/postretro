// Data-context descriptors: weapon/health/ai descriptors.
// See: context/lib/scripting.md

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::data_descriptors::DescriptorError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FireMode {
    Semi,
    Auto,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ResolutionMode {
    Hitscan,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum WeaponResource {
    Ammo(AmmoResource),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AmmoResource {
    #[serde(rename = "type")]
    pub ammo_type: String,
    pub magazine: u32,
    #[serde(default = "default_cost_per_shot", rename = "costPerShot")]
    pub cost_per_shot: u32,
    pub reserve: u32,
    #[serde(default = "default_reload_ms", rename = "reloadMs")]
    pub reload_ms: u32,
}

const fn default_cost_per_shot() -> u32 {
    1
}

const fn default_reload_ms() -> u32 {
    1000
}

/// Authored weapon component preset. This is descriptor-owned tuning data:
/// maps do not override these params, and the runtime materializes a separate
/// wieldable instance entity from the descriptor at player spawn.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WeaponDescriptor {
    pub damage: f32,
    pub range: f32,
    #[serde(rename = "fireRateMs")]
    pub cooldown_ms: f32,
    pub fire_mode: FireMode,
    pub resolution: ResolutionMode,
    #[serde(default, rename = "creditSource")]
    pub credit_source: Option<String>,
    /// Optional rigid prop model mounted at the pawn's third-person hand socket.
    #[serde(default, rename = "thirdPersonModel")]
    pub third_person_model: Option<String>,
    /// Optional model rendered by the first-person viewmodel pass.
    #[serde(default)]
    pub viewmodel: Option<String>,
    #[serde(default)]
    pub resource: Option<WeaponResource>,
}

impl WeaponDescriptor {
    pub fn validate(self) -> Result<Self, DescriptorError> {
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
        if let Some(credit_source) = self.credit_source.as_deref() {
            validate_credit_source(credit_source)?;
        }
        for (field, path) in [
            ("thirdPersonModel", self.third_person_model.as_deref()),
            ("viewmodel", self.viewmodel.as_deref()),
        ] {
            if matches!(path, Some("")) {
                return Err(DescriptorError::InvalidShape {
                    reason: format!("`components.weapon.{field}` must be a non-empty model path"),
                });
            }
        }
        if let Some(WeaponResource::Ammo(ammo)) = self.resource.as_ref() {
            validate_ascii_identifier("resource.type", &ammo.ammo_type)?;
            for (field, value) in [
                ("magazine", ammo.magazine),
                ("costPerShot", ammo.cost_per_shot),
                ("reloadMs", ammo.reload_ms),
            ] {
                if value < 1 {
                    return Err(DescriptorError::InvalidShape {
                        reason: format!(
                            "`components.weapon.resource.{field}` must be >= 1, got {value}"
                        ),
                    });
                }
            }
        }
        Ok(self)
    }
}

fn validate_credit_source(value: &str) -> Result<(), DescriptorError> {
    validate_ascii_identifier("creditSource", value)
}

fn validate_ascii_identifier(field: &str, value: &str) -> Result<(), DescriptorError> {
    if value.is_empty() {
        return Err(DescriptorError::InvalidShape {
            reason: format!("`components.weapon.{field}` must be a non-empty ASCII identifier"),
        });
    }
    if value.len() > 64 {
        return Err(DescriptorError::InvalidShape {
            reason: format!(
                "`components.weapon.{field}` must be at most 64 bytes, got {}",
                value.len()
            ),
        });
    }
    if !value
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b':' | b'-'))
    {
        return Err(DescriptorError::InvalidShape {
            reason: format!("`components.weapon.{field}` must match [A-Za-z0-9_.:-] and be ASCII"),
        });
    }
    Ok(())
}

/// Authored health component preset attached to an entity type descriptor.
/// `max` is the entity's hit-point ceiling; the optional `hitbox` makes the
/// entity hitscan-targetable (one world-aligned AABB, fixed per archetype).
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
/// box half-size on each axis; `offset` shifts the box center from the entity's
/// transform position (defaults to zero when absent).
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

/// The closed `components.ai.states` block: the four logical-state → animation-
/// state name mappings. `#[serde(deny_unknown_fields)]` makes an UNRECOGNIZED
/// key a parse error (the closed-set requirement), and every field is required
/// (no `#[serde(default)]`), so a MISSING key is also a parse error. Both
/// outcomes funnel through serde, so the QuickJS and Luau parse twins (which
/// both deserialize via `serde_json`) cannot diverge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AiStateNames {
    pub idle: String,
    pub alert: String,
    pub attack: String,
    pub death: String,
}

/// Authored AI brain component preset attached to an entity type descriptor.
/// Descriptor-owned tuning (entity_model.md §4): maps never override these. The
/// runtime data-archetype spawn path materializes this into an AI brain
/// component with logical state, timers, and resolved tuning.
///
/// Wire keys are camelCase (boundary inventory): `detectionRange`,
/// `attackRange`, `leashRange`, `attackDamage`, `attackCooldownMs`, `moveSpeed`,
/// `deathDespawnMs`, and the closed `states` block. The
/// logical-state → animation-state mapping cannot be validated at parse (the ai
/// block cannot see the mesh block — cross-component); it is validated at SPAWN
/// (`components::brain::validate_brain_animation_states`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiDescriptor {
    pub detection_range: f32,
    pub attack_range: f32,
    pub leash_range: f32,
    pub attack_damage: f32,
    pub attack_cooldown_ms: f32,
    pub move_speed: f32,
    pub death_despawn_ms: f32,
    pub states: AiStateNames,
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
    pub fn validate(self) -> Result<Self, DescriptorError> {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn weapon_descriptor(credit_source: Option<&str>) -> WeaponDescriptor {
        WeaponDescriptor {
            damage: 10.0,
            range: 64.0,
            cooldown_ms: 180.0,
            fire_mode: FireMode::Semi,
            resolution: ResolutionMode::Hitscan,
            credit_source: credit_source.map(str::to_string),
            third_person_model: None,
            viewmodel: None,
            resource: None,
        }
    }

    #[test]
    fn weapon_credit_source_accepts_allowed_ascii_identifier_and_omission() {
        let valid = "Alpha_09.source:primary-alt";

        let parsed = weapon_descriptor(Some(valid)).validate().unwrap();
        assert_eq!(parsed.credit_source.as_deref(), Some(valid));

        let omitted = weapon_descriptor(None).validate().unwrap();
        assert_eq!(omitted.credit_source, None);
    }

    #[test]
    fn weapon_credit_source_rejects_empty_overlength_and_disallowed_bytes() {
        for invalid in ["", "bad source", "rocket/primary", "plasma.\u{00e9}"] {
            let err = weapon_descriptor(Some(invalid)).validate().unwrap_err();
            assert!(
                err.to_string().contains("creditSource"),
                "unexpected error for {invalid:?}: {err}"
            );
        }

        let too_long = "a".repeat(65);
        let err = weapon_descriptor(Some(&too_long)).validate().unwrap_err();
        assert!(
            err.to_string().contains("64 bytes"),
            "unexpected overlength error: {err}"
        );
    }

    #[test]
    fn weapon_ammo_resource_defaults_and_validates() {
        let mut descriptor = weapon_descriptor(None);
        descriptor.resource = Some(WeaponResource::Ammo(AmmoResource {
            ammo_type: "shells.primary".to_string(),
            magazine: 8,
            cost_per_shot: 1,
            reserve: 0,
            reload_ms: 1000,
        }));
        assert!(descriptor.validate().is_ok());

        let parsed: WeaponDescriptor = serde_json::from_value(serde_json::json!({
            "damage": 10.0,
            "range": 64.0,
            "fireRateMs": 180.0,
            "fireMode": "semi",
            "resolution": "hitscan",
            "resource": {
                "kind": "ammo",
                "type": "shells",
                "magazine": 8,
                "reserve": 32
            }
        }))
        .unwrap();
        let Some(WeaponResource::Ammo(ammo)) = parsed.resource else {
            panic!("expected ammo resource");
        };
        assert_eq!(ammo.cost_per_shot, 1);
        assert_eq!(ammo.reload_ms, 1000);
    }

    #[test]
    fn weapon_ammo_resource_rejects_semantically_invalid_values() {
        for (field, value) in [
            ("type", serde_json::json!("bad ammo")),
            ("magazine", serde_json::json!(0)),
            ("costPerShot", serde_json::json!(0)),
            ("reloadMs", serde_json::json!(0)),
        ] {
            let mut ammo = serde_json::json!({
                "kind": "ammo",
                "type": "shells",
                "magazine": 8,
                "costPerShot": 1,
                "reserve": 32,
                "reloadMs": 1000
            });
            ammo[field] = value;
            let mut descriptor = weapon_descriptor(None);
            descriptor.resource = Some(serde_json::from_value(ammo).unwrap());
            let err = descriptor.validate().unwrap_err();
            assert!(err.to_string().contains(field), "unexpected error: {err}");
        }

        for invalid_type in ["", "rocket/primary", "plasma.\u{00e9}"] {
            let mut descriptor = weapon_descriptor(None);
            descriptor.resource = Some(WeaponResource::Ammo(AmmoResource {
                ammo_type: invalid_type.to_string(),
                magazine: 8,
                cost_per_shot: 1,
                reserve: 32,
                reload_ms: 1000,
            }));
            assert!(descriptor.validate().is_err(), "accepted {invalid_type:?}");
        }

        let mut descriptor = weapon_descriptor(None);
        descriptor.resource = Some(WeaponResource::Ammo(AmmoResource {
            ammo_type: "a".repeat(65),
            magazine: 8,
            cost_per_shot: 1,
            reserve: 32,
            reload_ms: 1000,
        }));
        let err = descriptor.validate().unwrap_err();
        assert!(err.to_string().contains("64 bytes"));
    }

    #[test]
    fn weapon_ammo_resource_rejects_invalid_serde_shapes() {
        for resource in [
            serde_json::json!({"kind": "cell", "type": "cells", "magazine": 8, "reserve": 32}),
            serde_json::json!({"kind": "ammo", "type": "cells", "magazine": -1, "reserve": 32}),
            serde_json::json!({"kind": "ammo", "type": "cells", "magazine": 8, "reserve": -1}),
            serde_json::json!({"kind": "ammo", "type": "cells", "magazine": "8", "reserve": 32}),
        ] {
            assert!(serde_json::from_value::<WeaponResource>(resource).is_err());
        }
    }
}
