//! Validate final shot values before any caller debits a resource.
use super::{ActivationScaleError, ResolvedShotScales};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ShotResourceCost {
    None,
    Ammo(u32),
    Heat(f32),
    Cell(f32),
}
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShotScaleInputs {
    pub damage: f32,
    pub range: f32,
    pub projectile_speed: Option<f32>,
    pub projectile_radius: Option<f32>,
    /// Sprite body size or model uniform scale; presentation-only.
    pub projectile_size: Option<f32>,
    pub knockback_speed: Option<f32>,
    pub splash_knockback_speed: Option<f32>,
    pub resource_cost: ShotResourceCost,
}
/// Copy-only resolved values; damage applies to direct OR splash, once.
pub type ScaledShotValues = ShotScaleInputs;

impl ResolvedShotScales {
    pub fn apply(self, base: ShotScaleInputs) -> Result<ScaledShotValues, ActivationScaleError> {
        let product = |value: f32, scale: f32, field: &'static str, zero_allowed: bool| {
            let result = value * scale;
            if result.is_finite()
                && if zero_allowed {
                    result >= 0.0
                } else {
                    result > 0.0
                }
            {
                Ok(result)
            } else {
                Err(ActivationScaleError { field })
            }
        };
        let optional = |value: Option<f32>, scale, field, zero| {
            value
                .map(|value| product(value, scale, field, zero))
                .transpose()
        };
        let resource_cost = match base.resource_cost {
            ShotResourceCost::None => ShotResourceCost::None,
            ShotResourceCost::Ammo(value) => {
                let rounded = (f64::from(value) * f64::from(self.resource_cost)).ceil();
                if !rounded.is_finite() || rounded < 1.0 || rounded > f64::from(u32::MAX) {
                    return Err(ActivationScaleError {
                        field: "resourceCost",
                    });
                }
                ShotResourceCost::Ammo(rounded as u32)
            }
            ShotResourceCost::Heat(value) => {
                ShotResourceCost::Heat(product(value, self.resource_cost, "resourceCost", false)?)
            }
            ShotResourceCost::Cell(value) => {
                ShotResourceCost::Cell(product(value, self.resource_cost, "resourceCost", false)?)
            }
        };
        Ok(ScaledShotValues {
            damage: product(base.damage, self.damage, "damage", true)?,
            range: product(base.range, self.range, "range", false)?,
            projectile_speed: optional(
                base.projectile_speed,
                self.projectile_speed,
                "projectileSpeed",
                false,
            )?,
            projectile_radius: optional(
                base.projectile_radius,
                self.projectile_radius,
                "projectileRadius",
                true,
            )?,
            projectile_size: optional(
                base.projectile_size,
                self.projectile_size,
                "projectileSize",
                false,
            )?,
            knockback_speed: optional(
                base.knockback_speed,
                self.knockback_speed,
                "knockbackSpeed",
                true,
            )?,
            splash_knockback_speed: optional(
                base.splash_knockback_speed,
                self.knockback_speed,
                "knockbackSpeed",
                true,
            )?,
            resource_cost,
        })
    }
}
