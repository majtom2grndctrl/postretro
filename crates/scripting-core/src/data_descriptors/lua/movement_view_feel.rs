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
    let slide = match read_optional_subtable_lua(table, "slide", "movement.viewFeel.slide")? {
        Some(t) => Some(slide_view_params_from_lua(&t)?),
        None => None,
    };
    Ok(ViewFeelParams {
        bob,
        tilt,
        sway,
        impulse,
        slide,
    })
}

pub fn slide_view_params_from_lua(table: &Table) -> Result<SlideViewParams, DescriptorError> {
    Ok(SlideViewParams {
        eye_drop: slide_view_number_from_lua(table, "eyeDrop", 0.0, f64::from(f32::MAX))?,
        fov_increase: slide_view_number_from_lua(
            table,
            "fovIncrease",
            0.0,
            f64::from(SlideViewParams::MAX_FOV_INCREASE),
        )?,
        enter_rate: slide_view_number_from_lua(
            table,
            "enterRate",
            0.1,
            f64::from(SlideViewParams::MAX_RATE),
        )?,
        exit_rate: slide_view_number_from_lua(
            table,
            "exitRate",
            0.1,
            f64::from(SlideViewParams::MAX_RATE),
        )?,
    })
}

// Check the authored number before f32 narrowing can round an invalid value
// onto a valid boundary (or a tiny negative eye drop to zero).
fn slide_view_number_from_lua(
    table: &Table,
    field: &'static str,
    min: f64,
    max: f64,
) -> Result<f32, DescriptorError> {
    if !table.contains_key(field).map_err(lua_err)? {
        return Err(DescriptorError::MissingField { field });
    }
    let raw: LuaValue = table.get(field).map_err(lua_err)?;
    let value = match raw {
        LuaValue::Nil => return Err(DescriptorError::MissingField { field }),
        LuaValue::Integer(value) => value as f64,
        LuaValue::Number(value) => value,
        _ => {
            return Err(DescriptorError::InvalidShape {
                reason: format!("`movement.viewFeel.slide.{field}` must be a number"),
            });
        }
    };
    if !value.is_finite() || !(min..=max).contains(&value) {
        return Err(DescriptorError::InvalidShape {
            reason: format!(
                "`movement.viewFeel.slide.{field}` must be finite in [{min}, {max}], got {value}"
            ),
        });
    }
    Ok(value as f32)
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
        if !matches!(key.as_str(), "normal" | "dash" | "crouch" | "slide") {
            return Err(DescriptorError::InvalidShape {
                reason: format!(
                    "`movement.viewFeel.impulse.states.{key}` is not a supported movement state; expected normal, dash, crouch, or slide"
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
