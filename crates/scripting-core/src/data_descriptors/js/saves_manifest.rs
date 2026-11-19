// QuickJS drain for the manifest's `saves` block. Classifies the raw JS value;
// the shared resolver owns every rule and warning.
// See: context/lib/scripting.md §5 (Durable State Store)

use super::super::saves_manifest::resolve_saves;
use super::super::*;
use super::loading_manifest::authored_field_js;

/// Drain the optional `saves` block. `None` when the manifest has none.
/// Malformed counts warn and are clamped or defaulted; only a VM access
/// failure is an error.
pub fn drain_saves_js<'js>(
    obj: &Object<'js>,
    scope: &str,
) -> Result<Option<SavesManifest>, DescriptorError> {
    Ok(resolve_saves(scope, authored_field_js(obj, "saves")?))
}
