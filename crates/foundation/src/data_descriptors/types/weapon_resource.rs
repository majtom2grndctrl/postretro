// Heat and cell variants of the weapon `resource` union: authored tuning and validation.
// See: context/lib/entity_model.md §Components (Weapon resources)

use serde::{Deserialize, Serialize};

use crate::data_descriptors::DescriptorError;

/// What a heat weapon does once it crosses `overheatAt`. Closed so an unknown
/// value is rejected; a second behavior lands as a new variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OverheatBehavior {
    /// Refuse fire until heat cools to zero.
    #[default]
    Lockout,
}

/// Heat accumulates per shot and dissipates over time; crossing `overheatAt`
/// latches the weapon until it cools fully. Units are author-scale heat,
/// rates per second, delays in milliseconds.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HeatResource {
    pub heat_per_shot: f32,
    pub overheat_at: f32,
    pub cool_per_second: f32,
    #[serde(default)]
    pub cool_delay_ms: f32,
    #[serde(default)]
    pub overheat_behavior: OverheatBehavior,
}

/// A per-instance charge that each shot drains and that regenerates over time.
/// Units are author-scale charge, rates per second, delays in milliseconds.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CellResource {
    pub capacity: f32,
    pub cost_per_shot: f32,
    pub regen_per_second: f32,
    #[serde(default)]
    pub regen_delay_ms: f32,
}

impl HeatResource {
    pub fn validate(&self) -> Result<(), DescriptorError> {
        require("overheatAt", self.overheat_at, Bound::Positive)?;
        require("heatPerShot", self.heat_per_shot, Bound::Positive)?;
        // Zero cooling would hold an overheat latch forever.
        require("coolPerSecond", self.cool_per_second, Bound::Positive)?;
        require("coolDelayMs", self.cool_delay_ms, Bound::NonNegative)?;
        // One shot may reach the threshold but never skip past a whole lockout.
        if self.heat_per_shot > self.overheat_at {
            return Err(invalid(format!(
                "`components.weapon.resource.heatPerShot` must be <= `overheatAt` ({}), got {}",
                self.overheat_at, self.heat_per_shot
            )));
        }
        Ok(())
    }
}

