// Data-context descriptors: Lua reaction/crossing converters.
// See: context/lib/scripting.md

use super::super::*;

// --- Lua deserialization ----------------------------------------------------

pub fn named_reaction_from_lua(value: LuaValue) -> Result<NamedReaction, DescriptorError> {
    let table = match value {
        LuaValue::Table(t) => t,
        other => {
            return Err(DescriptorError::InvalidShape {
                reason: format!("reaction entry must be a table, got {}", other.type_name()),
            });
        }
    };
    let name = get_required_string_lua(&table, "name")?;

    let has_progress = table.contains_key("progress").map_err(lua_err)?;
    let has_primitive = table.contains_key("primitive").map_err(lua_err)?;
    let has_sequence = table.contains_key("sequence").map_err(lua_err)?;

    let descriptor = if has_progress {
        let progress: Table = table.get("progress").map_err(lua_err)?;
        ReactionDescriptor::Progress(progress_descriptor_from_lua(&progress)?)
    } else if has_sequence {
        let arr: Table =
            table
                .get("sequence")
                .map_err(|e| DescriptorError::InvalidSequenceShape {
                    reason: e.to_string(),
                })?;
        ReactionDescriptor::Sequence(sequence_steps_from_lua(&arr)?)
    } else if has_primitive {
        ReactionDescriptor::Primitive(primitive_descriptor_from_lua(&table)?)
    } else {
        return Err(DescriptorError::UnknownShape);
    };

    Ok(NamedReaction { name, descriptor })
}

pub fn progress_descriptor_from_lua(table: &Table) -> Result<ProgressDescriptor, DescriptorError> {
    let tag = get_required_string_lua(table, "tag")?;
    let at = get_required_f32_lua(table, "at")?;
    let at = validate_at(at)?;
    let fire = get_required_string_lua(table, "fire")?;
    Ok(ProgressDescriptor { tag, at, fire })
}

/// Mirror of [`crossing_descriptor_from_js`] for Luau tables. Shape:
/// `{ slot: string, below?: number, above?: number, max?: number, fire: {string} }`.
/// Delegates validation to [`build_crossing`].
pub fn crossing_descriptor_from_lua(
    value: LuaValue,
) -> Result<CrossingDescriptor, DescriptorError> {
    let table = match value {
        LuaValue::Table(t) => t,
        other => {
            return Err(DescriptorError::InvalidShape {
                reason: format!("crossing entry must be a table, got {}", other.type_name()),
            });
        }
    };
    let slot = get_required_string_lua(&table, "slot")?;
    let below = get_optional_f32_lua(&table, "below")?;
    let above = get_optional_f32_lua(&table, "above")?;
    let max = get_optional_f32_lua(&table, "max")?;

    let fire_arr: Table = table
        .get("fire")
        .map_err(|_| DescriptorError::InvalidShape {
            reason: "crossing entry `fire` must be an array of event names".to_string(),
        })?;
    let len = validate_dense_lua_array(&fire_arr, "crossing entry `fire`")?;
    let mut fire = Vec::with_capacity(len);
    for i in 1..=(len as i64) {
        let item: LuaValue = fire_arr.get(i).map_err(lua_err)?;
        match item {
            LuaValue::String(s) => fire.push(s.to_str().map_err(lua_err)?.to_string()),
            other => {
                return Err(DescriptorError::InvalidShape {
                    reason: format!(
                        "crossing entry `fire` elements must be strings, got {}",
                        other.type_name()
                    ),
                });
            }
        }
    }

    build_crossing(slot, below, above, max, fire)
}

