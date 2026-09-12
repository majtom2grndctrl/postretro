//! Authored hit impulses and character response, shared by combat and movement.
//! See: context/lib/movement.md §6 · context/lib/entity_model.md §Components (Knockback).

use crate::data_descriptors::DescriptorError;
use serde::{Deserialize, Serialize};

/// Direct-hit velocity change, independent of damage.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct KnockbackDescriptor {
    /// Velocity change in metres per second, in 0..=1000.
    pub speed: f32,
    /// Blend from hit direction toward world up before normalization, in 0..=1.
    #[serde(default)]
    pub upward_bias: f32,
}

/// Radial velocity change using the blast's radius and occlusion.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SplashKnockbackDescriptor {
    pub speed: f32,
    #[serde(default)]
    pub upward_bias: f32,
    /// Fraction of speed at the blast edge, independently of damage falloff.
    #[serde(default)]
    pub min_fraction: f32,
    /// Owner-only impulse multiplier; zero disables self push.
    #[serde(default = "one")]
    pub self_scale: f32,
}

/// Character response to authored impulses. Omission retains these defaults.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct KnockbackResponse {
    pub scale: f32,
    /// Fraction of protected impulse removed per second while grounded.
    pub ground_drag: f32,
    /// Fraction of protected impulse removed per second while airborne.
    pub air_drag: f32,
    /// Fraction of normal steering while knockback remains.
    pub control: f32,
}

const fn one() -> f32 {
    1.0
}

impl Default for KnockbackResponse {
    fn default() -> Self {
        Self {
            scale: 1.0,
            ground_drag: 8.0,
            air_drag: 0.0,
            control: 1.0,
        }
    }
}

impl KnockbackDescriptor {
    pub fn validate(&self, path: &str) -> Result<(), DescriptorError> {
        bounded(path, "speed", self.speed, 1000.0)?;
        bounded(path, "upwardBias", self.upward_bias, 1.0)
    }
}

impl SplashKnockbackDescriptor {
    pub fn validate(&self, path: &str) -> Result<(), DescriptorError> {
        KnockbackDescriptor {
            speed: self.speed,
            upward_bias: self.upward_bias,
        }
        .validate(path)?;
        bounded(path, "minFraction", self.min_fraction, 1.0)?;
        bounded(path, "selfScale", self.self_scale, 10.0)
    }
}

impl KnockbackResponse {
    pub fn validate(&self, path: &str) -> Result<(), DescriptorError> {
        for (field, value, max) in [
            ("scale", self.scale, 10.0),
            ("groundDrag", self.ground_drag, 1000.0),
            ("airDrag", self.air_drag, 1000.0),
            ("control", self.control, 1.0),
        ] {
            bounded(path, field, value, max)?;
        }
        Ok(())
    }
}

fn bounded(path: &str, field: &str, value: f32, max: f32) -> Result<(), DescriptorError> {
    if !value.is_finite() || !(0.0..=max).contains(&value) {
        return Err(DescriptorError::InvalidShape {
            reason: format!("`{path}.{field}` must be finite and in 0..={max}, got {value}"),
        });
    }
    Ok(())
}
