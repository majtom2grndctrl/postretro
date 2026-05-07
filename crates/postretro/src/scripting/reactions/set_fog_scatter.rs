// `setFogScatter` reaction primitive: set the scatter value on every fog
// volume matching the reaction's tag.
// See: context/lib/scripting.md §11 (Reaction primitives) and
// `context/plans/in-progress/fog-volume-reactions/index.md`.

use serde::{Deserialize, Serialize};

use crate::scripting::registry::{EntityId, EntityRegistry, FogVolumeComponent};

use super::ReactionError;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SetFogScatterArgs {
    pub(crate) scatter: f32,
}

/// Apply `args.scatter` to every target's `FogVolumeComponent.scatter`.
///
/// Per-target behavior:
/// - Missing component → `log::warn!`, skip.
/// - Out-of-range / non-finite scatter → `log::warn!` once and clamp into
///   `[0.0, 1.0]`.
/// - Empty target set → no-op, debug log.
pub(crate) fn dispatch(
    registry: &mut EntityRegistry,
    targets: &[EntityId],
    args: &SetFogScatterArgs,
) -> Result<(), ReactionError> {
    if targets.is_empty() {
        log::debug!("[Scripting] setFogScatter: empty target set, no-op");
        return Ok(());
    }

    let scatter = if args.scatter.is_finite() && (0.0..=1.0).contains(&args.scatter) {
        args.scatter
    } else if args.scatter.is_finite() {
        let clamped = args.scatter.clamp(0.0, 1.0);
        log::warn!(
            "[Scripting] setFogScatter: scatter {} is outside [0.0, 1.0]; clamping to {}",
            args.scatter,
            clamped
        );
        clamped
    } else {
        log::warn!(
            "[Scripting] setFogScatter: scatter {} is non-finite; clamping to 0.0",
            args.scatter
        );
        0.0
    };

    for &id in targets {
        let current = match registry.get_component::<FogVolumeComponent>(id) {
            Ok(c) => *c,
            Err(_) => {
                log::warn!(
                    "[Scripting] setFogScatter: entity {id} has no FogVolumeComponent; skipping"
                );
                continue;
            }
        };
        let mut next = current;
        next.scatter = scatter;
        if let Err(e) = registry.set_component(id, next) {
            log::warn!("[Scripting] setFogScatter: failed to write component on {id}: {e:?}");
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
    fn writes_scatter_on_each_target() {
        let mut reg = EntityRegistry::new();
        let a = spawn_fog(&mut reg);
        let b = spawn_fog(&mut reg);
        dispatch(&mut reg, &[a, b], &SetFogScatterArgs { scatter: 0.25 }).unwrap();
        assert_eq!(
            reg.get_component::<FogVolumeComponent>(a).unwrap().scatter,
            0.25
        );
        assert_eq!(
            reg.get_component::<FogVolumeComponent>(b).unwrap().scatter,
            0.25
        );
    }

    #[test]
    fn negative_scatter_clamps_to_zero() {
        let mut reg = EntityRegistry::new();
        let id = spawn_fog(&mut reg);
        dispatch(&mut reg, &[id], &SetFogScatterArgs { scatter: -0.4 }).unwrap();
        assert_eq!(
            reg.get_component::<FogVolumeComponent>(id).unwrap().scatter,
            0.0
        );
    }

    #[test]
    fn over_one_scatter_clamps_to_one() {
        let mut reg = EntityRegistry::new();
        let id = spawn_fog(&mut reg);
        dispatch(&mut reg, &[id], &SetFogScatterArgs { scatter: 3.0 }).unwrap();
        assert_eq!(
            reg.get_component::<FogVolumeComponent>(id).unwrap().scatter,
            1.0
        );
    }

    #[test]
    fn non_finite_scatter_clamps_to_zero() {
        let mut reg = EntityRegistry::new();
        let id = spawn_fog(&mut reg);
        dispatch(
            &mut reg,
            &[id],
            &SetFogScatterArgs {
                scatter: f32::INFINITY,
            },
        )
        .unwrap();
        assert_eq!(
            reg.get_component::<FogVolumeComponent>(id).unwrap().scatter,
            0.0
        );
    }

    #[test]
    fn empty_target_set_is_a_noop() {
        let mut reg = EntityRegistry::new();
        let id = spawn_fog(&mut reg);
        dispatch(&mut reg, &[], &SetFogScatterArgs { scatter: 0.1 }).unwrap();
        assert_eq!(
            reg.get_component::<FogVolumeComponent>(id).unwrap().scatter,
            0.6
        );
    }

    #[test]
    fn non_fog_target_is_skipped_with_warn() {
        let mut reg = EntityRegistry::new();
        let bare = reg.spawn(Transform::default());
        let fog = spawn_fog(&mut reg);

        let captured = crate::scripting::reactions::log_capture::capture(|| {
            dispatch(&mut reg, &[bare, fog], &SetFogScatterArgs { scatter: 0.5 }).unwrap();
        });

        assert_eq!(
            reg.get_component::<FogVolumeComponent>(fog)
                .unwrap()
                .scatter,
            0.5
        );
        assert!(
            captured.iter().any(|(lvl, msg)| *lvl == log::Level::Warn
                && msg.contains("no FogVolumeComponent")),
            "expected a warn-level log naming the missing component, got: {captured:?}"
        );
    }

    #[test]
    fn out_of_range_scatter_emits_warn() {
        let mut reg = EntityRegistry::new();
        let id = spawn_fog(&mut reg);
        let captured = crate::scripting::reactions::log_capture::capture(|| {
            dispatch(&mut reg, &[id], &SetFogScatterArgs { scatter: 2.5 }).unwrap();
        });
        assert!(
            captured
                .iter()
                .any(|(lvl, msg)| *lvl == log::Level::Warn && msg.contains("clamping")),
            "expected a warn-level log about clamping, got: {captured:?}"
        );
    }
}
