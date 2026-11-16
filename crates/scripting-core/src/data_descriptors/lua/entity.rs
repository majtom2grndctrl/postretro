// Data-context descriptors: Lua entity-descriptor converters.
// See: context/lib/scripting.md

use std::collections::HashSet;

use super::super::*;

/// Mirror of [`entity_descriptor_from_js`] for Luau tables. Shape:
/// `{ canonicalName?: string, ghost?: boolean, components?: { inventory?: { loadout?: string[] }, mesh?: MeshDescriptor, movement?: PlayerMovementDescriptor, lifeStates?: LifeStatesDescriptor, weapon?: WeaponDescriptor, touchable?: TouchableDescriptor, health?: HealthDescriptor, faction?: string, tolerance?: number, behavior?: BehaviorGraphDescriptor, light?: LightDescriptor, emitter?: BillboardEmitterComponent } }`.
///
/// `canonicalName` is optional; absence means the descriptor has no direct
/// map-placement form (see `EntityTypeDescriptor`).
pub fn entity_descriptor_from_lua(
    value: LuaValue,
) -> Result<EntityTypeDescriptor, DescriptorError> {
    // Keep the descriptor parser's faction and tolerance validation in parity
    // with the mod-manifest path, which retains the faction name for commit.
    let _ = entity_faction_name_from_lua(value.clone())?;
    let tolerance = entity_tolerance_from_lua(value.clone())?;
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
    let ghost = ghost_flag_lua(&table, DESCRIPTOR_GHOST_PATH)?;
    let mut inventory = None;
    let mut light = None;
    let mut emitter = None;
    let mut movement = None;
    let mut life_states = None;
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
            if components_table
                .contains_key("lifeStates")
                .map_err(lua_err)?
            {
                let raw: LuaValue = components_table.get("lifeStates").map_err(lua_err)?;
                if !matches!(raw, LuaValue::Nil) {
                    life_states = Some(life_states_from_lua(raw, movement.as_ref())?);
                }
            }
            if components_table.contains_key("weapon").map_err(lua_err)? {
                let raw: LuaValue = components_table.get("weapon").map_err(lua_err)?;
                if !matches!(raw, LuaValue::Nil) {
                    weapon = Some(super::weapon::weapon_descriptor_from_lua(raw)?);
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
                    if let LuaValue::Table(behavior_table) = &raw {
                        validate_behavior_raw_fields_lua(behavior_table)?;
                    }
                    let mut json = conv::lua_to_json(raw).map_err(lua_err)?;
                    normalize_behavior_selectors(&mut json)?;
                    lower_behavior(&mut json, canonical_name.as_deref())?;
                    validate_optional_knockback_object(&json, "components.behavior.knockback")?;
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
        faction: None,
        tolerance,
        canonical_name,
        ghost,
        inventory,
        light,
        emitter,
        movement,
        life_states,
        weapon,
        touchable,
        mesh,
        health,
        behavior,
    };
    Ok(descriptor)
}

/// Read an optional per-archetype tolerance without treating an explicit zero
/// as omission. It is resolved only for brain-bearing descriptor spawns.
pub fn entity_tolerance_from_lua(value: LuaValue) -> Result<Option<f32>, DescriptorError> {
    let table = lua_table(value, "entity entry")?;
    if !table.contains_key("components").map_err(lua_err)? {
        return Ok(None);
    }
    let components: LuaValue = table.get("components").map_err(lua_err)?;
    let LuaValue::Table(components) = components else {
        if matches!(components, LuaValue::Nil) {
            return Ok(None);
        }
        return Err(DescriptorError::InvalidShape {
            reason: "`components` must be a table".to_string(),
        });
    };
    get_optional_f32_lua(&components, "tolerance")?
        .map(|value| validate_finite_f32(value, "components.tolerance"))
        .transpose()
}

/// Read an optional named faction without resolving it. Resolution belongs to
/// the complete manifest commit, after every declaration is known.
pub fn entity_faction_name_from_lua(value: LuaValue) -> Result<Option<String>, DescriptorError> {
    let table = lua_table(value, "entity entry")?;
    if !table.contains_key("components").map_err(lua_err)? {
        return Ok(None);
    }
    let components: LuaValue = table.get("components").map_err(lua_err)?;
    let LuaValue::Table(components) = components else {
        if matches!(components, LuaValue::Nil) {
            return Ok(None);
        }
        return Err(DescriptorError::InvalidShape {
            reason: "`components` must be a table".to_string(),
        });
    };
    if !components.contains_key("faction").map_err(lua_err)? {
        return Ok(None);
    }
    let raw: LuaValue = components.get("faction").map_err(lua_err)?;
    let name = match raw {
        LuaValue::Nil => return Ok(None),
        LuaValue::String(value) => value.to_str().map_err(lua_err)?.to_string(),
        other => {
            return Err(DescriptorError::InvalidShape {
                reason: format!(
                    "`components.faction` must be a string when supplied, got {}",
                    other.type_name()
                ),
            });
        }
    };
    if name.is_empty() {
        return Err(DescriptorError::InvalidShape {
            reason: "`components.faction` must be a non-empty string when supplied".to_string(),
        });
    }
    Ok(Some(name))
}

/// Reject a supplied unsupported VM value (function, userdata, thread) for an
/// optional presentation-only string field before Luau's generic JSON bridge
/// can coerce it to `null` and serde mistakes it for an omitted key. Shared by
/// the weapon-sound, handle-sound, and behavior-activity-sound guards.
pub(super) fn validate_optional_string_field_lua(
    table: &Table,
    field: &str,
    path: &str,
) -> Result<(), DescriptorError> {
    let raw: LuaValue = table.get(field).map_err(lua_err)?;
    if matches!(&raw, LuaValue::Nil | LuaValue::String(_)) {
        return Ok(());
    }
    Err(DescriptorError::InvalidShape {
        reason: format!(
            "`{path}` must be a string when supplied, got {}",
            raw.type_name()
        ),
    })
}

/// Tables a raw-table walk has already checked, by identity. A handle shared
/// across rows is checked once, and a cyclic table ends the walk instead of
/// repeating it (the JSON bridge then rejects the cycle at its depth limit).
type VisitedTables = HashSet<*const std::ffi::c_void>;

/// Validate the behavior graph's raw Luau fields the JSON bridge would
/// launder into `null`, read as omitted: every stance and attack handle's
/// declaration, wherever the handle stands (the `attacks` list, an action, a
/// mark, a row, `initial`, another handle), and
/// `BehaviorActivityDescriptor.sound` (every envelope, root and nested
/// layers). Walks the raw tables directly, before that bridge runs.
fn validate_behavior_raw_fields_lua(behavior: &Table) -> Result<(), DescriptorError> {
    validate_handle_fields_lua(
        behavior,
        "components.behavior",
        0,
        &mut VisitedTables::new(),
    )?;
    validate_activity_sound_keys_lua(behavior, "components.behavior", &mut VisitedTables::new())
}

/// Bounds the walk's recursion on deep nesting; the visited set stops cycles.
/// Anything deeper fails the JSON bridge's own depth limit.
const MAX_HANDLE_SEARCH_DEPTH: usize = 64;

/// Check the declaration of every stance and attack handle reachable from
/// `table`, naming the handle's path (sequence entries as `[index]`, from 0).
fn validate_handle_fields_lua(
    table: &Table,
    path: &str,
    depth: usize,
    visited: &mut VisitedTables,
) -> Result<(), DescriptorError> {
    if depth > MAX_HANDLE_SEARCH_DEPTH || !visited.insert(table.to_pointer()) {
        return Ok(());
    }
    match table.raw_get::<LuaValue>("kind").map_err(lua_err)? {
        LuaValue::String(kind) if kind == "attack" => {
            validate_handle_declaration_lua(table, path, &["release", "recover"])?;
        }
        LuaValue::String(kind) if kind == "stance" => {
            validate_handle_declaration_lua(table, path, &["breakOn"])?;
        }
        _ => {}
    }
    for pair in table.pairs::<LuaValue, LuaValue>() {
        let (key, value) = pair.map_err(lua_err)?;
        let LuaValue::Table(child) = value else {
            continue;
        };
        let child_path = match key {
            LuaValue::Integer(index) => format!("{path}[{}]", index - 1),
            other => format!("{path}.{}", lua_key_to_string(&other)),
        };
        validate_handle_fields_lua(&child, &child_path, depth + 1, visited)?;
    }
    Ok(())
}

/// One handle's declaration and the option tables nested in it (`nested`).
/// A function, userdata or thread value crosses the JSON bridge as `null`, so
/// an optional field would silently read as omitted (a function `release.at`
/// as the defaulted `hit`, a function `sound` as none); QuickJS rejects these.
fn validate_handle_declaration_lua(
    handle: &Table,
    path: &str,
    nested: &[&str],
) -> Result<(), DescriptorError> {
    validate_optional_string_field_lua(handle, "sound", &format!("{path}.sound"))?;
    reject_vm_values_lua(handle, path)?;
    for field in nested {
        if let LuaValue::Table(options) = handle.raw_get::<LuaValue>(*field).map_err(lua_err)? {
            reject_vm_values_lua(&options, &format!("{path}.{field}"))?;
        }
    }
    Ok(())
}

/// Reject any value among `table`'s own fields that the JSON bridge turns
/// into `null`: a function, userdata, thread, vector or buffer.
fn reject_vm_values_lua(table: &Table, path: &str) -> Result<(), DescriptorError> {
    for pair in table.pairs::<LuaValue, LuaValue>() {
        let (key, value) = pair.map_err(lua_err)?;
        if !matches!(
            value,
            LuaValue::Nil
                | LuaValue::Boolean(_)
                | LuaValue::Integer(_)
                | LuaValue::Number(_)
                | LuaValue::String(_)
                | LuaValue::Table(_)
        ) {
            return Err(DescriptorError::InvalidShape {
                reason: format!(
                    "`{path}.{}` must be a string, number, boolean or table, got {}; the engine would read it as omitted",
                    lua_key_to_string(&key),
                    value.type_name()
                ),
            });
        }
    }
    Ok(())
}

/// Recurses through one behavior envelope's `activities` map, then into every
/// nested `layers` envelope, checking each activity's `sound` field.
fn validate_activity_sound_keys_lua(
    envelope: &Table,
    path: &str,
    visited: &mut VisitedTables,
) -> Result<(), DescriptorError> {
    if !visited.insert(envelope.to_pointer()) {
        return Ok(());
    }
    let LuaValue::Table(activities) = envelope.get::<LuaValue>("activities").map_err(lua_err)?
    else {
        return Ok(());
    };
    for pair in activities.pairs::<LuaValue, LuaValue>() {
        let (name, activity) = pair.map_err(lua_err)?;
        let LuaValue::Table(activity) = activity else {
            continue;
        };
        let activity_path = format!("{path}.activities.{}", lua_key_to_string(&name));
        validate_optional_string_field_lua(&activity, "sound", &format!("{activity_path}.sound"))?;
        if let LuaValue::Table(layers) = activity.get::<LuaValue>("layers").map_err(lua_err)? {
            for pair in layers.pairs::<LuaValue, LuaValue>() {
                let (layer_name, layer) = pair.map_err(lua_err)?;
                let LuaValue::Table(layer) = layer else {
                    continue;
                };
                validate_activity_sound_keys_lua(
                    &layer,
                    &format!("{activity_path}.layers.{}", lua_key_to_string(&layer_name)),
                    visited,
                )?;
            }
        }
    }
    Ok(())
}

/// Best-effort display form of a Luau table key for an error path. Authored
/// `attacks`/`activities`/`layers` maps are always string-keyed; a non-string
/// key is itself malformed data serde will reject downstream, so this only
/// needs to name the offending entry, not validate the key.
fn lua_key_to_string(key: &LuaValue) -> String {
    match key {
        LuaValue::String(s) => s.to_str().map(|s| s.to_string()).unwrap_or_default(),
        LuaValue::Integer(i) => i.to_string(),
        LuaValue::Number(f) => f.to_string(),
        other => format!("<{}>", other.type_name()),
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
