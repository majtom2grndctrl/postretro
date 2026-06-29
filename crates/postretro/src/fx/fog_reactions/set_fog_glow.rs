// `setFogGlow` reaction primitive: set the glow value on every fog
// volume matching the reaction's tag.
// See: context/lib/scripting.md

use serde::{Deserialize, Serialize};

use crate::scripting::registry::{EntityId, EntityRegistry, FogVolumeComponent};

use postretro_scripting_core::reaction_registry::ReactionError;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SetFogGlowArgs {
    pub(crate) glow: f32,
}

/// Apply `args.glow` to every target's `FogVolumeComponent.glow`.
///
/// Per-target behavior:
/// - Missing component → `log::warn!`, skip.
/// - NaN glow → `log::warn!` once and use 0.0 (NaN does not clamp
///   predictably).
/// - Out-of-range / infinite glow → `log::warn!` once and clamp into
///   `[0.0, 1.0]` (so `+inf → 1.0`, `-inf → 0.0`).
/// - Empty target set → no-op, debug log.
pub(crate) fn dispatch(
    registry: &mut EntityRegistry,
    targets: &[EntityId],
    args: &SetFogGlowArgs,
) -> Result<(), ReactionError> {
    if targets.is_empty() {
        log::debug!("[Scripting] setFogGlow: empty target set, no-op");
        return Ok(());
    }

    // NaN cannot be clamped to a meaningful value; treat it as 0.0. Infinities
    // are handled naturally by clamp below.
    let glow = if args.glow.is_nan() {
        log::warn!("[Scripting] setFogGlow: glow is NaN; clamping to 0.0");
        0.0
    } else {
        let clamped = args.glow.clamp(0.0, 1.0);
        if !(0.0..=1.0).contains(&args.glow) {
            log::warn!(
                "[Scripting] setFogGlow: glow {} is outside [0.0, 1.0]; clamping to {}",
                args.glow,
                clamped
            );
        }
        clamped
    };

    for &id in targets {
        let mut next = match registry.get_component::<FogVolumeComponent>(id) {
            Ok(c) => c.clone(),
            Err(_) => {
                log::warn!(
                    "[Scripting] setFogGlow: entity {id} has no FogVolumeComponent; skipping"
                );
                continue;
            }
        };
        next.glow = glow;
        if let Err(e) = registry.set_component(id, next) {
            log::warn!("[Scripting] setFogGlow: failed to write component on {id}: {e:?}");
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scripting::registry::Transform;

    fn sample_fog() -> FogVolumeComponent {
        FogVolumeComponent {
            density: 0.5,
            glow: 0.6,
            edge_softness: 0.25,
            falloff: 2.0,
            tint: [1.0, 1.0, 1.0],
            saturation: 1.0,
            min_brightness: 0.0,
            light_range: 1.0,
            animation: None,
        }
    }

    fn spawn_fog(reg: &mut EntityRegistry) -> EntityId {
        let id = reg.spawn(Transform::default());
        reg.set_component(id, sample_fog()).unwrap();
        id
    }

    #[test]
    fn writes_glow_on_each_target() {
        let mut reg = EntityRegistry::new();
        let a = spawn_fog(&mut reg);
        let b = spawn_fog(&mut reg);
        dispatch(&mut reg, &[a, b], &SetFogGlowArgs { glow: 0.25 }).unwrap();
        assert_eq!(
            reg.get_component::<FogVolumeComponent>(a).unwrap().glow,
            0.25
        );
        assert_eq!(
            reg.get_component::<FogVolumeComponent>(b).unwrap().glow,
            0.25
        );
    }

    #[test]
    fn negative_glow_clamps_to_zero() {
        let mut reg = EntityRegistry::new();
        let id = spawn_fog(&mut reg);
        dispatch(&mut reg, &[id], &SetFogGlowArgs { glow: -0.4 }).unwrap();
        assert_eq!(
            reg.get_component::<FogVolumeComponent>(id).unwrap().glow,
            0.0
        );
    }

    #[test]
    fn over_one_glow_clamps_to_one() {
        let mut reg = EntityRegistry::new();
        let id = spawn_fog(&mut reg);
        dispatch(&mut reg, &[id], &SetFogGlowArgs { glow: 3.0 }).unwrap();
        assert_eq!(
            reg.get_component::<FogVolumeComponent>(id).unwrap().glow,
            1.0
        );
    }

    #[test]
    fn pos_infinity_glow_clamps_to_one() {
        let mut reg = EntityRegistry::new();
        let id = spawn_fog(&mut reg);
        dispatch(
            &mut reg,
            &[id],
            &SetFogGlowArgs {
                glow: f32::INFINITY,
            },
        )
        .unwrap();
        assert_eq!(
            reg.get_component::<FogVolumeComponent>(id).unwrap().glow,
            1.0
        );
    }

    #[test]
    fn nan_glow_clamps_to_zero() {
        let mut reg = EntityRegistry::new();
        let id = spawn_fog(&mut reg);
        dispatch(&mut reg, &[id], &SetFogGlowArgs { glow: f32::NAN }).unwrap();
        assert_eq!(
            reg.get_component::<FogVolumeComponent>(id).unwrap().glow,
            0.0
        );
    }

    #[test]
    fn neg_infinity_glow_clamps_to_zero() {
        let mut reg = EntityRegistry::new();
        let id = spawn_fog(&mut reg);
        dispatch(
            &mut reg,
            &[id],
            &SetFogGlowArgs {
                glow: f32::NEG_INFINITY,
            },
        )
        .unwrap();
        assert_eq!(
            reg.get_component::<FogVolumeComponent>(id).unwrap().glow,
            0.0
        );
    }

    #[test]
    fn empty_target_set_is_a_noop() {
        let mut reg = EntityRegistry::new();
        let id = spawn_fog(&mut reg);
        dispatch(&mut reg, &[], &SetFogGlowArgs { glow: 0.1 }).unwrap();
        assert_eq!(
            reg.get_component::<FogVolumeComponent>(id).unwrap().glow,
            0.6
        );
    }

    #[test]
    fn non_fog_target_is_skipped_with_warn() {
        let mut reg = EntityRegistry::new();
        let bare = reg.spawn(Transform::default());
        let fog = spawn_fog(&mut reg);

        let captured = crate::scripting::reactions::log_capture::capture(|| {
            dispatch(&mut reg, &[bare, fog], &SetFogGlowArgs { glow: 0.5 }).unwrap();
        });

        assert_eq!(
            reg.get_component::<FogVolumeComponent>(fog).unwrap().glow,
            0.5
        );
        assert!(
            captured.iter().any(|(lvl, msg)| *lvl == log::Level::Warn
                && msg.contains("no FogVolumeComponent")),
            "expected a warn-level log naming the missing component, got: {captured:?}"
        );
    }

    #[test]
    fn out_of_range_glow_emits_warn() {
        let mut reg = EntityRegistry::new();
        let id = spawn_fog(&mut reg);
        let captured = crate::scripting::reactions::log_capture::capture(|| {
            dispatch(&mut reg, &[id], &SetFogGlowArgs { glow: 2.5 }).unwrap();
        });
        assert!(
            captured
                .iter()
                .any(|(lvl, msg)| *lvl == log::Level::Warn && msg.contains("clamping")),
            "expected a warn-level log about clamping, got: {captured:?}"
        );
    }
}
