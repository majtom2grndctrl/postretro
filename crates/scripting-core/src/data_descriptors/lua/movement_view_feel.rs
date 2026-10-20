// Render-only movement view-feel descriptor parsing.
// See: context/lib/scripting.md

use super::super::*;

pub fn view_feel_params_from_lua(table: &Table) -> Result<ViewFeelParams, DescriptorError> {
    let bob = match read_optional_subtable_lua(table, "bob", "movement.viewFeel.bob")? {
        Some(t) => Some(bob_params_from_lua(&t)?),
        None => None,
    };
    let tilt = match read_optional_subtable_lua(table, "tilt", "movement.viewFeel.tilt")? {
        Some(t) => Some(tilt_params_from_lua(&t)?),
        None => None,
    };
    let sway = match read_optional_subtable_lua(table, "sway", "movement.viewFeel.sway")? {
        Some(t) => Some(sway_params_from_lua(&t)?),
        None => None,
    };
    let impulse = match read_optional_subtable_lua(table, "impulse", "movement.viewFeel.impulse")? {
        Some(t) => Some(impulse_params_from_lua(&t)?),
        None => None,
    };
    for moved in ["slide", "vault"] {
        if !table.get::<LuaValue>(moved).map_err(lua_err)?.is_nil() {
            return Err(moved_view_block(moved));
        }
    }
    let pose = pose_params_from_value(pose_from_lua(table)?)?;
    Ok(ViewFeelParams {
        bob,
        tilt,
        sway,
        impulse,
        pose,
    })
}

/// Read `viewFeel.pose` for the shared validator. Only a raw `pose` entry
/// counts; one `viewFeel` reaches through `__index` is rejected rather than
/// read.
fn pose_from_lua(view_feel: &Table) -> Result<Option<PoseValue>, DescriptorError> {
    let raw: LuaValue = view_feel.raw_get("pose").map_err(lua_err)?;
    if raw.is_nil() {
        if !view_feel.get::<LuaValue>("pose").map_err(lua_err)?.is_nil() {
            return Err(inherited_pose_block());
        }
        return Ok(None);
    }
    pose_value_from_lua(raw, POSE_BLOCK_DEPTH)
}

/// Classify one pose value, the twin of the JS `pose_value_from_js`.
/// `blocks_left` counts the blocks that may still nest here; a table past it
/// is not read.
fn pose_value_from_lua(
    raw: LuaValue,
    blocks_left: u8,
) -> Result<Option<PoseValue>, DescriptorError> {
    let table = match raw {
        LuaValue::Nil => return Ok(None),
        LuaValue::Integer(value) => return Ok(Some(PoseValue::Number(value as f64))),
        LuaValue::Number(value) => return Ok(Some(PoseValue::Number(value))),
        LuaValue::Table(table) if blocks_left > 0 => table,
        _ => return Ok(Some(PoseValue::Other)),
    };
    let mut block = PoseBlock::default();
    for pair in table.pairs::<LuaValue, LuaValue>() {
        let (key, value) = pair.map_err(lua_err)?;
        // JS rejects an array as a block, so a table with array entries, or
        // any key but a string, is no block either.
        let LuaValue::String(key) = key else {
            return Ok(Some(PoseValue::Other));
        };
        let key = key.to_str().map_err(lua_err)?.to_string();
        if let Some(value) = pose_value_from_lua(value, blocks_left - 1)? {
            block.fields.insert(key, value);
        }
    }
    // Walk the `__index` tables a read would fall through to. A function or
    // any other `__index` hides what it supplies, so it cannot be named.
    let mut current = table;
    let mut walked = 0;
    loop {
        let Some(metatable) = current.metatable() else {
            break;
        };
        match metatable.raw_get::<LuaValue>("__index").map_err(lua_err)? {
            LuaValue::Nil => break,
            LuaValue::Table(index) if walked < MAX_INHERITANCE_WALK => {
                for pair in index.pairs::<LuaValue, LuaValue>() {
                    match pair.map_err(lua_err)?.0 {
                        LuaValue::String(key) => {
                            block
                                .inherited
                                .insert(key.to_str().map_err(lua_err)?.to_string());
                        }
                        _ => block.inherits_unnamed = true,
                    }
                }
                current = index;
                walked += 1;
            }
            _ => {
                block.inherits_unnamed = true;
                break;
            }
        }
    }
    Ok(Some(PoseValue::Block(block)))
}

