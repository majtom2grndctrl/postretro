// Script-facing billboard emitter component plus FFI-adapter shape.
// See: context/plans/in-progress/scripting-foundation/plan-3-emitter-entity.md §Sub-plan 1

use serde::{Deserialize, Serialize};

use crate::scripting::conv::Vec3Lit;
use crate::scripting::error::ScriptError;

/// Per-emitter spin tween. When attached, the emitter bridge advances elapsed
/// time and writes interpolated samples from `rate_curve` into
/// [`BillboardEmitterComponent::spin_rate`] over `duration` seconds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct SpinAnimation {
    pub(crate) duration: f32,
    pub(crate) rate_curve: Vec<f32>,
}

/// Emitter configuration carried by the parent ECS entity. The bridge reads
/// this each tick to spawn particle entities; reactions mutate fields on it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) struct BillboardEmitterComponent {
    pub(crate) rate: f32,
    pub(crate) burst: Option<u32>,
    pub(crate) spread: f32,
    pub(crate) lifetime: f32,
    pub(crate) initial_velocity: [f32; 3],
    pub(crate) buoyancy: f32,
    pub(crate) drag: f32,
    pub(crate) size_over_lifetime: Vec<f32>,
    pub(crate) opacity_over_lifetime: Vec<f32>,
    pub(crate) color: [f32; 3],
    pub(crate) sprite: String,
    pub(crate) spin_rate: f32,
    pub(crate) spin_animation: Option<SpinAnimation>,
}

/// Script-side wire shape for [`SpinAnimation`]. `rate_curve` is a plain
/// `Vec<f32>`; only `duration` needs validation distinct from the storage
/// struct.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct SpinAnimationLit {
    pub(crate) duration: f32,
    pub(crate) rate_curve: Vec<f32>,
}

/// Script-side wire shape: `initial_velocity` and `color` cross as `Vec3Lit`
/// (accepting both `[x, y, z]` arrays and `{ x, y, z }` objects), then convert
/// into `[f32; 3]` for storage.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) struct BillboardEmitterComponentLit {
    pub(crate) rate: f32,
    #[serde(default)]
    pub(crate) burst: Option<u32>,
    pub(crate) spread: f32,
    pub(crate) lifetime: f32,
    pub(crate) initial_velocity: Vec3Lit,
    pub(crate) buoyancy: f32,
    pub(crate) drag: f32,
    pub(crate) size_over_lifetime: Vec<f32>,
    pub(crate) opacity_over_lifetime: Vec<f32>,
    pub(crate) color: Vec3Lit,
    pub(crate) sprite: String,
    pub(crate) spin_rate: f32,
    #[serde(default)]
    pub(crate) spin_animation: Option<SpinAnimationLit>,
}

impl SpinAnimationLit {
    fn validate_into(self) -> Result<SpinAnimation, ScriptError> {
        if !self.duration.is_finite() || self.duration <= 0.0 {
            return Err(ScriptError::InvalidArgument {
                reason: format!("spin_animation.duration must be > 0 (got {})", self.duration),
            });
        }
        if self.rate_curve.is_empty() {
            return Err(ScriptError::InvalidArgument {
                reason: "spin_animation.rate_curve must be nonempty".into(),
            });
        }
        Ok(SpinAnimation {
            duration: self.duration,
            rate_curve: self.rate_curve,
        })
    }
}

