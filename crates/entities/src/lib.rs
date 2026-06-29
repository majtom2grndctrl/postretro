//! VM-free entity registry and data substrate for Postretro.
//! See: context/lib/scripting.md §12.

#![deny(unsafe_code)]

pub mod components;
pub mod ctx;
pub mod data_descriptors;
pub mod data_registry;
pub mod engine_state_catalog;
pub mod provenance;
pub mod reactions;
pub mod registry;
pub mod scripting;
pub mod slot_table;

#[cfg(feature = "script-ffi")]
mod ffi;

pub use ctx::ScriptCtx;
pub use data_descriptors::*;
pub use data_registry::{DataRegistry, ScopedCrossing, ScopedReaction};
pub use engine_state_catalog::*;
pub use provenance::*;
pub use reactions::system_commands::{SystemCommandQueue, SystemReactionCommand};
pub use registry::{
    Component, ComponentKind, ComponentValue, EntityId, EntityRegistry, FogVolumeComponent,
    RegistryError, Transform,
};
pub use scripting::error::ScriptError;
pub use slot_table::*;
