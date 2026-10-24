// Container drain shared by the warn-and-drop manifest declaration lists
// (`fluids`, `ghostStyles`): each VM converts one entry at a time to JSON, so
// both runtimes judge each entry with one parser and one set of diagnostics.
// See: context/lib/scripting.md §2 (Context Model)

use super::player_events::{js_conversion_error, lua_conversion_error};
use super::*;

/// A declaration list holds at most this many slots, holes included. Past it
/// the whole field warns once and is ignored, so a sparse `[1000000] = ...`
/// costs neither a warning per hole nor an allocation per slot. Matches the
/// `events` and `triggerPools` container bound.
pub(super) const MAX_DECLARATION_LIST_ENTRIES: usize = 4_096;

/// Which manifest list is draining, for the field-level diagnostics.
#[derive(Clone, Copy)]
pub(super) struct DeclarationList {
    /// The manifest key, e.g. `fluids`.
    pub field: &'static str,
    /// What its entries are, plural, e.g. `fluids`.
    pub entries: &'static str,
}

/// One entry, as converted JSON, or its conversion failure.
pub(super) type ConvertedEntry = Result<serde_json::Value, DescriptorError>;

impl DeclarationList {
    fn ignored(self, scope: &str) -> Vec<ConvertedEntry> {
        log::warn!(
            "[Scripting] {scope}: `{}` must be an array of {}; ignoring the field",
            self.field,
            self.entries,
        );
        Vec::new()
    }

    fn oversized(self, scope: &str) -> Vec<ConvertedEntry> {
        log::warn!(
            "[Scripting] {scope}: `{}` exceeds {MAX_DECLARATION_LIST_ENTRIES} array slots; ignoring the field",
            self.field,
        );
        Vec::new()
    }
}

pub(super) fn shape(reason: String) -> DescriptorError {
    DescriptorError::InvalidShape { reason }
}

/// One entry's field path for diagnostics: `<list>[index].<field>`. Every
/// optional reader treats `null` (a Luau `nil`) as omitted.
pub(super) struct EntryPath {
    list: &'static str,
    index: usize,
}

impl EntryPath {
    pub(super) fn new(list: &'static str, index: usize) -> Self {
        Self { list, index }
    }

    pub(super) fn field(&self, field: &str) -> String {
        format!("{}[{}].{field}", self.list, self.index)
    }

    pub(super) fn required<'a>(
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

    pub(super) fn number(
        &self,
        value: &serde_json::Value,
        field: &str,
    ) -> Result<f32, DescriptorError> {
        value
            .as_f64()
            .map(|number| number as f32)
            .ok_or_else(|| shape(format!("`{}` must be a number", self.field(field))))
    }

