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

/// Mirror of [`crossing_descriptor_from_js`] for Luau tables. The discriminator
/// is presence of `predicate` (IR form) versus `slot` (threshold form).
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
    let edge = crossing_edge_from_lua(&table)?;

    if table.contains_key("predicate").map_err(lua_err)? {
        let raw: LuaValue = table.get("predicate").map_err(lua_err)?;
        let predicate = ir_node_from_json(
            conv::lua_to_json(raw).map_err(lua_err)?,
            "crossing entry `predicate`",
        )?;
        return Ok(build_predicate_crossing(predicate, edge, fire));
    }

    let slot = get_required_string_lua(&table, "slot")?;
    let below = get_optional_f32_lua(&table, "below")?;
    let above = get_optional_f32_lua(&table, "above")?;
    let max = get_optional_f32_lua(&table, "max")?;
    build_crossing(slot, below, above, max, edge, fire)
}

/// Luau twin of `crossing_edge_from_js`: present non-string values survive as
/// an invalid spelling for shared descriptor normalization to warn and drop.
fn crossing_edge_from_lua(table: &Table) -> Result<Option<String>, DescriptorError> {
    if !table.contains_key("edge").map_err(lua_err)? {
        return Ok(None);
    }
    let raw: LuaValue = table.get("edge").map_err(lua_err)?;
    match raw {
        LuaValue::String(value) => Ok(Some(value.to_str().map_err(lua_err)?.to_string())),
        _ => Ok(Some("<non-string>".to_string())),
    }
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
    let target = if table.contains_key("target").map_err(lua_err)? {
        match table.get("target").map_err(lua_err)? {
            LuaValue::Nil => None,
            LuaValue::String(s) => Some(s.to_str().map_err(lua_err)?.to_string()),
            other => {
                return Err(DescriptorError::InvalidShape {
                    reason: format!("'target' must be a string, got {}", other.type_name()),
                });
            }
        }
    } else {
        None
    };
    if target.is_some() && tag.is_some() {
        return Err(DescriptorError::InvalidShape {
            reason: "primitive reaction cannot carry both `target` and `tag`".to_string(),
        });
    }
    if target
        .as_deref()
        .is_some_and(|target| target != "@activators")
    {
        return Err(DescriptorError::InvalidShape {
            reason: "primitive `target` must be `@activators`".to_string(),
        });
    }
    if primitive == "spawnFromSpawner"
        && (tag.as_deref().is_none() || tag.as_deref().is_some_and(str::is_empty))
    {
        return Err(DescriptorError::InvalidShape {
            reason: "primitive `spawnFromSpawner` requires a non-empty `tag`".to_string(),
        });
    }

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

    validate_consequential_reaction(&primitive, tag.as_deref(), target.as_deref(), &args)?;

    Ok(PrimitiveDescriptor {
        primitive,
        target,
        tag,
        on_complete,
        args,
    })
}

/// Luau twin of the QuickJS primitive-specific consequential validation. The
/// two authoring runtimes must reject the same malformed reaction descriptors.
fn validate_consequential_reaction(
    primitive: &str,
    tag: Option<&str>,
    target: Option<&str>,
    args: &serde_json::Value,
) -> Result<(), DescriptorError> {
    if !matches!(primitive, "grantHealth" | "grantAmmo" | "addSlot") {
        return Ok(());
    }

    let has_non_empty_tag = tag.is_some_and(|tag| !tag.is_empty());
    if !matches!(
        (has_non_empty_tag, target),
        (true, None) | (false, Some("@activators"))
    ) {
        return Err(DescriptorError::InvalidShape {
            reason: format!(
                "primitive `{primitive}` requires exactly one of a non-empty `tag` or target `@activators`"
            ),
        });
    }

    let object = args
        .as_object()
        .ok_or_else(|| DescriptorError::InvalidShape {
            reason: format!("primitive `{primitive}` `args` must be an object"),
        })?;
    if primitive == "addSlot" {
        if object
            .get("slot")
            .and_then(serde_json::Value::as_str)
            .is_none()
        {
            return Err(DescriptorError::InvalidShape {
                reason: "primitive `addSlot` `args.slot` must be a string".to_string(),
            });
        }
        let Some(delta) = object.get("delta").and_then(serde_json::Value::as_f64) else {
            return Err(DescriptorError::InvalidShape {
                reason: "primitive `addSlot` `args.delta` must be a finite number".to_string(),
            });
        };
        if !delta.is_finite() || !(delta as f32).is_finite() {
            return Err(DescriptorError::InvalidShape {
                reason:
                    "primitive `addSlot` `args.delta` must be a finite number representable as f32"
                        .to_string(),
            });
        }
        return Ok(());
    }
    let Some(amount) = object.get("amount").and_then(serde_json::Value::as_f64) else {
        return Err(DescriptorError::InvalidShape {
            reason: format!("primitive `{primitive}` `args.amount` must be a finite number"),
        });
    };
    if !amount.is_finite() || !(amount as f32).is_finite() {
        return Err(DescriptorError::InvalidShape {
            reason: format!(
                "primitive `{primitive}` `args.amount` must be a finite number representable as f32"
            ),
        });
    }

    if primitive == "grantAmmo" {
        let Some(ammo_type) = object.get("type").and_then(serde_json::Value::as_str) else {
            return Err(DescriptorError::InvalidShape {
                reason: "primitive `grantAmmo` `args.type` must be a string".to_string(),
            });
        };
        validate_ascii_identifier("grantAmmo.type", ammo_type)?;
    }
    Ok(())
}

