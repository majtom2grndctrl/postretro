// Scripting subsystem: the Rust-owned entity/component surface that scripts manipulate.
// See: context/plans/in-progress/scripting-foundation/plan-1-runtime-foundation.md

// This subsystem is the *scripting surface*, not a general ECS pivot.
// Postretro is not ECS-architected (see context/lib/index.md §4 Non-Goals).
// The registry here is deliberately the narrow slice of state that scripts
// are allowed to see and mutate; renderer/audio/input own their own data
// structures and are unaffected by anything in this module.

#![deny(unsafe_code)]
// Sub-plan 2 lands the binding layer but does NOT yet instantiate script
// runtimes (sub-plans 3 and 4 will). Installer closures, ScriptCtx, error
// types, and day-one primitives are all built and covered by tests against
// sacrificial rquickjs / mlua contexts — but nothing in the rest of the
// engine calls them yet. Silence dead-code at the subsystem level until the
// first real consumer lands, rather than sprinkling `#[allow]` on every item.
#![allow(dead_code)]

pub(crate) mod conv;
pub(crate) mod ctx;
pub(crate) mod error;
pub(crate) mod luau;
pub(crate) mod pool;
pub(crate) mod primitives;
pub(crate) mod primitives_registry;
pub(crate) mod quickjs;
pub(crate) mod registry;
pub(crate) mod runtime;
pub(crate) mod typedef;
