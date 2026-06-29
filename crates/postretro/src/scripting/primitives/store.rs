// State-store primitive compatibility re-exports.
// See: context/lib/scripting.md §5

#![allow(unused_imports)]

pub(crate) use crate::scripting::state_store::{
    TextEdit, apply_store_slot_batch, apply_text_edit, read_store_slot, register_store_primitives,
    store_declaration, store_declaration_from_manifest_value, store_declaration_set_from_values,
    write_state_slot_json, write_store_slot,
};
pub(crate) use postretro_scripting_core::primitive_adapters::{
    Any, ScriptSlotValue, StoreDeclarationManifest, StoreDefinition, StoreSchemaJson,
    StoreStateRefs,
};
pub(crate) use postretro_scripting_core::store_bridge::{
    drain_store_declarations_js, drain_store_declarations_lua,
};
