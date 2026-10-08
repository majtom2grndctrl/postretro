// Fixed-payload validation for consequential primitives — resource grants and
// owner-slot additions — shared by the QuickJS and Luau reaction converters.
// A reaction body and a sequence entry reject the same payload with the same
// diagnostic, so no malformed grant reaches a reaction handler.
// See: context/lib/scripting.md §12 (Entity addressing)

use postretro_entities::GroupKind;

use super::super::DescriptorError;
use super::foundation::validate_ascii_identifier;

/// Primitives whose `args` are fixed author-time payloads checked at load.
/// Downstream grant dispatch assumes these checks ran.
pub fn is_consequential_primitive(primitive: &str) -> bool {
    matches!(primitive, "grantHealth" | "grantAmmo" | "addSlot")
}

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

/// Validate a consequential primitive's `args` at `site` (`"primitive"`,
/// `"sequence step 2"`). Non-consequential primitives pass unchecked. The
/// reason names the reaction so the caller's skip or reject points at it.
pub fn validate_consequential_args(
    reaction: &str,
    site: &str,
    primitive: &str,
    args: &serde_json::Value,
) -> Result<(), String> {
    if !is_consequential_primitive(primitive) {
        return Ok(());
    }
    let fail = |detail: String| format!("reaction `{reaction}` {site}: {detail}");

    let object = args
        .as_object()
        .ok_or_else(|| fail(format!("`{primitive}` `args` must be an object")))?;
    if primitive == "addSlot" {
        if object
            .get("slot")
            .and_then(serde_json::Value::as_str)
            .is_none()
        {
            return Err(fail("`addSlot` `args.slot` must be a string".to_string()));
        }
        return finite_f32(object.get("delta"))
            .map_err(|detail| fail(format!("`addSlot` `args.delta` {detail}")));
    }

    finite_f32(object.get("amount"))
        .map_err(|detail| fail(format!("`{primitive}` `args.amount` {detail}")))?;
    if primitive == "grantAmmo" {
        let Some(ammo_type) = object.get("type").and_then(serde_json::Value::as_str) else {
            return Err(fail("`grantAmmo` `args.type` must be a string".to_string()));
        };
        validate_ascii_identifier("grantAmmo.type", ammo_type).map_err(|error| match error {
            DescriptorError::InvalidShape { reason } => fail(reason),
            other => fail(other.to_string()),
        })?;
    }
    Ok(())
}

/// A payload number must be finite in f64 and still finite once narrowed to
/// the f32 the handlers store.
fn finite_f32(value: Option<&serde_json::Value>) -> Result<(), &'static str> {
    let Some(number) = value.and_then(serde_json::Value::as_f64) else {
        return Err("must be a finite number");
    };
    if !number.is_finite() || !(number as f32).is_finite() {
        return Err("must be a finite number representable as f32");
    }
    Ok(())
}
