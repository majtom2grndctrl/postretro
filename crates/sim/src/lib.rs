//! Fixed-tick gameplay simulation and scripting host for Postretro.
//!
//! The binary owns input, audio, rendering, and presentation orchestration;
//! this crate owns the data-injected game-logic stages they drive.

#![deny(unsafe_code)]

mod agent;
pub mod agent_steering;
pub mod ai_host;
#[cfg(any(test, feature = "test-support"))]
pub mod alloc_probe;
pub mod combat_positioning;
pub mod emission;
mod fx;
pub mod ghost_audio_command;
pub mod ghost_link_commands;
mod grant;
#[cfg(not(feature = "test-support"))]
pub(crate) mod health;
#[cfg(feature = "test-support")]
pub mod health;
/// Install-time check of a `.damage` step's args against the vocabulary.
pub use grant::validate_grant_layer_args;
pub use health::reactions::validate_apply_damage_args;

/// Check a reaction step's defense references against the committed
/// vocabulary: `applyDamage`'s damage type and `grantLayer`'s layer. Other
/// primitives pass. The error names the primitive.
pub fn validate_defense_reference_args(
    primitive: &str,
    args: &serde_json::Value,
    vocabulary: &postretro_foundation::defense::DefenseVocabulary,
) -> Result<(), String> {
    match primitive {
        "applyDamage" => validate_apply_damage_args(args, vocabulary)
            .map_err(|error| format!("applyDamage {error}")),
        "grantLayer" => validate_grant_layer_args(args, vocabulary),
        _ => Ok(()),
    }
}
pub mod impact_effects;
pub mod impact_policy;
pub mod life_state_commands;
pub mod mover_commands;
pub mod nav;
pub mod particle_pool;
pub mod placement_occupancy;
pub mod player_events;
pub mod residual_drain;
pub mod respawn_commands;
pub mod scripting;
pub mod scripting_systems;
pub mod sim;
pub mod spawner;
pub mod sprite_collection;
pub mod teleporter_commands;
pub mod teleporter_system;
pub mod traps;
pub mod trigger_bindings;
pub mod trigger_commands;
pub mod trigger_direct_commands;
pub mod trigger_pools;
pub mod trigger_system;
pub mod weapon;

pub(crate) use postretro_physics::{collision, kinematic_mover, movement};

pub use sim::{
    frame_timing, presentation_pool, resolve_mesh_entity_bindings,
    resolve_mesh_entity_bindings_for_entities,
};

/// Retarget the dev-tools chase agent through a narrow dev-tools facade; `agent_steering`
/// remains public for the AI boundary.
#[cfg(feature = "dev-tools")]
pub fn set_debug_agent_destination(
    registry: &mut postretro_entities::EntityRegistry,
    agent: postretro_entities::EntityId,
    destination: glam::Vec3,
) {
    agent_steering::set_destination(registry, agent, destination);
}

#[cfg(test)]
#[global_allocator]
static ALLOCATOR: alloc_probe::CountingAllocator = alloc_probe::CountingAllocator;
