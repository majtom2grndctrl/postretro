// Shared `#[cfg(test)]` descriptor/placement builders for the data-archetype
// dispatch path. Lifted out of `data_archetype.rs`'s own `mod tests` so the
// netcode-side agreement test (`netcode::descriptor_class`) can materialize
// entities FROM the same descriptor shapes without reaching back up into the
// scripting tree's private test helpers — and without duplicating the builders
// (which would invite drift). See context/lib/testing_guide.md §4.

use std::collections::{BTreeMap, HashMap};

use glam::Vec3;

use crate::scripting::map_entity::MapEntity;
use postretro_foundation::{BRAIN_TARGET_DISTANCE_INPUT, IrNode, IrValue};
use postretro_scripting_core::data_descriptors::{
    ActionVerb, AttackParams, BehaviorActivityDescriptor, BehaviorGraphDescriptor,
    BehaviorGraphEnvelope, EntityTypeDescriptor, GuardedRow, MotionVerb,
};

/// A `MapEntity` placement with the given classname and raw KVP bag. Origin is a
/// fixed non-zero point so spawned `Transform`s are distinguishable from defaults.
pub(crate) fn placement(classname: &str, kvps: &[(&str, &str)]) -> MapEntity {
    let mut kv = HashMap::new();
    for (k, v) in kvps {
        kv.insert((*k).to_string(), (*v).to_string());
    }
    MapEntity {
        classname: classname.to_string(),
        origin: Vec3::new(1.0, 2.0, 3.0),
        angles: Vec3::ZERO,
        key_values: kv,
        tags: vec![],
    }
}

/// Build an `EntityTypeDescriptor` carrying only a mesh component. `animated`
/// selects between a stateless mesh (model only) and a two-state animated
/// mesh (`idle` default + `attack`), mirroring the validated descriptor shape
/// the mesh parser produces.
pub(crate) fn mesh_descriptor(classname: &str, animated: bool) -> EntityTypeDescriptor {
    use postretro_entities::components::mesh::{AnimationState, InterruptPolicy};

    let (animations, default_state) = if animated {
        let mut states = HashMap::new();
        states.insert(
            "idle".to_string(),
            AnimationState {
                clip: "idle_clip".to_string(),
                looping: true,
                crossfade_ms: 150.0,
                interrupt: InterruptPolicy::Smooth,
                travel_speed: None,
                clip_index: None,
            },
        );
        states.insert(
            "attack".to_string(),
            AnimationState {
                clip: "attack_clip".to_string(),
                looping: false,
                crossfade_ms: 0.0,
                interrupt: InterruptPolicy::Snap,
                travel_speed: None,
                clip_index: None,
            },
        );
        (states, Some("idle".to_string()))
    } else {
        (HashMap::new(), None)
    };

    EntityTypeDescriptor {
        canonical_name: Some(classname.to_string()),
        inventory: None,
        light: None,
        emitter: None,
        movement: None,
        weapon: None,
        touchable: None,
        mesh: Some(postretro_scripting_core::data_descriptors::MeshDescriptor {
            model: "decraniated".to_string(),
            shadow_only: false,
            attachments: Default::default(),
            shadow_bias_scale: 1.0,
            animations,
            default_state,
            locomotion: None,
        }),
        health: None,
        behavior: None,
    }
}

/// Minimal valid `components.behavior` graph over the two animation states
/// [`mesh_descriptor`] declares (`idle` default + `attack`): rest in `idle`
/// until the target closes, then chase-and-attack. `initial`'s animation is the
/// mesh's `defaultState`, so this fixture is also rest-pose consistent
/// (`validate_brain_animation_states`).
fn sample_behavior_graph() -> BehaviorGraphDescriptor {
    BehaviorGraphDescriptor {
        envelope: BehaviorGraphEnvelope {
            initial: "idle".to_string(),
            activities: BTreeMap::from([
                (
                    "idle".to_string(),
                    BehaviorActivityDescriptor {
                        animation: Some("idle".to_string()),
                        motion: Some(MotionVerb::Hold),
                        action: None,
                        on_enter: None,
                        layers: BTreeMap::new(),
                    },
                ),
                (
                    "attack".to_string(),
                    BehaviorActivityDescriptor {
                        animation: Some("attack".to_string()),
                        motion: Some(MotionVerb::ChaseTarget),
                        action: Some(ActionVerb::Attack("claw".to_string())),
                        on_enter: None,
                        layers: BTreeMap::new(),
                    },
                ),
            ]),
            transitions: BTreeMap::from([(
                "idle".to_string(),
                vec![GuardedRow {
                    to: "attack".to_string(),
                    when: IrNode::Le {
                        a: Box::new(IrNode::Input {
                            name: BRAIN_TARGET_DISTANCE_INPUT.to_string(),
                            owner: None,
                        }),
                        b: Box::new(IrNode::Const {
                            value: IrValue::Number(16.0),
                        }),
                    },
                }],
            )]),
        },
        candidate_filter: None,
        patrol: None,
        attacks: BTreeMap::from([(
            "claw".to_string(),
            AttackParams {
                weapon: None,
                damage: Some(8.0),
                max_range: Some(2.0),
                cooldown_ms: Some(1200.0),
                engagement_radius: None,
                standoff_distance: None,
            },
        )]),
        engagement_radius: None,
        move_speed: 3.5,
    }
}

/// A behavior-authored enemy descriptor: map-placeable mesh plus a graph that
/// materializes `Brain` and `Agent`.
pub(crate) fn behavior_enemy_descriptor(classname: &str) -> EntityTypeDescriptor {
    let mut descriptor = mesh_descriptor(classname, true);
    descriptor.behavior = Some(sample_behavior_graph());
    descriptor
}
