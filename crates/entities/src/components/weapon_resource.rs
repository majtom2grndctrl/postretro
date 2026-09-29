// Heat and cell live state carried by a weapon instance, beside its authored tuning.
// See: context/lib/entity_model.md §Components (Weapon vocabulary, Weapon state)

use serde::{Deserialize, Serialize};

use crate::data_descriptors::{CellResource, HeatResource, OverheatBehavior};

/// Which resource model a weapon instance runs. At most one is present.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WeaponResourceKind {
    None,
    Ammo,
    Heat,
    Cell,
}

/// Author-facing names of every [`WeaponResourceKind`]. The engine-state
/// catalog declares its `player.weaponResource` enum from this list.
pub const WEAPON_RESOURCE_KIND_NAMES: &[&str] = &[
    WeaponResourceKind::None.as_str(),
    WeaponResourceKind::Ammo.as_str(),
    WeaponResourceKind::Heat.as_str(),
    WeaponResourceKind::Cell.as_str(),
];

impl WeaponResourceKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Ammo => "ammo",
            Self::Heat => "heat",
            Self::Cell => "cell",
        }
    }
}

/// Heat tuning plus its live accumulator. The overheat latch lives here, not in
/// `WieldableState`: lowering overwrites the state machine, and a quick switch
/// must not clear an overheat.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct WeaponHeat {
    pub tuning: HeatResource,
    /// Clamped to `0..=tuning.overheat_at`.
    pub heat: f32,
    /// Set by the shot that reaches `overheat_at`; cleared only at heat 0.
    pub overheated: bool,
    /// Milliseconds since the last accepted shot.
    pub idle_ms: f32,
}

