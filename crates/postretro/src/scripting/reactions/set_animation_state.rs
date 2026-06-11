// `setAnimationState` reaction primitive: switch the animation state on every
// entity matching the reaction's tag, routing each through the single validated
// switch path (`switch_animation_state`).
// See: context/lib/scripting.md §10.3 (Mesh Animation — Reaction primitives)

use serde::{Deserialize, Serialize};

use crate::scripting::components::mesh::{SwitchResult, switch_animation_state};
use crate::scripting::registry::{EntityId, EntityRegistry};

use super::ReactionError;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct SetAnimationStateArgs {
    pub(crate) state: String,
}

/// Switch every target to `args.state` through [`switch_animation_state`], the
/// single validated state-switch path (Phase 1). This dispatcher only maps each
/// [`SwitchResult`] to the appropriate log; all validation lives in the switch
/// function (mirrors the `setEmitterRate` validated-setter precedent).
///
/// Per-target behavior:
/// - [`SwitchResult::NotAnimated`] → `log::warn!`, skip (tag matched a non-mesh
///   entity or a stateless mesh — most likely a tag typo).
/// - [`SwitchResult::UnknownState`] → `log::warn!` (state not declared or its
///   clip did not resolve at level load). Current state left unchanged.
/// - [`SwitchResult::Switched`] / [`SwitchResult::AlreadyInState`] → proceed
///   quietly.
/// - Empty target set → no-op, debug log.
pub(crate) fn dispatch(
    registry: &mut EntityRegistry,
    targets: &[EntityId],
    args: &SetAnimationStateArgs,
) -> Result<(), ReactionError> {
    if targets.is_empty() {
        log::debug!("[Scripting] setAnimationState: empty target set, no-op");
        return Ok(());
    }

    for &id in targets {
        match switch_animation_state(registry, id, &args.state) {
            SwitchResult::Switched | SwitchResult::AlreadyInState => {}
            SwitchResult::NotAnimated => {
                log::warn!(
                    "[Scripting] setAnimationState: entity {id} is not an animated mesh \
                     (non-mesh or stateless); skipping"
                );
            }
            SwitchResult::UnknownState => {
                log::warn!(
                    "[Scripting] setAnimationState: entity {id} has no usable state '{}' \
                     (undeclared or unresolved); state unchanged",
                    args.state
                );
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::scripting::components::mesh::{
        AnimationState, DEFAULT_CROSSFADE_MS, InterruptPolicy, MeshAnimation, MeshComponent,
        resolve_pending_animation_stamps,
    };
    use crate::scripting::registry::Transform;

    fn usable_state(clip: &str, looping: bool, clip_index: usize) -> AnimationState {
        AnimationState {
            clip: clip.into(),
            looping,
            crossfade_ms: DEFAULT_CROSSFADE_MS,
            interrupt: InterruptPolicy::Smooth,
            clip_index: Some(clip_index),
        }
    }

    fn two_state_animation() -> MeshAnimation {
        let mut states = HashMap::new();
        states.insert("idle".into(), usable_state("idle_clip", true, 0));
        states.insert("attack".into(), usable_state("attack_clip", false, 1));
        MeshAnimation::new(states, "idle".into())
    }

    fn spawn_animated(reg: &mut EntityRegistry, tags: &[&str]) -> EntityId {
        let id = reg.spawn(Transform::default());
        reg.set_component(
            id,
            MeshComponent {
                model: "decraniated".into(),
                animation: Some(two_state_animation()),
            },
        )
        .unwrap();
        if !tags.is_empty() {
            reg.set_tags(id, tags.iter().map(|t| t.to_string()).collect())
                .unwrap();
        }
        id
    }

    fn current_state(reg: &EntityRegistry, id: EntityId) -> String {
        reg.get_component::<MeshComponent>(id)
            .unwrap()
            .animation
            .as_ref()
            .unwrap()
            .current_state
            .clone()
    }

    #[test]
    fn switches_state_on_each_target() {
        let mut reg = EntityRegistry::new();
        let a = spawn_animated(&mut reg, &["enemies"]);
        let b = spawn_animated(&mut reg, &["enemies"]);
        // Resolve spawn stamps so the switch records a real fade, not a collapse.
        resolve_pending_animation_stamps(&mut reg, 1.0);

        dispatch(
            &mut reg,
            &[a, b],
            &SetAnimationStateArgs {
                state: "attack".into(),
            },
        )
        .unwrap();

        assert_eq!(current_state(&reg, a), "attack");
        assert_eq!(current_state(&reg, b), "attack");
    }

    #[test]
    fn already_in_state_target_proceeds_quietly() {
        let mut reg = EntityRegistry::new();
        let id = spawn_animated(&mut reg, &[]);
        resolve_pending_animation_stamps(&mut reg, 1.0);

        let captured = crate::scripting::reactions::log_capture::capture(|| {
            dispatch(
                &mut reg,
                &[id],
                &SetAnimationStateArgs {
                    state: "idle".into(),
                },
            )
            .unwrap();
        });

        assert_eq!(current_state(&reg, id), "idle");
        assert!(
            !captured.iter().any(|(lvl, _)| *lvl == log::Level::Warn),
            "switching to the current state must not warn, got: {captured:?}"
        );
    }

    #[test]
    fn empty_target_set_is_a_noop() {
        let mut reg = EntityRegistry::new();
        let id = spawn_animated(&mut reg, &[]);
        resolve_pending_animation_stamps(&mut reg, 1.0);

        // The empty-set path debug-logs and returns Ok; we assert the
        // observable no-op (state unchanged) rather than the debug line, which
        // the global `log::max_level` can filter out under the parallel suite.
        dispatch(
            &mut reg,
            &[],
            &SetAnimationStateArgs {
                state: "attack".into(),
            },
        )
        .unwrap();

        // No target was touched.
        assert_eq!(current_state(&reg, id), "idle");
    }

    #[test]
    fn unknown_state_warns_and_leaves_state_unchanged() {
        let mut reg = EntityRegistry::new();
        let id = spawn_animated(&mut reg, &[]);
        resolve_pending_animation_stamps(&mut reg, 1.0);

        let captured = crate::scripting::reactions::log_capture::capture(|| {
            dispatch(
                &mut reg,
                &[id],
                &SetAnimationStateArgs {
                    state: "nonexistent".into(),
                },
            )
            .unwrap();
        });

        assert_eq!(
            current_state(&reg, id),
            "idle",
            "unknown state must not change the current state"
        );
        assert!(
            captured.iter().any(|(lvl, msg)| *lvl == log::Level::Warn
                && msg.contains("no usable state 'nonexistent'")),
            "expected a warn naming the unknown state, got: {captured:?}"
        );
    }

    #[test]
    fn non_mesh_target_is_skipped_with_warn() {
        let mut reg = EntityRegistry::new();
        // Entity with only a Transform — no mesh component at all.
        let bare = reg.spawn(Transform::default());
        let animated = spawn_animated(&mut reg, &[]);
        resolve_pending_animation_stamps(&mut reg, 1.0);

        let captured = crate::scripting::reactions::log_capture::capture(|| {
            dispatch(
                &mut reg,
                &[bare, animated],
                &SetAnimationStateArgs {
                    state: "attack".into(),
                },
            )
            .unwrap();
        });

        // The animated entity past the skipped non-mesh one still switched.
        assert_eq!(current_state(&reg, animated), "attack");
        assert!(
            captured
                .iter()
                .any(|(lvl, msg)| *lvl == log::Level::Warn && msg.contains("not an animated mesh")),
            "expected a warn about the non-mesh target, got: {captured:?}"
        );
    }

    #[test]
    fn stateless_mesh_target_is_skipped_with_warn() {
        let mut reg = EntityRegistry::new();
        let id = reg.spawn(Transform::default());
        // A stateless `prop_mesh` mesh: has a MeshComponent but no animation block.
        reg.set_component(id, MeshComponent::stateless("prop".into()))
            .unwrap();

        let captured = crate::scripting::reactions::log_capture::capture(|| {
            dispatch(
                &mut reg,
                &[id],
                &SetAnimationStateArgs {
                    state: "idle".into(),
                },
            )
            .unwrap();
        });

        // Stateless component is untouched (still has no animation block).
        assert!(
            reg.get_component::<MeshComponent>(id)
                .unwrap()
                .animation
                .is_none()
        );
        assert!(
            captured
                .iter()
                .any(|(lvl, msg)| *lvl == log::Level::Warn && msg.contains("not an animated mesh")),
            "expected a warn about the stateless target, got: {captured:?}"
        );
    }
}
