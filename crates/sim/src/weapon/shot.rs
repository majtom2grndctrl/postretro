// See: context/lib/entity_model.md §4, §5 · context/lib/networking.md
//! Immutable shot facts captured when a fixed-tick activation emits an attempt.
use super::execution::ResolvedActivationShot;
use glam::Vec3;
use postretro_entities::components::weapon::WeaponComponent;
use postretro_foundation::{
    KnockbackDescriptor, ProjectileBodyVisual, ProjectileDescriptor, ResolutionMode,
    SplashDescriptor, WeaponActivationDescriptor,
};
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedWeaponShot {
    pub activation: ResolvedActivationShot,
    pub pellet_count: u32,
    pub spread_degrees: f32,
    pub bloom_per_shot_degrees: f32,
    pub bloom_max_degrees: f32,
    pub bloom_decay_degrees_per_second: f32,
    pub bloom_decay_delay_ms: f32,
    pub movement_spread_degrees: f32,
    pub spread_vertical_bias: f32,
    pub resolution: ResolutionMode,
    pub knockback: Option<KnockbackDescriptor>,
    pub splash: Option<SplashDescriptor>,
    pub projectile: Option<ProjectileDescriptor>,
    pub muzzle_offset: Option<Vec3>,
    pub credit_source: String,
    /// Uniform model scale; sprites bake their size into the descriptor snapshot.
    pub projectile_model_scale: f32,
}
pub fn freeze_weapon_shot(
    weapon: &WeaponComponent,
    activation: ResolvedActivationShot,
) -> ResolvedWeaponShot {
    let values = activation.values;
    let mut projectile = weapon.projectile.clone();
    let mut model_scale = 1.0;
    if let Some(projectile) = projectile.as_mut() {
        projectile.speed = values.projectile_speed.unwrap_or(projectile.speed);
        projectile.radius = values.projectile_radius.unwrap_or(projectile.radius);
        match &mut projectile.visual.body {
            ProjectileBodyVisual::Sprite { size, .. } => {
                *size = values.projectile_size.unwrap_or(*size)
            }
            ProjectileBodyVisual::Model { .. } => {
                model_scale = values.projectile_size.unwrap_or(1.0)
            }
        }
    }
    let mut knockback = weapon.knockback;
    if let Some(knockback) = knockback.as_mut() {
        knockback.speed = values.knockback_speed.unwrap_or(knockback.speed);
    }
    let mut splash = weapon.splash.clone();
    if let Some(knockback) = splash.as_mut().and_then(|s| s.knockback.as_mut()) {
        knockback.speed = values.splash_knockback_speed.unwrap_or(knockback.speed);
    }
    ResolvedWeaponShot {
        activation,
        pellet_count: weapon.pellet_count,
        spread_degrees: weapon.spread_degrees,
        bloom_per_shot_degrees: weapon.bloom_per_shot_degrees,
        bloom_max_degrees: weapon.bloom_max_degrees,
        bloom_decay_degrees_per_second: weapon.bloom_decay_degrees_per_second,
        bloom_decay_delay_ms: weapon.bloom_decay_delay_ms,
        movement_spread_degrees: weapon.movement_spread_degrees,
        spread_vertical_bias: weapon.spread_vertical_bias,
        resolution: weapon.resolution,
        knockback,
        splash,
        projectile,
        muzzle_offset: weapon.muzzle_offset,
        credit_source: weapon.credit_source.clone(),
        projectile_model_scale: model_scale,
    }
}
impl ResolvedWeaponShot {
    /// Re-evaluate host charge against the original installed program and bases.
    pub fn with_authoritative_charge(
        &self,
        charge: f32,
    ) -> Result<Self, postretro_foundation::ActivationScaleError> {
        let mut corrected = self.clone();
        corrected.activation = self.activation.with_authoritative_charge(charge)?;
        let values = corrected.activation.values;
        if let Some(projectile) = corrected.projectile.as_mut() {
            projectile.speed = values.projectile_speed.unwrap_or(projectile.speed);
            projectile.radius = values.projectile_radius.unwrap_or(projectile.radius);
            match &mut projectile.visual.body {
                ProjectileBodyVisual::Sprite { size, .. } => {
                    *size = values.projectile_size.unwrap_or(*size)
                }
                ProjectileBodyVisual::Model { .. } => {
                    corrected.projectile_model_scale = values.projectile_size.unwrap_or(1.0)
                }
            }
        }
        if let Some(knockback) = corrected.knockback.as_mut() {
            knockback.speed = values.knockback_speed.unwrap_or(knockback.speed);
        }
        if let Some(knockback) = corrected
            .splash
            .as_mut()
            .and_then(|splash| splash.knockback.as_mut())
        {
            knockback.speed = values.splash_knockback_speed.unwrap_or(knockback.speed);
        }
        Ok(corrected)
    }
    pub fn action(&self) -> &Arc<WeaponActivationDescriptor> {
        &self.activation.action
    }
    pub(super) fn apply_to(&self, component: &mut WeaponComponent) {
        component.damage = self.activation.values.damage;
        component.range = self.activation.values.range;
        component.pellet_count = self.pellet_count;
        component.spread_degrees = self.spread_degrees;
        component.bloom_per_shot_degrees = self.bloom_per_shot_degrees;
        component.bloom_max_degrees = self.bloom_max_degrees;
        component.bloom_decay_degrees_per_second = self.bloom_decay_degrees_per_second;
        component.bloom_decay_delay_ms = self.bloom_decay_delay_ms;
        component.movement_spread_degrees = self.movement_spread_degrees;
        component.spread_vertical_bias = self.spread_vertical_bias;
        component.resolution = self.resolution;
        component.knockback = self.knockback;
        component.splash = self.splash.clone();
        component.projectile = self.projectile.clone();
        component.muzzle_offset = self.muzzle_offset;
        component.credit_source = self.credit_source.clone();
    }
}
