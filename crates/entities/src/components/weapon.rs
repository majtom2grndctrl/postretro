// Weapon component: descriptor-authored tuning plus live cooldown, magazine,
// input-edge, and reload state.
//
// See: context/lib/entity_model.md §4 (descriptor-owned tuning params)

use serde::{Deserialize, Serialize};
#[cfg(debug_assertions)]
use std::sync::Once;

use crate::data_descriptors::{FireMode, ResolutionMode, WeaponDescriptor, WeaponResource};

pub const UNKNOWN_WEAPON_CREDIT_SOURCE: &str = "weapon.unknown";

#[cfg(debug_assertions)]
static WARNED_UNKNOWN_CREDIT_SOURCE: Once = Once::new();

#[derive(Debug, Clone, PartialEq)]
pub struct EffectiveAmmoStats<'a> {
    pub ammo_type: &'a str,
    pub capacity: u32,
    pub cost_per_shot: u32,
    pub reload_ms: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EffectiveStats<'a> {
    pub damage: f32,
    pub range: f32,
    pub cooldown_ms: f32,
    pub fire_mode: FireMode,
    pub resolution: ResolutionMode,
    pub credit_source: &'a str,
    pub ammo: Option<EffectiveAmmoStats<'a>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WeaponAmmoTuning {
    pub ammo_type: String,
    pub capacity: u32,
    pub cost_per_shot: u32,
    pub reload_ms: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WeaponComponent {
    pub damage: f32,
    pub range: f32,
    pub cooldown_ms: f32,
    pub fire_mode: FireMode,
    pub resolution: ResolutionMode,
    pub cooldown_remaining_ms: f32,
    #[serde(default)]
    pub shoot_press_consumed: bool,
    #[serde(default)]
    pub reload_press_consumed: bool,
    #[serde(default = "default_credit_source")]
    pub credit_source: String,
    #[serde(default)]
    pub ammo: Option<WeaponAmmoTuning>,
    #[serde(default)]
    pub magazine: u32,
    #[serde(default)]
    pub reload_remaining_ms: u32,
    #[serde(default)]
    pub reload_total_ms: u32,
    /// Fractional elapsed milliseconds carried between fixed ticks. Public HUD
    /// and replication fields remain integer milliseconds; this remainder keeps
    /// their countdown from accumulating per-tick rounding bias.
    #[serde(default)]
    pub reload_elapsed_sub_ms: f64,
}

impl WeaponComponent {
    pub fn from_descriptor(desc: &WeaponDescriptor) -> Self {
        Self::from_descriptor_with_canonical(desc, None)
    }

    pub fn from_descriptor_with_canonical(
        desc: &WeaponDescriptor,
        canonical_name: Option<&str>,
    ) -> Self {
        let ammo = ammo_tuning(desc);
        let magazine = ammo.as_ref().map_or(0, |ammo| ammo.capacity);
        Self {
            damage: desc.damage,
            range: desc.range,
            cooldown_ms: desc.cooldown_ms,
            fire_mode: desc.fire_mode,
            resolution: desc.resolution,
            cooldown_remaining_ms: 0.0,
            shoot_press_consumed: false,
            reload_press_consumed: false,
            credit_source: resolve_credit_source(desc, canonical_name),
            ammo,
            magazine,
            reload_remaining_ms: 0,
            reload_total_ms: 0,
            reload_elapsed_sub_ms: 0.0,
        }
    }

    pub fn effective(&self) -> EffectiveStats<'_> {
        EffectiveStats {
            damage: self.damage,
            range: self.range,
            cooldown_ms: self.cooldown_ms,
            fire_mode: self.fire_mode,
            resolution: self.resolution,
            credit_source: &self.credit_source,
            ammo: self.ammo.as_ref().map(|ammo| EffectiveAmmoStats {
                ammo_type: &ammo.ammo_type,
                capacity: ammo.capacity,
                cost_per_shot: ammo.cost_per_shot,
                reload_ms: ammo.reload_ms,
            }),
        }
    }

    pub fn refresh_from_descriptor(&mut self, desc: &WeaponDescriptor) {
        self.damage = desc.damage;
        self.range = desc.range;
        self.cooldown_ms = desc.cooldown_ms;
        self.fire_mode = desc.fire_mode;
        self.resolution = desc.resolution;
        if let Some(credit_source) = desc.credit_source.as_ref() {
            self.credit_source = credit_source.clone();
        }
        self.ammo = ammo_tuning(desc);
        // Cooldown, input edges, magazine, and all reload timer values are live
        // instance state. Hot reload changes authored tuning, not
        // the current input edges, ammunition, active reload sample, or whether
        // this instance is mid-cooldown. An absent `creditSource` also keeps the
        // already-resolved spawn-time default so canonical defaults do not
        // regress to `weapon.unknown` on reload.
    }
}

