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

/// One entry's field path for diagnostics: `fluids[index].field`.
struct EntryPath {
    index: usize,
}

impl EntryPath {
    fn field(&self, field: &str) -> String {
        format!("fluids[{}].{field}", self.index)
    }

    fn required<'a>(
        &self,
        entry: &'a serde_json::Map<String, serde_json::Value>,
        field: &str,
    ) -> Result<&'a serde_json::Value, DescriptorError> {
        match entry.get(field) {
            None | Some(serde_json::Value::Null) => {
                Err(shape(format!("`{}` is required", self.field(field))))
            }
            Some(value) => Ok(value),
        }
    }

    fn number(&self, value: &serde_json::Value, field: &str) -> Result<f32, DescriptorError> {
        value
            .as_f64()
            .map(|number| number as f32)
            .ok_or_else(|| shape(format!("`{}` must be a number", self.field(field))))
    }

    fn optional_string(
        &self,
        entry: &serde_json::Map<String, serde_json::Value>,
        field: &str,
    ) -> Result<Option<String>, DescriptorError> {
        match entry.get(field) {
            None | Some(serde_json::Value::Null) => Ok(None),
            Some(serde_json::Value::String(text)) => Ok(Some(text.clone())),
            Some(_) => Err(shape(format!("`{}` must be a string", self.field(field)))),
        }
    }
}

/// Parse one converted entry's shape. Ranges are checked by the registry.
/// Diagnostics name the field as `fluids[index].<field>`; `index` counts
/// entries from 0 in both runtimes. A hole (`null`, `undefined` or `nil`) is a
/// malformed entry like any other non-object.
fn fluid_from_json(
    index: usize,
    entry: serde_json::Value,
) -> Result<FluidDescriptor, DescriptorError> {
    let path = EntryPath { index };
    let serde_json::Value::Object(entry) = entry else {
        return Err(shape(format!("`fluids[{index}]` must be a fluid object")));
    };
    if let Some(unknown) = entry.keys().find(|key| !FLUID_KEYS.contains(&key.as_str())) {
        return Err(shape(format!(
            "unknown fluid field `{}`",
            path.field(unknown)
        )));
    }
    let name = match path.required(&entry, "name")? {
        serde_json::Value::String(name) => name.clone(),
        _ => return Err(shape(format!("`{}` must be a string", path.field("name")))),
    };
    let tint = match path.required(&entry, "viewTint")? {
        serde_json::Value::Array(components) if components.len() == 4 => components,
        _ => {
            return Err(shape(format!(
                "`{}` must be an array of 4 numbers",
                path.field("viewTint")
            )));
        }
    };
    let mut view_tint = [0.0; 4];
    for (slot, component) in view_tint.iter_mut().zip(tint) {
        *slot = path.number(component, "viewTint")?;
    }
    Ok(FluidDescriptor {
        name,
        viscosity: path.number(path.required(&entry, "viscosity")?, "viscosity")?,
        view_tint,
        muffle: path.number(path.required(&entry, "muffle")?, "muffle")?,
        surface_opacity: path.number(path.required(&entry, "surfaceOpacity")?, "surfaceOpacity")?,
        enter_sound: path.optional_string(&entry, "enterSound")?,
        exit_sound: path.optional_string(&entry, "exitSound")?,
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
        // An empty object reads as an empty list, as an empty Luau table does.
        if raw
            .as_object()
            .is_some_and(|object| object.keys::<String>().next().is_none())
        {
            return Ok(FluidRegistry::default());
        }
        return Ok(ignored_field(scope));
    };
    let mut converted = Vec::with_capacity(entries.len());
    for index in 0..entries.len() {
        let entry: JsValue = entries.get(index).map_err(js_err)?;
        converted.push(
            conv::js_to_json(ctx, entry)
                .map_err(js_conversion_error)
                .and_then(|entry| fluid_from_json(index, entry)),
        );
    }
    Ok(FluidRegistry::from_declarations(scope, converted))
}

/// Drain `fluids` from a Luau manifest table. Mirrors [`drain_fluids_js`]:
/// every slot up to the highest index is an entry, so a `nil` hole is one
/// malformed entry, as a JS hole is, and its siblings keep their indices.
/// Diagnostics count entries from 0, matching the QuickJS drain. Only a
/// non-sequence table (a non-integer or non-positive key) ignores the field.
pub fn drain_fluids_lua(table: &Table, scope: &str) -> Result<FluidRegistry, DescriptorError> {
    let raw: LuaValue = table.get("fluids").map_err(lua_err)?;
    let entries = match raw {
        LuaValue::Nil => return Ok(FluidRegistry::default()),
        LuaValue::Table(entries) => entries,
        _ => return Ok(ignored_field(scope)),
    };
    let Some(len) = lua_sequence_extent(&entries)? else {
        return Ok(ignored_field(scope));
    };
    let mut converted = Vec::with_capacity(len);
    for slot in 1..=len {
        let entry: LuaValue = entries.get(slot).map_err(lua_err)?;
        converted.push(
            conv::lua_to_json(entry)
                .map_err(lua_conversion_error)
                .and_then(|entry| fluid_from_json(slot - 1, entry)),
        );
    }
    Ok(FluidRegistry::from_declarations(scope, converted))
}

/// The highest positive integer key of a Luau sequence, holes allowed, as a
/// JS array's length counts its holes; `None` when any key is not a positive
/// integer.
fn lua_sequence_extent(entries: &Table) -> Result<Option<usize>, DescriptorError> {
    let mut extent = 0_usize;
    for pair in entries.clone().pairs::<LuaValue, LuaValue>() {
        let (key, _) = pair.map_err(lua_err)?;
        let LuaValue::Integer(index) = key else {
            return Ok(None);
        };
        let Ok(index) = usize::try_from(index) else {
            return Ok(None);
        };
        if index == 0 {
            return Ok(None);
        }
        extent = extent.max(index);
    }
    Ok(Some(extent))
}
