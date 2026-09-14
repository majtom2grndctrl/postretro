// Entity primitive compatibility re-exports.
// See: context/lib/scripting.md

#![allow(unused_imports)]

pub(crate) use crate::scripting::entity_world_primitives::{
    entity_exists, get_entity_property, register_entity_primitives,
};
pub(crate) use postretro_scripting_core::primitive_adapters::NullableString;
