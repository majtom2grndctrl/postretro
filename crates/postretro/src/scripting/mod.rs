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
pub(crate) mod conv;
pub(crate) mod ctx;
pub(crate) mod data_descriptors;
pub(crate) mod data_registry;
pub(crate) mod error;
pub(crate) mod ir;
pub(crate) mod luau;
pub(crate) mod map_entity;
pub(crate) mod primitives;
pub(crate) mod primitives_registry;
pub(crate) mod provenance;
pub(crate) mod quickjs;
pub(crate) mod reaction_dispatch;
pub(crate) mod reactions;
pub(crate) mod refresh_plan;
pub(crate) mod registry;
pub(crate) mod runtime;
pub(crate) mod sequence;
pub(crate) mod slot_table;
pub(crate) mod staged_manifest;
pub(crate) mod state_crossings;
pub(crate) mod state_persistence;
pub(crate) mod typedef;

// Dev-mode hot reload. Compiled in debug builds only; the module itself has a
// `#![cfg(debug_assertions)]` gate, but we also gate the `mod` declaration so
// nothing downstream can accidentally reference its types in a release build.
#[cfg(debug_assertions)]
pub(crate) mod watcher;
