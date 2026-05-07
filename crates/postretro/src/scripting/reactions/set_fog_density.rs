// `setFogDensity` reaction primitive: set the density on every fog volume
// matching the reaction's tag.
// See: context/lib/scripting.md §11 (Reaction primitives) and
// `context/plans/in-progress/fog-volume-reactions/index.md`.

use serde::{Deserialize, Serialize};

use crate::scripting::registry::{EntityId, EntityRegistry, FogVolumeComponent};

use super::ReactionError;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SetFogDensityArgs {
    pub(crate) density: f32,
}

/// Apply `args.density` to every target's `FogVolumeComponent.density`.
///
/// Per-target behavior:
/// - Missing component → `log::warn!`, skip (tag matched a non-fog entity —
///   most likely a tag typo).
/// - Out-of-range / non-finite density → `log::warn!` once and clamp to `0.0`.
/// - Empty target set → no-op, debug log.
pub(crate) fn dispatch(
    registry: &mut EntityRegistry,
    targets: &[EntityId],
    args: &SetFogDensityArgs,
) -> Result<(), ReactionError> {
    if targets.is_empty() {
        log::debug!("[Scripting] setFogDensity: empty target set, no-op");
        return Ok(());
    }

    let density = if args.density.is_finite() && args.density >= 0.0 {
        args.density
    } else {
        log::warn!(
            "[Scripting] setFogDensity: density {} is negative or non-finite; clamping to 0.0",
            args.density
        );
        0.0
    };

    for &id in targets {
        let current = match registry.get_component::<FogVolumeComponent>(id) {
            Ok(c) => *c,
            Err(_) => {
                log::warn!(
                    "[Scripting] setFogDensity: entity {id} has no FogVolumeComponent; skipping"
                );
                continue;
            }
        };
        let mut next = current;
        next.density = density;
        if let Err(e) = registry.set_component(id, next) {
            log::warn!("[Scripting] setFogDensity: failed to write component on {id}: {e:?}");
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
            scatter: 0.6,
            edge_softness: 0.25,
            falloff: 2.0,
        }
    }

    fn spawn_fog(reg: &mut EntityRegistry) -> EntityId {
        let id = reg.spawn(Transform::default());
        reg.set_component(id, sample_fog()).unwrap();
        id
    }

    #[test]
    fn writes_density_on_each_target() {
        let mut reg = EntityRegistry::new();
        let a = spawn_fog(&mut reg);
        let b = spawn_fog(&mut reg);
        dispatch(&mut reg, &[a, b], &SetFogDensityArgs { density: 1.5 }).unwrap();
        assert_eq!(
            reg.get_component::<FogVolumeComponent>(a).unwrap().density,
            1.5
        );
        assert_eq!(
            reg.get_component::<FogVolumeComponent>(b).unwrap().density,
            1.5
        );
    }

    #[test]
    fn negative_density_clamps_to_zero() {
        let mut reg = EntityRegistry::new();
        let id = spawn_fog(&mut reg);
        dispatch(&mut reg, &[id], &SetFogDensityArgs { density: -3.0 }).unwrap();
        assert_eq!(
            reg.get_component::<FogVolumeComponent>(id).unwrap().density,
            0.0
        );
    }

    #[test]
    fn non_finite_density_clamps_to_zero() {
        let mut reg = EntityRegistry::new();
        let id = spawn_fog(&mut reg);
        dispatch(&mut reg, &[id], &SetFogDensityArgs { density: f32::NAN }).unwrap();
        assert_eq!(
            reg.get_component::<FogVolumeComponent>(id).unwrap().density,
            0.0
        );
    }

    #[test]
    fn empty_target_set_is_a_noop() {
        let mut reg = EntityRegistry::new();
        let id = spawn_fog(&mut reg);
        dispatch(&mut reg, &[], &SetFogDensityArgs { density: 99.0 }).unwrap();
        assert_eq!(
            reg.get_component::<FogVolumeComponent>(id).unwrap().density,
            0.5
        );
    }

    #[test]
    fn non_fog_target_is_skipped_with_warn() {
        let mut reg = EntityRegistry::new();
        let bare = reg.spawn(Transform::default());
        let fog = spawn_fog(&mut reg);

        let captured = crate::scripting::reactions::log_capture::capture(|| {
            dispatch(&mut reg, &[bare, fog], &SetFogDensityArgs { density: 2.0 }).unwrap();
        });

        assert_eq!(
            reg.get_component::<FogVolumeComponent>(fog)
                .unwrap()
                .density,
            2.0
        );
        assert!(
            captured.iter().any(|(lvl, msg)| *lvl == log::Level::Warn
                && msg.contains("no FogVolumeComponent")),
            "expected a warn-level log naming the missing component, got: {captured:?}"
        );
    }

    #[test]
    fn negative_density_emits_warn() {
        let mut reg = EntityRegistry::new();
        let id = spawn_fog(&mut reg);
        let captured = crate::scripting::reactions::log_capture::capture(|| {
            dispatch(&mut reg, &[id], &SetFogDensityArgs { density: -1.0 }).unwrap();
        });
        assert!(
            captured
                .iter()
                .any(|(lvl, msg)| *lvl == log::Level::Warn && msg.contains("clamping to 0.0")),
            "expected a warn-level log about clamping, got: {captured:?}"
        );
    }

    #[test]
    fn args_deserialize_camelcase() {
        let v = serde_json::json!({ "density": 0.75 });
        let parsed: SetFogDensityArgs = serde_json::from_value(v).unwrap();
        assert_eq!(parsed.density, 0.75);
    }
}