pub fn impulse_params_from_lua(table: &Table) -> Result<ImpulseParams, DescriptorError> {
    let tension = validate_in_range_finite(
        get_required_f32_lua(table, "tension")?,
        ImpulseParams::MIN_TENSION,
        ImpulseParams::MAX_TENSION,
        "movement.viewFeel.impulse.tension",
    )?;
    let max = get_required_table_lua(table, "max")?;
    let states = get_required_table_lua(table, "states")?;
    Ok(ImpulseParams {
        tension,
        max: impulse_max_from_lua(&max)?,
        states: impulse_states_from_lua(&states)?,
    })
}

fn impulse_max_from_lua(table: &Table) -> Result<ImpulseChannels, DescriptorError> {
    Ok(ImpulseChannels {
        fov: validate_in_range_finite(
            get_required_f32_lua(table, "fov")?,
            0.0,
            ImpulseParams::MAX_CHANNEL_MAGNITUDE,
            "movement.viewFeel.impulse.max.fov",
        )?,
        pitch: validate_in_range_finite(
            get_required_f32_lua(table, "pitch")?,
            0.0,
            ImpulseParams::MAX_CHANNEL_MAGNITUDE,
            "movement.viewFeel.impulse.max.pitch",
        )?,
        roll: validate_in_range_finite(
            get_required_f32_lua(table, "roll")?,
            0.0,
            ImpulseParams::MAX_CHANNEL_MAGNITUDE,
            "movement.viewFeel.impulse.max.roll",
        )?,
    })
}

fn impulse_states_from_lua(table: &Table) -> Result<ImpulseStates, DescriptorError> {
    validate_impulse_state_keys_lua(table)?;
    Ok(ImpulseStates {
        normal: optional_impulse_state_from_lua(table, "normal")?,
        dash: optional_impulse_state_from_lua(table, "dash")?,
        crouch: optional_impulse_state_from_lua(table, "crouch")?,
        slide: optional_impulse_state_from_lua(table, "slide")?,
        vault: optional_impulse_state_from_lua(table, "vault")?,
    })
}

fn validate_impulse_state_keys_lua(table: &Table) -> Result<(), DescriptorError> {
    for pair in table.clone().pairs::<LuaValue, LuaValue>() {
        let (key, _) = pair.map_err(lua_err)?;
        let LuaValue::String(key) = key else {
            return Err(DescriptorError::InvalidShape {
                reason: format!(
                    "`movement.viewFeel.impulse.states` keys must be strings, got {}",
                    key.type_name()
                ),
            });
        };
        let key = key.to_str().map_err(lua_err)?.to_string();
        if !matches!(
            key.as_str(),
            "normal" | "dash" | "crouch" | "slide" | "vault"
        ) {
            return Err(DescriptorError::InvalidShape {
                reason: format!(
                    "`movement.viewFeel.impulse.states.{key}` is not a supported movement state; expected normal, dash, crouch, slide, or vault"
                ),
            });
        }
    }
    Ok(())
}

fn optional_impulse_state_from_lua(
    table: &Table,
    field: &'static str,
) -> Result<Option<ImpulseStateParams>, DescriptorError> {
    // State rows are authored descriptor content only when stored directly on
    // the table. `raw_get` keeps metatable `__index` rows out of the contract.
    let state = match table.raw_get::<LuaValue>(field).map_err(lua_err)? {
        LuaValue::Nil => return Ok(None),
        LuaValue::Table(state) => state,
        other => {
            return Err(DescriptorError::InvalidShape {
                reason: format!(
                    "`movement.viewFeel.impulse.states.{field}` must be a table, got {}",
                    other.type_name()
                ),
            });
        }
    };
    let tension = match get_optional_f32_lua(&state, "tension")? {
        Some(value) => Some(validate_in_range_finite(
            value,
            ImpulseParams::MIN_TENSION,
            ImpulseParams::MAX_TENSION,
            &format!("movement.viewFeel.impulse.states.{field}.tension"),
        )?),
        None => None,
    };
    Ok(Some(ImpulseStateParams {
        tension,
        enter: optional_impulse_channels_from_lua(&state, "enter", field)?,
        exit: optional_impulse_channels_from_lua(&state, "exit", field)?,
    }))
}

