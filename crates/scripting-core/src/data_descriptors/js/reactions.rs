// Data-context descriptors: JS reaction/crossing converters.
// See: context/lib/scripting.md

use super::super::*;

pub fn named_reaction_from_js<'js>(
    ctx: &Ctx<'js>,
    value: JsValue<'js>,
) -> Result<NamedReaction, DescriptorError> {
    let obj = Object::from_value(value).map_err(|_| DescriptorError::InvalidShape {
        reason: "reaction entry must be an object".to_string(),
    })?;

    let name: String = get_required_string_js(&obj, "name")?;

    // Discriminator: presence of `progress` / `primitive` / `sequence` keys.
    let has_progress = obj.contains_key("progress").map_err(js_err)?;
    let has_primitive = obj.contains_key("primitive").map_err(js_err)?;
    let has_sequence = obj.contains_key("sequence").map_err(js_err)?;

    let descriptor = if has_progress {
        let progress_obj: Object = obj.get("progress").map_err(js_err)?;
        ReactionDescriptor::Progress(progress_descriptor_from_js(ctx, &progress_obj)?)
    } else if has_sequence {
        let arr: Array =
            obj.get("sequence")
                .map_err(|e| DescriptorError::InvalidSequenceShape {
                    reason: e.to_string(),
                })?;
        ReactionDescriptor::Sequence(sequence_steps_from_js(ctx, &arr)?)
    } else if has_primitive {
        ReactionDescriptor::Primitive(primitive_descriptor_from_js(ctx, &obj)?)
    } else {
        return Err(DescriptorError::UnknownShape);
    };

    Ok(NamedReaction { name, descriptor })
}

pub fn progress_descriptor_from_js<'js>(
    _ctx: &Ctx<'js>,
    obj: &Object<'js>,
) -> Result<ProgressDescriptor, DescriptorError> {
    let tag = get_required_string_js(obj, "tag")?;
    let at: f32 = get_required_f32_js(obj, "at")?;
    let at = validate_at(at)?;
    let fire = get_required_string_js(obj, "fire")?;
    Ok(ProgressDescriptor { tag, at, fire })
}

/// Deserialize one crossing entry from a JS object. The discriminator is
/// presence of `predicate` (IR form) versus `slot` (threshold form).
pub fn crossing_descriptor_from_js<'js>(
    ctx: &Ctx<'js>,
    value: &JsValue<'js>,
) -> Result<CrossingDescriptor, DescriptorError> {
    let obj = Object::from_value(value.clone()).map_err(|_| DescriptorError::InvalidShape {
        reason: "crossing entry must be an object".to_string(),
    })?;
    let fire_arr: Array = obj.get("fire").map_err(|_| DescriptorError::InvalidShape {
        reason: "crossing entry `fire` must be an array of event names".to_string(),
    })?;
    let mut fire = Vec::with_capacity(fire_arr.len());
    for i in 0..fire_arr.len() {
        let item: JsValue = fire_arr.get(i).map_err(js_err)?;
        fire.push(String::from_js_value_required(item, "fire")?);
    }
    let edge = crossing_edge_from_js(&obj)?;

    if obj.contains_key("predicate").map_err(js_err)? {
        let raw: JsValue = obj.get("predicate").map_err(js_err)?;
        let predicate = ir_node_from_json(
            conv::js_to_json(ctx, raw).map_err(js_err)?,
            "crossing entry `predicate`",
        )?;
        return Ok(build_predicate_crossing(predicate, edge, fire));
    }

    let slot = get_required_string_js(&obj, "slot")?;
    let below = get_optional_f32_js(&obj, "below")?;
    let above = get_optional_f32_js(&obj, "above")?;
    let max = get_optional_f32_js(&obj, "max")?;
    build_crossing(slot, below, above, max, edge, fire)
}

/// Preserve every present edge value for the shared validator. A non-string
/// marker is deliberately not a valid edge spelling, so both VM converters
/// reach the same warn-and-degrade path instead of rejecting the descriptor.
fn crossing_edge_from_js<'js>(obj: &Object<'js>) -> Result<Option<String>, DescriptorError> {
    if !obj.contains_key("edge").map_err(js_err)? {
        return Ok(None);
    }
    let raw: JsValue = obj.get("edge").map_err(js_err)?;
    match raw.as_string() {
        Some(value) => value.to_string().map(Some).map_err(js_err),
        None => Ok(Some("<non-string>".to_string())),
    }
}

