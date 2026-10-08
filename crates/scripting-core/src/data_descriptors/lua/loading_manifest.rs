// Luau drains for the manifest's `uiImages` map and `loading` block.
// Classifies raw Luau values; the shared resolver owns every rule and warning.
// See: context/lib/ui.md · context/lib/scripting.md §1

use std::collections::BTreeMap;

use super::super::input_block::AuthoredValue;
use super::super::loading_manifest::{resolve_loading, resolve_tree_pool, resolve_ui_images};
use super::super::*;
use super::input_block::authored_value_lua;

/// Luau twin of [`drain_ui_images_js`].
pub fn drain_ui_images_lua(
    table: &Table,
    scope: &str,
) -> Result<BTreeMap<String, String>, DescriptorError> {
    Ok(resolve_ui_images(
        scope,
        authored_field_lua(table, "uiImages")?,
    ))
}

/// Luau twin of [`drain_loading_js`].
pub fn drain_loading_lua(table: &Table, scope: &str) -> Result<ModLoading, DescriptorError> {
    Ok(resolve_loading(
        scope,
        authored_field_lua(table, "loading")?,
    ))
}

/// Luau twin of [`loading_tree_from_js`].
pub(crate) fn loading_tree_from_lua(
    table: &Table,
    scope: &str,
    path: &str,
) -> Result<Vec<String>, DescriptorError> {
    Ok(resolve_tree_pool(
        scope,
        &format!("{path}.loadingTree"),
        authored_field_lua(table, "loadingTree")?,
    ))
}

fn authored_field_lua(table: &Table, field: &str) -> Result<AuthoredValue, DescriptorError> {
    let raw: LuaValue = table.get(field).map_err(lua_err)?;
    authored_value_lua(raw, 0)
}