fn optional_impulse_channels_from_lua(
    state: &Table,
    field: &'static str,
    state_name: &'static str,
) -> Result<Option<ImpulseChannels>, DescriptorError> {
    let Some(channels) = read_optional_subtable_lua(
        state,
        field,
        &format!("movement.viewFeel.impulse.states.{state_name}.{field}"),
    )?
    else {
        return Ok(None);
    };
    let path =
        |channel: &str| format!("movement.viewFeel.impulse.states.{state_name}.{field}.{channel}");
    Ok(Some(ImpulseChannels {
        fov: validate_in_range_finite(
            get_required_f32_lua(&channels, "fov")?,
            -ImpulseParams::MAX_CHANNEL_MAGNITUDE,
            ImpulseParams::MAX_CHANNEL_MAGNITUDE,
            &path("fov"),
        )?,
        pitch: validate_in_range_finite(
            get_required_f32_lua(&channels, "pitch")?,
            -ImpulseParams::MAX_CHANNEL_MAGNITUDE,
            ImpulseParams::MAX_CHANNEL_MAGNITUDE,
            &path("pitch"),
        )?,
        roll: validate_in_range_finite(
            get_required_f32_lua(&channels, "roll")?,
            -ImpulseParams::MAX_CHANNEL_MAGNITUDE,
            ImpulseParams::MAX_CHANNEL_MAGNITUDE,
            &path("roll"),
        )?,
    }))
}

/// Read an optional sub-table from a Luau table: absent/nil → `None`, a table →
/// `Some(table)`, any other type → an `InvalidShape` error keyed by `path`.
pub fn read_optional_subtable_lua(
    table: &Table,
    field: &'static str,
    path: &str,
) -> Result<Option<Table>, DescriptorError> {
    if !table.contains_key(field).map_err(lua_err)? {
        return Ok(None);
    }
    let raw: LuaValue = table.get(field).map_err(lua_err)?;
    match raw {
        LuaValue::Nil => Ok(None),
        LuaValue::Table(t) => Ok(Some(t)),
        other => Err(DescriptorError::InvalidShape {
            reason: format!("`{path}` must be a table, got {}", other.type_name()),
        }),
    }
}

pub fn bob_params_from_lua(table: &Table) -> Result<BobParams, DescriptorError> {
    let vertical_frequency = validate_positive_finite(
        get_required_f32_lua(table, "verticalFrequency")?,
        "movement.viewFeel.bob.verticalFrequency",
    )?;
    let lateral_frequency = validate_positive_finite(
        get_required_f32_lua(table, "lateralFrequency")?,
        "movement.viewFeel.bob.lateralFrequency",
    )?;
    let vertical_amplitude = validate_non_negative_finite(
        get_required_f32_lua(table, "verticalAmplitude")?,
        "movement.viewFeel.bob.verticalAmplitude",
    )?;
    let lateral_amplitude = validate_non_negative_finite(
        get_required_f32_lua(table, "lateralAmplitude")?,
        "movement.viewFeel.bob.lateralAmplitude",
    )?;
    let speed_threshold = validate_non_negative_finite(
        get_required_f32_lua(table, "speedThreshold")?,
        "movement.viewFeel.bob.speedThreshold",
    )?;
    let grounded_only =
        get_optional_bool_lua(table, "groundedOnly")?.unwrap_or(BobParams::DEFAULT_GROUNDED_ONLY);
    Ok(BobParams {
        vertical_frequency,
        lateral_frequency,
        vertical_amplitude,
        lateral_amplitude,
        speed_threshold,
        grounded_only,
    })
}

pub fn tilt_params_from_lua(table: &Table) -> Result<TiltParams, DescriptorError> {
    let max_angle = validate_in_range_finite(
        get_required_f32_lua(table, "maxAngle")?,
        0.0,
        90.0,
        "movement.viewFeel.tilt.maxAngle",
    )?;
    let speed_reference = validate_positive_finite(
        get_required_f32_lua(table, "speedReference")?,
        "movement.viewFeel.tilt.speedReference",
    )?;
    let tension = validate_positive_finite(
        get_required_f32_lua(table, "tension")?,
        "movement.viewFeel.tilt.tension",
    )?;
    let grounded_only =
        get_optional_bool_lua(table, "groundedOnly")?.unwrap_or(TiltParams::DEFAULT_GROUNDED_ONLY);
    Ok(TiltParams {
        max_angle,
        speed_reference,
        tension,
        grounded_only,
    })
}

pub fn sway_params_from_lua(table: &Table) -> Result<SwayParams, DescriptorError> {
    let amplitude = validate_non_negative_finite(
        get_required_f32_lua(table, "amplitude")?,
        "movement.viewFeel.sway.amplitude",
    )?;
    let frequency = validate_positive_finite(
        get_required_f32_lua(table, "frequency")?,
        "movement.viewFeel.sway.frequency",
    )?;
    let speed_scale = validate_non_negative_finite(
        get_required_f32_lua(table, "speedScale")?,
        "movement.viewFeel.sway.speedScale",
    )?;
    let grounded_only =
        get_optional_bool_lua(table, "groundedOnly")?.unwrap_or(SwayParams::DEFAULT_GROUNDED_ONLY);
    Ok(SwayParams {
        amplitude,
        frequency,
        speed_scale,
        grounded_only,
    })
}
