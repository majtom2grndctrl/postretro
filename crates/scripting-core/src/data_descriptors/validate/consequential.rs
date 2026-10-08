// Recipient check for consequential reaction bodies (payload check: foundation).
// See: context/lib/scripting.md §12 (Entity addressing)

use postretro_entities::GroupKind;

pub use postretro_foundation::data_descriptors::validate::consequential::*;

use super::super::DescriptorError;

/// Validate a consequential reaction body: exactly one recipient form, then
/// its `args`. Errors name the reaction.
pub fn validate_consequential_reaction(
    reaction: &str,
    primitive: &str,
    kind: Option<GroupKind>,
    tag: Option<&str>,
    target: Option<&str>,
    args: &serde_json::Value,
) -> Result<(), DescriptorError> {
    if !is_consequential_primitive(primitive) {
        return Ok(());
    }

    // A group `kind` addresses its recipients with or without a tag filter.
    let has_recipients = kind.is_some() || tag.is_some_and(|tag| !tag.is_empty());
    if !matches!(
        (has_recipients, target),
        (true, None) | (false, Some("@activators"))
    ) {
        return Err(DescriptorError::InvalidShape {
            reason: format!(
                "reaction `{reaction}` primitive: `{primitive}` requires exactly one of a group `kind`, a non-empty `tag`, or target `@activators`"
            ),
        });
    }

    validate_consequential_args(reaction, "primitive", primitive, args)
        .map_err(|reason| DescriptorError::InvalidShape { reason })
}
