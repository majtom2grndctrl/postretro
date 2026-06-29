// Durable-state store contract helpers and VM manifest drains.
// See: context/lib/scripting.md §5

use std::collections::{BTreeMap, BTreeSet};

use mlua::{Table as LuaTable, Value as LuaValue};
use rquickjs::{Array, Ctx, Object as JsObject, Value as JsValue};
use serde::Deserialize;
use serde_json::Value;

use crate::conv::{js_to_json, lua_to_json};
use crate::ctx::ScriptCtx;
use crate::error::ScriptError;
use crate::primitive_adapters::{
    ScriptSlotValue, StoreDeclarationManifest, StoreDefinition, StoreStateRefs,
};
use crate::slot_table::{
    NumericRange, ReplicationScope, SlotOwnership, SlotRecord, SlotSchema, SlotTable, SlotType,
    SlotValue, StoreDeclaration, StoreDeclarationSet,
};

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
    #[serde(default)]
    network: Option<String>,
}

pub fn write_store_slot(ctx: &ScriptCtx, name: &str, value: SlotValue) -> Result<(), ScriptError> {
    let mut table = ctx.slot_table.borrow_mut();
    let slot = table
        .get_mut(name)
        .ok_or_else(|| unknown_slot("storeWrite", name))?;
    slot.value = Some(validate_slot_value(name, &slot.schema, value)?);
    Ok(())
}

pub fn read_store_slot(ctx: &ScriptCtx, name: &str) -> Result<SlotValue, ScriptError> {
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

pub fn write_script_store_slot(
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

pub fn apply_store_slot_batch(
    table: &mut SlotTable,
    writes: &[(String, SlotValue)],
) -> Result<(), ScriptError> {
    let mut validated = Vec::with_capacity(writes.len());
    for (name, value) in writes {
        let slot = table
            .get(name)
            .ok_or_else(|| unknown_slot("stateApply", name))?;
        let checked = validate_slot_value(name, &slot.schema, value.clone())?;
        validated.push((name, checked));
    }

    for (name, value) in validated {
        if let Some(slot) = table.get_mut(name) {
            slot.value = Some(value);
        }
    }
    Ok(())
}

pub fn write_state_slot_json(
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

#[derive(Clone, Debug, PartialEq)]
pub enum TextEdit<'a> {
    Append(&'a str),
    Backspace,
    Clear,
}

pub fn apply_text_edit(ctx: &ScriptCtx, name: &str, edit: TextEdit<'_>) -> Result<(), ScriptError> {
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
            if current.is_empty() {
                return Ok(());
            }
            let mut next = current;
            next.pop();
            next
        }
        TextEdit::Clear => String::new(),
    };

    write_state_slot_json(ctx, name, &Value::String(next))
}

pub fn define_store(namespace: &str, schema: Value) -> Result<StoreDefinition, ScriptError> {
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

pub fn store_declaration_from_manifest_value(
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

pub fn store_declaration_set_from_values(
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

pub fn drain_store_declarations_js<'js>(
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

pub fn drain_store_declarations_lua(table: &LuaTable) -> Result<StoreDeclarationSet, ScriptError> {
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

pub fn store_declaration(namespace: &str, schema: Value) -> Result<StoreDeclaration, ScriptError> {
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
        network,
    } = input;

    let network = replication_scope_for(slot_name, network.as_deref())?;
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
        network,
    }))
}

fn replication_scope_for(
    slot_name: &str,
    network: Option<&str>,
) -> Result<ReplicationScope, ScriptError> {
    match network {
        None => Ok(ReplicationScope::None),
        Some("shared") => Ok(ReplicationScope::SharedGlobal),
        Some("ownerPrivate") => Err(ScriptError::InvalidArgument {
            reason: format!(
                "defineStore: slot `{slot_name}` `network: \"ownerPrivate\"` is not supported for \
                 mod stores yet (no per-player authoring namespace exists); use `network: \"shared\"` \
                 for a server-replicated global slot, or omit `network` for a local-only slot"
            ),
        }),
        Some(other) => Err(ScriptError::InvalidArgument {
            reason: format!(
                "defineStore: slot `{slot_name}` has unknown `network` value `{other}`; the only \
                 accepted value is `\"shared\"` (replicate to every connected client), or omit \
                 `network` for a local-only slot"
            ),
        }),
    }
}

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

pub fn validate_slot_value(
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

fn wrong_default(slot_name: &str, expected: &str) -> ScriptError {
    ScriptError::InvalidArgument {
        reason: format!("defineStore: slot `{slot_name}` default must be {expected}"),
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

fn invalid_schema(slot_name: Option<&str>, error: serde_json::Error) -> ScriptError {
    let location = slot_name
        .map(|name| format!(" for slot `{name}`"))
        .unwrap_or_default();
    ScriptError::InvalidArgument {
        reason: format!("defineStore: malformed schema{location}: {error}"),
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
