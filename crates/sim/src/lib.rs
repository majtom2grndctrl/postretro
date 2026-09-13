//! Fixed-tick gameplay simulation and scripting host for Postretro.
//!
//! The binary owns input, audio, rendering, and presentation orchestration;
//! this crate owns the data-injected game-logic stages they drive.

#![deny(unsafe_code)]
#![allow(dead_code)]

pub mod agent;
pub mod agent_steering;
pub mod alloc_probe;
pub mod collision;
pub mod combat_positioning;
pub mod fx;
pub mod grant;
pub mod health;
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
    frame_timing, presentation_pool, presentation_projection, resolve_mesh_entity_bindings,
    resolve_mesh_entity_bindings_for_entities,
};

#[cfg(test)]
#[global_allocator]
static ALLOCATOR: alloc_probe::CountingAllocator = alloc_probe::CountingAllocator;