fn ammo_tuning(desc: &WeaponDescriptor) -> Option<WeaponAmmoTuning> {
    desc.resource.as_ref().map(|resource| match resource {
        WeaponResource::Ammo(ammo) => WeaponAmmoTuning {
            ammo_type: ammo.ammo_type.clone(),
            capacity: ammo.magazine,
            cost_per_shot: ammo.cost_per_shot,
            reload_ms: ammo.reload_ms,
        },
    })
}

fn resolve_credit_source(desc: &WeaponDescriptor, canonical_name: Option<&str>) -> String {
    if let Some(credit_source) = desc.credit_source.as_ref() {
        return credit_source.clone();
    }
    if let Some(canonical_name) = canonical_name {
        return canonical_name.to_string();
    }
    warn_unknown_credit_source_once();
    UNKNOWN_WEAPON_CREDIT_SOURCE.to_string()
}

fn default_credit_source() -> String {
    UNKNOWN_WEAPON_CREDIT_SOURCE.to_string()
}

#[cfg(debug_assertions)]
fn warn_unknown_credit_source_once() {
    WARNED_UNKNOWN_CREDIT_SOURCE.call_once(|| {
        log::warn!(
            "weapon descriptor materialized without authored creditSource or canonical name; using {UNKNOWN_WEAPON_CREDIT_SOURCE}"
        );
    });
}

