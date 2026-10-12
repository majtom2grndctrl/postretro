// `fluids` manifest drain: each VM converts one entry at a time to JSON and
// both parse it here, so the two runtimes judge each entry with the same
// diagnostics. Which keys count as entries differs at the container (see
// `drain_fluids_lua`). Range policy lives on `FluidRegistry`.
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

/// A `fluids` array holds at most this many slots, holes included. Past it the
/// whole field warns once and is ignored, so a sparse `[1000000] = ...` costs
/// neither a warning per hole nor an allocation per slot. Matches the
/// `events` and `triggerPools` container bound; well under
/// [`FluidRegistry::MAX_FLUIDS`].
const MAX_FLUID_CONTAINER_ENTRIES: usize = 4_096;

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

fn oversized_field(scope: &str) -> FluidRegistry {
    log::warn!(
        "[Scripting] {scope}: `fluids` exceeds {MAX_FLUID_CONTAINER_ENTRIES} array slots; ignoring the field"
    );
    FluidRegistry::default()
}

/// Whether a non-array JS value is a plain empty object: `{}` (or a
/// null-prototype object with no own keys). A function, `Map`, `Date`,
/// `Proxy` or other exotic object has no own keys either, but is not a list.
///
/// Never fails, so the check cannot reject the manifest: a value it cannot
/// read is "not a plain empty object", and the field warns and is ignored.
/// A proxy is refused before any of its traps run (rquickjs's
/// `get_prototype` cannot report a throwing `getPrototypeOf` trap), so only an
/// ordinary object, whose prototype read cannot throw, gets that far. The
/// built-in `Object.prototype` comes from a fresh object, never the mutable
/// global `Object`, which a mod may have replaced.
fn is_plain_empty_object<'js>(ctx: &Ctx<'js>, raw: &JsValue<'js>) -> bool {
    if raw.is_proxy() {
        return false;
    }
    let Some(object) = raw.as_object() else {
        return false;
    };
    if object.keys::<String>().next().is_some() {
        return false;
    }
    let Some(prototype) = object.get_prototype() else {
        return true;
    };
    Object::new(ctx.clone())
        .ok()
        .and_then(|plain| plain.get_prototype())
        .is_some_and(|object_prototype| prototype.as_value() == object_prototype.as_value())
}

/// Drain `fluids` from a QuickJS manifest object. Entries convert one at a
/// time, so an unconvertible entry costs only itself; a non-array field, or
/// an array longer than [`MAX_FLUID_CONTAINER_ENTRIES`], warns once and
/// declares no fluids. Never rejects the manifest.
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
        if is_plain_empty_object(ctx, &raw) {
            return Ok(FluidRegistry::default());
        }
        return Ok(ignored_field(scope));
    };
    // Read `length` as a number: past `i32::MAX` QuickJS stores it as a
    // float64, which `Array::len` asserts against. A JS array's length is an
    // integer below 2^32, so the comparison is exact.
    let length: f64 = entries.as_object().get("length").map_err(js_err)?;
    if length > MAX_FLUID_CONTAINER_ENTRIES as f64 {
        return Ok(oversized_field(scope));
    }
    let len = length as usize;
    let mut converted = Vec::with_capacity(len);
    for index in 0..len {
        let entry: JsValue = entries.get(index).map_err(js_err)?;
        converted.push(
            conv::js_to_json(ctx, entry)
                .map_err(js_conversion_error)
                .and_then(|entry| fluid_from_json(index, entry)),
        );
    }
    Ok(FluidRegistry::from_declarations(scope, converted))
}

/// Drain `fluids` from a Luau manifest table. Like [`drain_fluids_js`],
/// every slot up to the highest index is an entry, so a `nil` hole is one
/// malformed entry, as a JS hole is, and its siblings keep their indices.
/// Diagnostics count entries from 0, matching the QuickJS drain. A table
/// keyed past [`MAX_FLUID_CONTAINER_ENTRIES`] warns once and ignores the
/// field, as an over-long JS array does.
///
/// The runtimes diverge on keys that are not slots. A JS array keeps a
/// negative, fractional or named key, and an index at or past 2^32 − 1, as a
/// plain property outside its length, so the drain never sees it and the
/// array's entries stand. Luau has no such split: a table with any key that
/// is not a positive integer is not a sequence, so the whole field warns that
/// it must be an array and is ignored.
pub fn drain_fluids_lua(table: &Table, scope: &str) -> Result<FluidRegistry, DescriptorError> {
    let raw: LuaValue = table.get("fluids").map_err(lua_err)?;
    let entries = match raw {
        LuaValue::Nil => return Ok(FluidRegistry::default()),
        LuaValue::Table(entries) => entries,
        _ => return Ok(ignored_field(scope)),
    };
    let len = match lua_sequence_extent(&entries)? {
        SequenceExtent::Slots(len) => len,
        SequenceExtent::NotASequence => return Ok(ignored_field(scope)),
        SequenceExtent::Oversized => return Ok(oversized_field(scope)),
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

/// How many slots a Luau `fluids` table spans.
enum SequenceExtent {
    /// The highest positive integer key, holes allowed, as a JS array's
    /// length counts its holes.
    Slots(usize),
    /// Some key is not a positive integer.
    NotASequence,
    /// Some key is past [`MAX_FLUID_CONTAINER_ENTRIES`].
    Oversized,
}

fn lua_sequence_extent(entries: &Table) -> Result<SequenceExtent, DescriptorError> {
    let mut extent = 0_usize;
    for pair in entries.clone().pairs::<LuaValue, LuaValue>() {
        let (key, _) = pair.map_err(lua_err)?;
        let LuaValue::Integer(index) = key else {
            return Ok(SequenceExtent::NotASequence);
        };
        if index < 1 {
            return Ok(SequenceExtent::NotASequence);
        }
        let Some(index) = usize::try_from(index)
            .ok()
            .filter(|&index| index <= MAX_FLUID_CONTAINER_ENTRIES)
        else {
            return Ok(SequenceExtent::Oversized);
        };
        extent = extent.max(index);
    }
    Ok(SequenceExtent::Slots(extent))
}
