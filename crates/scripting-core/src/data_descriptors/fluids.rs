// `fluids` manifest drain: each VM converts one entry at a time to JSON and
// both parse it here, so the two runtimes accept and reject the same entries
// with the same diagnostics. Range policy lives on `FluidRegistry`.
// See: context/lib/scripting.md §2 (Mod-init context lifecycle)

use super::player_events::{js_conversion_error, lua_conversion_error};
use super::*;
use postretro_foundation::data_descriptors::{FluidDescriptor, FluidRegistry};

/// Keys a fluid entry may carry. Anything else rejects the entry, so a
/// misspelled optional sound is loud.
const FLUID_KEYS: [&str; 7] = [
    "name",
    "viscosity",
    "viewTint",
    "muffle",
    "surfaceOpacity",
    "enterSound",
    "exitSound",
];

fn shape(reason: String) -> DescriptorError {
    DescriptorError::InvalidShape { reason }
}

fn required<'a>(
    entry: &'a serde_json::Map<String, serde_json::Value>,
    field: &'static str,
) -> Result<&'a serde_json::Value, DescriptorError> {
    match entry.get(field) {
        None | Some(serde_json::Value::Null) => Err(DescriptorError::MissingField { field }),
        Some(value) => Ok(value),
    }
}

fn number(value: &serde_json::Value, field: &str) -> Result<f32, DescriptorError> {
    value
        .as_f64()
        .map(|number| number as f32)
        .ok_or_else(|| shape(format!("`{field}` must be a number")))
}

fn optional_string(
    entry: &serde_json::Map<String, serde_json::Value>,
    field: &str,
) -> Result<Option<String>, DescriptorError> {
    match entry.get(field) {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::String(text)) => Ok(Some(text.clone())),
        Some(_) => Err(shape(format!("`{field}` must be a string"))),
    }
}

/// Parse one converted entry's shape. Ranges are checked by the registry.
fn fluid_from_json(entry: serde_json::Value) -> Result<FluidDescriptor, DescriptorError> {
    let serde_json::Value::Object(entry) = entry else {
        return Err(shape("fluid entry must be an object".into()));
    };
    if let Some(unknown) = entry.keys().find(|key| !FLUID_KEYS.contains(&key.as_str())) {
        return Err(shape(format!("unknown fluid field `{unknown}`")));
    }
    let name = match required(&entry, "name")? {
        serde_json::Value::String(name) => name.clone(),
        _ => return Err(shape("`name` must be a string".into())),
    };
    let tint = match required(&entry, "viewTint")? {
        serde_json::Value::Array(components) if components.len() == 4 => components,
        _ => return Err(shape("`viewTint` must be an array of 4 numbers".into())),
    };
    let mut view_tint = [0.0; 4];
    for (slot, component) in view_tint.iter_mut().zip(tint) {
        *slot = number(component, "viewTint")?;
    }
    Ok(FluidDescriptor {
        name,
        viscosity: number(required(&entry, "viscosity")?, "viscosity")?,
        view_tint,
        muffle: number(required(&entry, "muffle")?, "muffle")?,
        surface_opacity: number(required(&entry, "surfaceOpacity")?, "surfaceOpacity")?,
        enter_sound: optional_string(&entry, "enterSound")?,
        exit_sound: optional_string(&entry, "exitSound")?,
    })
}

fn ignored_field(scope: &str) -> FluidRegistry {
    log::warn!("[Scripting] {scope}: `fluids` must be an array of fluids; ignoring the field");
    FluidRegistry::default()
}

/// Drain `fluids` from a QuickJS manifest object. Entries convert one at a
/// time, so an unconvertible entry costs only itself; a non-array field warns
/// and declares no fluids. Never rejects the manifest.
pub fn drain_fluids_js<'js>(
    ctx: &Ctx<'js>,
    obj: &Object<'js>,
    scope: &str,
) -> Result<FluidRegistry, DescriptorError> {
    if !obj.contains_key("fluids").map_err(js_err)? {
        return Ok(FluidRegistry::default());
    }
    let raw: JsValue = obj.get("fluids").map_err(js_err)?;
    if raw.is_null() || raw.is_undefined() {
        return Ok(FluidRegistry::default());
    }
    let Some(entries) = raw.as_array() else {
        return Ok(ignored_field(scope));
    };
    let mut converted = Vec::with_capacity(entries.len());
    for index in 0..entries.len() {
        let entry: JsValue = entries.get(index).map_err(js_err)?;
        converted.push(
            conv::js_to_json(ctx, entry)
                .map_err(js_conversion_error)
                .and_then(fluid_from_json),
        );
    }
    Ok(FluidRegistry::from_declarations(scope, converted))
}

/// Drain `fluids` from a Luau manifest table. Mirrors [`drain_fluids_js`];
/// diagnostics count entries from 0, matching the QuickJS drain.
pub fn drain_fluids_lua(table: &Table, scope: &str) -> Result<FluidRegistry, DescriptorError> {
    let raw: LuaValue = table.get("fluids").map_err(lua_err)?;
    let entries = match raw {
        LuaValue::Nil => return Ok(FluidRegistry::default()),
        LuaValue::Table(entries) => entries,
        _ => return Ok(ignored_field(scope)),
    };
    let Ok(len) = validate_dense_lua_array(&entries, "`fluids` field") else {
        return Ok(ignored_field(scope));
    };
    let mut converted = Vec::with_capacity(len);
    for slot in 1..=len {
        let entry: LuaValue = entries.get(slot).map_err(lua_err)?;
        converted.push(
            conv::lua_to_json(entry)
                .map_err(lua_conversion_error)
                .and_then(fluid_from_json),
        );
    }
    Ok(FluidRegistry::from_declarations(scope, converted))
}
