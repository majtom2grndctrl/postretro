// Group-target wire validation shared by the QuickJS and Luau reaction
// converters, so both runtimes reject the same malformed `kind` with the same
// diagnostic.
// See: context/lib/scripting.md §12 (Entity addressing)

use postretro_entities::GroupKind;

/// An authored `kind` field, lowered from either VM before validation.
#[derive(Debug, Clone)]
pub enum AuthoredKind {
    /// The key is absent, `null`, `undefined` or `nil`: the raw kindless path.
    Absent,
    Text(String),
    /// Any non-string value, named by its VM type for the diagnostic.
    NonString(String),
}

/// Validate one descriptor's `kind` against the fields beside it. `site` names
/// the entry inside the reaction (`"primitive"`, `"sequence step 2"`). An entry
/// carrying both an id (or a sentinel target) and a `kind` is rejected: the two
/// address different things, and a silent winner would hide the author's
/// mistake. Errors name the reaction so the skip warning points at it.
pub fn validate_authored_group_kind(
    reaction: &str,
    site: &str,
    kind: AuthoredKind,
    has_id: bool,
    has_target: bool,
) -> Result<Option<GroupKind>, String> {
    let kind = match kind {
        AuthoredKind::Absent => return Ok(None),
        AuthoredKind::Text(spelling) => GroupKind::from_wire(&spelling).ok_or_else(|| {
            format!(
                "reaction `{reaction}` {site}: `kind` must be \"npc\" or \"player\", got \"{spelling}\""
            )
        })?,
        AuthoredKind::NonString(type_name) => {
            return Err(format!(
                "reaction `{reaction}` {site}: `kind` must be \"npc\" or \"player\", got a {type_name}"
            ));
        }
    };
    if has_id {
        return Err(format!(
            "reaction `{reaction}` {site}: an entry cannot carry both `id` and `kind`"
        ));
    }
    if has_target {
        return Err(format!(
            "reaction `{reaction}` {site}: an entry cannot carry both `target` and `kind`"
        ));
    }
    Ok(Some(kind))
}
