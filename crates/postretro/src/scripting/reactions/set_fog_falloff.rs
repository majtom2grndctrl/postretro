// `setFogFalloff` reaction primitive: set the radial-falloff exponent on
// every fog volume matching the reaction's tag. Invalid args return early
// after one warn; the write loop is skipped entirely.
// See: context/lib/scripting.md

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
/// - `falloff` not in `(0.0, +∞)` (or non-finite) → one `log::warn!`, return
///   early; no targets are visited.
/// - Missing component → `log::warn!`, skip.
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

    // Clamping to 0 or ε would silently alter the fog curve in the shader, so
    // there is no safe fallback value — reject before touching any target.
    // Early return here means a tag-typo (targets matching no fog entities)
    // won't emit missing-component warns when falloff is also invalid. That's
    // an accepted tradeoff: one invalid-arg warn is enough signal. The test
    // `invalid_falloff_does_not_suppress_missing_component_warn` covered the
    // old per-target behavior and was intentionally removed when this path
    // changed to a single early return.
    if !args.falloff.is_finite() || args.falloff <= 0.0 {
        log::warn!(
            "[Scripting] setFogFalloff: falloff {} is non-positive or non-finite; \
             skipping write for all targets",
            args.falloff
        );
        return Ok(());
    }

    for &id in targets {
        let mut next = match registry.get_component::<FogVolumeComponent>(id) {
            Ok(c) => c.clone(),
            Err(_) => {
                log::warn!(
                    "[Scripting] setFogFalloff: entity {id} has no FogVolumeComponent; skipping"
                );
                continue;
            }
        };
        next.falloff = args.falloff;
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
