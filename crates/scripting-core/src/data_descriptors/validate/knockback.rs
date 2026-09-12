//! Knockback objects cross the JSON bridge as named fields, never positional arrays.
use super::super::DescriptorError;

pub fn validate_knockback_object(
    value: &serde_json::Value,
    path: &str,
) -> Result<(), DescriptorError> {
    if !value.is_object() {
        return Err(DescriptorError::InvalidShape {
            reason: format!("`{path}` must be an object"),
        });
    }
    Ok(())
}

pub fn validate_optional_knockback_object(
    parent: &serde_json::Value,
    path: &str,
) -> Result<(), DescriptorError> {
    if let Some(value) = parent.get("knockback").filter(|value| !value.is_null()) {
        validate_knockback_object(value, path)?;
    }
    Ok(())
}
