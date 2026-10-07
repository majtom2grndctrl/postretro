// QuickJS drain for the manifest's optional `input` block. Classifies raw JS
// values; the shared resolver owns every rule and warning.
// See: context/lib/scripting.md §1 · context/lib/input.md §2

use super::super::input_block::{
    AuthoredValue, MAX_INPUT_CONTAINER_DEPTH, resolve_authored_input_block,
};
use super::super::*;

/// Drain the optional `input` block. Absent, `null`, or `undefined` yields
/// `None`. Malformed parts warn and degrade (the `audio` precedent); only a VM
/// access failure is an error.
pub fn drain_input_block_js<'js>(
    obj: &Object<'js>,
    scope: &str,
) -> Result<Option<ModInputBlock>, DescriptorError> {
    if !obj.contains_key("input").map_err(js_err)? {
        return Ok(None);
    }
    let raw: JsValue = obj.get("input").map_err(js_err)?;
    Ok(resolve_authored_input_block(
        scope,
        authored_value_js(raw, 0)?,
    ))
}

fn authored_value_js(value: JsValue<'_>, depth: usize) -> Result<AuthoredValue, DescriptorError> {
    if value.is_null() || value.is_undefined() {
        return Ok(AuthoredValue::Absent);
    }
    if let Some(flag) = value.as_bool() {
        return Ok(AuthoredValue::Bool(flag));
    }
    if let Some(number) = value.as_number() {
        return Ok(AuthoredValue::Number(number));
    }
    // A string that is not valid UTF-8 (a lone surrogate) degrades like any
    // other unusable value, as the Luau drain does, rather than failing the
    // manifest.
    if let Some(string) = value.as_string() {
        return Ok(match string.to_string() {
            Ok(string) => AuthoredValue::String(string),
            Err(_) => AuthoredValue::Other,
        });
    }
    if value.is_function() || depth > MAX_INPUT_CONTAINER_DEPTH {
        return Ok(AuthoredValue::Other);
    }
    if let Some(array) = value.as_array() {
        let mut items = Vec::with_capacity(array.len());
        for index in 0..array.len() {
            let item: JsValue = array.get(index).map_err(js_err)?;
            items.push(authored_value_js(item, depth + 1)?);
        }
        return Ok(AuthoredValue::Array(items));
    }
    if let Some(object) = value.as_object() {
        let mut entries = Vec::new();
        // Keys arrive as JS values so a key that is not valid UTF-8 makes the
        // whole object unusable, as a Luau table with such a key is.
        for entry in object.props::<JsValue, JsValue>() {
            let (key, item) = entry.map_err(js_err)?;
            let Some(Ok(key)) = key.as_string().map(|key| key.to_string()) else {
                return Ok(AuthoredValue::Other);
            };
            entries.push((key, authored_value_js(item, depth + 1)?));
        }
        return Ok(AuthoredValue::Object(entries));
    }
    Ok(AuthoredValue::Other)
}
