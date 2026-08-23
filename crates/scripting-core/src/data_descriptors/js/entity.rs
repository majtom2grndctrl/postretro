// Data-context descriptors: JS entity-descriptor converters.
// See: context/lib/scripting.md

use super::super::*;
use rquickjs::object::Filter;

/// Deserialize an entity-type descriptor from a JS object. Shape:
/// `{ canonicalName?: string, components?: { inventory?: { loadout?: string[] }, mesh?: MeshDescriptor, movement?: PlayerMovementDescriptor, weapon?: WeaponDescriptor, touchable?: TouchableDescriptor, health?: HealthDescriptor, behavior?: BehaviorGraphDescriptor, light?: LightDescriptor, emitter?: BillboardEmitterComponent } }`.
/// Component sub-objects parse via `serde_json` after a recursive walk through
/// the existing `js_to_json` helper — matches how `LightAnimation` /
/// `BillboardEmitterComponent` cross the FFI elsewhere.
///
/// `canonicalName` is optional; absence means the descriptor has no direct
/// map-placement form (see `EntityTypeDescriptor`).
pub fn entity_descriptor_from_js<'js>(
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
    let mut inventory = None;
    let mut light = None;
    let mut emitter = None;
    let mut movement = None;
    let mut weapon = None;
    let mut touchable = None;
    let mut mesh = None;
    let mut health = None;
    let mut behavior = None;

    if obj.contains_key("components").map_err(js_err)? {
        let components_val: JsValue = obj.get("components").map_err(js_err)?;
        if !components_val.is_null() && !components_val.is_undefined() {
            let components_obj =
                Object::from_value(components_val).map_err(|_| DescriptorError::InvalidShape {
                    reason: "`components` must be an object".to_string(),
                })?;
            if components_obj.contains_key("inventory").map_err(js_err)? {
                let raw: JsValue = components_obj.get("inventory").map_err(js_err)?;
                if !raw.is_null() && !raw.is_undefined() {
                    let json = conv::js_to_json(ctx, raw).map_err(js_err)?;
                    inventory = Some(serde_json::from_value(json).map_err(|e| {
                        DescriptorError::InvalidShape {
                            reason: format!("`components.inventory` invalid: {e}"),
                        }
                    })?);
                }
            }
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
                    if let Some(weapon_obj) = raw.as_object() {
                        validate_optional_weapon_model_paths_js(weapon_obj)?;
                        validate_optional_projectile_shapes_js(weapon_obj)?;
                    }
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
            if components_obj.contains_key("touchable").map_err(js_err)? {
                let raw: JsValue = components_obj.get("touchable").map_err(js_err)?;
                if !raw.is_null() && !raw.is_undefined() {
                    let json = conv::js_to_json(ctx, raw).map_err(js_err)?;
                    let descriptor: TouchableDescriptor =
                        serde_json::from_value(json).map_err(|e| {
                            DescriptorError::InvalidShape {
                                reason: format!("`components.touchable` invalid: {e}"),
                            }
                        })?;
                    touchable = Some(descriptor.validate()?);
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
            if has_own_string_key(&components_obj, "ai")? {
                return Err(DescriptorError::InvalidShape {
                    reason:
                        "`components.ai` has been retired; author `components.behavior` instead"
                            .to_string(),
                });
            }
            if components_obj.contains_key("behavior").map_err(js_err)? {
                let raw: JsValue = components_obj.get("behavior").map_err(js_err)?;
                if !raw.is_null() && !raw.is_undefined() {
                    let json = conv::js_to_json(ctx, raw).map_err(js_err)?;
                    reject_object_where_move_selector_belongs(&json)?;
                    let descriptor: BehaviorGraphDescriptor = serde_json::from_value(json)
                        .map_err(|e| DescriptorError::InvalidShape {
                            reason: format!("`components.behavior` invalid: {e}"),
                        })?;
                    behavior = Some(descriptor.validate()?);
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

    let descriptor = EntityTypeDescriptor {
        canonical_name,
        inventory,
        light,
        emitter,
        movement,
        weapon,
        touchable,
        mesh,
        health,
        behavior,
    };
    Ok(descriptor)
}

/// `Object::contains_key` follows the JavaScript prototype chain. The migration
/// boundary deliberately rejects only an own legacy key: inherited data is not
/// authored descriptor content, while an own `null` or `undefined` key is.
fn has_own_string_key(object: &Object<'_>, wanted: &str) -> Result<bool, DescriptorError> {
    object
        .own_keys::<String>(Filter::new().string())
        .try_fold(false, |found, key| {
            Ok(found || key.map_err(js_err)? == wanted)
        })
}

/// The generic JSON bridge intentionally maps unsupported VM values to JSON
/// null for broad descriptor compatibility. These optional strings cannot use
/// that degradation: a supplied function/symbol would silently disable weapon
/// presentation after serde interpreted null as `None`.
fn validate_optional_weapon_model_paths_js<'js>(
    weapon: &Object<'js>,
) -> Result<(), DescriptorError> {
    for field in ["thirdPersonModel", "viewmodel"] {
        if !weapon.contains_key(field).map_err(js_err)? {
            continue;
        }
        let raw: JsValue = weapon.get(field).map_err(js_err)?;
        if raw.is_null() || raw.is_undefined() || raw.as_string().is_some() {
            continue;
        }
        return Err(DescriptorError::InvalidShape {
            reason: format!("`components.weapon.{field}` must be a string when supplied"),
        });
    }
    Ok(())
}

fn validate_optional_projectile_shapes_js<'js>(
    weapon: &Object<'js>,
) -> Result<(), DescriptorError> {
    let Some(projectile) =
        optional_object_field_js(weapon, "projectile", "components.weapon.projectile", false)?
    else {
        return Ok(());
    };
    let Some(visual) = optional_object_field_js(
        &projectile,
        "visual",
        "components.weapon.projectile.visual",
        false,
    )?
    else {
        return Ok(());
    };
    let Some(trail) = optional_object_field_js(
        &visual,
        "trail",
        "components.weapon.projectile.visual.trail",
        true,
    )?
    else {
        return Ok(());
    };
    optional_object_field_js(
        &trail,
        "spinAnimation",
        "components.weapon.projectile.visual.trail.spinAnimation",
        true,
    )?;
    Ok(())
}

fn optional_object_field_js<'js>(
    parent: &Object<'js>,
    field: &str,
    path: &str,
    reject_malformed: bool,
) -> Result<Option<Object<'js>>, DescriptorError> {
    if !parent.contains_key(field).map_err(js_err)? {
        return Ok(None);
    }
    let raw: JsValue = parent.get(field).map_err(js_err)?;
    if raw.is_null() || raw.is_undefined() {
        return Ok(None);
    }
    if raw.type_of() != rquickjs::Type::Object {
        if reject_malformed {
            return Err(DescriptorError::InvalidShape {
                reason: format!("`{path}` must be an object when supplied"),
            });
        }
        return Ok(None);
    }
    Ok(raw.as_object().cloned())
}

/// JavaScript distinguishes arrays from objects. A `move` layer is always a
/// selector list, so name its path rather than leaving serde to report an
/// unhelpful untagged-enum failure. Nested graph layers are objects and recurse
/// through the same check.
fn reject_object_where_move_selector_belongs(
    json: &serde_json::Value,
) -> Result<(), DescriptorError> {
    fn visit(envelope: &serde_json::Value, path: &str) -> Result<(), DescriptorError> {
        let Some(activities) = envelope
            .get("activities")
            .and_then(|value| value.as_object())
        else {
            return Ok(());
        };
        for (activity_name, activity) in activities {
            let activity_path = format!("{path}.activities.{activity_name}");
            let Some(layers) = activity.get("layers").and_then(|value| value.as_object()) else {
                continue;
            };
            for (layer_name, layer) in layers {
                let layer_path = format!("{activity_path}.layers.{layer_name}");
                if layer_name == "move" && layer.is_object() {
                    return Err(DescriptorError::InvalidShape {
                        reason: format!(
                            "`{layer_path}` must be an array selector ending in a MotionVerb fallback"
                        ),
                    });
                }
                if layer.is_object() {
                    visit(layer, &layer_path)?;
                }
            }
        }
        Ok(())
    }

    visit(json, "components.behavior")
}

/// Parse a `components.mesh` object (JS). Shape:
/// `{ model: string, attachments?: { [socket]: string }, animations?: { [state]: { clip, loop?, crossfadeMs?, interrupt? } }, defaultState?: string }`.
/// Gathers raw fields and delegates validation to [`MeshDescriptor::build`] so
/// both FFI paths share identical rules.
pub fn mesh_descriptor_from_js<'js>(obj: &Object<'js>) -> Result<MeshDescriptor, DescriptorError> {
    let model = get_required_string_js(obj, "model")?;

    let mut attachments = HashMap::new();
    if obj.contains_key("attachments").map_err(js_err)? {
        let raw: JsValue = obj.get("attachments").map_err(js_err)?;
        if raw.type_of() != rquickjs::Type::Object {
            return Err(DescriptorError::InvalidShape {
                reason: "`components.mesh.attachments` must be a plain object map".to_string(),
            });
        }
        let attachment_obj =
            Object::from_value(raw).map_err(|_| DescriptorError::InvalidShape {
                reason: "`components.mesh.attachments` must be a plain object map".to_string(),
            })?;
        if let Some(prototype) = attachment_obj.get_prototype() {
            let object_constructor: Object = attachment_obj
                .ctx()
                .globals()
                .get("Object")
                .map_err(js_err)?;
            let object_prototype: Object = object_constructor.get("prototype").map_err(js_err)?;
            if prototype.as_value() != object_prototype.as_value() {
                return Err(DescriptorError::InvalidShape {
                    reason: "`components.mesh.attachments` must be a plain object map".to_string(),
                });
            }
        }
        for entry in attachment_obj.props::<String, JsValue>() {
            let (socket, value) = entry.map_err(js_err)?;
            let attachment_model = value
                .as_string()
                .ok_or_else(|| DescriptorError::InvalidShape {
                    reason: format!("`components.mesh.attachments.{socket}` must be a string"),
                })?
                .to_string()
                .map_err(js_err)?;
            attachments.insert(socket, attachment_model);
        }
    }

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

    let shadow_bias_scale = get_optional_f32_js(obj, "shadowBiasScale")?;
    let shadow_only = get_optional_bool_js(obj, "shadowOnly")?.unwrap_or(false);

    // Optional `locomotion` block: `{ speedScale?: bool }`. Absent block ⇒ None
    // ⇒ the runtime `speed_scale = true` default; `speedScale` itself defaults
    // to `true` when the block is present but omits the field.
    let locomotion = if obj.contains_key("locomotion").map_err(js_err)? {
        let raw: JsValue = obj.get("locomotion").map_err(js_err)?;
        if raw.is_null() || raw.is_undefined() {
            None
        } else {
            let loco_obj = Object::from_value(raw).map_err(|_| DescriptorError::InvalidShape {
                reason: "`components.mesh.locomotion` must be an object".to_string(),
            })?;
            Some(LocomotionDescriptor::from_optional_speed_scale(
                get_optional_bool_js(&loco_obj, "speedScale")?,
            ))
        }
    } else {
        None
    };

    MeshDescriptor::build(RawMeshDescriptor {
        model,
        attachments,
        states,
        default_state,
        animations_present,
        locomotion,
        shadow_bias_scale,
        shadow_only,
    })
}

/// Gather one animation-state entry from a JS object. `loop` defaults to
/// `false`, `crossfadeMs` to [`DEFAULT_CROSSFADE_MS`],
/// `interrupt` is read raw (absent ⇒ `None`). Validation is deferred to
/// [`MeshDescriptor::build`].
pub fn raw_animation_state_from_js<'js>(
    name: &str,
    obj: &Object<'js>,
) -> Result<RawAnimationState, DescriptorError> {
    let clip = get_required_string_js(obj, "clip")?;
    let looping = get_optional_bool_js(obj, "loop")?.unwrap_or(false);
    let crossfade_ms = get_optional_f32_js(obj, "crossfadeMs")?.unwrap_or(DEFAULT_CROSSFADE_MS);
    // Optional per-state `travelSpeed` override, read raw here; positivity /
    // finiteness is validated in `MeshDescriptor::build`.
    let travel_speed = get_optional_f32_js(obj, "travelSpeed")?;
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
        travel_speed,
    })
}