impl CellResource {
    pub fn validate(&self) -> Result<(), DescriptorError> {
        require("capacity", self.capacity, Bound::Positive)?;
        require("costPerShot", self.cost_per_shot, Bound::Positive)?;
        // Zero is a non-recharging battery.
        require("regenPerSecond", self.regen_per_second, Bound::NonNegative)?;
        require("regenDelayMs", self.regen_delay_ms, Bound::NonNegative)?;
        // A cost above capacity could never fire.
        if self.cost_per_shot > self.capacity {
            return Err(invalid(format!(
                "`components.weapon.resource.costPerShot` must be <= `capacity` ({}), got {}",
                self.capacity, self.cost_per_shot
            )));
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
enum Bound {
    Positive,
    NonNegative,
}

fn require(field: &str, value: f32, bound: Bound) -> Result<(), DescriptorError> {
    let (ok, rule) = match bound {
        Bound::Positive => (value > 0.0, "> 0.0"),
        Bound::NonNegative => (value >= 0.0, ">= 0.0"),
    };
    if value.is_finite() && ok {
        return Ok(());
    }
    Err(invalid(format!(
        "`components.weapon.resource.{field}` must be a finite value {rule}, got {value}"
    )))
}

fn invalid(reason: String) -> DescriptorError {
    DescriptorError::InvalidShape { reason }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data_descriptors::{WeaponDescriptor, WeaponResource};

    fn weapon_with(resource: serde_json::Value) -> Result<WeaponDescriptor, DescriptorError> {
        let descriptor: WeaponDescriptor = serde_json::from_value(serde_json::json!({
            "damage": 10.0,
            "range": 64.0,
            "primary": { "trigger": "hold", "recoveryMs": 100.0, "steps": [{ "kind": "shot" }] },

            "resolution": "hitscan",
            "resource": resource,
        }))
        .map_err(|error| DescriptorError::InvalidShape {
            reason: error.to_string(),
        })?;
        descriptor.validate()
    }

    fn heat() -> serde_json::Value {
        serde_json::json!({
            "kind": "heat",
            "heatPerShot": 10.0,
            "overheatAt": 100.0,
            "coolPerSecond": 50.0,
            "coolDelayMs": 250.0,
        })
    }

    fn cell() -> serde_json::Value {
        serde_json::json!({
            "kind": "cell",
            "capacity": 40.0,
            "costPerShot": 4.0,
            "regenPerSecond": 8.0,
            "regenDelayMs": 1500.0,
        })
    }

    #[test]
    fn weapon_heat_resource_parses_with_serde_defaults() {
        let descriptor = weapon_with(serde_json::json!({
            "kind": "heat",
            "heatPerShot": 12.5,
            "overheatAt": 100.0,
            "coolPerSecond": 40.0,
        }))
        .unwrap();
        assert_eq!(
            descriptor.resource,
            Some(WeaponResource::Heat(HeatResource {
                heat_per_shot: 12.5,
                overheat_at: 100.0,
                cool_per_second: 40.0,
                cool_delay_ms: 0.0,
                overheat_behavior: OverheatBehavior::Lockout,
            }))
        );
    }

    #[test]
    fn weapon_cell_resource_parses_with_serde_defaults() {
        let descriptor = weapon_with(serde_json::json!({
            "kind": "cell",
            "capacity": 30.0,
            "costPerShot": 3.0,
            "regenPerSecond": 0.0,
        }))
        .unwrap();
        assert_eq!(
            descriptor.resource,
            Some(WeaponResource::Cell(CellResource {
                capacity: 30.0,
                cost_per_shot: 3.0,
                regen_per_second: 0.0,
                regen_delay_ms: 0.0,
            }))
        );
    }

    #[test]
    fn weapon_heat_resource_accepts_boundary_values() {
        let mut resource = heat();
        resource["heatPerShot"] = serde_json::json!(100.0);
        resource["coolDelayMs"] = serde_json::json!(0.0);
        resource["overheatBehavior"] = serde_json::json!("lockout");
        assert!(weapon_with(resource).is_ok());
    }

    #[test]
    fn weapon_cell_resource_accepts_boundary_values() {
        let mut resource = cell();
        resource["costPerShot"] = serde_json::json!(40.0);
        resource["regenPerSecond"] = serde_json::json!(0.0);
        resource["regenDelayMs"] = serde_json::json!(0.0);
        assert!(weapon_with(resource).is_ok());
    }

    #[test]
    fn weapon_heat_resource_rejects_each_invalid_row_naming_its_field() {
        let rows: &[(&str, serde_json::Value)] = &[
            ("heatPerShot", serde_json::json!(0.0)),
            ("heatPerShot", serde_json::json!(-1.0)),
            ("heatPerShot", serde_json::json!(100.5)),
            ("overheatAt", serde_json::json!(0.0)),
            ("overheatAt", serde_json::json!(-5.0)),
            ("coolPerSecond", serde_json::json!(0.0)),
            ("coolPerSecond", serde_json::json!(-1.0)),
            ("coolDelayMs", serde_json::json!(-1.0)),
        ];
        for (field, value) in rows {
            let mut resource = heat();
            resource[*field] = value.clone();
            let error = weapon_with(resource).unwrap_err().to_string();
            assert!(
                error.contains(&format!("components.weapon.resource.{field}")),
                "{field} = {value}: {error}"
            );
        }
    }

    #[test]
    fn weapon_cell_resource_rejects_each_invalid_row_naming_its_field() {
        let rows: &[(&str, serde_json::Value)] = &[
            ("capacity", serde_json::json!(0.0)),
            ("capacity", serde_json::json!(-1.0)),
            ("costPerShot", serde_json::json!(0.0)),
            ("costPerShot", serde_json::json!(-2.0)),
            ("costPerShot", serde_json::json!(40.5)),
            ("regenPerSecond", serde_json::json!(-0.5)),
            ("regenDelayMs", serde_json::json!(-1.0)),
        ];
        for (field, value) in rows {
            let mut resource = cell();
            resource[*field] = value.clone();
            let error = weapon_with(resource).unwrap_err().to_string();
            assert!(
                error.contains(&format!("components.weapon.resource.{field}")),
                "{field} = {value}: {error}"
            );
        }
    }

    // JSON cannot carry a non-finite number, so construct the rows directly.
    #[test]
    fn weapon_heat_and_cell_resources_reject_non_finite_numbers() {
        let valid_heat = HeatResource {
            heat_per_shot: 10.0,
            overheat_at: 100.0,
            cool_per_second: 50.0,
            cool_delay_ms: 0.0,
            overheat_behavior: OverheatBehavior::Lockout,
        };
        let valid_cell = CellResource {
            capacity: 40.0,
            cost_per_shot: 4.0,
            regen_per_second: 8.0,
            regen_delay_ms: 0.0,
        };
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let heat_rows: [(&str, HeatResource); 4] = [
                (
                    "heatPerShot",
                    HeatResource {
                        heat_per_shot: bad,
                        ..valid_heat
                    },
                ),
                (
                    "overheatAt",
                    HeatResource {
                        overheat_at: bad,
                        ..valid_heat
                    },
                ),
                (
                    "coolPerSecond",
                    HeatResource {
                        cool_per_second: bad,
                        ..valid_heat
                    },
                ),
                (
                    "coolDelayMs",
                    HeatResource {
                        cool_delay_ms: bad,
                        ..valid_heat
                    },
                ),
            ];
            for (field, resource) in heat_rows {
                let error = resource.validate().unwrap_err().to_string();
                assert!(error.contains(field), "{field} = {bad}: {error}");
            }
            let cell_rows: [(&str, CellResource); 4] = [
                (
                    "capacity",
                    CellResource {
                        capacity: bad,
                        ..valid_cell
                    },
                ),
                (
                    "costPerShot",
                    CellResource {
                        cost_per_shot: bad,
                        ..valid_cell
                    },
                ),
                (
                    "regenPerSecond",
                    CellResource {
                        regen_per_second: bad,
                        ..valid_cell
                    },
                ),
                (
                    "regenDelayMs",
                    CellResource {
                        regen_delay_ms: bad,
                        ..valid_cell
                    },
                ),
            ];
            for (field, resource) in cell_rows {
                let error = resource.validate().unwrap_err().to_string();
                assert!(error.contains(field), "{field} = {bad}: {error}");
            }
        }
    }

    #[test]
    fn weapon_heat_and_cell_resources_reject_unknown_behavior_and_missing_fields() {
        let mut vent = heat();
        vent["overheatBehavior"] = serde_json::json!("vent");
        assert!(serde_json::from_value::<WeaponResource>(vent).is_err());

        for required in ["heatPerShot", "overheatAt", "coolPerSecond"] {
            let mut resource = heat();
            resource.as_object_mut().unwrap().remove(required);
            assert!(
                serde_json::from_value::<WeaponResource>(resource).is_err(),
                "{required} is required"
            );
        }
        for required in ["capacity", "costPerShot", "regenPerSecond"] {
            let mut resource = cell();
            resource.as_object_mut().unwrap().remove(required);
            assert!(
                serde_json::from_value::<WeaponResource>(resource).is_err(),
                "{required} is required"
            );
        }
    }

    #[test]
    fn weapon_sounds_accept_the_overheat_key() {
        let sounds: crate::data_descriptors::WeaponSounds =
            serde_json::from_value(serde_json::json!({ "overheat": "sfx/vent" })).unwrap();
        assert_eq!(sounds.overheat.as_deref(), Some("sfx/vent"));
        assert_eq!(
            sounds.keys().collect::<Vec<_>>(),
            vec![("overheat", "sfx/vent")]
        );
    }
}
