// World primitive compatibility re-exports.
// See: context/lib/scripting.md

#![allow(unused_imports)]

pub(crate) use crate::scripting::entity_world_primitives::register_world_primitives;
pub(crate) use postretro_scripting_core::primitive_adapters::{JsonValue, WorldQueryFilterInput};