pub fn sequence_steps_from_lua(arr: &Table) -> Result<Vec<SequenceStep>, DescriptorError> {
    let len = validate_dense_lua_array(arr, "`sequence` field").map_err(|e| {
        DescriptorError::InvalidSequenceShape {
            reason: e.to_string(),
        }
    })?;
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
        let id = match step_table.get("id").map_err(lua_err)? {
            LuaValue::String(value) => match value.to_str().map_err(lua_err)?.as_ref() {
                "@activators" => SequenceTarget::Activators,
                "@trigger" => SequenceTarget::FiredTrigger,
                "@wait" => SequenceTarget::Wait,
                "@fire" => SequenceTarget::Fire,
                spelling => {
                    return Err(DescriptorError::InvalidSequenceShape {
                        reason: format!("step {i} has illegal sentinel `{spelling}`"),
                    });
                }
            },
            _ => {
                SequenceTarget::Entity(EntityId::from_raw(get_required_u32_lua(&step_table, "id")?))
            }
        };
        let primitive = get_required_string_lua(&step_table, "primitive")?;
        let primitive = validate_primitive_name(primitive)?;
        validate_control_step_pair(i, id, &primitive)?;
        if matches!(id, SequenceTarget::Activators)
            && matches!(primitive.as_str(), "armTrigger" | "disarmTrigger")
        {
            return Err(DescriptorError::InvalidSequenceShape {
                reason: format!(
                    "step {i} primitive `{primitive}` requires an entity id or `@trigger`, not `@activators`"
                ),
            });
        }
        let args = if step_table.contains_key("args").map_err(lua_err)? {
            let raw: LuaValue = step_table.get("args").map_err(lua_err)?;
            conv::lua_to_json(raw).map_err(lua_err)?
        } else {
            serde_json::Value::Null
        };
        out.push(SequenceStep {
            id,
            primitive,
            args,
        });
    }
    Ok(out)
}

/// Luau twin of the QuickJS canonical control-pair check. Keep the diagnostic
/// wording aligned so malformed raw descriptors degrade the same way.
fn validate_control_step_pair(
    step_index: i64,
    target: SequenceTarget,
    primitive: &str,
) -> Result<(), DescriptorError> {
    let mismatch = match (target, primitive) {
        (SequenceTarget::Wait, "wait") | (SequenceTarget::Fire, "fire") => None,
        (SequenceTarget::Wait, _) => Some(format!(
            "step {step_index} sentinel `@wait` requires primitive `wait`, got `{primitive}`"
        )),
        (SequenceTarget::Fire, _) => Some(format!(
            "step {step_index} sentinel `@fire` requires primitive `fire`, got `{primitive}`"
        )),
        (_, "wait") => Some(format!(
            "step {step_index} control primitive `wait` requires sentinel `@wait`; it cannot be entity-targeted"
        )),
        (_, "fire") => Some(format!(
            "step {step_index} control primitive `fire` requires sentinel `@fire`; it cannot be entity-targeted"
        )),
        _ => None,
    };
    match mismatch {
        Some(reason) => Err(DescriptorError::InvalidSequenceShape { reason }),
        None => Ok(()),
    }
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
