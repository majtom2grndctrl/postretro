// `setFogEdgeSoftness` reaction primitive: set the edge-softness value on
// every fog volume matching the reaction's tag. Script-facing field name is
// `edgeSoftness` (camelCase); the Rust-side struct uses `edge_softness`.
// See: context/lib/scripting.md

use serde::{Deserialize, Serialize};

use crate::scripting::registry::{EntityId, EntityRegistry, FogVolumeComponent};

use super::ReactionError;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SetFogEdgeSoftnessArgs {
    pub(crate) edge_softness: f32,
}

/// Apply `args.edge_softness` to every target's
/// `FogVolumeComponent.edge_softness`.
///
/// Per-target behavior:
/// - Missing component → `log::warn!`, skip.
/// - Edge-softness outside `[0, +∞)` or non-finite → `log::warn!` once and
///   clamp to `0.0`.
/// - Empty target set → no-op, debug log.
pub(crate) fn dispatch(
    registry: &mut EntityRegistry,
    targets: &[EntityId],
    args: &SetFogEdgeSoftnessArgs,
) -> Result<(), ReactionError> {
    if targets.is_empty() {
        log::debug!("[Scripting] setFogEdgeSoftness: empty target set, no-op");
        return Ok(());
    }

    let edge_softness = if args.edge_softness.is_finite() && args.edge_softness >= 0.0 {
        args.edge_softness
    } else {
        log::warn!(
            "[Scripting] setFogEdgeSoftness: edgeSoftness {} is outside [0, +\u{221e}) and finite; clamping to 0.0",
            args.edge_softness
        );
        0.0
    };

    for &id in targets {
        let current = match registry.get_component::<FogVolumeComponent>(id) {
            Ok(c) => *c,
            Err(_) => {
                log::warn!(
                    "[Scripting] setFogEdgeSoftness: entity {id} has no FogVolumeComponent; skipping"
                );
                continue;
            }
        };
        let mut next = current;
        next.edge_softness = edge_softness;
        if let Err(e) = registry.set_component(id, next) {
            log::warn!("[Scripting] setFogEdgeSoftness: failed to write component on {id}: {e:?}");
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
    fn writes_edge_softness_on_each_target() {
        let mut reg = EntityRegistry::new();
        let a = spawn_fog(&mut reg);
        let b = spawn_fog(&mut reg);
        dispatch(
            &mut reg,
            &[a, b],
            &SetFogEdgeSoftnessArgs {
                edge_softness: 0.75,
            },
        )
        .unwrap();
        assert_eq!(
            reg.get_component::<FogVolumeComponent>(a)
                .unwrap()
                .edge_softness,
            0.75
        );
        assert_eq!(
            reg.get_component::<FogVolumeComponent>(b)
                .unwrap()
                .edge_softness,
            0.75
        );
    }

    #[test]
    fn negative_edge_softness_clamps_to_zero() {
        let mut reg = EntityRegistry::new();
        let id = spawn_fog(&mut reg);
        dispatch(
            &mut reg,
            &[id],
            &SetFogEdgeSoftnessArgs {
                edge_softness: -0.5,
            },
        )
        .unwrap();
        assert_eq!(
            reg.get_component::<FogVolumeComponent>(id)
                .unwrap()
                .edge_softness,
            0.0
        );
    }

    #[test]
    fn non_finite_edge_softness_clamps_to_zero() {
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let mut reg = EntityRegistry::new();
            let id = spawn_fog(&mut reg);
            dispatch(
                &mut reg,
                &[id],
                &SetFogEdgeSoftnessArgs { edge_softness: bad },
            )
            .unwrap();
            assert_eq!(
                reg.get_component::<FogVolumeComponent>(id)
                    .unwrap()
                    .edge_softness,
                0.0,
                "expected 0.0 for non-finite input {bad}"
            );
        }
    }

    #[test]
    fn empty_target_set_is_a_noop() {
        let mut reg = EntityRegistry::new();
        let id = spawn_fog(&mut reg);
        dispatch(
            &mut reg,
            &[],
            &SetFogEdgeSoftnessArgs { edge_softness: 9.0 },
        )
        .unwrap();
        assert_eq!(
            reg.get_component::<FogVolumeComponent>(id)
                .unwrap()
                .edge_softness,
            0.25
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
                &SetFogEdgeSoftnessArgs { edge_softness: 0.5 },
            )
            .unwrap();
        });

        assert_eq!(
            reg.get_component::<FogVolumeComponent>(fog)
                .unwrap()
                .edge_softness,
            0.5
        );
        assert!(
            captured.iter().any(|(lvl, msg)| *lvl == log::Level::Warn
                && msg.contains("no FogVolumeComponent")),
            "expected a warn-level log naming the missing component, got: {captured:?}"
        );
    }

    #[test]
    fn set_fog_edge_softness_args_deserialize_edge_softness_from_camelcase_json() {
        let v = serde_json::json!({ "edgeSoftness": 0.42 });
        let parsed: SetFogEdgeSoftnessArgs = serde_json::from_value(v).unwrap();
        assert_eq!(parsed.edge_softness, 0.42);
    }
}