pub fn primitive_descriptor_from_lua(
    table: &Table,
) -> Result<PrimitiveDescriptor, DescriptorError> {
    let primitive = get_required_string_lua(table, "primitive")?;
    let primitive = validate_primitive_name(primitive)?;
    // `tag` is optional: absent ⇒ system-targeted reaction (no entities).
    let tag = if table.contains_key("tag").map_err(lua_err)? {
        let raw: LuaValue = table.get("tag").map_err(lua_err)?;
        match raw {
            LuaValue::Nil => None,
            LuaValue::String(s) => Some(s.to_str().map_err(lua_err)?.to_string()),
            other => {
                return Err(DescriptorError::InvalidShape {
                    reason: format!("'tag' must be a string, got {}", other.type_name()),
                });
            }
        }
    } else {
        None
    };

    let on_complete = if table.contains_key("onComplete").map_err(lua_err)? {
        let raw: LuaValue = table.get("onComplete").map_err(lua_err)?;
        match raw {
            LuaValue::Nil => None,
            LuaValue::String(s) => Some(s.to_str().map_err(lua_err)?.to_string()),
            other => {
                return Err(DescriptorError::InvalidShape {
                    reason: format!("'onComplete' must be a string, got {}", other.type_name()),
                });
            }
        }
    } else {
        None
    };

    // `args` carries the primitive's payload. Absent / nil defaults to an
    // empty object so primitives that take no arguments still deserialize.
    let args = if table.contains_key("args").map_err(lua_err)? {
        let raw: LuaValue = table.get("args").map_err(lua_err)?;
        match raw {
            LuaValue::Nil => serde_json::Value::Object(Default::default()),
            other => conv::lua_to_json(other).map_err(lua_err)?,
        }
    } else {
        serde_json::Value::Object(Default::default())
    };

    Ok(PrimitiveDescriptor {
        primitive,
        tag,
        on_complete,
        args,
    })
}

pub fn sequence_steps_from_lua(arr: &Table) -> Result<Vec<SequenceStep>, DescriptorError> {
    let len = arr.raw_len();
    let mut out = Vec::with_capacity(len);
    for i in 1..=(len as i64) {
        let item: LuaValue = arr.get(i).map_err(lua_err)?;
        let step_table = match item {
            LuaValue::Table(t) => t,
            other => {
                return Err(DescriptorError::InvalidSequenceShape {
                    reason: format!("step {i} must be a table, got {}", other.type_name()),
                });
            }
        };
        let id_raw = get_required_u32_lua(&step_table, "id")?;
        let primitive = get_required_string_lua(&step_table, "primitive")?;
        let primitive = validate_primitive_name(primitive)?;
        let args = if step_table.contains_key("args").map_err(lua_err)? {
            let raw: LuaValue = step_table.get("args").map_err(lua_err)?;
            conv::lua_to_json(raw).map_err(lua_err)?
        } else {
            serde_json::Value::Null
        };
        out.push(SequenceStep {
            id: EntityId::from_raw(id_raw),
            primitive,
            args,
        });
    }
    Ok(out)
}

pub fn get_required_u32_lua(table: &Table, field: &'static str) -> Result<u32, DescriptorError> {
    if !table.contains_key(field).map_err(lua_err)? {
        return Err(DescriptorError::MissingField { field });
    }
    let raw: LuaValue = table.get(field).map_err(lua_err)?;
    match raw {
        LuaValue::Nil => Err(DescriptorError::MissingField { field }),
        LuaValue::Integer(i) => {
            if i < 0 || i > u32::MAX as i64 {
                Err(DescriptorError::InvalidShape {
                    reason: format!("'{field}' must be a non-negative integer in u32 range"),
                })
            } else {
                Ok(i as u32)
            }
        }
        LuaValue::Number(f) => {
            if !f.is_finite() || f < 0.0 || f > u32::MAX as f64 || f.fract() != 0.0 {
                Err(DescriptorError::InvalidShape {
                    reason: format!("'{field}' must be an integer in u32 range"),
                })
            } else {
                Ok(f as u32)
            }
        }
        other => Err(DescriptorError::InvalidShape {
            reason: format!("'{field}' must be a number, got {}", other.type_name()),
        }),
    }
}