    pub(super) fn optional_string(
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

    pub(super) fn optional_number(
        &self,
        entry: &serde_json::Map<String, serde_json::Value>,
        field: &str,
    ) -> Result<Option<f32>, DescriptorError> {
        match entry.get(field) {
            None | Some(serde_json::Value::Null) => Ok(None),
            Some(value) => self.number(value, field).map(Some),
        }
    }

    pub(super) fn optional_bool(
        &self,
        entry: &serde_json::Map<String, serde_json::Value>,
        field: &str,
    ) -> Result<Option<bool>, DescriptorError> {
        match entry.get(field) {
            None | Some(serde_json::Value::Null) => Ok(None),
            Some(serde_json::Value::Bool(value)) => Ok(Some(*value)),
            Some(_) => Err(shape(format!("`{}` must be a boolean", self.field(field)))),
        }
    }

    /// An optional `[r, g, b]`: exactly three numbers.
    pub(super) fn optional_rgb(
        &self,
        entry: &serde_json::Map<String, serde_json::Value>,
        field: &str,
    ) -> Result<Option<[f32; 3]>, DescriptorError> {
        let components = match entry.get(field) {
            None | Some(serde_json::Value::Null) => return Ok(None),
            Some(serde_json::Value::Array(components)) if components.len() == 3 => components,
            Some(_) => {
                return Err(shape(format!(
                    "`{}` must be an array of 3 numbers",
                    self.field(field)
                )));
            }
        };
        let mut rgb = [0.0; 3];
        for (slot, component) in rgb.iter_mut().zip(components) {
            *slot = self.number(component, field)?;
        }
        Ok(Some(rgb))
    }
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

/// Convert a QuickJS manifest list's entries, one at a time, so an
/// unconvertible entry costs only itself. An absent or nullish field is no
/// entries; a non-array field, or an array longer than
/// [`MAX_DECLARATION_LIST_ENTRIES`], warns once and is no entries. Errors
/// only on a VM failure, never on authored content.
pub(super) fn convert_declaration_list_js<'js>(
    ctx: &Ctx<'js>,
    obj: &Object<'js>,
    list: DeclarationList,
    scope: &str,
) -> Result<Vec<ConvertedEntry>, DescriptorError> {
    if !obj.contains_key(list.field).map_err(js_err)? {
        return Ok(Vec::new());
    }
    let raw: JsValue = obj.get(list.field).map_err(js_err)?;
    if raw.is_null() || raw.is_undefined() {
        return Ok(Vec::new());
    }
    let Some(entries) = raw.as_array() else {
        // An empty object reads as an empty list, as an empty Luau table does.
        if is_plain_empty_object(ctx, &raw) {
            return Ok(Vec::new());
        }
        return Ok(list.ignored(scope));
    };
    // Read `length` as a number: past `i32::MAX` QuickJS stores it as a
    // float64, which `Array::len` asserts against. A JS array's length is an
    // integer below 2^32, so the comparison is exact.
    let length: f64 = entries.as_object().get("length").map_err(js_err)?;
    if length > MAX_DECLARATION_LIST_ENTRIES as f64 {
        return Ok(list.oversized(scope));
    }
    let len = length as usize;
    let mut converted = Vec::with_capacity(len);
    for index in 0..len {
        let entry: JsValue = entries.get(index).map_err(js_err)?;
        converted.push(conv::js_to_json(ctx, entry).map_err(js_conversion_error));
    }
    Ok(converted)
}

/// Convert a Luau manifest list's entries. Like
/// [`convert_declaration_list_js`], every slot up to the highest index is an
/// entry, so a `nil` hole is one malformed entry, as a JS hole is, and its
/// siblings keep their indices; diagnostics count entries from 0 in both. A
/// table keyed past [`MAX_DECLARATION_LIST_ENTRIES`] warns once and is no
/// entries, as an over-long JS array is.
///
/// The runtimes diverge on keys that are not slots. A JS array keeps a
/// negative, fractional or named key, and an index at or past 2^32 − 1, as a
/// plain property outside its length, so the drain never sees it and the
/// array's entries stand. Luau has no such split: a table with any key that
/// is not a positive integer is not a sequence, so the whole field warns that
/// it must be an array and is ignored.
pub(super) fn convert_declaration_list_lua(
    table: &Table,
    list: DeclarationList,
    scope: &str,
) -> Result<Vec<ConvertedEntry>, DescriptorError> {
    let raw: LuaValue = table.get(list.field).map_err(lua_err)?;
    let entries = match raw {
        LuaValue::Nil => return Ok(Vec::new()),
        LuaValue::Table(entries) => entries,
        _ => return Ok(list.ignored(scope)),
    };
    let len = match lua_sequence_extent(&entries)? {
        SequenceExtent::Slots(len) => len,
        SequenceExtent::NotASequence => return Ok(list.ignored(scope)),
        SequenceExtent::Oversized => return Ok(list.oversized(scope)),
    };
    let mut converted = Vec::with_capacity(len);
    for slot in 1..=len {
        let entry: LuaValue = entries.get(slot).map_err(lua_err)?;
        converted.push(conv::lua_to_json(entry).map_err(lua_conversion_error));
    }
    Ok(converted)
}

/// How many slots a Luau list table spans.
enum SequenceExtent {
    /// The highest positive integer key, holes allowed, as a JS array's
    /// length counts its holes.
    Slots(usize),
    /// Some key is not a positive integer.
    NotASequence,
    /// Some key is past [`MAX_DECLARATION_LIST_ENTRIES`].
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
            .filter(|&index| index <= MAX_DECLARATION_LIST_ENTRIES)
        else {
            return Ok(SequenceExtent::Oversized);
        };
        extent = extent.max(index);
    }
    Ok(SequenceExtent::Slots(extent))
}
