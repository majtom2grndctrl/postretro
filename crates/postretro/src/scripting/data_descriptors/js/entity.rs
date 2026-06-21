// Data-context descriptors: JS entity-descriptor converters.
// See: context/lib/scripting.md

use super::super::*;

/// Deserialize an entity-type descriptor from a JS object. Shape:
/// `{ canonicalName?: string, defaultWeapon?: string, components?: { light?: LightDescriptor, emitter?: BillboardEmitterComponent, movement?: PlayerMovementDescriptor, weapon?: WeaponDescriptor } }`.
/// Component sub-objects parse via `serde_json` after a recursive walk through
/// the existing `js_to_json` helper — matches how `LightAnimation` /
/// `BillboardEmitterComponent` cross the FFI elsewhere.
///
/// `canonicalName` is optional; absence means the descriptor has no direct
/// map-placement form (see `EntityTypeDescriptor`).
pub(crate) fn entity_descriptor_from_js<'js>(
    ctx: &Ctx<'js>,
    value: JsValue<'js>,
) -> Result<EntityTypeDescriptor, DescriptorError> {
    let obj = Object::from_value(value).map_err(|_| DescriptorError::InvalidShape {
        reason: "entity entry must be an object".to_string(),
    })?;
    let canonical_name = if obj.contains_key("canonicalName").map_err(js_err)? {
        let raw: JsValue = obj.get("canonicalName").map_err(js_err)?;
        if raw.is_null() || raw.is_undefined() {
            None
        } else {
            Some(String::from_js_value_required(raw, "canonicalName")?)
        }
    } else {
        None
    };
    let default_weapon = if obj.contains_key("defaultWeapon").map_err(js_err)? {
        let raw: JsValue = obj.get("defaultWeapon").map_err(js_err)?;
        if raw.is_null() || raw.is_undefined() {
            None
        } else {
            Some(String::from_js_value_required(raw, "defaultWeapon")?)
        }
    } else {
        None
    };

    let mut light = None;
    let mut emitter = None;
    let mut movement = None;
    let mut weapon = None;
    let mut mesh = None;
    let mut health = None;
    let mut ai = None;

    if obj.contains_key("components").map_err(js_err)? {
        let components_val: JsValue = obj.get("components").map_err(js_err)?;
        if !components_val.is_null() && !components_val.is_undefined() {
            let components_obj =
                Object::from_value(components_val).map_err(|_| DescriptorError::InvalidShape {
                    reason: "`components` must be an object".to_string(),
                })?;
            if components_obj.contains_key("mesh").map_err(js_err)? {
                let raw: JsValue = components_obj.get("mesh").map_err(js_err)?;
                if !raw.is_null() && !raw.is_undefined() {
                    let mesh_obj =
                        Object::from_value(raw).map_err(|_| DescriptorError::InvalidShape {
                            reason: "`components.mesh` must be an object".to_string(),
                        })?;
                    mesh = Some(mesh_descriptor_from_js(&mesh_obj)?);
                }
            }
            if components_obj.contains_key("movement").map_err(js_err)? {
                let raw: JsValue = components_obj.get("movement").map_err(js_err)?;
                if !raw.is_null() && !raw.is_undefined() {
                    let m_obj =
                        Object::from_value(raw).map_err(|_| DescriptorError::InvalidShape {
                            reason: "`components.movement` must be an object".to_string(),
                        })?;
                    movement = Some(movement_descriptor_from_js(ctx, &m_obj)?);
                }
            }
            if components_obj.contains_key("weapon").map_err(js_err)? {
                let raw: JsValue = components_obj.get("weapon").map_err(js_err)?;
                if !raw.is_null() && !raw.is_undefined() {
                    let json = conv::js_to_json(ctx, raw).map_err(js_err)?;
                    let descriptor: WeaponDescriptor =
                        serde_json::from_value(json).map_err(|e| {
                            DescriptorError::InvalidShape {
                                reason: format!("`components.weapon` invalid: {e}"),
                            }
                        })?;
                    weapon = Some(descriptor.validate()?);
                }
            }
            if components_obj.contains_key("health").map_err(js_err)? {
                let raw: JsValue = components_obj.get("health").map_err(js_err)?;
                if !raw.is_null() && !raw.is_undefined() {
                    let json = conv::js_to_json(ctx, raw).map_err(js_err)?;
                    let descriptor: HealthDescriptor =
                        serde_json::from_value(json).map_err(|e| {
                            DescriptorError::InvalidShape {
                                reason: format!("`components.health` invalid: {e}"),
                            }
                        })?;
                    health = Some(descriptor.validate()?);
                }
            }
            if components_obj.contains_key("ai").map_err(js_err)? {
                let raw: JsValue = components_obj.get("ai").map_err(js_err)?;
                if !raw.is_null() && !raw.is_undefined() {
                    let json = conv::js_to_json(ctx, raw).map_err(js_err)?;
                    let descriptor: AiDescriptor = serde_json::from_value(json).map_err(|e| {
                        DescriptorError::InvalidShape {
                            reason: format!("`components.ai` invalid: {e}"),
                        }
                    })?;
                    ai = Some(descriptor.validate()?);
                }
            }
            if components_obj.contains_key("light").map_err(js_err)? {
                let raw: JsValue = components_obj.get("light").map_err(js_err)?;
                if !raw.is_null() && !raw.is_undefined() {
                    let json = conv::js_to_json(ctx, raw).map_err(js_err)?;
                    let descriptor: LightDescriptor =
                        serde_json::from_value(json).map_err(|e| {
                            DescriptorError::InvalidShape {
                                reason: format!("`components.light` invalid: {e}"),
                            }
                        })?;
                    light = Some(descriptor.validate()?);
                }
            }
            if components_obj.contains_key("emitter").map_err(js_err)? {
                let raw: JsValue = components_obj.get("emitter").map_err(js_err)?;
                if !raw.is_null() && !raw.is_undefined() {
                    let json = conv::js_to_json(ctx, raw).map_err(js_err)?;
                    let lit: BillboardEmitterComponentLit =
                        serde_json::from_value(json).map_err(|e| {
                            DescriptorError::InvalidShape {
                                reason: format!("`components.emitter` invalid: {e}"),
                            }
                        })?;
                    let validated =
                        lit.validate_into()
                            .map_err(|e| DescriptorError::InvalidShape {
                                reason: format!("`components.emitter` invalid: {e}"),
                            })?;
                    emitter = Some(validated);
                }
            }
        }
    }

    Ok(EntityTypeDescriptor {
        canonical_name,
        default_weapon,
        light,
        emitter,
        movement,
        weapon,
        mesh,
        health,
        ai,
    })
}