impl BillboardEmitterComponentLit {
    /// Validate per the FFI contract and convert into the storage component.
    /// Validation rules (each emits `ScriptError::InvalidArgument` naming the
    /// offending field):
    ///   - `lifetime > 0`
    ///   - `rate >= 0`
    ///   - `spread >= 0`
    ///   - `drag >= 0`
    ///   - `size_over_lifetime` / `opacity_over_lifetime` nonempty
    ///   - `sprite` nonempty
    ///   - `spin_animation` (when `Some`): `duration > 0`, `rate_curve` nonempty
    pub(crate) fn validate_into(self) -> Result<BillboardEmitterComponent, ScriptError> {
        if !self.lifetime.is_finite() || self.lifetime <= 0.0 {
            return Err(ScriptError::InvalidArgument {
                reason: format!("lifetime must be > 0 (got {})", self.lifetime),
            });
        }
        if !self.rate.is_finite() || self.rate < 0.0 {
            return Err(ScriptError::InvalidArgument {
                reason: format!("rate must be >= 0 (got {})", self.rate),
            });
        }
        if !self.spread.is_finite() || self.spread < 0.0 {
            return Err(ScriptError::InvalidArgument {
                reason: format!("spread must be >= 0 (got {})", self.spread),
            });
        }
        if !self.drag.is_finite() || self.drag < 0.0 {
            return Err(ScriptError::InvalidArgument {
                reason: format!("drag must be >= 0 (got {})", self.drag),
            });
        }
        if self.size_over_lifetime.is_empty() {
            return Err(ScriptError::InvalidArgument {
                reason: "size_over_lifetime must be nonempty".into(),
            });
        }
        if self.opacity_over_lifetime.is_empty() {
            return Err(ScriptError::InvalidArgument {
                reason: "opacity_over_lifetime must be nonempty".into(),
            });
        }
        if self.sprite.is_empty() {
            return Err(ScriptError::InvalidArgument {
                reason: "sprite must be a nonempty string".into(),
            });
        }
        let spin_animation = match self.spin_animation {
            Some(a) => Some(a.validate_into()?),
            None => None,
        };
        Ok(BillboardEmitterComponent {
            rate: self.rate,
            burst: self.burst,
            spread: self.spread,
            lifetime: self.lifetime,
            initial_velocity: self.initial_velocity.as_f32_3(),
            buoyancy: self.buoyancy,
            drag: self.drag,
            size_over_lifetime: self.size_over_lifetime,
            opacity_over_lifetime: self.opacity_over_lifetime,
            color: self.color.as_f32_3(),
            sprite: self.sprite,
            spin_rate: self.spin_rate,
            spin_animation,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_component() -> BillboardEmitterComponent {
        BillboardEmitterComponent {
            rate: 6.0,
            burst: Some(4),
            spread: 0.4,
            lifetime: 3.0,
            initial_velocity: [0.0, 1.5, 0.0],
            buoyancy: 0.2,
            drag: 0.5,
            size_over_lifetime: vec![0.3, 1.0, 0.5],
            opacity_over_lifetime: vec![0.0, 0.8, 0.0],
            color: [1.0, 0.6, 0.2],
            sprite: "smoke".into(),
            spin_rate: 1.2,
            spin_animation: Some(SpinAnimation {
                duration: 2.0,
                rate_curve: vec![0.0, 3.14, 0.0],
            }),
        }
    }

    fn sample_lit() -> BillboardEmitterComponentLit {
        BillboardEmitterComponentLit {
            rate: 6.0,
            burst: Some(4),
            spread: 0.4,
            lifetime: 3.0,
            initial_velocity: Vec3Lit([0.0, 1.5, 0.0]),
            buoyancy: 0.2,
            drag: 0.5,
            size_over_lifetime: vec![0.3, 1.0, 0.5],
            opacity_over_lifetime: vec![0.0, 0.8, 0.0],
            color: Vec3Lit([1.0, 0.6, 0.2]),
            sprite: "smoke".into(),
            spin_rate: 1.2,
            spin_animation: Some(SpinAnimationLit {
                duration: 2.0,
                rate_curve: vec![0.0, 3.14, 0.0],
            }),
        }
    }

    #[test]
    fn billboard_emitter_component_serde_round_trip_with_curves_and_spin() {
        let value = sample_component();
        let json = serde_json::to_string(&value).unwrap();
        let back: BillboardEmitterComponent = serde_json::from_str(&json).unwrap();
        assert_eq!(value, back);
    }

    #[test]
    fn spin_animation_serde_round_trip() {
        let value = SpinAnimation {
            duration: 1.25,
            rate_curve: vec![0.0, 1.0, 2.0, 1.0, 0.0],
        };
        let json = serde_json::to_string(&value).unwrap();
        let back: SpinAnimation = serde_json::from_str(&json).unwrap();
        assert_eq!(value, back);
    }

    #[test]
    fn lit_validate_into_accepts_valid_input() {
        let lit = sample_lit();
        let stored = lit.validate_into().expect("valid lit should convert");
        assert_eq!(stored, sample_component());
    }

    #[test]
    fn lit_rejects_non_positive_lifetime() {
        let mut lit = sample_lit();
        lit.lifetime = 0.0;
        let err = lit.validate_into().unwrap_err();
        assert!(
            matches!(err, ScriptError::InvalidArgument { ref reason } if reason.contains("lifetime")),
            "got {err:?}"
        );
    }

    #[test]
    fn lit_rejects_negative_rate() {
        let mut lit = sample_lit();
        lit.rate = -0.5;
        let err = lit.validate_into().unwrap_err();
        assert!(
            matches!(err, ScriptError::InvalidArgument { ref reason } if reason.contains("rate")),
            "got {err:?}"
        );
    }

    #[test]
    fn lit_rejects_negative_spread() {
        let mut lit = sample_lit();
        lit.spread = -0.1;
        let err = lit.validate_into().unwrap_err();
        assert!(
            matches!(err, ScriptError::InvalidArgument { ref reason } if reason.contains("spread")),
            "got {err:?}"
        );
    }

    #[test]
    fn lit_rejects_negative_drag() {
        let mut lit = sample_lit();
        lit.drag = -0.2;
        let err = lit.validate_into().unwrap_err();
        assert!(
            matches!(err, ScriptError::InvalidArgument { ref reason } if reason.contains("drag")),
            "got {err:?}"
        );
    }

    #[test]
    fn lit_rejects_empty_size_curve() {
        let mut lit = sample_lit();
        lit.size_over_lifetime.clear();
        let err = lit.validate_into().unwrap_err();
        assert!(
            matches!(err, ScriptError::InvalidArgument { ref reason } if reason.contains("size_over_lifetime")),
            "got {err:?}"
        );
    }

    #[test]
    fn lit_rejects_empty_opacity_curve() {
        let mut lit = sample_lit();
        lit.opacity_over_lifetime.clear();
        let err = lit.validate_into().unwrap_err();
        assert!(
            matches!(err, ScriptError::InvalidArgument { ref reason } if reason.contains("opacity_over_lifetime")),
            "got {err:?}"
        );
    }

    #[test]
    fn lit_rejects_empty_sprite() {
        let mut lit = sample_lit();
        lit.sprite.clear();
        let err = lit.validate_into().unwrap_err();
        assert!(
            matches!(err, ScriptError::InvalidArgument { ref reason } if reason.contains("sprite")),
            "got {err:?}"
        );
    }

    #[test]
    fn lit_rejects_zero_duration_spin_animation() {
        let mut lit = sample_lit();
        lit.spin_animation = Some(SpinAnimationLit {
            duration: 0.0,
            rate_curve: vec![0.0, 1.0],
        });
        let err = lit.validate_into().unwrap_err();
        assert!(
            matches!(err, ScriptError::InvalidArgument { ref reason } if reason.contains("spin_animation.duration")),
            "got {err:?}"
        );
    }

    #[test]
    fn lit_rejects_empty_spin_rate_curve() {
        let mut lit = sample_lit();
        lit.spin_animation = Some(SpinAnimationLit {
            duration: 1.0,
            rate_curve: vec![],
        });
        let err = lit.validate_into().unwrap_err();
        assert!(
            matches!(err, ScriptError::InvalidArgument { ref reason } if reason.contains("spin_animation.rate_curve")),
            "got {err:?}"
        );
    }

    #[test]
    fn lit_rejects_two_element_initial_velocity_array() {
        // Vec3Lit must reject 2- and 4-element arrays at deserialize time.
        // The error surfaces during deserialization, before validate_into runs.
        let json = r#"{
            "rate": 1.0,
            "spread": 0.0,
            "lifetime": 1.0,
            "initial_velocity": [0.0, 1.0],
            "buoyancy": 0.0,
            "drag": 0.0,
            "size_over_lifetime": [1.0],
            "opacity_over_lifetime": [1.0],
            "color": [1.0, 1.0, 1.0],
            "sprite": "smoke",
            "spin_rate": 0.0
        }"#;
        let err = serde_json::from_str::<BillboardEmitterComponentLit>(json).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("initial_velocity") || msg.contains("3 elements"),
            "expected a clear shape/length message, got: {msg}"
        );
    }

    #[test]
    fn lit_rejects_four_element_color_array() {
        let json = r#"{
            "rate": 1.0,
            "spread": 0.0,
            "lifetime": 1.0,
            "initial_velocity": [0.0, 1.0, 0.0],
            "buoyancy": 0.0,
            "drag": 0.0,
            "size_over_lifetime": [1.0],
            "opacity_over_lifetime": [1.0],
            "color": [1.0, 1.0, 1.0, 1.0],
            "sprite": "smoke",
            "spin_rate": 0.0
        }"#;
        let err = serde_json::from_str::<BillboardEmitterComponentLit>(json).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("color") || msg.contains("3 elements"),
            "expected a clear shape/length message, got: {msg}"
        );
    }

    #[test]
    fn lit_rejects_non_number_initial_velocity_element() {
        let json = r#"{
            "rate": 1.0,
            "spread": 0.0,
            "lifetime": 1.0,
            "initial_velocity": [0.0, "oops", 0.0],
            "buoyancy": 0.0,
            "drag": 0.0,
            "size_over_lifetime": [1.0],
            "opacity_over_lifetime": [1.0],
            "color": [1.0, 1.0, 1.0],
            "sprite": "smoke",
            "spin_rate": 0.0
        }"#;
        let err = serde_json::from_str::<BillboardEmitterComponentLit>(json).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("initial_velocity")
                || msg.contains("number")
                || msg.contains("float")
                || msg.contains("f32")
                || msg.contains("string"),
            "expected a number-typed element error, got: {msg}"
        );
    }

    #[test]
    fn lit_rejects_non_array_initial_velocity_shape() {
        let json = r#"{
            "rate": 1.0,
            "spread": 0.0,
            "lifetime": 1.0,
            "initial_velocity": "not a vec",
            "buoyancy": 0.0,
            "drag": 0.0,
            "size_over_lifetime": [1.0],
            "opacity_over_lifetime": [1.0],
            "color": [1.0, 1.0, 1.0],
            "sprite": "smoke",
            "spin_rate": 0.0
        }"#;
        let err = serde_json::from_str::<BillboardEmitterComponentLit>(json).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("initial_velocity") || msg.contains("array") || msg.contains("object"),
            "expected a shape mismatch error, got: {msg}"
        );
    }
}
