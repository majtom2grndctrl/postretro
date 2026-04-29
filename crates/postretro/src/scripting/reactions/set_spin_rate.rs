// `setSpinRate` reaction primitive: set or tween the spin rate on every
// entity matching the reaction's tag.
// See: context/plans/in-progress/scripting-foundation/plan-3-emitter-entity.md §Sub-plan 5

use crate::scripting::components::billboard_emitter::{
    BillboardEmitterComponent, SpinAnimation, SpinAnimationLit,
};
use crate::scripting::registry::{EntityId, EntityRegistry};

use super::ReactionError;

/// Two-shape arg union: either an immediate spin-rate write, or a tween.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum SetSpinRateArgs {
    /// Set `spin_rate` immediately and clear any in-flight tween.
    Rate(f32),
    /// Install a tween. The bridge interpolates `spin_rate` from
    /// `animation.rate_curve` over `animation.duration` seconds.
    Animation(SpinAnimation),
}

impl SetSpinRateArgs {
    /// Parse from the descriptor JSON shape `{ "rate": number }` or
    /// `{ "animation": { duration, rate_curve } }`. Validates the animation
    /// the same way the FFI lit shape does so dispatch-time errors mirror
    /// definition-time errors.
    pub(crate) fn from_json(value: &serde_json::Value) -> Result<Self, ReactionError> {
        let obj = value.as_object().ok_or_else(|| ReactionError::InvalidArgument {
            reason: "setSpinRate args must be an object".into(),
        })?;
        let has_rate = obj.contains_key("rate");
        let has_anim = obj.contains_key("animation");
        match (has_rate, has_anim) {
            (true, false) => {
                let rate = obj["rate"]
                    .as_f64()
                    .ok_or_else(|| ReactionError::InvalidArgument {
                        reason: "setSpinRate.rate must be a number".into(),
                    })? as f32;
                Ok(SetSpinRateArgs::Rate(rate))
            }
            (false, true) => {
                let lit: SpinAnimationLit =
                    serde_json::from_value(obj["animation"].clone()).map_err(|e| {
                        ReactionError::InvalidArgument {
                            reason: format!("setSpinRate.animation: {e}"),
                        }
                    })?;
                let anim = lit
                    .validate_into_public()
                    .map_err(|e| ReactionError::InvalidArgument { reason: e })?;
                Ok(SetSpinRateArgs::Animation(anim))
            }
            (true, true) => Err(ReactionError::InvalidArgument {
                reason: "setSpinRate args must contain exactly one of `rate` or `animation`"
                    .into(),
            }),
            (false, false) => Err(ReactionError::InvalidArgument {
                reason: "setSpinRate args missing `rate` or `animation`".into(),
            }),
        }
    }
}