/// Parse a `components.mesh` object (JS). Shape:
/// `{ model: string, animations?: { [state]: { clip, loop?, crossfadeMs?, interrupt? } }, defaultState?: string }`.
/// Gathers raw fields and delegates validation to [`MeshDescriptor::build`] so
/// both FFI paths share identical rules.
pub(crate) fn mesh_descriptor_from_js<'js>(
    obj: &Object<'js>,
) -> Result<MeshDescriptor, DescriptorError> {
    let model = get_required_string_js(obj, "model")?;

    let mut animations_present = false;
    let mut states = Vec::new();
    if obj.contains_key("animations").map_err(js_err)? {
        let raw: JsValue = obj.get("animations").map_err(js_err)?;
        if !raw.is_null() && !raw.is_undefined() {
            animations_present = true;
            let anim_obj = Object::from_value(raw).map_err(|_| DescriptorError::InvalidShape {
                reason: "`components.mesh.animations` must be an object".to_string(),
            })?;
            // Iterate the map's own (name → state-object) entries.
            for entry in anim_obj.props::<String, JsValue>() {
                let (name, value) = entry.map_err(js_err)?;
                let state_obj =
                    Object::from_value(value).map_err(|_| DescriptorError::InvalidShape {
                        reason: format!("`components.mesh.animations.{name}` must be an object"),
                    })?;
                states.push(raw_animation_state_from_js(&name, &state_obj)?);
            }
        }
    }

    let default_state = if obj.contains_key("defaultState").map_err(js_err)? {
        let raw: JsValue = obj.get("defaultState").map_err(js_err)?;
        if raw.is_null() || raw.is_undefined() {
            None
        } else {
            Some(String::from_js_value_required(raw, "defaultState")?)
        }
    } else {
        None
    };

    MeshDescriptor::build(model, states, default_state, animations_present)
}

/// Gather one animation-state entry from a JS object. `loop` defaults to
/// `false`, `crossfadeMs` to [`DEFAULT_CROSSFADE_MS`],
/// `interrupt` is read raw (absent ⇒ `None`). Validation is deferred to
/// [`MeshDescriptor::build`].
pub(crate) fn raw_animation_state_from_js<'js>(
    name: &str,
    obj: &Object<'js>,
) -> Result<RawAnimationState, DescriptorError> {
    let clip = get_required_string_js(obj, "clip")?;
    let looping = get_optional_bool_js(obj, "loop")?.unwrap_or(false);
    let crossfade_ms = get_optional_f32_js(obj, "crossfadeMs")?.unwrap_or(DEFAULT_CROSSFADE_MS);
    let interrupt = if obj.contains_key("interrupt").map_err(js_err)? {
        let raw: JsValue = obj.get("interrupt").map_err(js_err)?;
        if raw.is_null() || raw.is_undefined() {
            None
        } else {
            Some(String::from_js_value_required(raw, "interrupt")?)
        }
    } else {
        None
    };
    Ok(RawAnimationState {
        name: name.to_string(),
        clip,
        looping,
        crossfade_ms,
        interrupt,
    })
}
