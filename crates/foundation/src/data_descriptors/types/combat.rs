// Data-context descriptors: weapon/health/ai descriptors.
// See: context/lib/scripting.md

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::data_descriptors::{
    DescriptorError, is_portable_content_relative_asset_path, validate_ascii_identifier,
};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ReloadStyle {
    Magazine,
    PerShell,
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
    #[serde(default = "default_reload_style", rename = "reloadStyle")]
    pub reload_style: ReloadStyle,
}

const fn default_cost_per_shot() -> u32 {
    1
}

const fn default_reload_ms() -> u32 {
    1000
}

const fn default_reload_style() -> ReloadStyle {
    ReloadStyle::Magazine
}

/// Hard upper bound for authored weapon pellets per shell.
pub const MAX_PELLET_COUNT: u32 = 32;

const fn default_pellet_count() -> u32 {
    1
}

/// Authored weapon component preset. This is descriptor-owned tuning data:
/// maps do not override these params, and the runtime materializes a separate
/// wieldable instance entity from the descriptor at player spawn.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WeaponDescriptor {
    pub damage: f32,
    #[serde(default = "default_pellet_count")]
    pub pellet_count: u32,
    #[serde(default)]
    pub spread_degrees: f32,
    pub range: f32,
    #[serde(rename = "fireRateMs")]
    pub cooldown_ms: f32,
    pub fire_mode: FireMode,
    pub resolution: ResolutionMode,
    #[serde(default, rename = "creditSource")]
    pub credit_source: Option<String>,
    /// Optional content-relative rigid prop model mounted at the pawn's third-person hand socket.
    /// Uses forward slashes and may not be absolute or contain parent traversal.
    #[serde(default, rename = "thirdPersonModel")]
    pub third_person_model: Option<String>,
    /// Optional content-relative model rendered by the first-person viewmodel pass.
    /// Uses forward slashes and may not be absolute or contain parent traversal.
    #[serde(default)]
    pub viewmodel: Option<String>,
    #[serde(default)]
    pub resource: Option<WeaponResource>,
    #[serde(default, rename = "lowerMs")]
    pub lower_ms: u32,
    #[serde(default, rename = "raiseMs")]
    pub raise_ms: u32,
    /// Optional override of the mod-global reload-interrupt policy. Resolution
    /// belongs to the commit gate, so the component retains this unresolved.
    #[serde(default, rename = "blockDuringReload")]
    pub block_during_reload: Option<bool>,
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
        if !(1..=MAX_PELLET_COUNT).contains(&self.pellet_count) {
            return Err(DescriptorError::InvalidShape {
                reason: format!(
                    "`components.weapon.pelletCount` must be in 1..={MAX_PELLET_COUNT}, got {}",
                    self.pellet_count
                ),
            });
        }
        if !self.spread_degrees.is_finite() || !(0.0..=45.0).contains(&self.spread_degrees) {
            return Err(DescriptorError::InvalidShape {
                reason: format!(
                    "`components.weapon.spreadDegrees` must be a finite value in 0.0..=45.0, got {}",
                    self.spread_degrees
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
            if let Some(path) = path
                && !is_portable_content_relative_asset_path(path)
            {
                return Err(DescriptorError::InvalidShape {
                    reason: format!(
                        "`components.weapon.{field}` must be a non-empty, content-relative model path using forward slashes with no parent traversal"
                    ),
                });
            }
        }
        if let Some(WeaponResource::Ammo(ammo)) = self.resource.as_ref() {
            validate_ascii_identifier("components.weapon.resource.type", &ammo.ammo_type)?;
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TouchMode {
    Auto,
    Press,
}

const fn default_touch_mode() -> TouchMode {
    TouchMode::Auto
}

const fn default_touch_radius() -> f32 {
    40.0
}

/// Authored touch interaction preset for a world-placeable descriptor.
/// Both fields are descriptor-owned gameplay tuning; maps provide placement,
/// never interaction tuning.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TouchableDescriptor {
    #[serde(default = "default_touch_mode")]
    pub mode: TouchMode,
    #[serde(default = "default_touch_radius")]
    pub radius: f32,
}

impl TouchableDescriptor {
    pub fn validate(self) -> Result<Self, DescriptorError> {
        if !self.radius.is_finite() || self.radius <= 0.0 {
            return Err(DescriptorError::InvalidShape {
                reason: format!(
                    "`components.touchable.radius` must be a finite value > 0.0, got {}",
                    self.radius
                ),
            });
        }
        Ok(self)
    }
}

fn validate_credit_source(value: &str) -> Result<(), DescriptorError> {
    validate_ascii_identifier("components.weapon.creditSource", value)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn weapon_descriptor(credit_source: Option<&str>) -> WeaponDescriptor {
        WeaponDescriptor {
            damage: 10.0,
            pellet_count: 1,
            spread_degrees: 0.0,
            range: 64.0,
            cooldown_ms: 180.0,
            fire_mode: FireMode::Semi,
            resolution: ResolutionMode::Hitscan,
            credit_source: credit_source.map(str::to_string),
            third_person_model: None,
            viewmodel: None,
            resource: None,
            lower_ms: 0,
            raise_ms: 0,
            block_during_reload: None,
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
    fn weapon_pellet_stats_validate_their_authored_bounds() {
        let mut descriptor = weapon_descriptor(None);
        descriptor.pellet_count = MAX_PELLET_COUNT;
        descriptor.spread_degrees = 45.0;
        assert!(descriptor.clone().validate().is_ok());

        for pellet_count in [0, MAX_PELLET_COUNT + 1] {
            let mut invalid = descriptor.clone();
            invalid.pellet_count = pellet_count;
            let error = invalid.validate().unwrap_err();
            assert!(error.to_string().contains("pelletCount"), "{error}");
        }

        for spread_degrees in [-0.1, 45.1, f32::NAN, f32::INFINITY] {
            let mut invalid = descriptor.clone();
            invalid.spread_degrees = spread_degrees;
            let error = invalid.validate().unwrap_err();
            assert!(error.to_string().contains("spreadDegrees"), "{error}");
        }
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
            reload_style: ReloadStyle::Magazine,
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
        assert_eq!(ammo.reload_style, ReloadStyle::Magazine);
    }

    #[test]
    fn weapon_ammo_resource_reload_style_serde_accepts_known_values_and_rejects_unknown() {
        for (value, expected) in [
            ("magazine", ReloadStyle::Magazine),
            ("perShell", ReloadStyle::PerShell),
        ] {
            let resource: WeaponResource = serde_json::from_value(serde_json::json!({
                "kind": "ammo",
                "type": "shells",
                "magazine": 8,
                "reserve": 32,
                "reloadStyle": value,
            }))
            .unwrap();
            let WeaponResource::Ammo(ammo) = resource;
            assert_eq!(ammo.reload_style, expected);
        }

        let error = serde_json::from_value::<WeaponResource>(serde_json::json!({
            "kind": "ammo",
            "type": "shells",
            "magazine": 8,
            "reserve": 32,
            "reloadStyle": "belt",
        }))
        .unwrap_err();
        assert!(error.to_string().contains("unknown variant"), "{error}");
    }

    #[test]
    fn optional_weapon_model_paths_must_be_contained_content_relative_paths() {
        for invalid in [
            "",
            "/tmp/model.gltf",
            "../model.gltf",
            "models/../model.gltf",
            r"..\model.gltf",
            r"C:\models\model.gltf",
            "C:/models/model.gltf",
            "C:models/model.gltf",
            r"\\server\share\model.gltf",
        ] {
            for field in ["thirdPersonModel", "viewmodel"] {
                let mut descriptor = weapon_descriptor(None);
                if field == "thirdPersonModel" {
                    descriptor.third_person_model = Some(invalid.to_string());
                } else {
                    descriptor.viewmodel = Some(invalid.to_string());
                }
                let error = descriptor.validate().unwrap_err().to_string();
                assert!(
                    error.contains(field),
                    "unexpected error for {invalid:?}: {error}"
                );
                assert!(
                    error.contains("content-relative"),
                    "unexpected error for {invalid:?}: {error}"
                );
            }
        }

        let mut valid = weapon_descriptor(None);
        valid.third_person_model = Some("models/smg/model.gltf".to_string());
        valid.viewmodel = Some("./models/smg/view.gltf".to_string());
        assert!(valid.validate().is_ok());
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
                reload_style: ReloadStyle::Magazine,
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
            reload_style: ReloadStyle::Magazine,
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

    #[test]
    fn touchable_descriptor_defaults_and_rejects_nonpositive_or_nonfinite_radius() {
        let defaults: TouchableDescriptor = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(defaults.mode, TouchMode::Auto);
        assert!((defaults.radius - 40.0).abs() <= f32::EPSILON);

        let press_only: TouchableDescriptor =
            serde_json::from_value(serde_json::json!({ "mode": "press" })).unwrap();
        assert_eq!(press_only.mode, TouchMode::Press);
        assert!((press_only.radius - 40.0).abs() <= f32::EPSILON);

        for radius in [0.0, -1.0, f32::INFINITY, f32::NAN] {
            let error = TouchableDescriptor {
                mode: TouchMode::Auto,
                radius,
            }
            .validate()
            .expect_err("non-positive and non-finite touch radii must reject");
            assert!(error.to_string().contains("components.touchable.radius"));
        }
    }
}
