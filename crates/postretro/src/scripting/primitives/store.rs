// State-store declaration, read/write primitives, and engine accessors.
// See: context/lib/scripting.md §5 "Durable State Store"

use std::collections::{BTreeMap, BTreeSet};

use mlua::{FromLua, IntoLua, Lua, Table as LuaTable, Value as LuaValue};
use rquickjs::{Array, Ctx, FromJs, IntoJs, Object as JsObject, Value as JsValue};
use serde::Deserialize;
use serde_json::Value;

use crate::scripting::conv::{js_to_json, json_to_js, json_to_lua, lua_to_json};
use crate::scripting::ctx::ScriptCtx;
use crate::scripting::error::ScriptError;
use crate::scripting::primitives_registry::{ContextScope, PrimitiveRegistry};
use crate::scripting::slot_table::{
    NumericRange, SlotOwnership, SlotRecord, SlotSchema, SlotType, SlotValue, StoreDeclaration,
    StoreDeclarationSet,
};

struct StoreSchemaJson(Value);

impl<'js> FromJs<'js> for StoreSchemaJson {
    fn from_js(ctx: &Ctx<'js>, value: JsValue<'js>) -> rquickjs::Result<Self> {
        js_to_json(ctx, value).map(Self)
    }
}

impl FromLua for StoreSchemaJson {
    fn from_lua(value: LuaValue, _lua: &Lua) -> mlua::Result<Self> {
        lua_to_json(value).map(Self)
    }
}

