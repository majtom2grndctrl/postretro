// VM-coupled scripting substrate: runtime hosts, FFI conversion, registries, and SDK types.
// See: context/lib/scripting.md §12

#![deny(unsafe_code)]
#![allow(dead_code)]

pub mod components {
    pub use postretro_entities::components::*;
}

pub mod ctx {
    pub use postretro_entities::ctx::*;
}

pub mod data_registry {
    pub use postretro_entities::data_registry::*;
}

pub mod engine_state_catalog {
    pub use postretro_entities::engine_state_catalog::*;
}

pub mod error {
    pub use postretro_entities::scripting::error::*;
}

pub mod foundation_pods {
    pub use postretro_foundation::foundation_pods::*;
}

pub mod ir {
    pub use postretro_foundation::ir::*;

    #[cfg(test)]
    mod e2e_tests;
    #[cfg(test)]
    mod parity_tests;
    #[cfg(test)]
    pub mod test_scope;
}

#[path = "ir/scopes.rs"]
pub mod ir_scopes;

pub mod provenance {
    pub use postretro_entities::provenance::*;
}

pub mod registry {
    pub use postretro_entities::registry::*;
}

pub mod slot_table {
    pub use postretro_entities::slot_table::*;
}

pub mod value_types {
    pub use postretro_foundation::value_types::*;
}

pub mod ui;

pub mod conv;
pub mod data_descriptors;
pub mod game_state_refs;
pub mod luau;
pub mod luau_prelude;
pub mod luau_require;
pub mod luau_virtual_modules;
pub mod primitives_registry;
pub mod quickjs;
pub mod reaction_dispatch;
pub mod reaction_registry;
pub mod refresh_plan;
pub mod runtime;
pub mod sequence;
pub mod staged_manifest;
pub mod state_crossings;
pub mod store_bridge;
pub mod typedef;

#[cfg(debug_assertions)]
pub mod watcher;
