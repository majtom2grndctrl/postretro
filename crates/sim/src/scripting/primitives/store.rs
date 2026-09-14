// State-store primitive compatibility re-exports.
// See: context/lib/scripting.md §5

#![allow(unused_imports)]

pub use crate::scripting::state_store::{
    TextEdit, apply_store_slot_batch, apply_text_edit, write_state_slot_json, write_store_slot,
};
pub(crate) use crate::scripting::state_store::{
    read_store_slot, register_store_primitives, store_declaration_from_manifest_value,
    store_declaration_set_from_values,
};
pub(crate) use postretro_scripting_core::primitive_adapters::{
    Any, ScriptSlotValue, StoreDeclarationManifest, StoreDefinition, StoreSchemaJson,
    StoreStateRefs,
};
#[cfg(any(test, feature = "test-support"))]
pub use postretro_scripting_core::store_bridge::store_declaration;
pub(crate) use postretro_scripting_core::store_bridge::{
    drain_store_declarations_js, drain_store_declarations_lua,
};
