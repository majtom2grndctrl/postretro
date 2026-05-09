// `setFogAnimation` reaction primitive: install or clear a per-frame density
// animation on every target entity.
// See: context/lib/scripting.md

use serde::{Deserialize, Serialize};

use crate::scripting::components::fog_volume::FogAnimation;
use crate::scripting::registry::{EntityId, EntityRegistry, FogVolumeComponent};

use super::ReactionError;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct SetFogAnimationArgs(pub(crate) Option<FogAnimation>);

/// Apply `args.0` to every target's `FogVolumeComponent.animation`.
///
/// Per-target behavior:
/// - Missing component → `log::warn!`, skip (tag matched a non-fog entity —
///   most likely a tag typo).
/// - Empty target set → no-op, debug log.
/// - `None` args → clear `animation` on each target.
/// - `Some(anim)` args → validate, then install. A failed validation skips the
///   install (pre-existing `animation` is untouched).
pub(crate) fn dispatch(
    registry: &mut EntityRegistry,
    targets: &[EntityId],
    args: &SetFogAnimationArgs,
) -> Result<(), ReactionError> {
    if targets.is_empty() {
        log::debug!("[Scripting] setFogAnimation: empty target set, no-op");
        return Ok(());
    }

    let install: Option<FogAnimation> = match &args.0 {
        None => None,
        Some(anim) => match validate(anim) {
            Some(v) => Some(v),
            None => return Ok(()),
        },
    };

    for &id in targets {
        let mut next = match registry.get_component::<FogVolumeComponent>(id) {
            Ok(c) => c.clone(),
            Err(_) => {
                log::warn!(
                    "[Scripting] setFogAnimation: entity {id} has no FogVolumeComponent; skipping"
                );
                continue;
            }
        };
        next.animation = install.clone();
        if let Err(e) = registry.set_component(id, next) {
            log::warn!("[Scripting] setFogAnimation: failed to write component on {id}: {e:?}");
        }
    }

    Ok(())
}

