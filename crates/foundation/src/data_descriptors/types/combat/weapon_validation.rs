// Pure weapon descriptor validation, shared by both authoring runtimes.
use super::projectile_validation::{validate_projectile_descriptor, validate_splash_descriptor};
use super::*;

impl WeaponDescriptor {
    pub fn validate(self) -> Result<Self, DescriptorError> {
        if let Some(knockback) = &self.knockback {
            knockback.validate("components.weapon.knockback")?;
        }
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
        for (field, value) in [
            ("bloomPerShotDegrees", self.bloom_per_shot_degrees),
            ("bloomMaxDegrees", self.bloom_max_degrees),
            ("movementSpreadDegrees", self.movement_spread_degrees),
        ] {
            if !value.is_finite() || !(0.0..=45.0).contains(&value) {
                return Err(DescriptorError::InvalidShape {
                    reason: format!(
                        "`components.weapon.{field}` must be a finite value in 0.0..=45.0, got {value}"
                    ),
                });
            }
        }
        for (field, value) in [
            (
                "bloomDecayDegreesPerSecond",
                self.bloom_decay_degrees_per_second,
            ),
            ("bloomDecayDelayMs", self.bloom_decay_delay_ms),
        ] {
            if !value.is_finite() || value < 0.0 {
                return Err(DescriptorError::InvalidShape {
                    reason: format!(
                        "`components.weapon.{field}` must be a finite value >= 0.0, got {value}"
                    ),
                });
            }
        }
        if !self.spread_vertical_bias.is_finite()
            || !(0.0..=1.0).contains(&self.spread_vertical_bias)
        {
            return Err(DescriptorError::InvalidShape {
                reason: format!(
                    "`components.weapon.spreadVerticalBias` must be a finite value in 0.0..=1.0, got {}",
                    self.spread_vertical_bias
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
        self.primary.validate("components.weapon.primary")?;
        if let Some(secondary) = &self.secondary {
            secondary.validate("components.weapon.secondary")?;
        }
        if self.splash.is_some() && self.resolution != ResolutionMode::Projectile {
            return Err(DescriptorError::InvalidShape {
                reason: "`components.weapon.splash` must be omitted unless `components.weapon.resolution` is `projectile`; hitscan splash is not supported"
                    .to_string(),
            });
        }
        match (self.resolution, self.projectile.as_ref()) {
            (ResolutionMode::Projectile, Some(projectile)) => {
                if self.pellet_count != 1 {
                    return Err(DescriptorError::InvalidShape {
                        reason: format!(
                            "`components.weapon.pelletCount` must be exactly 1 when `components.weapon.resolution` is `projectile`, got {}",
                            self.pellet_count
                        ),
                    });
                }
                validate_projectile_descriptor(projectile)?;
            }
            (ResolutionMode::Projectile, None) => {
                return Err(DescriptorError::InvalidShape {
                    reason: "`components.weapon.projectile` is required when `components.weapon.resolution` is `projectile`".to_string(),
                });
            }
            (ResolutionMode::Hitscan, Some(_)) => {
                return Err(DescriptorError::InvalidShape {
                    reason: "`components.weapon.projectile` must be omitted when `components.weapon.resolution` is `hitscan`".to_string(),
                });
            }
            (ResolutionMode::Hitscan, None) => {}
        }
        if let Some(splash) = self.splash.as_ref() {
            validate_splash_descriptor(splash)?;
        }
        if let Some(credit_source) = self.credit_source.as_deref() {
            validate_credit_source(credit_source)?;
        }
        if let Some(placement) = self.placement.as_ref() {
            placement.validate()?;
        }
        if let Some(muzzle_offset) = self.muzzle_offset {
            for (index, component) in muzzle_offset.into_iter().enumerate() {
                if !component.is_finite() {
                    return Err(DescriptorError::InvalidShape {
                        reason: format!(
                            "`components.weapon.muzzleOffset[{index}]` must be a finite value, got {component}"
                        ),
                    });
                }
            }
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
        if let Some(sounds) = self.sounds.as_ref() {
            for (field, key) in sounds.keys() {
                validate_sound_key(&format!("components.weapon.sounds.{field}"), key)?;
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
        if let Some(WeaponResource::Heat(heat)) = self.resource.as_ref() {
            heat.validate()?;
        }
        if let Some(WeaponResource::Cell(cell)) = self.resource.as_ref() {
            cell.validate()?;
        }
        Ok(self)
    }
}
