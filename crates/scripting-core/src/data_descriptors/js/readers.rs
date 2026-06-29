// Data-context descriptors: JS field readers and value coercion trait.
// See: context/lib/scripting.md

use super::super::*;

pub fn get_required_string_js<'js>(
    obj: &Object<'js>,
    field: &'static str,
) -> Result<String, DescriptorError> {
    if !obj.contains_key(field).map_err(js_err)? {
        return Err(DescriptorError::MissingField { field });
    }
    let raw: JsValue = obj.get(field).map_err(js_err)?;
    if raw.is_null() || raw.is_undefined() {
        return Err(DescriptorError::MissingField { field });
    }
    String::from_js_value_required(raw, field)
}

/// Read an optional boolean field; returns `Ok(None)` when the key is absent
/// or null/undefined, `Err` when the key is present but the value is not a
/// boolean. Used by descriptor fields that have a meaningful default.
pub fn get_optional_bool_js<'js>(
    obj: &Object<'js>,
    field: &'static str,
) -> Result<Option<bool>, DescriptorError> {
    if !obj.contains_key(field).map_err(js_err)? {
        return Ok(None);
    }
    let raw: JsValue = obj.get(field).map_err(js_err)?;
    if raw.is_null() || raw.is_undefined() {
        return Ok(None);
    }
    raw.as_bool()
        .map(Some)
        .ok_or_else(|| DescriptorError::InvalidShape {
            reason: format!("'{field}' must be a boolean"),
        })
}

/// Read an optional finite f32 field. Returns `Ok(None)` when absent/null,
/// `Err` when present but non-numeric. Numeric values are returned as-is;
/// callers are responsible for range validation.
pub fn get_optional_f32_js<'js>(
    obj: &Object<'js>,
    field: &'static str,
) -> Result<Option<f32>, DescriptorError> {
    if !obj.contains_key(field).map_err(js_err)? {
        return Ok(None);
    }
    let raw: JsValue = obj.get(field).map_err(js_err)?;
    if raw.is_null() || raw.is_undefined() {
        return Ok(None);
    }
    if let Some(i) = raw.as_int() {
        return Ok(Some(i as f32));
    }
    if let Some(f) = raw.as_float() {
        return Ok(Some(f as f32));
    }
    Err(DescriptorError::InvalidShape {
        reason: format!("'{field}' must be a number"),
    })
}

pub fn get_required_f32_js<'js>(
    obj: &Object<'js>,
    field: &'static str,
) -> Result<f32, DescriptorError> {
    if !obj.contains_key(field).map_err(js_err)? {
        return Err(DescriptorError::MissingField { field });
    }
    let raw: JsValue = obj.get(field).map_err(js_err)?;
    if raw.is_null() || raw.is_undefined() {
        return Err(DescriptorError::MissingField { field });
    }
    if let Some(i) = raw.as_int() {
        return Ok(i as f32);
    }
    if let Some(f) = raw.as_float() {
        return Ok(f as f32);
    }
    Err(DescriptorError::InvalidShape {
        reason: format!("'{field}' must be a number"),
    })
}

// Small extension trait so the JS field readers above can coerce a `JsValue`
// into a `String` while reporting a `DescriptorError` on type mismatch.
pub trait FromJsValueRequired: Sized {
    fn from_js_value_required<'js>(
        value: JsValue<'js>,
        field: &'static str,
    ) -> Result<Self, DescriptorError>;
}

impl FromJsValueRequired for String {
    fn from_js_value_required<'js>(
        value: JsValue<'js>,
        field: &'static str,
    ) -> Result<Self, DescriptorError> {
        let s = value
            .as_string()
            .ok_or_else(|| DescriptorError::InvalidShape {
                reason: format!("'{field}' must be a string"),
            })?;
        s.to_string().map_err(js_err)
    }
}
