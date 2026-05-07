// `setFogFalloff` reaction primitive: set the radial-falloff exponent on
// every fog volume matching the reaction's tag. Out-of-range values skip the
// write entirely (component unchanged for that target).
// See: context/lib/scripting.md §11 (Reaction primitives) and
// `context/plans/in-progress/fog-volume-reactions/index.md`.

use serde::{Deserialize, Serialize};

use crate::scripting::registry::{EntityId, EntityRegistry, FogVolumeComponent};

use super::ReactionError;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SetFogFalloffArgs {
    pub(crate) falloff: f32,
}

/// Apply `args.falloff` to every target's `FogVolumeComponent.falloff`.
///
/// Per-target behavior:
/// - Missing component → `log::warn!`, skip.
/// - `falloff` not in `(0.0, +∞)` (or non-finite) → `log::warn!` and skip the
///   write entirely; component unchanged for this target.
/// - Empty target set → no-op, debug log.
pub(crate) fn dispatch(
    registry: &mut EntityRegistry,
    targets: &[EntityId],
    args: &SetFogFalloffArgs,
) -> Result<(), ReactionError> {
    if targets.is_empty() {
        log::debug!("[Scripting] setFogFalloff: empty target set, no-op");
        return Ok(());
    }

    let valid = args.falloff.is_finite() && args.falloff > 0.0;
    if !valid {
        log::warn!(
            "[Scripting] setFogFalloff: falloff {} is non-positive or non-finite; skipping write for all targets",
            args.falloff
        );
        return Ok(());
    }

    let falloff = args.falloff;
    for &id in targets {
        let current = match registry.get_component::<FogVolumeComponent>(id) {
            Ok(c) => *c,
            Err(_) => {
                log::warn!(
                    "[Scripting] setFogFalloff: entity {id} has no FogVolumeComponent; skipping"
                );
                continue;
            }
        };
        let mut next = current;
        next.falloff = falloff;
        if let Err(e) = registry.set_component(id, next) {
            log::warn!("[Scripting] setFogFalloff: failed to write component on {id}: {e:?}");
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
    fn writes_falloff_on_each_target() {
        let mut reg = EntityRegistry::new();
        let a = spawn_fog(&mut reg);
        let b = spawn_fog(&mut reg);
        dispatch(&mut reg, &[a, b], &SetFogFalloffArgs { falloff: 1.5 }).unwrap();
        assert_eq!(
            reg.get_component::<FogVolumeComponent>(a).unwrap().falloff,
            1.5
        );
        assert_eq!(
            reg.get_component::<FogVolumeComponent>(b).unwrap().falloff,
            1.5
        );
    }

    #[test]
    fn zero_falloff_skips_write_entirely() {
        let mut reg = EntityRegistry::new();
        let id = spawn_fog(&mut reg);
        dispatch(&mut reg, &[id], &SetFogFalloffArgs { falloff: 0.0 }).unwrap();
        assert_eq!(
            reg.get_component::<FogVolumeComponent>(id).unwrap().falloff,
            2.0
        );
    }

    #[test]
    fn negative_falloff_skips_write_entirely() {
        let mut reg = EntityRegistry::new();
        let id = spawn_fog(&mut reg);
        dispatch(&mut reg, &[id], &SetFogFalloffArgs { falloff: -1.0 }).unwrap();
        assert_eq!(
            reg.get_component::<FogVolumeComponent>(id).unwrap().falloff,
            2.0
        );
    }

    #[test]
    fn non_finite_falloff_skips_write_entirely() {
        let mut reg = EntityRegistry::new();
        let id = spawn_fog(&mut reg);
        dispatch(&mut reg, &[id], &SetFogFalloffArgs { falloff: f32::NAN }).unwrap();
        assert_eq!(
            reg.get_component::<FogVolumeComponent>(id).unwrap().falloff,
            2.0
        );
    }

    #[test]
    fn invalid_falloff_emits_warn() {
        let mut reg = EntityRegistry::new();
        let id = spawn_fog(&mut reg);
        let captured = crate::scripting::reactions::log_capture::capture(|| {
            dispatch(&mut reg, &[id], &SetFogFalloffArgs { falloff: 0.0 }).unwrap();
        });
        assert!(
            captured
                .iter()
                .any(|(lvl, msg)| *lvl == log::Level::Warn
                    && msg.contains("non-positive or non-finite")),
            "expected a warn-level log about invalid falloff, got: {captured:?}"
        );
    }

    #[test]
    fn empty_target_set_is_a_noop() {
        let mut reg = EntityRegistry::new();
        let id = spawn_fog(&mut reg);
        dispatch(&mut reg, &[], &SetFogFalloffArgs { falloff: 9.0 }).unwrap();
        assert_eq!(
            reg.get_component::<FogVolumeComponent>(id).unwrap().falloff,
            2.0
        );
    }

    #[test]
    fn non_fog_target_is_skipped_with_warn() {
        let mut reg = EntityRegistry::new();
        let bare = reg.spawn(Transform::default());
        let fog = spawn_fog(&mut reg);

        let captured = crate::scripting::reactions::log_capture::capture(|| {
            dispatch(&mut reg, &[bare, fog], &SetFogFalloffArgs { falloff: 3.0 }).unwrap();
        });

        assert_eq!(
            reg.get_component::<FogVolumeComponent>(fog)
                .unwrap()
                .falloff,
            3.0
        );
        assert!(
            captured.iter().any(|(lvl, msg)| *lvl == log::Level::Warn
                && msg.contains("no FogVolumeComponent")),
            "expected a warn-level log naming the missing component, got: {captured:?}"
        );
    }
}
