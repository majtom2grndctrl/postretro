// Pure-JSON payload check for consequential primitives (grants, `addSlot`).
// See: context/lib/scripting.md §12 (Entity addressing)

use super::super::DescriptorError;
use super::foundation::validate_ascii_identifier;

/// Primitives whose `args` are fixed author-time payloads checked at load.
/// Downstream grant dispatch assumes these checks ran.
pub fn is_consequential_primitive(primitive: &str) -> bool {
    matches!(primitive, "grantHealth" | "grantAmmo" | "addSlot")
}

/// Validate a consequential primitive's `args` at `site` (`"primitive"`,
/// `"sequence step 2"`). Non-consequential primitives pass unchecked. The
/// reason names the reaction so the caller's skip or reject points at it.
///
/// Both VM converters and the build-time light pass call this one check, so
/// a reaction the runtime drops for a bad sequence step reserves no light. A
/// bad primitive body is different: the runtime rejects the whole manifest,
/// while the light pass still reserves for that manifest's other reactions.
pub fn validate_consequential_args(
    reaction: &str,
    site: &str,
    primitive: &str,
    args: &serde_json::Value,
) -> Result<(), String> {
    if primitive == "startTrap" {
        return validate_start_trap_args(args)
            .map_err(|detail| format!("reaction `{reaction}` {site}: {detail}"));
    }
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

/// A trap `start`'s per-run options: absent, or an object whose optional
/// `intervalMs` is finite and positive and whose optional `phaseMs` is finite
/// and non-negative. A literal outside that rejects the reaction at install, as
/// the same value on the map KVP leaves the trap inert.
fn validate_start_trap_args(args: &serde_json::Value) -> Result<(), String> {
    let object = match args {
        serde_json::Value::Null => return Ok(()),
        serde_json::Value::Object(object) => object,
        _ => return Err("`startTrap` `args` must be an object".to_string()),
    };
    for (key, positive) in [("intervalMs", true), ("phaseMs", false)] {
        let Some(value) = object.get(key) else {
            continue;
        };
        finite_f32(Some(value)).map_err(|detail| format!("`startTrap` `args.{key}` {detail}"))?;
        let number = value.as_f64().expect("finite_f32 admitted a number");
        if positive && number <= 0.0 {
            return Err(format!(
                "`startTrap` `args.{key}` must be positive, got {number}"
            ));
        }
        if !positive && number < 0.0 {
            return Err(format!(
                "`startTrap` `args.{key}` must not be negative, got {number}"
            ));
        }
    }
    if let Some(unknown) = object
        .keys()
        .find(|key| !matches!(key.as_str(), "intervalMs" | "phaseMs"))
    {
        return Err(format!(
            "`startTrap` `args.{unknown}` is not a trap start option"
        ));
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
