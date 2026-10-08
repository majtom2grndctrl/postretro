// QuickJS drains for the manifest's `uiImages` map and `loading` block.
// Classifies raw JS values; the shared resolver owns every rule and warning.
// See: context/lib/ui.md · context/lib/scripting.md §1

use std::collections::BTreeMap;

use super::super::input_block::AuthoredValue;
use super::super::loading_manifest::{resolve_loading, resolve_tree_pool, resolve_ui_images};
use super::super::*;
use super::input_block::authored_value_js;

/// Drain the optional `uiImages` map (image key → mod-relative PNG path).
/// Malformed entries warn and are skipped; only a VM access failure is an
/// error.
pub fn drain_ui_images_js<'js>(
    obj: &Object<'js>,
    scope: &str,
) -> Result<BTreeMap<String, String>, DescriptorError> {
    Ok(resolve_ui_images(
        scope,
        authored_field_js(obj, "uiImages")?,
    ))
}

/// Drain the optional `loading` block. Malformed parts warn and degrade to
/// no mod pool; only a VM access failure is an error.
pub fn drain_loading_js<'js>(
    obj: &Object<'js>,
    scope: &str,
) -> Result<ModLoading, DescriptorError> {
    Ok(resolve_loading(scope, authored_field_js(obj, "loading")?))
}

/// Read a map catalog entry's optional `loadingTree` pool. `path` names the
/// entry in warnings (`maps[2]`).
pub(crate) fn loading_tree_from_js<'js>(
    obj: &Object<'js>,
    scope: &str,
    path: &str,
) -> Result<Vec<String>, DescriptorError> {
    Ok(resolve_tree_pool(
        scope,
        &format!("{path}.loadingTree"),
        authored_field_js(obj, "loadingTree")?,
    ))
}

fn authored_field_js<'js>(
    obj: &Object<'js>,
    field: &str,
) -> Result<AuthoredValue, DescriptorError> {
    if !obj.contains_key(field).map_err(js_err)? {
        return Ok(AuthoredValue::Absent);
    }
    let raw: JsValue = obj.get(field).map_err(js_err)?;
    authored_value_js(raw, 0)
}
