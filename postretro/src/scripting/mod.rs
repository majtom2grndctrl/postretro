// Scripting subsystem: the Rust-owned entity/component surface that scripts manipulate.
// See: context/plans/in-progress/scripting-foundation/plan-1-runtime-foundation.md

// This subsystem is the *scripting surface*, not a general ECS pivot.
// Postretro is not ECS-architected (see context/lib/index.md §4 Non-Goals).
// The registry here is deliberately the narrow slice of state that scripts
// are allowed to see and mutate; renderer/audio/input own their own data
// structures and are unaffected by anything in this module.

#![deny(unsafe_code)]

pub(crate) mod registry;
