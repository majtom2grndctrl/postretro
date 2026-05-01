// `setEmitterRate` reaction primitive: zero or modulate emission rate on
// every entity matching the reaction's tag.
// See: context/lib/scripting.md §11 (Emitter and Particles — Reaction primitives)

use serde::{Deserialize, Serialize};

use crate::scripting::components::billboard_emitter::BillboardEmitterComponent;
use crate::scripting::registry::{EntityId, EntityRegistry};

use super::ReactionError;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct SetEmitterRateArgs {
    pub(crate) rate: f32,
}

/// Apply `args.rate` to every target's `BillboardEmitterComponent.rate`.
///
/// Per-target behavior:
/// - Missing component → `log::warn!`, skip (tag matched a non-emitter — most
///   likely a tag typo).
/// - `rate < 0.0` → `log::warn!` once and clamp to `0.0`. Continues for the
///   remaining targets.
/// - Empty target set → no-op, debug log.
pub(crate) fn dispatch(
    registry: &mut EntityRegistry,
    targets: &[EntityId],
    args: &SetEmitterRateArgs,
) -> Result<(), ReactionError> {
    if targets.is_empty() {
        log::debug!("[Scripting] setEmitterRate: empty target set, no-op");
        return Ok(());
    }

    let rate = if args.rate.is_finite() && args.rate >= 0.0 {
        args.rate
    } else {
        log::warn!(
            "[Scripting] setEmitterRate: rate {} is negative or non-finite; clamping to 0.0",
            args.rate
        );
        0.0
    };

    for &id in targets {
        let current = match registry.get_component::<BillboardEmitterComponent>(id) {
            Ok(c) => c.clone(),
            Err(_) => {
                log::warn!(
                    "[Scripting] setEmitterRate: entity {id} has no BillboardEmitterComponent; skipping"
                );
                continue;
            }
        };
        let mut next = current;
        next.rate = rate;
        if let Err(e) = registry.set_component(id, next) {
            log::warn!("[Scripting] setEmitterRate: failed to write component on {id}: {e:?}");
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
            velocity: [0.0, 1.0, 0.0],
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

    fn spawn_emitter(reg: &mut EntityRegistry, tags: &[&str]) -> EntityId {
        let id = reg.spawn(Transform::default());
        reg.set_component(id, sample_emitter()).unwrap();
        if !tags.is_empty() {
            reg.set_tags(id, tags.iter().map(|t| t.to_string()).collect())
                .unwrap();
        }
        id
    }

    #[test]
    fn zeroes_rate_on_each_target() {
        let mut reg = EntityRegistry::new();
        let a = spawn_emitter(&mut reg, &["campfires"]);
        let b = spawn_emitter(&mut reg, &["campfires"]);
        dispatch(&mut reg, &[a, b], &SetEmitterRateArgs { rate: 0.0 }).unwrap();
        assert_eq!(
            reg.get_component::<BillboardEmitterComponent>(a)
                .unwrap()
                .rate,
            0.0
        );
        assert_eq!(
            reg.get_component::<BillboardEmitterComponent>(b)
                .unwrap()
                .rate,
            0.0
        );
    }

    #[test]
    fn updates_rate_to_positive_value() {
        let mut reg = EntityRegistry::new();
        let id = spawn_emitter(&mut reg, &[]);
        dispatch(&mut reg, &[id], &SetEmitterRateArgs { rate: 20.0 }).unwrap();
        let after = reg.get_component::<BillboardEmitterComponent>(id).unwrap();
        assert_eq!(after.rate, 20.0);
    }

    #[test]
    fn negative_rate_clamps_to_zero() {
        let mut reg = EntityRegistry::new();
        let id = spawn_emitter(&mut reg, &[]);
        dispatch(&mut reg, &[id], &SetEmitterRateArgs { rate: -7.5 }).unwrap();
        let after = reg.get_component::<BillboardEmitterComponent>(id).unwrap();
        assert_eq!(after.rate, 0.0);
    }

    #[test]
    fn empty_target_set_is_a_noop() {
        let mut reg = EntityRegistry::new();
        let id = spawn_emitter(&mut reg, &[]);
        dispatch(&mut reg, &[], &SetEmitterRateArgs { rate: 99.0 }).unwrap();
        let after = reg.get_component::<BillboardEmitterComponent>(id).unwrap();
        assert_eq!(after.rate, 5.0);
    }

    #[test]
    fn non_emitter_target_is_skipped_with_warn() {
        let mut reg = EntityRegistry::new();
        // Entity with only a Transform — no emitter component.
        let bare = reg.spawn(Transform::default());
        let emitter = spawn_emitter(&mut reg, &[]);

        let captured = crate::scripting::reactions::log_capture::capture(|| {
            dispatch(
                &mut reg,
                &[bare, emitter],
                &SetEmitterRateArgs { rate: 0.0 },
            )
            .unwrap();
        });

        assert_eq!(
            reg.get_component::<BillboardEmitterComponent>(emitter)
                .unwrap()
                .rate,
            0.0
        );
        assert!(
            captured.iter().any(|(lvl, msg)| *lvl == log::Level::Warn
                && msg.contains("no BillboardEmitterComponent")),
            "expected a warn-level log naming the missing component, got: {captured:?}"
        );
    }

    #[test]
    fn negative_rate_emits_warn() {
        let mut reg = EntityRegistry::new();
        let id = spawn_emitter(&mut reg, &[]);
        let captured = crate::scripting::reactions::log_capture::capture(|| {
            dispatch(&mut reg, &[id], &SetEmitterRateArgs { rate: -1.0 }).unwrap();
        });
        assert!(
            captured
                .iter()
                .any(|(lvl, msg)| *lvl == log::Level::Warn && msg.contains("clamping to 0.0")),
            "expected a warn-level log about clamping, got: {captured:?}"
        );
    }
}