#[derive(Clone, Debug)]
enum ScriptSlotValue {
    Number(f64),
    Boolean(bool),
    String(String),
    Array(Vec<f64>),
    Unsupported(&'static str),
}

impl<'js> FromJs<'js> for ScriptSlotValue {
    fn from_js(ctx: &Ctx<'js>, value: JsValue<'js>) -> rquickjs::Result<Self> {
        if let Some(value) = value.as_bool() {
            return Ok(Self::Boolean(value));
        }
        if let Some(value) = value.as_int() {
            return Ok(Self::Number(f64::from(value)));
        }
        if let Some(value) = value.as_float() {
            return Ok(Self::Number(value));
        }
        if let Some(value) = value.as_string() {
            return Ok(Self::String(value.to_string()?));
        }
        if let Some(array) = value.as_array() {
            let mut values = Vec::with_capacity(array.len());
            for index in 0..array.len() {
                let item: JsValue = array.get(index)?;
                let number = if let Some(value) = item.as_int() {
                    f64::from(value)
                } else if let Some(value) = item.as_float() {
                    value
                } else {
                    return Ok(Self::Unsupported("array containing a non-number"));
                };
                values.push(number);
            }
            return Ok(Self::Array(values));
        }
        let _ = ctx;
        Ok(Self::Unsupported("unsupported value"))
    }
}

impl FromLua for ScriptSlotValue {
    fn from_lua(value: LuaValue, _lua: &Lua) -> mlua::Result<Self> {
        match value {
            LuaValue::Boolean(value) => Ok(Self::Boolean(value)),
            LuaValue::Integer(value) => Ok(Self::Number(value as f64)),
            LuaValue::Number(value) => Ok(Self::Number(value)),
            LuaValue::String(value) => Ok(Self::String(value.to_str()?.to_string())),
            LuaValue::Table(table) => {
                let len = table.raw_len();
                for pair in table.clone().pairs::<LuaValue, LuaValue>() {
                    let (key, _) = pair?;
                    let LuaValue::Integer(index) = key else {
                        return Ok(Self::Unsupported("table with non-array keys"));
                    };
                    if index < 1 || index as usize > len {
                        return Ok(Self::Unsupported("sparse array"));
                    }
                }

                let mut values = Vec::with_capacity(len);
                for index in 1..=len {
                    match table.get::<LuaValue>(index)? {
                        LuaValue::Integer(value) => values.push(value as f64),
                        LuaValue::Number(value) => values.push(value),
                        _ => {
                            return Ok(Self::Unsupported("array containing a non-number"));
                        }
                    }
                }
                Ok(Self::Array(values))
            }
            _ => Ok(Self::Unsupported("unsupported value")),
        }
    }
}

struct Any(SlotValue);

impl<'js> IntoJs<'js> for Any {
    fn into_js(self, ctx: &Ctx<'js>) -> rquickjs::Result<JsValue<'js>> {
        match self.0 {
            SlotValue::Number(value) => value.into_js(ctx),
            SlotValue::Boolean(value) => value.into_js(ctx),
            SlotValue::String(value) | SlotValue::Enum(value) => value.into_js(ctx),
            SlotValue::Array(values) => {
                let array = Array::new(ctx.clone())?;
                for (index, value) in values.into_iter().enumerate() {
                    array.set(index, value)?;
                }
                Ok(array.into_value())
            }
        }
    }
}

/// Script-facing state references returned by `defineStore`.
///
/// Each property value is the stable `{ slot }` reference object. Type brands
/// exist only in generated types.
#[derive(Debug)]
pub(crate) struct StoreStateRefs(BTreeMap<String, String>);

impl<'js> IntoJs<'js> for StoreStateRefs {
    fn into_js(self, ctx: &Ctx<'js>) -> rquickjs::Result<JsValue<'js>> {
        let object = JsObject::new(ctx.clone())?;
        for (slot_name, dotted_name) in self.0 {
            let reference = JsObject::new(ctx.clone())?;
            reference.set("slot", dotted_name)?;
            object.set(slot_name, reference)?;
        }
        Ok(object.into_value())
    }
}

impl IntoLua for StoreStateRefs {
    fn into_lua(self, lua: &Lua) -> mlua::Result<LuaValue> {
        let table: LuaTable = lua.create_table()?;
        for (slot_name, dotted_name) in self.0 {
            let reference = lua.create_table()?;
            reference.set("slot", dotted_name)?;
            table.set(slot_name, reference)?;
        }
        Ok(LuaValue::Table(table))
    }
}

#[derive(Debug)]
pub(crate) struct StoreDefinition {
    declaration: StoreDeclarationManifest,
    state: StoreStateRefs,
}

impl<'js> IntoJs<'js> for StoreDefinition {
    fn into_js(self, ctx: &Ctx<'js>) -> rquickjs::Result<JsValue<'js>> {
        let object = JsObject::new(ctx.clone())?;
        object.set("declaration", self.declaration)?;
        object.set("state", self.state)?;
        Ok(object.into_value())
    }
}

impl IntoLua for StoreDefinition {
    fn into_lua(self, lua: &Lua) -> mlua::Result<LuaValue> {
        let table = lua.create_table()?;
        table.set("declaration", self.declaration)?;
        table.set("state", self.state)?;
        Ok(LuaValue::Table(table))
    }
}

#[derive(Debug)]
struct StoreDeclarationManifest {
    namespace: String,
    schema: Value,
}

impl<'js> IntoJs<'js> for StoreDeclarationManifest {
    fn into_js(self, ctx: &Ctx<'js>) -> rquickjs::Result<JsValue<'js>> {
        let object = JsObject::new(ctx.clone())?;
        object.set("namespace", self.namespace)?;
        object.set("schema", json_to_js(ctx, &self.schema)?)?;
        Ok(object.into_value())
    }
}

impl IntoLua for StoreDeclarationManifest {
    fn into_lua(self, lua: &Lua) -> mlua::Result<LuaValue> {
        let table = lua.create_table()?;
        table.set("namespace", self.namespace)?;
        table.set("schema", json_to_lua(lua, &self.schema)?)?;
        Ok(LuaValue::Table(table))
    }
}

impl IntoLua for Any {
    fn into_lua(self, lua: &Lua) -> mlua::Result<LuaValue> {
        match self.0 {
            SlotValue::Number(value) => value.into_lua(lua),
            SlotValue::Boolean(value) => value.into_lua(lua),
            SlotValue::String(value) | SlotValue::Enum(value) => value.into_lua(lua),
            SlotValue::Array(values) => values.into_lua(lua),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SlotSchemaInput {
    #[serde(rename = "type")]
    slot_type: String,
    #[serde(default)]
    default: Option<Value>,
    #[serde(default)]
    range: Option<Value>,
    #[serde(default)]
    persist: bool,
    #[serde(default)]
    readonly: bool,
    #[serde(default)]
    values: Option<Value>,
}

const DEFINE_STORE_DOC: &str = "Build a typed state-store declaration for setupMod().stores. \
     Every mod-owned slot requires a default. Supported types are number, boolean, string, enum, and array. \
     Calling this builder does not mutate engine state. Returned declarations commit atomically after setupMod succeeds. \
     Returns { declaration, state }, where state leaves are stable { slot } references. Definition context.";

const STORE_READ_DOC: &str = "Read the current value of an engine-global state slot by stable dotted name. \
     Available in definition and data contexts.";

const STORE_WRITE_DOC: &str = "Write an engine-global state slot by stable dotted name. \
     The value must exactly match the declared slot type. Finite numbers are clamped to the declared inclusive range. \
     Readonly slots reject script writes with a warning and remain unchanged. Available in definition and data contexts.";

pub(crate) fn register_store_primitives(registry: &mut PrimitiveRegistry, ctx: ScriptCtx) {
    registry
        .register("defineStore", {
            move |namespace: String,
                  schema: StoreSchemaJson|
                  -> Result<StoreDefinition, ScriptError> {
                define_store(&namespace, schema.0)
            }
        })
        .scope(ContextScope::DefinitionOnly)
        .doc(DEFINE_STORE_DOC)
        .param("namespace", "String")
        .param("schema", "Any")
        .finish();

    registry
        .register("storeRead", {
            let ctx = ctx.clone();
            move |name: String| -> Result<Any, ScriptError> {
                read_store_slot(&ctx, &name).map(Any)
            }
        })
        .scope(ContextScope::Both)
        .doc(STORE_READ_DOC)
        .param("name", "String")
        .finish();

    registry
        .register(
            "storeWrite",
            move |name: String, value: ScriptSlotValue| -> Result<(), ScriptError> {
                write_script_store_slot(&ctx, &name, value)
            },
        )
        .scope(ContextScope::Both)
        .doc(STORE_WRITE_DOC)
        .param("name", "String")
        .param("value", "Any")
        .finish();
}

pub(crate) fn read_store_slot(ctx: &ScriptCtx, name: &str) -> Result<SlotValue, ScriptError> {
    let table = ctx.slot_table.borrow();
    let slot = table
        .get(name)
        .ok_or_else(|| unknown_slot("storeRead", name))?;
    slot.value
        .clone()
        .ok_or_else(|| ScriptError::InvalidArgument {
            reason: format!("storeRead: state slot `{name}` has no current value"),
        })
}

/// Engine-side write path. Readonly is a script ownership rule, so engine
/// systems bypass it while still applying the declared type/range validation.
pub(crate) fn write_store_slot(
    ctx: &ScriptCtx,
    name: &str,
    value: SlotValue,
) -> Result<(), ScriptError> {
    let mut table = ctx.slot_table.borrow_mut();
    let slot = table
        .get_mut(name)
        .ok_or_else(|| unknown_slot("storeWrite", name))?;
    slot.value = Some(validate_slot_value(name, &slot.schema, value)?);
    Ok(())
}

/// Readonly-gated write of a JSON value to a slot by dotted name (M13 Goal F,
/// Task 4 — the `setState` reaction's slot-write path). Unlike [`write_store_slot`]
/// (the engine bypass), this gates on **writability**: a readonly slot warns and
/// no-ops, leaving the value unchanged; an engine-owned but writable slot is a
/// valid target. The JSON value is coerced to the slot's declared type (the same
/// type/range/enum validation [`write_store_slot`] applies), so `setState` reuses
/// one validation path. An unknown slot or a type mismatch returns an error the
/// drain logs — never a panic. NEVER use the engine bypass for `setState`.
pub(crate) fn write_state_slot_json(
    ctx: &ScriptCtx,
    name: &str,
    value: &Value,
) -> Result<(), ScriptError> {
    let mut table = ctx.slot_table.borrow_mut();
    let slot = table
        .get_mut(name)
        .ok_or_else(|| unknown_slot("setState", name))?;
    if slot.schema.readonly {
        log::warn!("[Scripting] setState: rejected write to readonly slot `{name}`");
        return Ok(());
    }
    let coerced = json_value_for_slot(name, &slot.schema.slot_type, value)?;
    slot.value = Some(validate_slot_value(name, &slot.schema, coerced)?);
    Ok(())
}

/// A single text-edit operation against a String slot (M13 Text Entry, Task 1).
/// The three text-edit system reactions (`appendText` / `backspaceText` /
/// `clearText`) each map to one variant; [`apply_text_edit`] reads the slot's
/// current string, applies the edit, and writes it back through the
/// readonly-gated path.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum TextEdit<'a> {
    /// Append `text` to the current string value.
    Append(&'a str),
    /// Remove the last character (one Unicode scalar value). No-op on empty.
    Backspace,
    /// Empty the slot.
    Clear,
}

/// Apply a [`TextEdit`] to a String slot by dotted name through the SAME
/// readonly-gated write path as `setState` (M13 Text Entry, Task 1). The current
/// value is read, the edit applied, and the result written back via
/// [`write_state_slot_json`]: a readonly slot warns and no-ops (the `setState`
/// warning); an engine-owned writable slot (`ui.textEntry`) is a valid target.
/// NEVER uses the engine bypass.
///
/// A readonly slot warns and no-ops (the `setState` warning) — the readonly gate
/// is consulted BEFORE the slot's type or value, so a readonly slot is rejected
/// uniformly regardless of its declared type (matching `setState`, which gates on
/// writability before coercion).
///
/// `Backspace` on an empty value is a no-op with NO warning — it returns early
/// before touching the write path, so an empty slot produces neither a write nor
/// a log line. An unknown slot or a non-String writable slot surfaces an error
/// the drain logs (never a panic). Backspace pops one `char` (one Unicode scalar
/// value) — never splits a UTF-8 sequence, but does not segment grapheme clusters.
pub(crate) fn apply_text_edit(
    ctx: &ScriptCtx,
    name: &str,
    edit: TextEdit<'_>,
) -> Result<(), ScriptError> {
    // Read the slot once: gate on writability first (a readonly slot warns and
    // no-ops, same as setState — checked before type so it is rejected uniformly
    // regardless of declared type), then read the current value as a string. An
    // absent value starts from empty so a fresh slot can be appended to; a
    // writable but non-String slot is a type error.
    let current = {
        let table = ctx.slot_table.borrow();
        let slot = table
            .get(name)
            .ok_or_else(|| unknown_slot("text-edit", name))?;
        if slot.schema.readonly {
            log::warn!("[Scripting] text-edit: rejected write to readonly slot `{name}`");
            return Ok(());
        }
        match &slot.value {
            Some(SlotValue::String(value)) => value.clone(),
            None => String::new(),
            Some(other) => {
                return Err(wrong_write_type(
                    name,
                    &SlotType::String,
                    slot_value_kind(other),
                ));
            }
        }
    };

    let next = match edit {
        TextEdit::Append(text) => {
            let mut next = current;
            next.push_str(text);
            next
        }
        TextEdit::Backspace => {
            // Empty: no-op, no warning, no write. Returning here keeps the
            // readonly-gated path (and its warning) out of the empty case.
            if current.is_empty() {
                return Ok(());
            }
            let mut next = current;
            // `char`-pop floor: drop the last Unicode scalar value. `pop`
            // removes one whole `char`, so it never splits a UTF-8 sequence.
            next.pop();
            next
        }
        TextEdit::Clear => String::new(),
    };

    write_state_slot_json(ctx, name, &Value::String(next))
}

/// Coerce a JSON value to a `SlotValue` matching the slot's declared type for the
/// `setState` path. Mirrors `script_value_for_slot` but takes a `serde_json::Value`
/// (the reaction-args representation) instead of a runtime `ScriptSlotValue`.
fn json_value_for_slot(
    name: &str,
    slot_type: &SlotType,
    value: &Value,
) -> Result<SlotValue, ScriptError> {
    match slot_type {
        SlotType::Number => value
            .as_f64()
            .map(|n| n as f32)
            .map(SlotValue::Number)
            .ok_or_else(|| wrong_write_type(name, slot_type, json_value_kind(value))),
        SlotType::Boolean => value
            .as_bool()
            .map(SlotValue::Boolean)
            .ok_or_else(|| wrong_write_type(name, slot_type, json_value_kind(value))),
        SlotType::String => value
            .as_str()
            .map(|s| SlotValue::String(s.to_string()))
            .ok_or_else(|| wrong_write_type(name, slot_type, json_value_kind(value))),
        SlotType::Enum { .. } => value
            .as_str()
            .map(|s| SlotValue::Enum(s.to_string()))
            .ok_or_else(|| wrong_write_type(name, slot_type, json_value_kind(value))),
        SlotType::Array => {
            let array = value
                .as_array()
                .ok_or_else(|| wrong_write_type(name, slot_type, json_value_kind(value)))?;
            let values = array
                .iter()
                .enumerate()
                .map(|(index, element)| {
                    let number = element.as_f64().ok_or_else(|| {
                        wrong_write_type(name, slot_type, json_value_kind(element))
                    })?;
                    finite_array_f32(name, index, number)
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(SlotValue::Array(values))
        }
    }
}

fn json_value_kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn write_script_store_slot(
    ctx: &ScriptCtx,
    name: &str,
    value: ScriptSlotValue,
) -> Result<(), ScriptError> {
    let mut table = ctx.slot_table.borrow_mut();
    let slot = table
        .get_mut(name)
        .ok_or_else(|| unknown_slot("storeWrite", name))?;
    if slot.schema.readonly {
        log::warn!("[Scripting] storeWrite: rejected write to readonly slot `{name}`");
        return Ok(());
    }

    let value = script_value_for_slot(name, &slot.schema.slot_type, value)?;
    slot.value = Some(validate_slot_value(name, &slot.schema, value)?);
    Ok(())
}

fn script_value_for_slot(
    name: &str,
    slot_type: &SlotType,
    value: ScriptSlotValue,
) -> Result<SlotValue, ScriptError> {
    match (slot_type, value) {
        (SlotType::Number, ScriptSlotValue::Number(value)) => {
            Ok(SlotValue::Number(finite_f32(name, value)?))
        }
        (SlotType::Boolean, ScriptSlotValue::Boolean(value)) => Ok(SlotValue::Boolean(value)),
        (SlotType::String, ScriptSlotValue::String(value)) => Ok(SlotValue::String(value)),
        (SlotType::Enum { .. }, ScriptSlotValue::String(value)) => Ok(SlotValue::Enum(value)),
        (SlotType::Array, ScriptSlotValue::Array(values)) => Ok(SlotValue::Array(
            values
                .into_iter()
                .enumerate()
                .map(|(index, value)| finite_array_f32(name, index, value))
                .collect::<Result<Vec<_>, _>>()?,
        )),
        (_, ScriptSlotValue::Unsupported(actual)) => Err(wrong_write_type(name, slot_type, actual)),
        (_, actual) => Err(wrong_write_type(
            name,
            slot_type,
            script_value_kind(&actual),
        )),
    }
}

fn validate_slot_value(
    name: &str,
    schema: &SlotSchema,
    value: SlotValue,
) -> Result<SlotValue, ScriptError> {
    match (&schema.slot_type, value) {
        (SlotType::Number, SlotValue::Number(value)) => {
            if !value.is_finite() {
                return Err(non_finite_write(name, "number"));
            }
            if let Some(range) = schema.range {
                let clamped = value.clamp(range.min, range.max);
                if clamped != value {
                    log::warn!(
                        "[Scripting] storeWrite: clamped slot `{name}` from {value} to {clamped} within [{}, {}]",
                        range.min,
                        range.max
                    );
                }
                Ok(SlotValue::Number(clamped))
            } else {
                Ok(SlotValue::Number(value))
            }
        }
        (SlotType::Boolean, SlotValue::Boolean(value)) => Ok(SlotValue::Boolean(value)),
        (SlotType::String, SlotValue::String(value)) => Ok(SlotValue::String(value)),
        (SlotType::Enum { values }, SlotValue::Enum(value)) => {
            if values.contains(&value) {
                Ok(SlotValue::Enum(value))
            } else {
                Err(ScriptError::InvalidArgument {
                    reason: format!(
                        "storeWrite: value `{value}` is not declared for enum slot `{name}`"
                    ),
                })
            }
        }
        (SlotType::Array, SlotValue::Array(values)) => {
            if values.iter().all(|value| value.is_finite()) {
                Ok(SlotValue::Array(values))
            } else {
                Err(non_finite_write(name, "array element"))
            }
        }
        (expected, actual) => Err(wrong_write_type(name, expected, slot_value_kind(&actual))),
    }
}

fn finite_f32(name: &str, value: f64) -> Result<f32, ScriptError> {
    let narrowed = value as f32;
    if value.is_finite() && narrowed.is_finite() {
        Ok(narrowed)
    } else {
        Err(non_finite_write(name, "number"))
    }
}

fn finite_array_f32(name: &str, index: usize, value: f64) -> Result<f32, ScriptError> {
    finite_f32(name, value).map_err(|_| ScriptError::InvalidArgument {
        reason: format!("storeWrite: slot `{name}` array element [{index}] must be finite"),
    })
}

fn unknown_slot(primitive: &str, name: &str) -> ScriptError {
    ScriptError::InvalidArgument {
        reason: format!("{primitive}: unknown state slot `{name}`"),
    }
}

fn non_finite_write(name: &str, kind: &str) -> ScriptError {
    ScriptError::InvalidArgument {
        reason: format!("storeWrite: slot `{name}` {kind} must be finite"),
    }
}

fn wrong_write_type(name: &str, expected: &SlotType, actual: &str) -> ScriptError {
    ScriptError::InvalidArgument {
        reason: format!(
            "storeWrite: slot `{name}` expects {}, got {actual}",
            slot_type_name(expected)
        ),
    }
}

fn slot_type_name(slot_type: &SlotType) -> &'static str {
    match slot_type {
        SlotType::Number => "number",
        SlotType::Boolean => "boolean",
        SlotType::String => "string",
        SlotType::Enum { .. } => "enum string",
        SlotType::Array => "number array",
    }
}

fn script_value_kind(value: &ScriptSlotValue) -> &'static str {
    match value {
        ScriptSlotValue::Number(_) => "number",
        ScriptSlotValue::Boolean(_) => "boolean",
        ScriptSlotValue::String(_) => "string",
        ScriptSlotValue::Array(_) => "array",
        ScriptSlotValue::Unsupported(kind) => kind,
    }
}

fn slot_value_kind(value: &SlotValue) -> &'static str {
    match value {
        SlotValue::Number(_) => "number",
        SlotValue::Boolean(_) => "boolean",
        SlotValue::String(_) => "string",
        SlotValue::Enum(_) => "enum",
        SlotValue::Array(_) => "array",
    }
}

fn define_store(namespace: &str, schema: Value) -> Result<StoreDefinition, ScriptError> {
    let declaration = store_declaration(namespace, schema.clone())?;
    let state = state_refs_for(&declaration);
    Ok(StoreDefinition {
        declaration: StoreDeclarationManifest {
            namespace: namespace.to_string(),
            schema,
        },
        state,
    })
}

pub(crate) fn store_declaration_from_manifest_value(
    value: Value,
) -> Result<StoreDeclaration, ScriptError> {
    let object = value
        .as_object()
        .ok_or_else(|| ScriptError::InvalidArgument {
            reason: "defineStore: returned store declaration must be an object".to_string(),
        })?;
    let namespace = object
        .get("namespace")
        .and_then(Value::as_str)
        .ok_or_else(|| ScriptError::InvalidArgument {
            reason: "defineStore: returned store declaration requires `namespace`".to_string(),
        })?;
    let schema = object
        .get("schema")
        .cloned()
        .ok_or_else(|| ScriptError::InvalidArgument {
            reason: "defineStore: returned store declaration requires `schema`".to_string(),
        })?;
    store_declaration(namespace, schema)
}

pub(crate) fn store_declaration_set_from_values(
    values: impl IntoIterator<Item = Value>,
) -> Result<StoreDeclarationSet, ScriptError> {
    let mut declarations = StoreDeclarationSet::default();
    for value in values {
        let declaration = store_declaration_from_manifest_value(value)?;
        declarations
            .add(declaration)
            .map_err(|error| ScriptError::InvalidArgument {
                reason: format!("defineStore: {error}"),
            })?;
    }
    Ok(declarations)
}

pub(crate) fn drain_store_declarations_js<'js>(
    ctx: &Ctx<'js>,
    obj: &JsObject<'js>,
) -> Result<StoreDeclarationSet, ScriptError> {
    match obj.contains_key("stores") {
        Ok(false) => Ok(StoreDeclarationSet::default()),
        Ok(true) => {
            let arr: Array = obj
                .get("stores")
                .map_err(|e| ScriptError::InvalidArgument {
                    reason: format!("`stores` field must be an array: {e}"),
                })?;
            let mut values = Vec::with_capacity(arr.len());
            for i in 0..arr.len() {
                let value: JsValue = arr.get(i).map_err(|e| ScriptError::InvalidArgument {
                    reason: format!("`stores[{i}]` could not be read: {e}"),
                })?;
                values.push(
                    js_to_json(ctx, value).map_err(|e| ScriptError::InvalidArgument {
                        reason: format!("`stores[{i}]` could not be lowered: {e}"),
                    })?,
                );
            }
            store_declaration_set_from_values(values)
        }
        Err(e) => Err(ScriptError::InvalidArgument {
            reason: format!("`stores` lookup failed: {e}"),
        }),
    }
}

pub(crate) fn drain_store_declarations_lua(
    table: &LuaTable,
) -> Result<StoreDeclarationSet, ScriptError> {
    if !table
        .contains_key("stores")
        .map_err(|e| ScriptError::InvalidArgument {
            reason: format!("`stores` lookup failed: {e}"),
        })?
    {
        return Ok(StoreDeclarationSet::default());
    }

    let raw: LuaValue = table
        .get("stores")
        .map_err(|e| ScriptError::InvalidArgument {
            reason: format!("`stores` field could not be read: {e}"),
        })?;
    let LuaValue::Table(arr) = raw else {
        return Err(ScriptError::InvalidArgument {
            reason: format!("`stores` field must be an array, got {}", raw.type_name()),
        });
    };

    let len = validate_dense_lua_array(&arr, "stores")?;
    let mut values = Vec::with_capacity(len);
    for i in 1..=(len as i64) {
        let value: LuaValue = arr.get(i).map_err(|e| ScriptError::InvalidArgument {
            reason: format!("`stores[{i}]` could not be read: {e}"),
        })?;
        values.push(
            lua_to_json(value).map_err(|e| ScriptError::InvalidArgument {
                reason: format!("`stores[{i}]` could not be lowered: {e}"),
            })?,
        );
    }
    store_declaration_set_from_values(values)
}

fn validate_dense_lua_array(table: &LuaTable, field_name: &str) -> Result<usize, ScriptError> {
    let mut keys = BTreeSet::new();
    let mut max_index = 0_i64;

    for pair in table.clone().pairs::<LuaValue, LuaValue>() {
        let (key, _) = pair.map_err(|e| ScriptError::InvalidArgument {
            reason: format!("`{field_name}` keys could not be read: {e}"),
        })?;
        let LuaValue::Integer(index) = key else {
            return Err(ScriptError::InvalidArgument {
                reason: format!(
                    "`{field_name}` field must be a dense array; found {} key",
                    key.type_name()
                ),
            });
        };
        if index < 1 {
            return Err(ScriptError::InvalidArgument {
                reason: format!(
                    "`{field_name}` field must be a dense array; index {index} is out of range"
                ),
            });
        }
        keys.insert(index);
        max_index = max_index.max(index);
    }

    if keys.len() != max_index as usize {
        return Err(ScriptError::InvalidArgument {
            reason: format!("`{field_name}` field must be a dense array; holes are not allowed"),
        });
    }

    Ok(max_index as usize)
}

fn store_declaration(namespace: &str, schema: Value) -> Result<StoreDeclaration, ScriptError> {
    let inputs: BTreeMap<String, SlotSchemaInput> =
        serde_json::from_value(schema).map_err(|error| invalid_schema(None, error))?;

    let mut records = Vec::with_capacity(inputs.len());
    for (slot_name, input) in inputs {
        let record = validate_slot_schema(&slot_name, input)?;
        records.push((slot_name, record));
    }

    Ok(StoreDeclaration {
        namespace: namespace.to_string(),
        records,
    })
}

fn state_refs_for(declaration: &StoreDeclaration) -> StoreStateRefs {
    StoreStateRefs(
        declaration
            .records
            .iter()
            .map(|(slot_name, _)| {
                (
                    slot_name.clone(),
                    format!("{}.{}", declaration.namespace, slot_name),
                )
            })
            .collect(),
    )
}

fn validate_slot_schema(
    slot_name: &str,
    input: SlotSchemaInput,
) -> Result<SlotRecord, ScriptError> {
    let SlotSchemaInput {
        slot_type,
        default,
        range,
        persist,
        readonly,
        values,
    } = input;

    let default = default.ok_or_else(|| ScriptError::InvalidArgument {
        reason: format!("defineStore: slot `{slot_name}` requires `default`"),
    })?;

    let (slot_type, default, range) = match slot_type.as_str() {
        "number" => {
            let default = json_number(&default, slot_name, "default")?;
            let range = range
                .as_ref()
                .map(|value| number_range(value, slot_name, default))
                .transpose()?;
            (SlotType::Number, SlotValue::Number(default), range)
        }
        "boolean" => (
            SlotType::Boolean,
            SlotValue::Boolean(
                default
                    .as_bool()
                    .ok_or_else(|| wrong_default(slot_name, "boolean"))?,
            ),
            None,
        ),
        "string" => (
            SlotType::String,
            SlotValue::String(
                default
                    .as_str()
                    .ok_or_else(|| wrong_default(slot_name, "string"))?
                    .to_string(),
            ),
            None,
        ),
        "enum" => {
            let values: Vec<String> =
                serde_json::from_value(values.ok_or_else(|| ScriptError::InvalidArgument {
                    reason: format!("defineStore: enum slot `{slot_name}` requires `values`"),
                })?)
                .map_err(|error| invalid_schema(Some(slot_name), error))?;
            if values.is_empty() {
                return Err(ScriptError::InvalidArgument {
                    reason: format!(
                        "defineStore: enum slot `{slot_name}` requires at least one value"
                    ),
                });
            }
            let default = default
                .as_str()
                .ok_or_else(|| wrong_default(slot_name, "enum string"))?
                .to_string();
            if !values.contains(&default) {
                return Err(ScriptError::InvalidArgument {
                    reason: format!(
                        "defineStore: enum slot `{slot_name}` default `{default}` is not in `values`"
                    ),
                });
            }
            (SlotType::Enum { values }, SlotValue::Enum(default), None)
        }
        "array" => {
            let elements = default
                .as_array()
                .ok_or_else(|| wrong_default(slot_name, "number array"))?;
            let values = elements
                .iter()
                .enumerate()
                .map(|(index, value)| json_number(value, slot_name, &format!("default[{index}]")))
                .collect::<Result<Vec<_>, _>>()?;
            (SlotType::Array, SlotValue::Array(values), None)
        }
        other => {
            return Err(ScriptError::InvalidArgument {
                reason: format!("defineStore: slot `{slot_name}` has unknown type `{other}`"),
            });
        }
    };

    Ok(SlotRecord::new(SlotSchema {
        slot_type,
        default: Some(default),
        range,
        persist,
        readonly,
        ownership: SlotOwnership::Mod,
    }))
}

fn json_number(value: &Value, slot_name: &str, field: &str) -> Result<f32, ScriptError> {
    let Some(number) = value.as_f64() else {
        return Err(ScriptError::InvalidArgument {
            reason: format!("defineStore: slot `{slot_name}` `{field}` must be a finite number"),
        });
    };
    let narrowed = number as f32;
    if !number.is_finite() || !narrowed.is_finite() {
        return Err(ScriptError::InvalidArgument {
            reason: format!("defineStore: slot `{slot_name}` `{field}` must be a finite number"),
        });
    }
    Ok(narrowed)
}

fn number_range(value: &Value, slot_name: &str, default: f32) -> Result<NumericRange, ScriptError> {
    let values = value
        .as_array()
        .ok_or_else(|| ScriptError::InvalidArgument {
            reason: format!("defineStore: number slot `{slot_name}` `range` must be [min, max]"),
        })?;
    if values.len() != 2 {
        return Err(ScriptError::InvalidArgument {
            reason: format!("defineStore: number slot `{slot_name}` `range` must be [min, max]"),
        });
    }
    let min = json_number(&values[0], slot_name, "range[0]")?;
    let max = json_number(&values[1], slot_name, "range[1]")?;
    if min > max {
        return Err(ScriptError::InvalidArgument {
            reason: format!(
                "defineStore: number slot `{slot_name}` range minimum {min} exceeds maximum {max}"
            ),
        });
    }
    if !(min..=max).contains(&default) {
        return Err(ScriptError::InvalidArgument {
            reason: format!(
                "defineStore: number slot `{slot_name}` default {default} is outside inclusive range [{min}, {max}]"
            ),
        });
    }
    Ok(NumericRange { min, max })
}

fn wrong_default(slot_name: &str, expected: &str) -> ScriptError {
    ScriptError::InvalidArgument {
        reason: format!("defineStore: slot `{slot_name}` default must be {expected}"),
    }
}

fn invalid_schema(slot_name: Option<&str>, error: serde_json::Error) -> ScriptError {
    let location = slot_name
        .map(|name| format!(" for slot `{name}`"))
        .unwrap_or_default();
    ScriptError::InvalidArgument {
        reason: format!("defineStore: malformed schema{location}: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scripting::luau::{LuauConfig, LuauSubsystem, Which};
    use crate::scripting::primitives_registry::PrimitiveRegistry;
    use crate::scripting::quickjs::{QuickJsConfig, QuickJsSubsystem, run_script};
    use crate::scripting::slot_table::NamespaceInsertError;

    fn registry_for(ctx: ScriptCtx) -> PrimitiveRegistry {
        let mut registry = PrimitiveRegistry::new();
        register_store_primitives(&mut registry, ctx);
        registry
    }

    fn commit_store_for_test(
        ctx: &ScriptCtx,
        namespace: &str,
        schema: Value,
    ) -> Result<(), ScriptError> {
        let declaration = store_declaration(namespace, schema)?;
        ctx.slot_table
            .borrow_mut()
            .insert_namespace(&declaration.namespace, declaration.records)
            .map_err(|error| ScriptError::InvalidArgument {
                reason: format!("defineStore: {error}"),
            })
    }

    fn define_runtime_test_store(ctx: &ScriptCtx) {
        commit_store_for_test(
            ctx,
            "test",
            serde_json::json!({
                "number": { "type": "number", "default": 0.5, "range": [0, 1] },
                "boolean": { "type": "boolean", "default": false },
                "string": { "type": "string", "default": "before" },
                "enum": { "type": "enum", "values": ["idle", "active"], "default": "idle" },
                "array": { "type": "array", "default": [0, 1] },
            }),
        )
        .unwrap();
    }

    #[test]
    fn define_store_is_definition_only() {
        let registry = registry_for(ScriptCtx::new());
        let primitive = registry
            .iter()
            .find(|primitive| primitive.name == "defineStore")
            .unwrap();
        assert_eq!(primitive.context_scope, ContextScope::DefinitionOnly);
    }

    #[test]
    fn store_read_and_write_are_both_scoped_and_avoid_reserved_name() {
        let registry = registry_for(ScriptCtx::new());
        for name in ["storeRead", "storeWrite"] {
            let primitive = registry
                .iter()
                .find(|primitive| primitive.name == name)
                .unwrap();
            assert_eq!(primitive.context_scope, ContextScope::Both);
        }
        assert!(
            registry
                .iter()
                .all(|primitive| primitive.name != "setState")
        );
    }

    #[test]
    fn define_store_quickjs_and_luau_return_equivalent_declarations() {
        let js_ctx = ScriptCtx::new();
        let js_registry = registry_for(js_ctx.clone());
        let quickjs = QuickJsSubsystem::new(&js_registry, &QuickJsConfig::default()).unwrap();
        quickjs.definition_ctx().with(|qjs| {
            run_script::<()>(
                &qjs,
                r#"
                const store = defineStore("audio", {
                    master: { type: "number", default: 0.8, range: [0, 1], persist: true },
                    muted: { type: "boolean", default: false },
                    label: { type: "string", default: "" },
                    mode: { type: "enum", values: ["quiet", "loud"], default: "quiet" },
                    curve: { type: "array", default: [0, 0.5, 1] },
                });
                if (store.declaration.namespace !== "audio") throw new Error("namespace");
                if (store.state.master.slot !== "audio.master") throw new Error("state ref");
                "#,
                "store.js",
            )
            .unwrap();
        });

        let luau_ctx = ScriptCtx::new();
        let luau_registry = registry_for(luau_ctx.clone());
        let luau = LuauSubsystem::new(&luau_registry, &LuauConfig::default()).unwrap();
        luau.run_source::<()>(
            Which::Definition,
            r#"
            local store = defineStore("audio", {
                master = { type = "number", default = 0.8, range = {0, 1}, persist = true },
                muted = { type = "boolean", default = false },
                label = { type = "string", default = "" },
                mode = { type = "enum", values = {"quiet", "loud"}, default = "quiet" },
                curve = { type = "array", default = {0, 0.5, 1} },
            })
            assert(store.declaration.namespace == "audio")
            assert(store.state.master.slot == "audio.master")
            "#,
            "store.luau",
        )
        .unwrap();

        let js_slots = js_ctx.slot_table.borrow();
        let luau_slots = luau_ctx.slot_table.borrow();
        for name in [
            "audio.master",
            "audio.muted",
            "audio.label",
            "audio.mode",
            "audio.curve",
        ] {
            assert!(js_slots.get(name).is_none(), "js slot {name}");
            assert!(luau_slots.get(name).is_none(), "luau slot {name}");
        }
    }

    #[test]
    fn malformed_schemas_return_errors_without_partial_insertion() {
        let cases = [
            serde_json::json!({ "value": { "type": "number" } }),
            serde_json::json!({ "value": { "type": "number", "default": "one" } }),
            serde_json::json!({ "value": { "type": "number", "default": 2, "range": [0, 1] } }),
            serde_json::json!({ "value": { "type": "number", "default": 1, "range": [2, 1] } }),
            serde_json::json!({ "value": { "type": "boolean", "default": 1 } }),
            serde_json::json!({ "value": { "type": "string", "default": false } }),
            serde_json::json!({ "value": { "type": "enum", "values": [], "default": "a" } }),
            serde_json::json!({ "value": { "type": "enum", "values": ["a"], "default": "b" } }),
            serde_json::json!({ "value": { "type": "array", "default": [1, null] } }),
            serde_json::json!({ "value": { "type": "vector", "default": 1 } }),
        ];

        for (index, schema) in cases.into_iter().enumerate() {
            let err = store_declaration_from_manifest_value(serde_json::json!({
                "namespace": format!("bad{index}"),
                "schema": schema,
            }))
            .expect_err("malformed returned declaration should fail validation");
            assert!(matches!(err, ScriptError::InvalidArgument { .. }));
        }

        let ctx = ScriptCtx::new();
        let err = commit_store_for_test(
            &ctx,
            "mixed",
            serde_json::json!({
                "good": { "type": "number", "default": 1 },
                "bad": { "type": "enum", "values": [], "default": "x" },
            }),
        )
        .unwrap_err();
        assert!(matches!(err, ScriptError::InvalidArgument { .. }));
        assert!(ctx.slot_table.borrow().get("mixed.good").is_none());
    }

    #[test]
    fn define_store_rejects_engine_namespace_and_prefix_collisions() {
        let ctx = ScriptCtx::new();
        let schema = serde_json::json!({
            "shield": { "type": "number", "default": 100 }
        });

        for (namespace, expected) in [
            ("player", "incompatible schema"),
            ("player.stats", "namespace collision"),
        ] {
            let declaration = store_declaration(namespace, schema.clone()).unwrap();
            let mut declarations = StoreDeclarationSet::default();
            declarations.add(declaration).unwrap();
            let err = ctx
                .slot_table
                .borrow()
                .plan_reconcile(&declarations)
                .unwrap_err();
            match expected {
                "incompatible schema" => assert!(matches!(
                    err,
                    NamespaceInsertError::IncompatibleSchema { .. }
                )),
                "namespace collision" => assert!(matches!(
                    err,
                    NamespaceInsertError::NamespaceCollision { .. }
                )),
                _ => unreachable!("test expectation covers every case"),
            }
        }
        assert!(ctx.slot_table.borrow().get("player.shield").is_none());
        assert!(ctx.slot_table.borrow().get("player.stats.shield").is_none());
    }

    #[test]
    fn duplicate_mod_namespace_is_rejected_without_mutation() {
        let ctx = ScriptCtx::new();
        commit_store_for_test(
            &ctx,
            "audio",
            serde_json::json!({
                "master": { "type": "number", "default": 1 }
            }),
        )
        .unwrap();
        let before = ctx.slot_table.borrow().len();

        let err = commit_store_for_test(
            &ctx,
            "audio",
            serde_json::json!({
                "music": { "type": "number", "default": 0.5 }
            }),
        )
        .unwrap_err();
        assert!(err.to_string().contains("collides"));
        assert_eq!(ctx.slot_table.borrow().len(), before);
        assert!(ctx.slot_table.borrow().get("audio.music").is_none());
    }

    #[test]
    fn namespace_error_type_remains_matchable() {
        let mut table = crate::scripting::slot_table::SlotTable::new();
        let err = table.insert_namespace("player", Vec::new()).unwrap_err();
        assert!(matches!(
            err,
            NamespaceInsertError::NamespaceCollision { .. }
        ));
    }

    #[test]
    fn define_store_returns_stable_state_refs_in_both_runtimes() {
        let js_ctx = ScriptCtx::new();
        let js_registry = registry_for(js_ctx);
        let quickjs = QuickJsSubsystem::new(&js_registry, &QuickJsConfig::default()).unwrap();
        quickjs.definition_ctx().with(|qjs| {
            run_script::<()>(
                &qjs,
                r#"
                const store = defineStore("audio", {
                    master: { type: "number", default: 1 },
                    muted: { type: "boolean", default: false },
                });
                if (store.state.master.slot !== "audio.master") throw new Error("master handle");
                if (store.state.muted.slot !== "audio.muted") throw new Error("muted handle");
                "#,
                "store-handles.js",
            )
            .unwrap();
        });

        let luau_ctx = ScriptCtx::new();
        let luau_registry = registry_for(luau_ctx);
        let luau = LuauSubsystem::new(&luau_registry, &LuauConfig::default()).unwrap();
        luau.run_source::<()>(
            Which::Definition,
            r#"
            local store = defineStore("audio", {
                master = { type = "number", default = 1 },
                muted = { type = "boolean", default = false },
            })
            assert(store.state.master.slot == "audio.master")
            assert(store.state.muted.slot == "audio.muted")
            "#,
            "store-handles.luau",
        )
        .unwrap();
    }

    #[test]
    fn malformed_luau_builders_do_not_insert_before_manifest_validation() {
        let cases = [
            r#"{ value = { type = "number" } }"#,
            r#"{ value = { type = "number", default = "one" } }"#,
            r#"{ value = { type = "number", default = 2, range = {0, 1} } }"#,
            r#"{ value = { type = "boolean", default = 1 } }"#,
            r#"{ value = { type = "enum", values = {}, default = "a" } }"#,
            r#"{ value = { type = "array", default = {1, 0 / 0} } }"#,
            r#"{ value = { type = "vector", default = 1 } }"#,
        ];

        for (index, schema) in cases.into_iter().enumerate() {
            let ctx = ScriptCtx::new();
            let registry = registry_for(ctx.clone());
            let luau = LuauSubsystem::new(&registry, &LuauConfig::default()).unwrap();
            let source = format!(r#"defineStore("bad{index}", {schema})"#);
            luau.run_source::<()>(Which::Definition, &source, "bad-store.luau")
                .expect("pure builder does not validate until manifest drain");
            assert!(
                ctx.slot_table
                    .borrow()
                    .get(&format!("bad{index}.value"))
                    .is_none()
            );
        }
    }

    fn drain_luau_store_manifest(source: &str) -> Result<StoreDeclarationSet, ScriptError> {
        let lua = mlua::Lua::new();
        let manifest: LuaTable = lua.load(source).eval().unwrap();
        drain_store_declarations_lua(&manifest)
    }

    fn drain_quickjs_store_manifest(source: &str) -> Result<StoreDeclarationSet, ScriptError> {
        let rt = rquickjs::Runtime::new().unwrap();
        let ctx = rquickjs::Context::full(&rt).unwrap();
        ctx.with(|qjs| {
            let value: JsValue = qjs.eval(source).unwrap();
            let manifest = JsObject::from_value(value).unwrap();
            drain_store_declarations_js(&qjs, &manifest)
        })
    }

    #[test]
    fn luau_setup_mod_stores_accepts_dense_arrays() {
        let declarations = drain_luau_store_manifest(
            r#"
            local first = {
                namespace = "audio",
                schema = { master = { type = "number", default = 1 } },
            }
            local second = {
                namespace = "video",
                schema = { brightness = { type = "number", default = 0.5 } },
            }
            return { stores = { first, second } }
            "#,
        )
        .unwrap();

        assert_eq!(declarations.len(), 2);
    }

    #[test]
    fn luau_setup_mod_stores_rejects_non_dense_tables() {
        // Regression: raw_len iteration treated map-shaped and sparse `stores`
        // tables as empty or partial arrays, silently dropping declarations.
        let cases = [
            (
                r#"
                local declaration = {
                    namespace = "audio",
                    schema = { master = { type = "number", default = 1 } },
                }
                return { stores = { audio = declaration } }
                "#,
                "map-shaped table",
            ),
            (
                r#"
                local declaration = {
                    namespace = "audio",
                    schema = { master = { type = "number", default = 1 } },
                }
                return { stores = { declaration, extra = declaration } }
                "#,
                "extra string key",
            ),
            (
                r#"
                local declaration = {
                    namespace = "audio",
                    schema = { master = { type = "number", default = 1 } },
                }
                return { stores = { [2] = declaration } }
                "#,
                "hole",
            ),
            (
                r#"
                local declaration = {
                    namespace = "audio",
                    schema = { master = { type = "number", default = 1 } },
                }
                return { stores = { [0] = declaration } }
                "#,
                "zero index",
            ),
            (
                r#"
                local declaration = {
                    namespace = "audio",
                    schema = { master = { type = "number", default = 1 } },
                }
                return { stores = { [1.5] = declaration } }
                "#,
                "non-integer index",
            ),
        ];

        for (source, label) in cases {
            let err = match drain_luau_store_manifest(source) {
                Ok(_) => panic!("{label} should be rejected"),
                Err(err) => err,
            };
            assert!(
                err.to_string().contains("dense array"),
                "{label} produced unexpected error: {err}"
            );
        }
    }

    #[test]
    fn returned_store_declaration_rejects_cyclic_schema_before_commit() {
        let quickjs_err = drain_quickjs_store_manifest(
            r#"
            const schema = { value: { type: "number", default: 1 } };
            schema.value.self = schema;
            ({ stores: [{ namespace: "cyclic", schema }] })
            "#,
        )
        .unwrap_err();
        assert!(
            quickjs_err.to_string().contains("maximum conversion depth"),
            "unexpected QuickJS error: {quickjs_err}"
        );

        let luau_err = drain_luau_store_manifest(
            r#"
            local schema = { value = { type = "number", default = 1 } }
            schema.value.self = schema
            return { stores = { { namespace = "cyclic", schema = schema } } }
            "#,
        )
        .unwrap_err();
        assert!(
            luau_err.to_string().contains("maximum conversion depth"),
            "unexpected Luau error: {luau_err}"
        );
    }

    #[test]
    fn store_read_write_quickjs_and_luau_cover_all_value_kinds_and_clamp() {
        let js_ctx = ScriptCtx::new();
        define_runtime_test_store(&js_ctx);
        let js_registry = registry_for(js_ctx.clone());
        let quickjs = QuickJsSubsystem::new(&js_registry, &QuickJsConfig::default()).unwrap();
        quickjs.definition_ctx().with(|qjs| {
            run_script::<()>(
                &qjs,
                r#"
                storeWrite("test.number", 5);
                storeWrite("test.boolean", true);
                storeWrite("test.string", "after");
                storeWrite("test.enum", "active");
                storeWrite("test.array", [2, 3.5, 4]);
                if (storeRead("test.number") !== 1) throw new Error("number");
                if (storeRead("test.boolean") !== true) throw new Error("boolean");
                if (storeRead("test.string") !== "after") throw new Error("string");
                if (storeRead("test.enum") !== "active") throw new Error("enum");
                const array = storeRead("test.array");
                if (array.length !== 3 || array[0] !== 2 || array[1] !== 3.5 || array[2] !== 4) {
                    throw new Error("array");
                }
                "#,
                "store-read-write.js",
            )
            .unwrap();
        });

        let luau_ctx = ScriptCtx::new();
        define_runtime_test_store(&luau_ctx);
        let luau_registry = registry_for(luau_ctx.clone());
        let luau = LuauSubsystem::new(&luau_registry, &LuauConfig::default()).unwrap();
        luau.run_source::<()>(
            Which::Definition,
            r#"
            storeWrite("test.number", 5)
            storeWrite("test.boolean", true)
            storeWrite("test.string", "after")
            storeWrite("test.enum", "active")
            storeWrite("test.array", {2, 3.5, 4})
            assert(storeRead("test.number") == 1)
            assert(storeRead("test.boolean") == true)
            assert(storeRead("test.string") == "after")
            assert(storeRead("test.enum") == "active")
            local array = storeRead("test.array")
            assert(#array == 3 and array[1] == 2 and array[2] == 3.5 and array[3] == 4)
            "#,
            "store-read-write.luau",
        )
        .unwrap();

        let js_slots = js_ctx.slot_table.borrow();
        let luau_slots = luau_ctx.slot_table.borrow();
        for name in [
            "test.number",
            "test.boolean",
            "test.string",
            "test.enum",
            "test.array",
        ] {
            assert_eq!(js_slots.get(name), luau_slots.get(name), "slot {name}");
        }
    }

    #[test]
    fn readonly_script_write_is_rejected_but_engine_write_succeeds() {
        for runtime in ["quickjs", "luau"] {
            let ctx = ScriptCtx::new();
            let registry = registry_for(ctx.clone());
            match runtime {
                "quickjs" => {
                    let quickjs =
                        QuickJsSubsystem::new(&registry, &QuickJsConfig::default()).unwrap();
                    quickjs.definition_ctx().with(|qjs| {
                        run_script::<()>(
                            &qjs,
                            r#"storeWrite("player.health", 25);"#,
                            "readonly-store.js",
                        )
                        .unwrap();
                    });
                }
                "luau" => {
                    let luau = LuauSubsystem::new(&registry, &LuauConfig::default()).unwrap();
                    luau.run_source::<()>(
                        Which::Definition,
                        r#"storeWrite("player.health", 25)"#,
                        "readonly-store.luau",
                    )
                    .unwrap();
                }
                _ => unreachable!(),
            }

            assert_eq!(
                ctx.slot_table
                    .borrow()
                    .get("player.health")
                    .and_then(|slot| slot.value.as_ref()),
                None
            );
            write_store_slot(&ctx, "player.health", SlotValue::Number(75.0)).unwrap();
            assert_eq!(
                read_store_slot(&ctx, "player.health").unwrap(),
                SlotValue::Number(75.0)
            );
        }
    }

    #[test]
    fn store_write_type_enum_array_and_unknown_name_errors_preserve_values() {
        let js_ctx = ScriptCtx::new();
        define_runtime_test_store(&js_ctx);
        let js_registry = registry_for(js_ctx.clone());
        let quickjs = QuickJsSubsystem::new(&js_registry, &QuickJsConfig::default()).unwrap();
        quickjs.definition_ctx().with(|qjs| {
            run_script::<()>(
                &qjs,
                r#"
                for (const write of [
                    () => storeWrite("test.number", true),
                    () => storeWrite("test.enum", "missing"),
                    () => storeWrite("test.array", [1, NaN]),
                    () => storeWrite("test.missing", 1),
                ]) {
                    let threw = false;
                    try { write(); } catch (_) { threw = true; }
                    if (!threw) throw new Error("invalid write did not throw");
                }
                "#,
                "invalid-store-write.js",
            )
            .unwrap();
        });

        let luau_ctx = ScriptCtx::new();
        define_runtime_test_store(&luau_ctx);
        let luau_registry = registry_for(luau_ctx.clone());
        let luau = LuauSubsystem::new(&luau_registry, &LuauConfig::default()).unwrap();
        luau.run_source::<()>(
            Which::Definition,
            r#"
            local writes = {
                function() storeWrite("test.number", true) end,
                function() storeWrite("test.enum", "missing") end,
                function() storeWrite("test.array", {1, 0 / 0}) end,
                function() storeWrite("test.missing", 1) end,
            }
            for _, write in writes do
                local ok = pcall(write)
                assert(not ok)
            end
            "#,
            "invalid-store-write.luau",
        )
        .unwrap();

        for ctx in [&js_ctx, &luau_ctx] {
            assert_eq!(
                read_store_slot(ctx, "test.number").unwrap(),
                SlotValue::Number(0.5)
            );
            assert_eq!(
                read_store_slot(ctx, "test.enum").unwrap(),
                SlotValue::Enum("idle".to_string())
            );
            assert_eq!(
                read_store_slot(ctx, "test.array").unwrap(),
                SlotValue::Array(vec![0.0, 1.0])
            );
        }
    }

    #[test]
    fn engine_write_validates_types_enum_values_arrays_and_ranges() {
        let ctx = ScriptCtx::new();
        define_runtime_test_store(&ctx);

        write_store_slot(&ctx, "test.number", SlotValue::Number(-10.0)).unwrap();
        assert_eq!(
            read_store_slot(&ctx, "test.number").unwrap(),
            SlotValue::Number(0.0)
        );

        for (name, value) in [
            ("test.number", SlotValue::Boolean(true)),
            ("test.enum", SlotValue::Enum("missing".to_string())),
            ("test.array", SlotValue::Array(vec![1.0, f32::INFINITY])),
        ] {
            assert!(write_store_slot(&ctx, name, value).is_err());
        }
        assert!(read_store_slot(&ctx, "test.missing").is_err());
        assert!(write_store_slot(&ctx, "test.missing", SlotValue::Number(1.0)).is_err());
    }

    // --- M13 Goal F, Task 4: setState readonly-gated JSON write ---

    #[test]
    fn set_state_json_write_applies_to_writable_slot_with_validation() {
        let ctx = ScriptCtx::new();
        define_runtime_test_store(&ctx);

        // A number write coerces and clamps to the declared [0, 1] range.
        write_state_slot_json(&ctx, "test.number", &serde_json::json!(5)).unwrap();
        assert_eq!(
            read_store_slot(&ctx, "test.number").unwrap(),
            SlotValue::Number(1.0)
        );
        // Boolean / string / enum / array all coerce from their JSON forms.
        write_state_slot_json(&ctx, "test.boolean", &serde_json::json!(true)).unwrap();
        write_state_slot_json(&ctx, "test.string", &serde_json::json!("after")).unwrap();
        write_state_slot_json(&ctx, "test.enum", &serde_json::json!("active")).unwrap();
        write_state_slot_json(&ctx, "test.array", &serde_json::json!([2.0, 3.5])).unwrap();
        assert_eq!(
            read_store_slot(&ctx, "test.boolean").unwrap(),
            SlotValue::Boolean(true)
        );
        assert_eq!(
            read_store_slot(&ctx, "test.enum").unwrap(),
            SlotValue::Enum("active".to_string())
        );
        assert_eq!(
            read_store_slot(&ctx, "test.array").unwrap(),
            SlotValue::Array(vec![2.0, 3.5])
        );
    }

    #[test]
    fn set_state_json_write_rejects_readonly_slot_and_leaves_value_unchanged() {
        // `player.health` is an engine-owned readonly-to-scripts slot. setState
        // warns and no-ops; the value is unchanged. Distinct from the engine
        // bypass (`write_store_slot`), which succeeds — proven below.
        let ctx = ScriptCtx::new();
        write_store_slot(&ctx, "player.health", SlotValue::Number(50.0)).unwrap();

        write_state_slot_json(&ctx, "player.health", &serde_json::json!(25.0)).unwrap();
        assert_eq!(
            read_store_slot(&ctx, "player.health").unwrap(),
            SlotValue::Number(50.0),
            "readonly slot is unchanged after setState"
        );

        // The engine bypass still writes the readonly slot.
        write_store_slot(&ctx, "player.health", SlotValue::Number(75.0)).unwrap();
        assert_eq!(
            read_store_slot(&ctx, "player.health").unwrap(),
            SlotValue::Number(75.0)
        );
    }

    // --- M13 Text Entry, Task 1: text-edit reactions ---

    fn read_string(ctx: &ScriptCtx, name: &str) -> String {
        match read_store_slot(ctx, name).unwrap() {
            SlotValue::String(value) => value,
            other => panic!("expected string, got {other:?}"),
        }
    }

    #[test]
    fn ui_text_entry_is_engine_writable_string_slot() {
        let ctx = ScriptCtx::new();
        let table = ctx.slot_table.borrow();
        let slot = table.get("ui.textEntry").expect("ui.textEntry exists");
        assert_eq!(slot.schema.slot_type, SlotType::String);
        assert!(!slot.schema.readonly);
        assert_eq!(slot.schema.ownership, SlotOwnership::Engine);
        assert_eq!(slot.value, Some(SlotValue::String(String::new())));
    }

    #[test]
    fn append_text_appends_to_target_slot() {
        let ctx = ScriptCtx::new();
        apply_text_edit(&ctx, "ui.textEntry", TextEdit::Append("ab")).unwrap();
        apply_text_edit(&ctx, "ui.textEntry", TextEdit::Append("c")).unwrap();
        assert_eq!(read_string(&ctx, "ui.textEntry"), "abc");
    }

    #[test]
    fn backspace_text_removes_last_char() {
        let ctx = ScriptCtx::new();
        apply_text_edit(&ctx, "ui.textEntry", TextEdit::Append("abc")).unwrap();
        apply_text_edit(&ctx, "ui.textEntry", TextEdit::Backspace).unwrap();
        assert_eq!(read_string(&ctx, "ui.textEntry"), "ab");
    }

    #[test]
    fn backspace_text_on_empty_is_noop() {
        // No-op and (by design) no warning/no write: the value stays empty.
        let ctx = ScriptCtx::new();
        apply_text_edit(&ctx, "ui.textEntry", TextEdit::Backspace).unwrap();
        assert_eq!(read_string(&ctx, "ui.textEntry"), "");
    }

    #[test]
    fn backspace_text_removes_one_precomposed_multibyte_char() {
        // `é` as U+00E9 is a single `char` but two UTF-8 bytes. The char-pop
        // floor removes it whole, never splitting the UTF-8 sequence — the
        // result is valid UTF-8 ("a"), not a truncated byte sequence.
        let ctx = ScriptCtx::new();
        apply_text_edit(&ctx, "ui.textEntry", TextEdit::Append("a\u{00E9}")).unwrap();
        assert_eq!(read_string(&ctx, "ui.textEntry").chars().count(), 2);
        apply_text_edit(&ctx, "ui.textEntry", TextEdit::Backspace).unwrap();
        assert_eq!(read_string(&ctx, "ui.textEntry"), "a");
    }

    #[test]
    fn clear_text_empties_the_slot() {
        let ctx = ScriptCtx::new();
        apply_text_edit(&ctx, "ui.textEntry", TextEdit::Append("hello")).unwrap();
        apply_text_edit(&ctx, "ui.textEntry", TextEdit::Clear).unwrap();
        assert_eq!(read_string(&ctx, "ui.textEntry"), "");
    }

    #[test]
    fn text_edits_reject_readonly_slot_and_leave_value_unchanged() {
        // `input.mode` is an engine-owned readonly slot. Text edits ride the
        // same readonly-gated write as setState, so they warn and no-op. (It is
        // an enum, but readonly is checked before type coercion, so the write is
        // rejected on the readonly gate, leaving the value unchanged.)
        let ctx = ScriptCtx::new();
        apply_text_edit(&ctx, "input.mode", TextEdit::Append("x")).unwrap();
        apply_text_edit(&ctx, "input.mode", TextEdit::Clear).unwrap();
        assert_eq!(
            read_store_slot(&ctx, "input.mode").unwrap(),
            SlotValue::Enum("focus".to_string()),
            "readonly slot unchanged after text edits"
        );
    }

    #[test]
    fn text_edit_unknown_slot_errors() {
        let ctx = ScriptCtx::new();
        assert!(apply_text_edit(&ctx, "ui.missing", TextEdit::Append("x")).is_err());
    }

    #[test]
    fn set_state_json_write_errors_on_unknown_slot_and_type_mismatch() {
        let ctx = ScriptCtx::new();
        define_runtime_test_store(&ctx);
        assert!(write_state_slot_json(&ctx, "test.missing", &serde_json::json!(1)).is_err());
        // A boolean into a number slot is a type mismatch.
        assert!(write_state_slot_json(&ctx, "test.number", &serde_json::json!(true)).is_err());
        // The number slot is unchanged after the rejected write.
        assert_eq!(
            read_store_slot(&ctx, "test.number").unwrap(),
            SlotValue::Number(0.5)
        );
    }
}