pub fn primitive_descriptor_from_js<'js>(
    ctx: &Ctx<'js>,
    obj: &Object<'js>,
) -> Result<PrimitiveDescriptor, DescriptorError> {
    let primitive = get_required_string_js(obj, "primitive")?;
    let primitive = validate_primitive_name(primitive)?;
    // `tag` is optional: absent ⇒ system-targeted reaction (no entities).
    let tag = if obj.contains_key("tag").map_err(js_err)? {
        let raw: JsValue = obj.get("tag").map_err(js_err)?;
        if raw.is_null() || raw.is_undefined() {
            None
        } else {
            Some(String::from_js_value_required(raw, "tag")?)
        }
    } else {
        None
    };
    let target = if obj.contains_key("target").map_err(js_err)? {
        let raw: JsValue = obj.get("target").map_err(js_err)?;
        if raw.is_null() || raw.is_undefined() {
            None
        } else {
            Some(String::from_js_value_required(raw, "target")?)
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

    let on_complete = if obj.contains_key("onComplete").map_err(js_err)? {
        let raw: JsValue = obj.get("onComplete").map_err(js_err)?;
        if raw.is_null() || raw.is_undefined() {
            None
        } else {
            Some(String::from_js_value_required(raw, "onComplete")?)
        }
    } else {
        None
    };

    // `args` is the primitive's typed payload. Absent / null defaults to an
    // empty object so primitives that take no arguments still deserialize.
    let args = if obj.contains_key("args").map_err(js_err)? {
        let raw: JsValue = obj.get("args").map_err(js_err)?;
        if raw.is_null() || raw.is_undefined() {
            serde_json::Value::Object(Default::default())
        } else {
            conv::js_to_json(ctx, raw).map_err(js_err)?
        }
    } else {
        serde_json::Value::Object(Default::default())
    };

    validate_grant_reaction(&primitive, tag.as_deref(), target.as_deref(), &args)?;

    Ok(PrimitiveDescriptor {
        primitive,
        target,
        tag,
        on_complete,
        args,
    })
}

/// Grant reactions are the only primitive descriptors whose payload is a
/// fixed author-time literal rather than free-form JSON. Keep this validation
/// in the VM converter so a malformed setup descriptor cannot reach a
/// reaction handler or a fixed-tick trigger binding.
fn validate_grant_reaction(
    primitive: &str,
    tag: Option<&str>,
    target: Option<&str>,
    args: &serde_json::Value,
) -> Result<(), DescriptorError> {
    if !matches!(primitive, "grantHealth" | "grantAmmo") {
        return Ok(());
    }

    if tag.is_none() && target != Some("@activators") {
        return Err(DescriptorError::InvalidShape {
            reason: format!(
                "primitive `{primitive}` requires exactly one of a `tag` or target `@activators`"
            ),
        });
    }

    let object = args
        .as_object()
        .ok_or_else(|| DescriptorError::InvalidShape {
            reason: format!("primitive `{primitive}` `args` must be an object"),
        })?;
    let Some(amount) = object.get("amount").and_then(serde_json::Value::as_f64) else {
        return Err(DescriptorError::InvalidShape {
            reason: format!("primitive `{primitive}` `args.amount` must be a finite number"),
        });
    };
    if !amount.is_finite() {
        return Err(DescriptorError::InvalidShape {
            reason: format!("primitive `{primitive}` `args.amount` must be a finite number"),
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

pub fn sequence_steps_from_js<'js>(
    ctx: &Ctx<'js>,
    arr: &Array<'js>,
) -> Result<Vec<SequenceStep>, DescriptorError> {
    let mut out = Vec::with_capacity(arr.len());
    for i in 0..arr.len() {
        let item: JsValue = arr.get(i).map_err(js_err)?;
        let obj = Object::from_value(item).map_err(|_| DescriptorError::InvalidSequenceShape {
            reason: format!("step {i} must be an object"),
        })?;
        let id_value: JsValue = obj.get("id").map_err(js_err)?;
        let id = if let Some(value) = id_value.as_string() {
            match value.to_string().map_err(js_err)?.as_str() {
                "@activators" => SequenceTarget::Activators,
                "@trigger" => SequenceTarget::FiredTrigger,
                spelling => {
                    return Err(DescriptorError::InvalidSequenceShape {
                        reason: format!("step {i} has illegal sentinel `{spelling}`"),
                    });
                }
            }
        } else {
            SequenceTarget::Entity(EntityId::from_raw(get_required_u32_js(&obj, "id")?))
        };
        let primitive = get_required_string_js(&obj, "primitive")?;
        let primitive = validate_primitive_name(primitive)?;
        if matches!(id, SequenceTarget::Activators)
            && matches!(primitive.as_str(), "armTrigger" | "disarmTrigger")
        {
            return Err(DescriptorError::InvalidSequenceShape {
                reason: format!(
                    "step {i} primitive `{primitive}` requires an entity id or `@trigger`, not `@activators`"
                ),
            });
        }
        let args = if obj.contains_key("args").map_err(js_err)? {
            let raw: JsValue = obj.get("args").map_err(js_err)?;
            conv::js_to_json(ctx, raw).map_err(js_err)?
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

pub fn get_required_u32_js<'js>(
    obj: &Object<'js>,
    field: &'static str,
) -> Result<u32, DescriptorError> {
    if !obj.contains_key(field).map_err(js_err)? {
        return Err(DescriptorError::MissingField { field });
    }
    let raw: JsValue = obj.get(field).map_err(js_err)?;
    if raw.is_null() || raw.is_undefined() {
        return Err(DescriptorError::MissingField { field });
    }
    if let Some(i) = raw.as_int() {
        if i < 0 {
            return Err(DescriptorError::InvalidShape {
                reason: format!("'{field}' must be a non-negative integer"),
            });
        }
        return Ok(i as u32);
    }
    // Entity IDs are safe as f64: they use `index << 16 | generation`, keeping
    // the high bits clear and well within the 2^53 integer-exact range of f64.
    if let Some(f) = raw.as_float() {
        if !f.is_finite() || f < 0.0 || f > u32::MAX as f64 || f.fract() != 0.0 {
            return Err(DescriptorError::InvalidShape {
                reason: format!("'{field}' must be an integer in u32 range"),
            });
        }
        return Ok(f as u32);
    }
    Err(DescriptorError::InvalidShape {
        reason: format!("'{field}' must be a number"),
    })
}