/// Cell tuning plus its live charge.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct WeaponCell {
    pub tuning: CellResource,
    /// Clamped to `0..=tuning.capacity`.
    pub charge: f32,
    /// Milliseconds since the last accepted shot.
    pub idle_ms: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EffectiveHeatStats {
    pub heat_per_shot: f32,
    pub overheat_at: f32,
    pub cool_per_second: f32,
    pub cool_delay_ms: f32,
    pub overheat_behavior: OverheatBehavior,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EffectiveCellStats {
    pub capacity: f32,
    pub cost_per_shot: f32,
    pub regen_per_second: f32,
    pub regen_delay_ms: f32,
}

impl WeaponHeat {
    pub fn fresh(tuning: HeatResource) -> Self {
        Self {
            tuning,
            heat: 0.0,
            overheated: false,
            idle_ms: 0.0,
        }
    }

    /// Hot reload within the heat kind: new tuning, live values kept inside the
    /// new bounds. The latch survives so a reload cannot end a lockout early.
    pub fn retune(&mut self, tuning: HeatResource) {
        self.tuning = tuning;
        self.heat = self.heat.clamp(0.0, tuning.overheat_at);
    }

    pub fn effective(&self) -> EffectiveHeatStats {
        EffectiveHeatStats {
            heat_per_shot: self.tuning.heat_per_shot,
            overheat_at: self.tuning.overheat_at,
            cool_per_second: self.tuning.cool_per_second,
            cool_delay_ms: self.tuning.cool_delay_ms,
            overheat_behavior: self.tuning.overheat_behavior,
        }
    }
}

impl WeaponCell {
    pub fn fresh(tuning: CellResource) -> Self {
        Self {
            tuning,
            charge: tuning.capacity,
            idle_ms: 0.0,
        }
    }

    /// Hot reload within the cell kind: new tuning, live charge kept inside the
    /// new capacity.
    pub fn retune(&mut self, tuning: CellResource) {
        self.tuning = tuning;
        self.charge = self.charge.clamp(0.0, tuning.capacity);
    }

    pub fn effective(&self) -> EffectiveCellStats {
        EffectiveCellStats {
            capacity: self.tuning.capacity,
            cost_per_shot: self.tuning.cost_per_shot,
            regen_per_second: self.tuning.regen_per_second,
            regen_delay_ms: self.tuning.regen_delay_ms,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::weapon::WeaponComponent;
    use crate::data_descriptors::{
        AmmoResource, FireMode, ReloadStyle, ResolutionMode, WeaponDescriptor, WeaponResource,
    };

    fn descriptor(resource: Option<WeaponResource>) -> WeaponDescriptor {
        let mut descriptor: WeaponDescriptor = serde_json::from_value(serde_json::json!({
            "damage": 10.0,
            "range": 64.0,
            "fireRateMs": 100.0,
            "fireMode": "auto",
            "resolution": "hitscan",
        }))
        .unwrap();
        assert_eq!(descriptor.fire_mode, FireMode::Auto);
        assert_eq!(descriptor.resolution, ResolutionMode::Hitscan);
        descriptor.resource = resource;
        descriptor
    }

    fn heat_tuning(overheat_at: f32) -> HeatResource {
        HeatResource {
            heat_per_shot: 10.0,
            overheat_at,
            cool_per_second: 50.0,
            cool_delay_ms: 200.0,
            overheat_behavior: OverheatBehavior::Lockout,
        }
    }

    fn cell_tuning(capacity: f32) -> CellResource {
        CellResource {
            capacity,
            cost_per_shot: 4.0,
            regen_per_second: 8.0,
            regen_delay_ms: 1_500.0,
        }
    }

    fn ammo() -> WeaponResource {
        WeaponResource::Ammo(AmmoResource {
            ammo_type: "cells".to_string(),
            magazine: 12,
            cost_per_shot: 1,
            reserve: 24,
            reload_ms: 800,
            reload_style: ReloadStyle::Magazine,
        })
    }

    fn resource_count(component: &WeaponComponent) -> usize {
        usize::from(component.ammo.is_some())
            + usize::from(component.heat.is_some())
            + usize::from(component.cell.is_some())
    }

    #[test]
    fn weapon_heat_spawns_cold_unlatched_with_zero_idle() {
        let component = WeaponComponent::from_descriptor(&descriptor(Some(WeaponResource::Heat(
            heat_tuning(100.0),
        ))));
        assert_eq!(
            component.heat,
            Some(WeaponHeat {
                tuning: heat_tuning(100.0),
                heat: 0.0,
                overheated: false,
                idle_ms: 0.0,
            })
        );
        assert_eq!(component.resource_kind(), WeaponResourceKind::Heat);
        assert_eq!(resource_count(&component), 1);
    }

    #[test]
    fn weapon_cell_spawns_at_capacity_with_zero_idle() {
        let component = WeaponComponent::from_descriptor(&descriptor(Some(WeaponResource::Cell(
            cell_tuning(40.0),
        ))));
        assert_eq!(
            component.cell,
            Some(WeaponCell {
                tuning: cell_tuning(40.0),
                charge: 40.0,
                idle_ms: 0.0,
            })
        );
        assert_eq!(component.resource_kind(), WeaponResourceKind::Cell);
        assert_eq!(resource_count(&component), 1);
    }

    #[test]
    fn weapon_resource_construction_and_reload_keep_at_most_one_resource() {
        let variants = [
            None,
            Some(ammo()),
            Some(WeaponResource::Heat(heat_tuning(100.0))),
            Some(WeaponResource::Cell(cell_tuning(40.0))),
        ];
        let expected = [
            WeaponResourceKind::None,
            WeaponResourceKind::Ammo,
            WeaponResourceKind::Heat,
            WeaponResourceKind::Cell,
        ];
        for (from, from_kind) in variants.iter().zip(expected) {
            let component = WeaponComponent::from_descriptor(&descriptor(from.clone()));
            assert!(resource_count(&component) <= 1);
            assert_eq!(component.resource_kind(), from_kind);
            for (to, to_kind) in variants.iter().zip(expected) {
                let mut reloaded = component.clone();
                reloaded.refresh_from_descriptor(&descriptor(to.clone()));
                assert!(
                    resource_count(&reloaded) <= 1,
                    "{from_kind:?} -> {to_kind:?}"
                );
                assert_eq!(reloaded.resource_kind(), to_kind);
            }
        }
    }

    #[test]
    fn weapon_heat_same_kind_reload_clamps_heat_and_keeps_the_latch() {
        let mut component = WeaponComponent::from_descriptor(&descriptor(Some(
            WeaponResource::Heat(heat_tuning(100.0)),
        )));
        let heat = component.heat.as_mut().unwrap();
        heat.heat = 90.0;
        heat.overheated = true;
        heat.idle_ms = 75.0;

        component
            .refresh_from_descriptor(&descriptor(Some(WeaponResource::Heat(heat_tuning(60.0)))));

        let heat = component.heat.unwrap();
        assert_eq!(heat.tuning, heat_tuning(60.0));
        assert!((heat.heat - 60.0).abs() < f32::EPSILON);
        assert!(heat.overheated, "a reload must not end a lockout");
        assert!((heat.idle_ms - 75.0).abs() < f32::EPSILON);
    }

    #[test]
    fn weapon_cell_same_kind_reload_clamps_charge_and_keeps_idle() {
        let mut component = WeaponComponent::from_descriptor(&descriptor(Some(
            WeaponResource::Cell(cell_tuning(40.0)),
        )));
        let cell = component.cell.as_mut().unwrap();
        cell.charge = 35.0;
        cell.idle_ms = 900.0;

        component
            .refresh_from_descriptor(&descriptor(Some(WeaponResource::Cell(cell_tuning(20.0)))));
        let cell = component.cell.unwrap();
        assert_eq!(cell.tuning, cell_tuning(20.0));
        assert!((cell.charge - 20.0).abs() < f32::EPSILON);
        assert!((cell.idle_ms - 900.0).abs() < f32::EPSILON);

        // A larger capacity keeps the drained charge rather than refilling it.
        component
            .refresh_from_descriptor(&descriptor(Some(WeaponResource::Cell(cell_tuning(80.0)))));
        assert!((component.cell.unwrap().charge - 20.0).abs() < f32::EPSILON);
    }

    #[test]
    fn weapon_resource_kind_change_on_reload_rebuilds_fresh() {
        let mut component = WeaponComponent::from_descriptor(&descriptor(Some(
            WeaponResource::Heat(heat_tuning(100.0)),
        )));
        let heat = component.heat.as_mut().unwrap();
        heat.heat = 100.0;
        heat.overheated = true;

        component
            .refresh_from_descriptor(&descriptor(Some(WeaponResource::Cell(cell_tuning(40.0)))));
        assert_eq!(component.heat, None);
        assert_eq!(component.cell, Some(WeaponCell::fresh(cell_tuning(40.0))));

        component.cell.as_mut().unwrap().charge = 3.0;
        component
            .refresh_from_descriptor(&descriptor(Some(WeaponResource::Heat(heat_tuning(100.0)))));
        assert_eq!(component.cell, None);
        assert_eq!(component.heat, Some(WeaponHeat::fresh(heat_tuning(100.0))));
    }

    #[test]
    fn weapon_effective_projects_heat_and_cell_tuning() {
        let heat = WeaponComponent::from_descriptor(&descriptor(Some(WeaponResource::Heat(
            heat_tuning(100.0),
        ))));
        let stats = heat.effective();
        assert_eq!(
            stats.heat,
            Some(EffectiveHeatStats {
                heat_per_shot: 10.0,
                overheat_at: 100.0,
                cool_per_second: 50.0,
                cool_delay_ms: 200.0,
                overheat_behavior: OverheatBehavior::Lockout,
            })
        );
        assert_eq!(stats.ammo, None);
        assert_eq!(stats.cell, None);

        let cell = WeaponComponent::from_descriptor(&descriptor(Some(WeaponResource::Cell(
            cell_tuning(40.0),
        ))));
        let stats = cell.effective();
        assert_eq!(
            stats.cell,
            Some(EffectiveCellStats {
                capacity: 40.0,
                cost_per_shot: 4.0,
                regen_per_second: 8.0,
                regen_delay_ms: 1_500.0,
            })
        );
        assert_eq!(stats.ammo, None);
        assert_eq!(stats.heat, None);
    }

    #[test]
    fn weapon_heat_cell_state_defaults_when_absent_from_persisted_component() {
        let component = WeaponComponent::from_descriptor(&descriptor(Some(WeaponResource::Cell(
            cell_tuning(40.0),
        ))));
        let mut persisted = serde_json::to_value(&component).unwrap();
        assert_eq!(
            serde_json::from_value::<WeaponComponent>(persisted.clone())
                .unwrap()
                .cell,
            component.cell
        );
        let object = persisted.as_object_mut().unwrap();
        object.remove("heat");
        object.remove("cell");
        let legacy: WeaponComponent = serde_json::from_value(persisted).unwrap();
        assert_eq!(legacy.heat, None);
        assert_eq!(legacy.cell, None);
    }
}