#[cfg(not(debug_assertions))]
fn warn_unknown_credit_source_once() {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data_descriptors::AmmoResource;

    fn descriptor(damage: f32, range: f32, cooldown_ms: f32) -> WeaponDescriptor {
        WeaponDescriptor {
            damage,
            range,
            cooldown_ms,
            fire_mode: FireMode::Semi,
            resolution: ResolutionMode::Hitscan,
            credit_source: None,
            resource: None,
        }
    }

    fn ammo_descriptor(
        ammo_type: &str,
        capacity: u32,
        cost_per_shot: u32,
        reload_ms: u32,
    ) -> WeaponDescriptor {
        let mut descriptor = descriptor(10.0, 20.0, 100.0);
        descriptor.resource = Some(WeaponResource::Ammo(AmmoResource {
            ammo_type: ammo_type.to_string(),
            magazine: capacity,
            cost_per_shot,
            reserve: 48,
            reload_ms,
        }));
        descriptor
    }

    #[test]
    fn from_descriptor_seeds_ammo_tuning_full_magazine_and_idle_reload() {
        let component =
            WeaponComponent::from_descriptor(&ammo_descriptor("bullets.light", 12, 2, 850));

        assert_eq!(
            component.ammo,
            Some(WeaponAmmoTuning {
                ammo_type: "bullets.light".to_string(),
                capacity: 12,
                cost_per_shot: 2,
                reload_ms: 850,
            })
        );
        assert_eq!(component.magazine, 12);
        assert_eq!(component.reload_remaining_ms, 0);
        assert_eq!(component.reload_total_ms, 0);
        assert_eq!(component.reload_elapsed_sub_ms, 0.0);
    }

    #[test]
    fn from_descriptor_without_resource_preserves_unlimited_fire_state() {
        let component = WeaponComponent::from_descriptor(&descriptor(10.0, 20.0, 100.0));

        assert_eq!(component.ammo, None);
        assert_eq!(component.magazine, 0);
        assert_eq!(component.reload_remaining_ms, 0);
        assert_eq!(component.reload_total_ms, 0);
        assert_eq!(component.effective().ammo, None);
    }

    #[test]
    fn effective_projects_authored_ammo_stats() {
        let component =
            WeaponComponent::from_descriptor(&ammo_descriptor("shells.heavy", 8, 1, 1200));

        assert_eq!(
            component.effective().ammo,
            Some(EffectiveAmmoStats {
                ammo_type: "shells.heavy",
                capacity: 8,
                cost_per_shot: 1,
                reload_ms: 1200,
            })
        );
    }

    #[test]
    fn refresh_updates_ammo_tuning_and_preserves_all_live_state() {
        let mut component =
            WeaponComponent::from_descriptor(&ammo_descriptor("bullets", 12, 1, 800));
        component.magazine = 3;
        component.reload_remaining_ms = 275;
        component.reload_total_ms = 800;
        component.reload_elapsed_sub_ms = 0.625;
        component.cooldown_remaining_ms = 42.0;
        component.shoot_press_consumed = true;
        component.reload_press_consumed = true;

        component.refresh_from_descriptor(&ammo_descriptor("cells", 30, 3, 1400));

        assert_eq!(
            component.ammo,
            Some(WeaponAmmoTuning {
                ammo_type: "cells".to_string(),
                capacity: 30,
                cost_per_shot: 3,
                reload_ms: 1400,
            })
        );
        assert_eq!(component.effective().ammo.unwrap().reload_ms, 1400);
        assert_eq!(component.magazine, 3);
        assert_eq!(component.reload_remaining_ms, 275);
        assert_eq!(component.reload_total_ms, 800);
        assert_eq!(component.reload_elapsed_sub_ms, 0.625);
        assert_eq!(component.cooldown_remaining_ms, 42.0);
        assert!(component.shoot_press_consumed);
        assert!(component.reload_press_consumed);
    }

    #[test]
    fn refresh_can_remove_ammo_tuning_without_aborting_live_reload() {
        let mut component =
            WeaponComponent::from_descriptor(&ammo_descriptor("bullets", 12, 1, 800));
        component.magazine = 4;
        component.reload_remaining_ms = 300;
        component.reload_total_ms = 800;

        component.refresh_from_descriptor(&descriptor(10.0, 20.0, 100.0));

        assert_eq!(component.ammo, None);
        assert_eq!(component.magazine, 4);
        assert_eq!(component.reload_remaining_ms, 300);
        assert_eq!(component.reload_total_ms, 800);
    }

    #[test]
    fn refresh_from_descriptor_updates_stats_and_preserves_live_state() {
        let mut component = WeaponComponent::from_descriptor_with_canonical(
            &descriptor(10.0, 20.0, 100.0),
            Some("reference_pistol"),
        );
        component.cooldown_remaining_ms = 42.0;
        component.shoot_press_consumed = true;

        component.refresh_from_descriptor(&descriptor(25.0, 80.0, 250.0));

        assert_eq!(component.damage, 25.0);
        assert_eq!(component.range, 80.0);
        assert_eq!(component.cooldown_ms, 250.0);
        assert_eq!(component.cooldown_remaining_ms, 42.0);
        assert!(component.shoot_press_consumed);
        assert_eq!(component.credit_source, "reference_pistol");
    }

    #[test]
    fn from_descriptor_prefers_authored_credit_source_over_canonical_name() {
        let mut descriptor = descriptor(10.0, 20.0, 100.0);
        descriptor.credit_source = Some("plasma.primary".to_string());

        let component =
            WeaponComponent::from_descriptor_with_canonical(&descriptor, Some("plasma_rifle"));

        assert_eq!(component.credit_source, "plasma.primary");
        assert_eq!(component.effective().credit_source, "plasma.primary");
    }

    #[test]
    fn from_descriptor_uses_canonical_name_when_credit_source_is_absent() {
        let component = WeaponComponent::from_descriptor_with_canonical(
            &descriptor(10.0, 20.0, 100.0),
            Some("reference_pistol"),
        );

        assert_eq!(component.credit_source, "reference_pistol");
        assert_eq!(component.effective().credit_source, "reference_pistol");
    }

    #[test]
    fn from_descriptor_uses_unknown_fallback_without_authored_or_canonical_source() {
        let component = WeaponComponent::from_descriptor(&descriptor(10.0, 20.0, 100.0));

        assert_eq!(component.credit_source, UNKNOWN_WEAPON_CREDIT_SOURCE);
    }

    #[test]
    fn refresh_from_descriptor_updates_authored_credit_source_when_present() {
        let mut component = WeaponComponent::from_descriptor_with_canonical(
            &descriptor(10.0, 20.0, 100.0),
            Some("reference_pistol"),
        );
        let mut reloaded = descriptor(25.0, 80.0, 250.0);
        reloaded.credit_source = Some("pistol.alt".to_string());

        component.refresh_from_descriptor(&reloaded);

        assert_eq!(component.credit_source, "pistol.alt");
    }
}
