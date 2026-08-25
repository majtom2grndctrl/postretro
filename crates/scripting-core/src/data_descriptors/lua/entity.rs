// Data-context descriptors: Lua entity-descriptor converters.
// See: context/lib/scripting.md

use super::super::*;

/// Mirror of [`entity_descriptor_from_js`] for Luau tables. Shape:
/// `{ canonicalName?: string, components?: { inventory?: { loadout?: string[] }, mesh?: MeshDescriptor, movement?: PlayerMovementDescriptor, weapon?: WeaponDescriptor, touchable?: TouchableDescriptor, health?: HealthDescriptor, behavior?: BehaviorGraphDescriptor, light?: LightDescriptor, emitter?: BillboardEmitterComponent } }`.
///
/// `canonicalName` is optional; absence means the descriptor has no direct
/// map-placement form (see `EntityTypeDescriptor`).
pub fn entity_descriptor_from_lua(
    value: LuaValue,
) -> Result<EntityTypeDescriptor, DescriptorError> {
    let table = match value {
        LuaValue::Table(t) => t,
        other => {
            return Err(DescriptorError::InvalidShape {
                reason: format!("entity entry must be a table, got {}", other.type_name()),
            });
        }
    };
    let canonical_name = if table.contains_key("canonicalName").map_err(lua_err)? {
        let raw: LuaValue = table.get("canonicalName").map_err(lua_err)?;
        match raw {
            LuaValue::Nil => None,
            LuaValue::String(s) => Some(s.to_str().map_err(lua_err)?.to_string()),
            other => {
                return Err(DescriptorError::InvalidShape {
                    reason: format!(
                        "'canonicalName' must be a string, got {}",
                        other.type_name()
                    ),
                });
            }
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

    if table.contains_key("components").map_err(lua_err)? {
        let raw: LuaValue = table.get("components").map_err(lua_err)?;
        if !matches!(raw, LuaValue::Nil) {
            let components_table = match raw {
                LuaValue::Table(t) => t,
                other => {
                    return Err(DescriptorError::InvalidShape {
                        reason: format!("`components` must be a table, got {}", other.type_name()),
                    });
                }
            };
            if components_table
                .contains_key("inventory")
                .map_err(lua_err)?
            {
                let raw: LuaValue = components_table.get("inventory").map_err(lua_err)?;
                if !matches!(raw, LuaValue::Nil) {
                    let json = conv::lua_to_json(raw).map_err(lua_err)?;
                    inventory = Some(serde_json::from_value(json).map_err(|e| {
                        DescriptorError::InvalidShape {
                            reason: format!("`components.inventory` invalid: {e}"),
                        }
                    })?);
                }
            }
            if components_table.contains_key("mesh").map_err(lua_err)? {
                let raw: LuaValue = components_table.get("mesh").map_err(lua_err)?;
                if !matches!(raw, LuaValue::Nil) {
                    let mesh_table = match raw {
                        LuaValue::Table(t) => t,
                        other => {
                            return Err(DescriptorError::InvalidShape {
                                reason: format!(
                                    "`components.mesh` must be a table, got {}",
                                    other.type_name()
                                ),
                            });
                        }
                    };
                    mesh = Some(mesh_descriptor_from_lua(&mesh_table)?);
                }
            }
            if components_table.contains_key("movement").map_err(lua_err)? {
                let raw: LuaValue = components_table.get("movement").map_err(lua_err)?;
                if !matches!(raw, LuaValue::Nil) {
                    let m_table = match raw {
                        LuaValue::Table(t) => t,
                        other => {
                            return Err(DescriptorError::InvalidShape {
                                reason: format!(
                                    "`components.movement` must be a table, got {}",
                                    other.type_name()
                                ),
                            });
                        }
                    };
                    movement = Some(movement_descriptor_from_lua(&m_table)?);
                }
            }
            if components_table.contains_key("weapon").map_err(lua_err)? {
                let raw: LuaValue = components_table.get("weapon").map_err(lua_err)?;
                if !matches!(raw, LuaValue::Nil) {
                    if let LuaValue::Table(weapon_table) = &raw {
                        validate_optional_weapon_model_paths_lua(weapon_table)?;
                        validate_optional_weapon_placement_shape_lua(weapon_table)?;
                        validate_optional_projectile_shapes_lua(weapon_table)?;
                    }
                    let json = conv::lua_to_json(raw).map_err(lua_err)?;
                    let descriptor: WeaponDescriptor =
                        serde_json::from_value(json).map_err(|e| {
                            DescriptorError::InvalidShape {
                                reason: format!("`components.weapon` invalid: {e}"),
                            }
                        })?;
                    weapon = Some(descriptor.validate()?);
                }
            }
            if components_table
                .contains_key("touchable")
                .map_err(lua_err)?
            {
                let raw: LuaValue = components_table.get("touchable").map_err(lua_err)?;
                if !matches!(raw, LuaValue::Nil) {
                    let json = conv::lua_to_json(raw).map_err(lua_err)?;
                    let descriptor: TouchableDescriptor =
                        serde_json::from_value(json).map_err(|e| {
                            DescriptorError::InvalidShape {
                                reason: format!("`components.touchable` invalid: {e}"),
                            }
                        })?;
                    touchable = Some(descriptor.validate()?);
                }
            }
            if components_table.contains_key("health").map_err(lua_err)? {
                let raw: LuaValue = components_table.get("health").map_err(lua_err)?;
                if !matches!(raw, LuaValue::Nil) {
                    let json = conv::lua_to_json(raw).map_err(lua_err)?;
                    let descriptor: HealthDescriptor =
                        serde_json::from_value(json).map_err(|e| {
                            DescriptorError::InvalidShape {
                                reason: format!("`components.health` invalid: {e}"),
                            }
                        })?;
                    health = Some(descriptor.validate()?);
                }
            }
            if components_table.contains_key("ai").map_err(lua_err)? {
                let raw: LuaValue = components_table.get("ai").map_err(lua_err)?;
                if !matches!(raw, LuaValue::Nil) {
                    return Err(DescriptorError::InvalidShape {
                        reason:
                            "`components.ai` has been retired; author `components.behavior` instead"
                                .to_string(),
                    });
                }
            }
            if components_table.contains_key("behavior").map_err(lua_err)? {
                let raw: LuaValue = components_table.get("behavior").map_err(lua_err)?;
                if !matches!(raw, LuaValue::Nil) {
                    let mut json = conv::lua_to_json(raw).map_err(lua_err)?;
                    normalize_behavior_selectors(&mut json)?;
                    let descriptor: BehaviorGraphDescriptor = serde_json::from_value(json)
                        .map_err(|e| DescriptorError::InvalidShape {
                            reason: format!("`components.behavior` invalid: {e}"),
                        })?;
                    behavior = Some(descriptor.validate()?);
                }
            }
            if components_table.contains_key("light").map_err(lua_err)? {
                let raw: LuaValue = components_table.get("light").map_err(lua_err)?;
                if !matches!(raw, LuaValue::Nil) {
                    let json = conv::lua_to_json(raw).map_err(lua_err)?;
                    let descriptor: LightDescriptor =
                        serde_json::from_value(json).map_err(|e| {
                            DescriptorError::InvalidShape {
                                reason: format!("`components.light` invalid: {e}"),
                            }
                        })?;
                    light = Some(descriptor.validate()?);
                }
            }
            if components_table.contains_key("emitter").map_err(lua_err)? {
                let raw: LuaValue = components_table.get("emitter").map_err(lua_err)?;
                if !matches!(raw, LuaValue::Nil) {
                    let json = conv::lua_to_json(raw).map_err(lua_err)?;
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

/// Luau's generic JSON bridge maps functions/userdata/threads to JSON null.
/// Reject those values for optional weapon presentation strings before serde
/// can mistake malformed supplied input for omission.
fn validate_optional_weapon_model_paths_lua(weapon: &Table) -> Result<(), DescriptorError> {
    for field in ["thirdPersonModel", "viewmodel"] {
        if !weapon.contains_key(field).map_err(lua_err)? {
            continue;
        }
        let raw: LuaValue = weapon.get(field).map_err(lua_err)?;
        if matches!(&raw, LuaValue::Nil | LuaValue::String(_)) {
            continue;
        }
        return Err(DescriptorError::InvalidShape {
            reason: format!(
                "`components.weapon.{field}` must be a string when supplied, got {}",
                raw.type_name()
            ),
        });
    }
    Ok(())
}

/// Placement is authored presentation data. Reject a supplied unsupported VM
/// value before the JSON bridge can coerce it to `null` and serde treats it as
/// an omitted placement.
fn validate_optional_weapon_placement_shape_lua(weapon: &Table) -> Result<(), DescriptorError> {
    optional_table_field_lua(weapon, "placement", "components.weapon.placement", true)?;
    Ok(())
}

fn validate_optional_projectile_shapes_lua(weapon: &Table) -> Result<(), DescriptorError> {
    let Some(projectile) =
        optional_table_field_lua(weapon, "projectile", "components.weapon.projectile", false)?
    else {
        return Ok(());
    };
    let Some(visual) = optional_table_field_lua(
        &projectile,
        "visual",
        "components.weapon.projectile.visual",
        false,
    )?
    else {
        return Ok(());
    };
    let Some(trail) = optional_table_field_lua(
        &visual,
        "trail",
        "components.weapon.projectile.visual.trail",
        true,
    )?
    else {
        return Ok(());
    };
    optional_table_field_lua(
        &trail,
        "spinAnimation",
        "components.weapon.projectile.visual.trail.spinAnimation",
        true,
    )?;
    Ok(())
}

fn optional_table_field_lua(
    parent: &Table,
    field: &str,
    path: &str,
    reject_malformed: bool,
) -> Result<Option<Table>, DescriptorError> {
    if !parent.contains_key(field).map_err(lua_err)? {
        return Ok(None);
    }
    match parent.get::<LuaValue>(field).map_err(lua_err)? {
        LuaValue::Nil => Ok(None),
        LuaValue::Table(table) => Ok(Some(table)),
        _ if !reject_malformed => Ok(None),
        _ => Err(DescriptorError::InvalidShape {
            reason: format!("`{path}` must be an object when supplied"),
        }),
    }
}

/// Mirror of [`mesh_descriptor_from_js`] for Luau tables. Gathers raw fields
/// and delegates validation to [`MeshDescriptor::build`].
pub fn mesh_descriptor_from_lua(table: &Table) -> Result<MeshDescriptor, DescriptorError> {
    let model = get_required_string_lua(table, "model")?;

    let mut attachments = HashMap::new();
    if table.contains_key("attachments").map_err(lua_err)? {
        let raw: LuaValue = table.get("attachments").map_err(lua_err)?;
        match raw {
            LuaValue::Table(attachment_table) => {
                for pair in attachment_table.pairs::<LuaValue, LuaValue>() {
                    let (key, value) = pair.map_err(lua_err)?;
                    let socket = match key {
                        LuaValue::String(value) => value.to_str().map_err(lua_err)?.to_string(),
                        other => {
                            return Err(DescriptorError::InvalidShape {
                                reason: format!(
                                    "`components.mesh.attachments` must be a socket-name map, got {} key",
                                    other.type_name()
                                ),
                            });
                        }
                    };
                    let attachment_model = match value {
                        LuaValue::String(value) => value.to_str().map_err(lua_err)?.to_string(),
                        other => {
                            return Err(DescriptorError::InvalidShape {
                                reason: format!(
                                    "`components.mesh.attachments.{socket}` must be a string, got {}",
                                    other.type_name()
                                ),
                            });
                        }
                    };
                    attachments.insert(socket, attachment_model);
                }
            }
            other => {
                return Err(DescriptorError::InvalidShape {
                    reason: format!(
                        "`components.mesh.attachments` must be a table, got {}",
                        other.type_name()
                    ),
                });
            }
        }
    }

    let mut animations_present = false;
    let mut states = Vec::new();
    if table.contains_key("animations").map_err(lua_err)? {
        let raw: LuaValue = table.get("animations").map_err(lua_err)?;
        if !matches!(raw, LuaValue::Nil) {
            animations_present = true;
            let anim_table = match raw {
                LuaValue::Table(t) => t,
                other => {
                    return Err(DescriptorError::InvalidShape {
                        reason: format!(
                            "`components.mesh.animations` must be a table, got {}",
                            other.type_name()
                        ),
                    });
                }
            };
            // Iterate the map's (name → state-table) pairs.
            for pair in anim_table.pairs::<String, LuaValue>() {
                let (name, value) = pair.map_err(lua_err)?;
                let state_table = match value {
                    LuaValue::Table(t) => t,
                    other => {
                        return Err(DescriptorError::InvalidShape {
                            reason: format!(
                                "`components.mesh.animations.{name}` must be a table, got {}",
                                other.type_name()
                            ),
                        });
                    }
                };
                states.push(raw_animation_state_from_lua(&name, &state_table)?);
            }
        }
    }

    let default_state = if table.contains_key("defaultState").map_err(lua_err)? {
        let raw: LuaValue = table.get("defaultState").map_err(lua_err)?;
        match raw {
            LuaValue::Nil => None,
            LuaValue::String(s) => Some(s.to_str().map_err(lua_err)?.to_string()),
            other => {
                return Err(DescriptorError::InvalidShape {
                    reason: format!("'defaultState' must be a string, got {}", other.type_name()),
                });
            }
        }
    } else {
        None
    };

    let shadow_bias_scale = get_optional_f32_lua(table, "shadowBiasScale")?;
    let shadow_only = get_optional_bool_lua(table, "shadowOnly")?.unwrap_or(false);

    // Optional `locomotion` block: `{ speedScale?: bool }`. Absent block ⇒ None
    // ⇒ the runtime `speed_scale = true` default; `speedScale` itself defaults
    // to `true` when the block is present but omits the field.
    let locomotion = if table.contains_key("locomotion").map_err(lua_err)? {
        let raw: LuaValue = table.get("locomotion").map_err(lua_err)?;
        match raw {
            LuaValue::Nil => None,
            LuaValue::Table(loco_table) => Some(LocomotionDescriptor::from_optional_speed_scale(
                get_optional_bool_lua(&loco_table, "speedScale")?,
            )),
            other => {
                return Err(DescriptorError::InvalidShape {
                    reason: format!(
                        "`components.mesh.locomotion` must be a table, got {}",
                        other.type_name()
                    ),
                });
            }
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

/// Gather one animation-state entry from a Luau table. Mirrors
/// [`raw_animation_state_from_js`]: `loop` defaults to `false`, `crossfadeMs`
/// to [`DEFAULT_CROSSFADE_MS`], `interrupt` read raw.
pub fn raw_animation_state_from_lua(
    name: &str,
    table: &Table,
) -> Result<RawAnimationState, DescriptorError> {
    let clip = get_required_string_lua(table, "clip")?;
    let looping = get_optional_bool_lua(table, "loop")?.unwrap_or(false);
    let crossfade_ms = get_optional_f32_lua(table, "crossfadeMs")?.unwrap_or(DEFAULT_CROSSFADE_MS);
    // Optional per-state `travelSpeed` override, read raw here; positivity /
    // finiteness is validated in `MeshDescriptor::build`.
    let travel_speed = get_optional_f32_lua(table, "travelSpeed")?;
    let interrupt = if table.contains_key("interrupt").map_err(lua_err)? {
        let raw: LuaValue = table.get("interrupt").map_err(lua_err)?;
        match raw {
            LuaValue::Nil => None,
            LuaValue::String(s) => Some(s.to_str().map_err(lua_err)?.to_string()),
            other => {
                return Err(DescriptorError::InvalidShape {
                    reason: format!("'interrupt' must be a string, got {}", other.type_name()),
                });
            }
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

/// Re-seat empty Luau tables only where the recursive descriptor declares an
/// array. The `transitions` container is now an adjacency map, but every one
/// of its VALUES is an ordered row list; the map itself remains an object.
fn normalize_behavior_selectors(json: &mut serde_json::Value) -> Result<(), DescriptorError> {
    fn visit(envelope: &mut serde_json::Value, path: &str) -> Result<(), DescriptorError> {
        if let Some(transitions) = envelope
            .get_mut("transitions")
            .and_then(|value| value.as_object_mut())
        {
            for (source, rows) in transitions.iter_mut() {
                let rows_path = format!("{path}.transitions.{source}");
                let Some(map) = rows.as_object() else {
                    continue;
                };
                if !map.is_empty() {
                    return Err(DescriptorError::InvalidShape {
                        reason: format!(
                            "`{rows_path}` must be an ordered array of guarded rows; found a table with named keys ({})",
                            map.keys().cloned().collect::<Vec<_>>().join(", "),
                        ),
                    });
                }
                *rows = serde_json::Value::Array(Vec::new());
            }
        }
        let Some(activities) = envelope
            .get_mut("activities")
            .and_then(|value| value.as_object_mut())
        else {
            return Ok(());
        };
        for (activity_name, activity) in activities.iter_mut() {
            let activity_path = format!("{path}.activities.{activity_name}");
            let Some(layers) = activity
                .get_mut("layers")
                .and_then(|value| value.as_object_mut())
            else {
                continue;
            };
            for (layer_name, layer) in layers.iter_mut() {
                let layer_path = format!("{activity_path}.layers.{layer_name}");
                let empty_table = layer.as_object().is_some_and(|map| map.is_empty());
                if empty_table && matches!(layer_name.as_str(), "move" | "offense") {
                    *layer = serde_json::Value::Array(Vec::new());
                    continue;
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
