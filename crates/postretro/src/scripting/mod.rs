// Scripting subsystem: Rust-owned entity/component APIs, engine-global typed state, and persistence.
// See: context/lib/scripting.md for governing scripting contracts and ownership.

// Renderer, audio, and input own their own data structures and are unaffected
// by anything in this module.

#![deny(unsafe_code)]
// Some component types not yet wired to their bridges — silence dead-code at
// the subsystem level rather than scattering item-level annotations.
#![allow(dead_code)]

pub(crate) mod builtins;
pub(crate) mod components;
pub(crate) mod conv {
    #![allow(unused_imports)]
    pub(crate) use postretro_scripting_core::conv::*;
}
pub(crate) mod ctx;
pub(crate) mod data_descriptors {
    #![allow(unused_imports)]
    pub(crate) use postretro_scripting_core::data_descriptors::*;
}
pub(crate) mod data_registry;
pub(crate) mod engine_state_catalog;
pub(crate) mod error;
pub(crate) mod foundation_pods;
pub(crate) mod game_state_refs {
    #![allow(unused_imports)]
    pub(crate) use postretro_scripting_core::game_state_refs::*;
}
pub(crate) mod ir;
pub(crate) mod ir_scopes {
    #![allow(unused_imports)]
    pub(crate) use postretro_scripting_core::ir_scopes::*;
}
pub(crate) mod luau {
    #![allow(unused_imports)]
    pub(crate) use postretro_scripting_core::luau::*;
}
pub(crate) mod luau_prelude;
pub(crate) mod luau_require {
    #![allow(unused_imports)]
    pub(crate) use postretro_scripting_core::luau_require::*;
}
pub(crate) mod luau_virtual_modules {
    #![allow(unused_imports)]
    pub(crate) use postretro_scripting_core::luau_virtual_modules::*;
}
pub(crate) mod map_entity;
pub(crate) mod primitives;
pub(crate) mod primitives_registry {
    #![allow(unused_imports)]
    pub(crate) use postretro_scripting_core::primitives_registry::*;
}
pub(crate) mod provenance;
pub(crate) mod quickjs {
    #![allow(unused_imports)]
    pub(crate) use postretro_scripting_core::quickjs::*;
}
pub(crate) mod reaction_dispatch {
    #![allow(unused_imports)]
    pub(crate) use postretro_scripting_core::reaction_dispatch::*;
}
pub(crate) mod reactions;
pub(crate) mod refresh_plan {
    #![allow(unused_imports)]
    pub(crate) use postretro_scripting_core::refresh_plan::*;
}
pub(crate) mod registry;
pub(crate) mod runtime {
    #![allow(unused_imports)]
    pub(crate) use postretro_scripting_core::runtime::*;
}
pub(crate) mod sequence {
    #![allow(unused_imports)]
    pub(crate) use postretro_scripting_core::sequence::*;
}
pub(crate) mod slot_table;
pub(crate) mod staged_manifest {
    #![allow(unused_imports)]
    pub(crate) use postretro_scripting_core::staged_manifest::*;
}
pub(crate) mod state_crossings;
pub(crate) mod state_persistence;
pub(crate) mod typedef {
    #![allow(unused_imports)]
    pub(crate) use postretro_scripting_core::typedef::*;
}
pub(crate) mod value_types;

// Dev-mode hot reload. Compiled in debug builds only; the module itself has a
// `#![cfg(debug_assertions)]` gate, but we also gate the `mod` declaration so
// nothing downstream can accidentally reference its types in a release build.
#[cfg(debug_assertions)]
pub(crate) mod watcher {
    #![allow(unused_imports)]
    pub(crate) use postretro_scripting_core::watcher::*;
}
