// Scripting subsystem: the Rust-owned entity/component surface that scripts manipulate.
// See: context/lib/scripting.md (not ECS; scripting owns a narrow surface only).

// Renderer, audio, and input own their own data structures and are unaffected
// by anything in this module.

#![deny(unsafe_code)]
// No engine consumers yet — silence dead-code at the subsystem level rather
// than sprinkling `#[allow]` on every item.
#![allow(dead_code)]

pub(crate) mod builtins;
pub(crate) mod call_context;
pub(crate) mod components;
pub(crate) mod conv;
pub(crate) mod ctx;
pub(crate) mod data_descriptors;
pub(crate) mod data_registry;
pub(crate) mod error;
pub(crate) mod event_dispatch;
pub(crate) mod luau;
pub(crate) mod pool;
pub(crate) mod primitives;
pub(crate) mod primitives_light;
pub(crate) mod primitives_registry;
pub(crate) mod quickjs;
pub(crate) mod reaction_dispatch;
pub(crate) mod reactions;
pub(crate) mod registry;
pub(crate) mod runtime;
pub(crate) mod sequence;
pub(crate) mod typedef;

// Dev-mode hot reload. Compiled in debug builds only; the module itself has a
// `#![cfg(debug_assertions)]` gate, but we also gate the `mod` declaration so
// nothing downstream can accidentally reference its types in a release build.
#[cfg(debug_assertions)]
pub(crate) mod watcher;
