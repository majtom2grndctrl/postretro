// Shared `#[cfg(test)]` descriptor/placement builders for the data-archetype
// dispatch path. Lifted out of `data_archetype.rs`'s own `mod tests` so the
// netcode-side agreement test (`netcode::descriptor_class`) can materialize
// entities FROM the same descriptor shapes without reaching back up into the
// scripting tree's private test helpers — and without duplicating the builders
// (which would invite drift). See context/lib/testing_guide.md §4.

use std::collections::HashMap;

use glam::Vec3;

use crate::scripting::map_entity::MapEntity;
use postretro_scripting_core::data_descriptors::{
    AiDescriptor, AiStateNames, EntityTypeDescriptor,
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
                clip_index: None,
            },
        );
        (states, Some("idle".to_string()))
    } else {
        (HashMap::new(), None)
    };

    EntityTypeDescriptor {
        canonical_name: Some(classname.to_string()),
        default_weapon: None,
        light: None,
        emitter: None,
        movement: None,
        weapon: None,
        mesh: Some(postretro_scripting_core::data_descriptors::MeshDescriptor {
            model: "decraniated".to_string(),
            animations,
            default_state,
        }),
        health: None,
        ai: None,
    }
}

/// Minimal valid `ai` block: its mere presence is what attaches `Brain` +
/// `Agent`, which is the single thing the pre-materialization classifier and
/// the live predicate both key on. Tuning values are not exercised here.
fn sample_ai_descriptor() -> AiDescriptor {
    AiDescriptor {
        detection_range: 18.0,
        attack_range: 2.0,
        leash_range: 26.0,
        attack_damage: 8.0,
        attack_cooldown_ms: 1200.0,
        move_speed: 3.5,
        death_despawn_ms: 1500.0,
        states: AiStateNames {
            idle: "idle".into(),
            alert: "walk".into(),
            attack: "attack".into(),
            death: "die".into(),
        },
    }
}

/// An AI-enemy descriptor: a mesh placement (so it is directly map-placeable)
/// plus an `ai` block (so materialization attaches `Brain` + `Agent`). This
/// is the shape a real map-placed enemy descriptor has.
pub(crate) fn ai_enemy_descriptor(classname: &str) -> EntityTypeDescriptor {
    let mut descriptor = mesh_descriptor(classname, true);
    descriptor.ai = Some(sample_ai_descriptor());
    descriptor
}
