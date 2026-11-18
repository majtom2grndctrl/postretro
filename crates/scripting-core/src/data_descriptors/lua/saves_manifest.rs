// Luau drain for the manifest's `saves` block. Classifies the raw Luau value;
// the shared resolver owns every rule and warning.
// See: context/lib/scripting.md §5 (Durable State Store)

use super::super::saves_manifest::resolve_saves;
use super::super::*;
use super::loading_manifest::authored_field_lua;

/// Luau twin of [`drain_saves_js`].
pub fn drain_saves_lua(
    table: &Table,
    scope: &str,
) -> Result<Option<SavesManifest>, DescriptorError> {
    Ok(resolve_saves(scope, authored_field_lua(table, "saves")?))
}