/// Validate `anim` and return a normalized clone, or `None` to reject the
/// payload (caller skips install).
fn validate(anim: &FogAnimation) -> Option<FogAnimation> {
    if !anim.period_ms.is_finite() || anim.period_ms <= 0.0 {
        log::warn!(
            "[Scripting] setFogAnimation: periodMs must be > 0 and finite (got {}); skipping install",
            anim.period_ms
        );
        return None;
    }

    let mut next = anim.clone();

    match next.saturation.as_mut() {
        Some(curve) if curve.is_empty() => {
            log::warn!(
                "[Scripting] setFogAnimation: saturation curve is empty (use null to omit); skipping install"
            );
            return None;
        }
        Some(curve) => {
            let mut clamped_any = false;
            for sample in curve.iter_mut() {
                if !sample.is_finite() || *sample < 0.0 {
                    if !clamped_any {
                        log::warn!(
                            "[Scripting] setFogAnimation: saturation sample {sample} is negative or non-finite; clamping to 0.0"
                        );
                        clamped_any = true;
                    }
                    *sample = 0.0;
                }
            }
        }
        None => {}
    }

    match next.density.as_mut() {
        Some(curve) if curve.is_empty() => {
            log::warn!(
                "[Scripting] setFogAnimation: density curve is empty (use null to omit); skipping install"
            );
            return None;
        }
        Some(curve) => {
            let mut clamped_any = false;
            for sample in curve.iter_mut() {
                if !sample.is_finite() || *sample < 0.0 {
                    if !clamped_any {
                        log::warn!(
                            "[Scripting] setFogAnimation: density sample {sample} is negative or non-finite; clamping to 0.0"
                        );
                        clamped_any = true;
                    }
                    *sample = 0.0;
                }
            }
        }
        None => {}
    }

    next.phase = match next.phase {
        Some(p) if p.is_finite() => Some(p.rem_euclid(1.0)),
        Some(p) => {
            log::warn!("[Scripting] setFogAnimation: phase {p} is non-finite; treating as null");
            None
        }
        None => None,
    };

    if next.play_count == Some(0) {
        log::warn!(
            "[Scripting] setFogAnimation: playCount of 0 has no defensible meaning under the CPU evaluator; coercing to 1 (one-shot)"
        );
        next.play_count = Some(1);
    }

    if next.density.is_none() && next.saturation.is_none() && next.play_count.is_some() {
        log::warn!(
            "[Scripting] setFogAnimation: playCount is set but neither density nor saturation curve is provided; the animation would never settle. Skipping install"
        );
        return None;
    }

    Some(next)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scripting::registry::Transform;

    fn sample_fog() -> FogVolumeComponent {
        FogVolumeComponent {
            density: 0.5,
            scatter: 0.6,
            edge_softness: 0.25,
            falloff: 2.0,
            tint: [1.0, 1.0, 1.0],
            saturation: 1.0,
            animation: None,
        }
    }

    fn spawn_fog(reg: &mut EntityRegistry) -> EntityId {
        let id = reg.spawn(Transform::default());
        reg.set_component(id, sample_fog()).unwrap();
        id
    }

    fn valid_anim() -> FogAnimation {
        FogAnimation {
            period_ms: 800.0,
            phase: None,
            play_count: None,
            density: Some(vec![0.1, 0.5, 1.0]),
            saturation: None,
        }
    }

    #[test]
    fn installs_animation_on_target() {
        let mut reg = EntityRegistry::new();
        let id = spawn_fog(&mut reg);
        dispatch(&mut reg, &[id], &SetFogAnimationArgs(Some(valid_anim()))).unwrap();
        let stored = reg
            .get_component::<FogVolumeComponent>(id)
            .unwrap()
            .animation
            .clone()
            .expect("animation must be installed");
        assert_eq!(stored.period_ms, 800.0);
        assert_eq!(
            stored.density.as_deref(),
            Some([0.1_f32, 0.5, 1.0].as_ref())
        );
    }

    #[test]
    fn null_args_clears_animation() {
        let mut reg = EntityRegistry::new();
        let id = spawn_fog(&mut reg);
        dispatch(&mut reg, &[id], &SetFogAnimationArgs(Some(valid_anim()))).unwrap();
        assert!(
            reg.get_component::<FogVolumeComponent>(id)
                .unwrap()
                .animation
                .is_some()
        );
        dispatch(&mut reg, &[id], &SetFogAnimationArgs(None)).unwrap();
        assert!(
            reg.get_component::<FogVolumeComponent>(id)
                .unwrap()
                .animation
                .is_none()
        );
    }

    #[test]
    fn rejects_zero_period_ms() {
        let mut reg = EntityRegistry::new();
        let id = spawn_fog(&mut reg);
        let prior = FogAnimation {
            period_ms: 200.0,
            phase: None,
            play_count: None,
            density: Some(vec![0.0, 1.0]),
            saturation: None,
        };
        reg.set_component(
            id,
            FogVolumeComponent {
                animation: Some(prior.clone()),
                ..sample_fog()
            },
        )
        .unwrap();

        let bad = FogAnimation {
            period_ms: 0.0,
            ..valid_anim()
        };
        let captured = crate::scripting::reactions::log_capture::capture(|| {
            dispatch(&mut reg, &[id], &SetFogAnimationArgs(Some(bad))).unwrap();
        });

        assert_eq!(
            reg.get_component::<FogVolumeComponent>(id)
                .unwrap()
                .animation,
            Some(prior),
            "pre-existing animation must be unchanged"
        );
        assert!(
            captured
                .iter()
                .any(|(lvl, msg)| *lvl == log::Level::Warn && msg.contains("periodMs must be > 0")),
            "expected a warn-level log about periodMs, got: {captured:?}"
        );
    }

    #[test]
    fn rejects_empty_density_curve() {
        let mut reg = EntityRegistry::new();
        let id = spawn_fog(&mut reg);
        let prior = valid_anim();
        reg.set_component(
            id,
            FogVolumeComponent {
                animation: Some(prior.clone()),
                ..sample_fog()
            },
        )
        .unwrap();

        let bad = FogAnimation {
            density: Some(vec![]),
            ..valid_anim()
        };
        let captured = crate::scripting::reactions::log_capture::capture(|| {
            dispatch(&mut reg, &[id], &SetFogAnimationArgs(Some(bad))).unwrap();
        });

        assert_eq!(
            reg.get_component::<FogVolumeComponent>(id)
                .unwrap()
                .animation,
            Some(prior)
        );
        assert!(
            captured.iter().any(
                |(lvl, msg)| *lvl == log::Level::Warn && msg.contains("density curve is empty")
            ),
            "expected a warn-level log about empty curve, got: {captured:?}"
        );
    }

    #[test]
    fn clamps_negative_density_sample() {
        let mut reg = EntityRegistry::new();
        let id = spawn_fog(&mut reg);
        let anim = FogAnimation {
            density: Some(vec![0.5, -2.0, 0.75]),
            ..valid_anim()
        };
        let captured = crate::scripting::reactions::log_capture::capture(|| {
            dispatch(&mut reg, &[id], &SetFogAnimationArgs(Some(anim))).unwrap();
        });

        let stored = reg
            .get_component::<FogVolumeComponent>(id)
            .unwrap()
            .animation
            .clone()
            .unwrap();
        assert_eq!(
            stored.density.as_deref(),
            Some([0.5_f32, 0.0, 0.75].as_ref())
        );
        assert!(
            captured
                .iter()
                .any(|(lvl, msg)| *lvl == log::Level::Warn && msg.contains("clamping to 0.0")),
            "expected a warn-level log about clamping, got: {captured:?}"
        );
    }

    #[test]
    fn bumps_play_count_zero_to_one() {
        let mut reg = EntityRegistry::new();
        let id = spawn_fog(&mut reg);
        let anim = FogAnimation {
            play_count: Some(0),
            ..valid_anim()
        };
        let captured = crate::scripting::reactions::log_capture::capture(|| {
            dispatch(&mut reg, &[id], &SetFogAnimationArgs(Some(anim))).unwrap();
        });
        let stored = reg
            .get_component::<FogVolumeComponent>(id)
            .unwrap()
            .animation
            .clone()
            .unwrap();
        assert_eq!(stored.play_count, Some(1));
        assert!(
            captured
                .iter()
                .any(|(lvl, msg)| *lvl == log::Level::Warn && msg.contains("playCount of 0")),
            "expected a warn-level log about playCount, got: {captured:?}"
        );
    }

    #[test]
    fn rejects_play_count_without_any_curve() {
        let mut reg = EntityRegistry::new();
        let id = spawn_fog(&mut reg);
        let prior = valid_anim();
        reg.set_component(
            id,
            FogVolumeComponent {
                animation: Some(prior.clone()),
                ..sample_fog()
            },
        )
        .unwrap();

        let bad = FogAnimation {
            period_ms: 500.0,
            phase: None,
            play_count: Some(1),
            density: None,
            saturation: None,
        };
        let captured = crate::scripting::reactions::log_capture::capture(|| {
            dispatch(&mut reg, &[id], &SetFogAnimationArgs(Some(bad))).unwrap();
        });

        assert_eq!(
            reg.get_component::<FogVolumeComponent>(id)
                .unwrap()
                .animation,
            Some(prior),
            "pre-existing animation must be unchanged when install is rejected"
        );
        assert!(
            captured.iter().any(|(lvl, msg)| *lvl == log::Level::Warn
                && msg.contains("playCount is set but neither density nor saturation")),
            "expected a warn-level log about missing curves, got: {captured:?}"
        );
    }

    #[test]
    fn clamps_non_finite_saturation_sample() {
        // Parity with the density curve's `!is_finite()` check: a +inf
        // sample must be clamped to 0.0 rather than slipping into the GPU
        // buffer where `mix(luma, scatter, vs_saturation)` would NaN out
        // pixels.
        let mut reg = EntityRegistry::new();
        let id = spawn_fog(&mut reg);
        let anim = FogAnimation {
            period_ms: 500.0,
            phase: None,
            play_count: None,
            density: None,
            saturation: Some(vec![0.5, f32::INFINITY, 0.75]),
        };
        let captured = crate::scripting::reactions::log_capture::capture(|| {
            dispatch(&mut reg, &[id], &SetFogAnimationArgs(Some(anim))).unwrap();
        });
        let stored = reg
            .get_component::<FogVolumeComponent>(id)
            .unwrap()
            .animation
            .clone()
            .unwrap();
        assert_eq!(
            stored.saturation.as_deref(),
            Some([0.5_f32, 0.0, 0.75].as_ref())
        );
        assert!(
            captured.iter().any(|(lvl, msg)| *lvl == log::Level::Warn
                && msg.contains("saturation sample")
                && msg.contains("non-finite")),
            "expected a warn-level log about non-finite saturation sample, got: {captured:?}"
        );
    }

    #[test]
    fn play_count_with_saturation_only_curve_is_valid() {
        // The dual-channel validation rejects only when *both* curves are
        // None — a saturation-only animation with playCount must install
        // and settle on completion to its final saturation keyframe.
        let mut reg = EntityRegistry::new();
        let id = spawn_fog(&mut reg);
        let anim = FogAnimation {
            period_ms: 500.0,
            phase: None,
            play_count: Some(2),
            density: None,
            saturation: Some(vec![0.5, 1.0, 1.5]),
        };
        dispatch(&mut reg, &[id], &SetFogAnimationArgs(Some(anim.clone()))).unwrap();
        let stored = reg
            .get_component::<FogVolumeComponent>(id)
            .unwrap()
            .animation
            .clone()
            .expect("animation must be installed");
        assert_eq!(stored.density, None);
        assert_eq!(
            stored.saturation.as_deref(),
            Some([0.5_f32, 1.0, 1.5].as_ref())
        );
        assert_eq!(stored.play_count, Some(2));
    }

    #[test]
    fn empty_target_set_is_noop() {
        let mut reg = EntityRegistry::new();
        let id = spawn_fog(&mut reg);
        dispatch(&mut reg, &[], &SetFogAnimationArgs(Some(valid_anim()))).unwrap();
        assert_eq!(
            reg.get_component::<FogVolumeComponent>(id)
                .unwrap()
                .animation,
            None
        );
    }

    #[test]
    fn set_fog_animation_args_null_deserializes_to_none() {
        let v = serde_json::json!(null);
        let parsed: SetFogAnimationArgs = serde_json::from_value(v).unwrap();
        assert_eq!(parsed, SetFogAnimationArgs(None));
    }

    #[test]
    fn non_finite_phase_coerces_to_none_with_warn() {
        let mut reg = EntityRegistry::new();
        let id = spawn_fog(&mut reg);
        let anim = FogAnimation {
            phase: Some(f32::INFINITY),
            density: Some(vec![0.5, 1.0]),
            period_ms: 500.0,
            play_count: None,
            saturation: None,
        };
        let captured = crate::scripting::reactions::log_capture::capture(|| {
            dispatch(&mut reg, &[id], &SetFogAnimationArgs(Some(anim))).unwrap();
        });

        let stored = reg
            .get_component::<FogVolumeComponent>(id)
            .unwrap()
            .animation
            .clone()
            .expect("animation must be installed");
        assert!(
            stored.phase.is_none(),
            "non-finite phase must be coerced to None, got: {:?}",
            stored.phase
        );
        assert!(
            captured.iter().any(|(lvl, msg)| *lvl == log::Level::Warn
                && (msg.contains("non-finite") || msg.contains("treating as null"))),
            "expected a warn-level log about non-finite phase, got: {captured:?}"
        );
    }

    #[test]
    fn finite_phase_outside_unit_range_is_normalized_via_rem_euclid() {
        let mut reg = EntityRegistry::new();
        let id = spawn_fog(&mut reg);
        let anim = FogAnimation {
            phase: Some(1.5),
            density: Some(vec![0.5, 1.0]),
            period_ms: 500.0,
            play_count: None,
            saturation: None,
        };
        let captured = crate::scripting::reactions::log_capture::capture(|| {
            dispatch(&mut reg, &[id], &SetFogAnimationArgs(Some(anim))).unwrap();
        });

        let stored = reg
            .get_component::<FogVolumeComponent>(id)
            .unwrap()
            .animation
            .clone()
            .expect("animation must be installed");
        let phase = stored
            .phase
            .expect("phase must be Some after normalization");
        assert!(
            (phase - 0.5_f32).abs() < 1e-6,
            "phase 1.5 must normalize to 0.5 via rem_euclid(1.0), got: {phase}"
        );
        assert!(
            !captured.iter().any(|(lvl, _)| *lvl == log::Level::Warn),
            "no warn should be emitted for a finite phase, got: {captured:?}"
        );
    }

    #[test]
    fn non_fog_target_is_skipped_with_warn() {
        let mut reg = EntityRegistry::new();
        let bare = reg.spawn(Transform::default());
        let fog = spawn_fog(&mut reg);

        let captured = crate::scripting::reactions::log_capture::capture(|| {
            dispatch(
                &mut reg,
                &[bare, fog],
                &SetFogAnimationArgs(Some(valid_anim())),
            )
            .unwrap();
        });

        assert!(
            reg.get_component::<FogVolumeComponent>(fog)
                .unwrap()
                .animation
                .is_some()
        );
        assert!(
            captured.iter().any(|(lvl, msg)| *lvl == log::Level::Warn
                && msg.contains("no FogVolumeComponent")),
            "expected a warn-level log naming the missing component, got: {captured:?}"
        );
    }
}
