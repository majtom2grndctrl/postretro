//! Fixed-tick gameplay simulation and scripting host for Postretro.
//!
//! The binary owns input, audio, rendering, and presentation orchestration;
//! this crate owns the data-injected game-logic stages they drive.

#![deny(unsafe_code)]

mod agent;
mod agent_steering;
pub mod ai_host;
#[cfg(any(test, feature = "test-support"))]
pub mod alloc_probe;
pub mod collision;
mod combat_positioning;
mod fx;
mod grant;
mod health;
pub mod impact_effects;
pub mod impact_policy;
pub mod kinematic_mover;
pub mod movement;
pub mod nav;
pub mod scripting;
pub mod scripting_systems;
pub mod sim;
pub mod spawner;
pub mod sprite_collection;
pub mod trigger_bindings;
pub mod trigger_commands;
pub mod trigger_pools;
pub mod trigger_system;
pub mod weapon;

pub use sim::{
    frame_timing, presentation_pool, resolve_mesh_entity_bindings,
    resolve_mesh_entity_bindings_for_entities,
};

/// Retarget the dev-tools chase agent without exposing the steering subsystem.
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