/// Apply `args` to every target's `BillboardEmitterComponent`.
///
/// Per-target behavior:
/// - Missing component → `log::warn!`, skip.
/// - `Rate(r)`: write `spin_rate = r`, clear `spin_animation` (cancels any
///   in-flight tween). The bridge resets `spin_elapsed = 0.0` next tick when
///   it observes the cleared animation.
/// - `Animation(anim)`: write `spin_animation = Some(anim)`. The bridge
///   detects the new animation and resets `spin_elapsed = 0.0` next tick.
/// - Empty target set → no-op, debug log.
pub(crate) fn dispatch(
    registry: &mut EntityRegistry,
    targets: &[EntityId],
    args: &SetSpinRateArgs,
) -> Result<(), ReactionError> {
    if targets.is_empty() {
        log::debug!("[Scripting] setSpinRate: empty target set, no-op");
        return Ok(());
    }

    for &id in targets {
        let current = match registry.get_component::<BillboardEmitterComponent>(id) {
            Ok(c) => c.clone(),
            Err(_) => {
                log::warn!(
                    "[Scripting] setSpinRate: entity {id} has no BillboardEmitterComponent; skipping"
                );
                continue;
            }
        };
        let mut next = current;
        match args {
            SetSpinRateArgs::Rate(r) => {
                next.spin_rate = *r;
                next.spin_animation = None;
            }
            SetSpinRateArgs::Animation(anim) => {
                next.spin_animation = Some(anim.clone());
            }
        }
        if let Err(e) = registry.set_component(id, next) {
            log::warn!("[Scripting] setSpinRate: failed to write component on {id}: {e:?}");
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scripting::registry::Transform;

    fn sample_emitter() -> BillboardEmitterComponent {
        BillboardEmitterComponent {
            rate: 5.0,
            burst: None,
            spread: 0.1,
            lifetime: 1.0,
            initial_velocity: [0.0, 1.0, 0.0],
            buoyancy: 0.0,
            drag: 0.0,
            size_over_lifetime: vec![1.0],
            opacity_over_lifetime: vec![1.0],
            color: [1.0, 1.0, 1.0],
            sprite: "smoke".into(),
            spin_rate: 0.0,
            spin_animation: None,
        }
    }

    fn spawn_emitter(reg: &mut EntityRegistry) -> EntityId {
        let id = reg.spawn(Transform::default());
        reg.set_component(id, sample_emitter()).unwrap();
        id
    }

    #[test]
    fn rate_variant_sets_spin_rate_and_clears_animation() {
        let mut reg = EntityRegistry::new();
        let id = spawn_emitter(&mut reg);
        // Seed an existing tween to verify cancellation.
        let mut comp = sample_emitter();
        comp.spin_animation = Some(SpinAnimation {
            duration: 1.0,
            rate_curve: vec![0.0, 1.0],
        });
        reg.set_component(id, comp).unwrap();

        dispatch(&mut reg, &[id], &SetSpinRateArgs::Rate(3.5)).unwrap();
        let after = reg.get_component::<BillboardEmitterComponent>(id).unwrap();
        assert_eq!(after.spin_rate, 3.5);
        assert!(after.spin_animation.is_none());
    }

    #[test]
    fn animation_variant_installs_spin_animation() {
        let mut reg = EntityRegistry::new();
        let id = spawn_emitter(&mut reg);
        let anim = SpinAnimation {
            duration: 2.0,
            rate_curve: vec![0.0, std::f32::consts::TAU],
        };
        dispatch(&mut reg, &[id], &SetSpinRateArgs::Animation(anim.clone())).unwrap();
        let after = reg.get_component::<BillboardEmitterComponent>(id).unwrap();
        assert_eq!(after.spin_animation.as_ref(), Some(&anim));
    }

    #[test]
    fn animation_variant_replaces_prior_tween() {
        let mut reg = EntityRegistry::new();
        let id = spawn_emitter(&mut reg);
        let first = SpinAnimation {
            duration: 4.0,
            rate_curve: vec![0.0, 1.0],
        };
        let second = SpinAnimation {
            duration: 1.0,
            rate_curve: vec![5.0, 10.0],
        };
        dispatch(&mut reg, &[id], &SetSpinRateArgs::Animation(first)).unwrap();
        dispatch(&mut reg, &[id], &SetSpinRateArgs::Animation(second.clone())).unwrap();
        let after = reg.get_component::<BillboardEmitterComponent>(id).unwrap();
        assert_eq!(after.spin_animation.as_ref(), Some(&second));
    }

    #[test]
    fn empty_target_set_is_a_noop() {
        let mut reg = EntityRegistry::new();
        let id = spawn_emitter(&mut reg);
        dispatch(&mut reg, &[], &SetSpinRateArgs::Rate(99.0)).unwrap();
        let after = reg.get_component::<BillboardEmitterComponent>(id).unwrap();
        assert_eq!(after.spin_rate, 0.0);
    }

    #[test]
    fn non_emitter_target_is_skipped_with_warn() {
        let mut reg = EntityRegistry::new();
        let bare = reg.spawn(Transform::default());
        let emitter = spawn_emitter(&mut reg);

        let captured = crate::scripting::reactions::log_capture::capture(|| {
            dispatch(&mut reg, &[bare, emitter], &SetSpinRateArgs::Rate(2.0)).unwrap();
        });

        assert_eq!(
            reg.get_component::<BillboardEmitterComponent>(emitter)
                .unwrap()
                .spin_rate,
            2.0
        );
        assert!(
            captured
                .iter()
                .any(|(lvl, msg)| *lvl == log::Level::Warn
                    && msg.contains("no BillboardEmitterComponent")),
            "expected a warn-level log naming the missing component, got: {captured:?}"
        );
    }

    #[test]
    fn from_json_parses_rate_variant() {
        let v = serde_json::json!({ "rate": 1.5 });
        let parsed = SetSpinRateArgs::from_json(&v).unwrap();
        assert_eq!(parsed, SetSpinRateArgs::Rate(1.5));
    }

    #[test]
    fn from_json_parses_animation_variant() {
        let v = serde_json::json!({
            "animation": { "duration": 2.0, "rate_curve": [0.0, 1.0, 2.0] }
        });
        let parsed = SetSpinRateArgs::from_json(&v).unwrap();
        match parsed {
            SetSpinRateArgs::Animation(a) => {
                assert_eq!(a.duration, 2.0);
                assert_eq!(a.rate_curve, vec![0.0, 1.0, 2.0]);
            }
            other => panic!("expected Animation, got {other:?}"),
        }
    }

    #[test]
    fn from_json_rejects_both_keys() {
        let v = serde_json::json!({
            "rate": 1.0,
            "animation": { "duration": 1.0, "rate_curve": [0.0] }
        });
        assert!(SetSpinRateArgs::from_json(&v).is_err());
    }

    #[test]
    fn from_json_rejects_neither_key() {
        let v = serde_json::json!({});
        assert!(SetSpinRateArgs::from_json(&v).is_err());
    }

    #[test]
    fn from_json_rejects_zero_duration_animation() {
        let v = serde_json::json!({
            "animation": { "duration": 0.0, "rate_curve": [0.0, 1.0] }
        });
        assert!(SetSpinRateArgs::from_json(&v).is_err());
    }
}
