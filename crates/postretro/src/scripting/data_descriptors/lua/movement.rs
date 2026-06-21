// Data-context descriptors: Lua movement-descriptor converters.
// See: context/lib/scripting.md

use super::super::*;

pub(crate) fn movement_descriptor_from_lua(
    table: &Table,
) -> Result<PlayerMovementDescriptor, DescriptorError> {
    let capsule_table = get_required_table_lua(table, "capsule")?;
    let radius = validate_positive_finite(
        get_required_f32_lua(&capsule_table, "radius")?,
        "movement.capsule.radius",
    )?;
    let half_height = validate_positive_finite(
        get_required_f32_lua(&capsule_table, "halfHeight")?,
        "movement.capsule.halfHeight",
    )?;
    let eye_height = validate_in_range_finite_exclusive_min(
        get_required_f32_lua(&capsule_table, "eyeHeight")?,
        0.0,
        half_height + radius,
        "movement.capsule.eyeHeight",
    )?;
    let capsule = CapsuleParams {
        radius,
        half_height,
        eye_height,
    };

    let ground_table = get_required_table_lua(table, "ground")?;
    let speed_table = get_required_table_lua(&ground_table, "speed")?;
    let speed = SpeedParams {
        walk: validate_non_negative_finite(
            get_required_f32_lua(&speed_table, "walk")?,
            "movement.ground.speed.walk",
        )?,
        run: validate_non_negative_finite(
            get_required_f32_lua(&speed_table, "run")?,
            "movement.ground.speed.run",
        )?,
        crouch: validate_non_negative_finite(
            get_required_f32_lua(&speed_table, "crouch")?,
            "movement.ground.speed.crouch",
        )?,
    };
    let ground = GroundParams {
        speed,
        accel: validate_non_negative_finite(
            get_required_f32_lua(&ground_table, "accel")?,
            "movement.ground.accel",
        )?,
        step_height: validate_non_negative_finite(
            get_required_f32_lua(&ground_table, "stepHeight")?,
            "movement.ground.stepHeight",
        )?,
        max_slope: validate_in_range_finite(
            get_required_f32_lua(&ground_table, "maxSlope")?,
            0.0,
            90.0,
            "movement.ground.maxSlope",
        )?,
    };

    let air_table = get_required_table_lua(table, "air")?;
    let jumps = get_required_u32_lua(&air_table, "jumps")?;
    let jump_ceiling_present = air_table.contains_key("jumpCeiling").map_err(lua_err)?;
    if jumps > 0 && !jump_ceiling_present {
        return Err(DescriptorError::MissingField {
            field: "jumpCeiling",
        });
    }
    let air = AirParams {
        forward_steer: validate_in_range_finite(
            get_required_f32_lua(&air_table, "forwardSteer")?,
            0.0,
            1.0,
            "movement.air.forwardSteer",
        )?,
        accel: validate_non_negative_finite(
            get_required_f32_lua(&air_table, "accel")?,
            "movement.air.accel",
        )?,
        max_control_speed: validate_non_negative_finite(
            get_required_f32_lua(&air_table, "maxControlSpeed")?,
            "movement.air.maxControlSpeed",
        )?,
        bunny_hop: get_required_bool_lua(&air_table, "bunnyHop")?,
        jumps,
        jump_velocity: validate_non_negative_finite(
            get_required_f32_lua(&air_table, "jumpVelocity")?,
            "movement.air.jumpVelocity",
        )?,
        jump_ceiling: if jumps > 0 || jump_ceiling_present {
            get_required_f32_lua(&air_table, "jumpCeiling")?
        } else {
            0.0
        },
    };

    let fall_table = get_required_table_lua(table, "fall")?;
    let fall = FallParams {
        terminal_velocity: validate_positive_finite(
            get_required_f32_lua(&fall_table, "terminalVelocity")?,
            "movement.fall.terminalVelocity",
        )?,
    };

    // Optional at the wire layer; see JS parser for rationale.
    let stuck_stop_enabled = get_optional_bool_lua(table, "stuckStopEnabled")?
        .unwrap_or(PlayerMovementDescriptor::DEFAULT_STUCK_STOP_ENABLED);
    let stuck_stop_threshold = match get_optional_f32_lua(table, "stuckStopThreshold")? {
        Some(v) => validate_non_negative_finite(v, "movement.stuckStopThreshold")?,
        None => PlayerMovementDescriptor::DEFAULT_STUCK_STOP_THRESHOLD,
    };

    // `dash` is optional: absence disables dash. When present, every field is
    // required and validated, mirroring the JS path.
    let dash = if table.contains_key("dash").map_err(lua_err)? {
        let raw: LuaValue = table.get("dash").map_err(lua_err)?;
        match raw {
            LuaValue::Nil => None,
            LuaValue::Table(t) => Some(dash_params_from_lua(&t)?),
            other => {
                return Err(DescriptorError::InvalidShape {
                    reason: format!("`movement.dash` must be a table, got {}", other.type_name()),
                });
            }
        }
    } else {
        None
    };

    // `forgiveness` is an optional sub-object; mirrors the JS path (absent →
    // engine defaults; each field optional with a 0-disables semantic).
    let forgiveness = if table.contains_key("forgiveness").map_err(lua_err)? {
        let raw: LuaValue = table.get("forgiveness").map_err(lua_err)?;
        match raw {
            LuaValue::Nil => None,
            LuaValue::Table(t) => Some(forgiveness_params_from_lua(&t)?),
            other => {
                return Err(DescriptorError::InvalidShape {
                    reason: format!(
                        "`movement.forgiveness` must be a table, got {}",
                        other.type_name()
                    ),
                });
            }
        }
    } else {
        None
    };

    // `crouch` is optional: absence disables crouch. When present, every field
    // is required and validated, mirroring the JS path. The crouched `eyeHeight`
    // bound is computed against the crouched capsule extent
    // (`crouch.halfHeight + capsule.radius`).
    let crouch = if table.contains_key("crouch").map_err(lua_err)? {
        let raw: LuaValue = table.get("crouch").map_err(lua_err)?;
        match raw {
            LuaValue::Nil => None,
            LuaValue::Table(t) => Some(crouch_params_from_lua(&t, radius)?),
            other => {
                return Err(DescriptorError::InvalidShape {
                    reason: format!(
                        "`movement.crouch` must be a table, got {}",
                        other.type_name()
                    ),
                });
            }
        }
    } else {
        None
    };

    // `viewFeel` is optional: absence disables view feel. Mirrors the JS path —
    // two-level present-then-all-required across `bob`/`tilt`/`sway`.
    let view_feel = if table.contains_key("viewFeel").map_err(lua_err)? {
        let raw: LuaValue = table.get("viewFeel").map_err(lua_err)?;
        match raw {
            LuaValue::Nil => None,
            LuaValue::Table(t) => Some(view_feel_params_from_lua(&t)?),
            other => {
                return Err(DescriptorError::InvalidShape {
                    reason: format!(
                        "`movement.viewFeel` must be a table, got {}",
                        other.type_name()
                    ),
                });
            }
        }
    } else {
        None
    };

    Ok(PlayerMovementDescriptor {
        capsule,
        ground,
        air,
        fall,
        stuck_stop_enabled,
        stuck_stop_threshold,
        dash,
        forgiveness,
        crouch,
        view_feel,
    })
}

pub(crate) fn view_feel_params_from_lua(table: &Table) -> Result<ViewFeelParams, DescriptorError> {
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
    Ok(ViewFeelParams { bob, tilt, sway })
}

/// Read an optional sub-table from a Luau table: absent/nil → `None`, a table →
/// `Some(table)`, any other type → an `InvalidShape` error keyed by `path`.
pub(crate) fn read_optional_subtable_lua(
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

pub(crate) fn bob_params_from_lua(table: &Table) -> Result<BobParams, DescriptorError> {
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

pub(crate) fn tilt_params_from_lua(table: &Table) -> Result<TiltParams, DescriptorError> {
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

pub(crate) fn sway_params_from_lua(table: &Table) -> Result<SwayParams, DescriptorError> {
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

pub(crate) fn forgiveness_params_from_lua(
    table: &Table,
) -> Result<ForgivenessParams, DescriptorError> {
    let coyote_ms = match get_optional_f32_lua(table, "coyoteMs")? {
        Some(v) => validate_non_negative_finite(v, "movement.forgiveness.coyoteMs")?,
        None => ForgivenessParams::DEFAULT_COYOTE_MS,
    };
    let jump_buffer_ms = match get_optional_f32_lua(table, "jumpBufferMs")? {
        Some(v) => validate_non_negative_finite(v, "movement.forgiveness.jumpBufferMs")?,
        None => ForgivenessParams::DEFAULT_JUMP_BUFFER_MS,
    };
    Ok(ForgivenessParams {
        coyote_ms,
        jump_buffer_ms,
    })
}

pub(crate) fn dash_params_from_lua(table: &Table) -> Result<DashParams, DescriptorError> {
    let boost_speed = read_dash_number_lua(table, "boostSpeed", "movement.dash.boostSpeed", |v| {
        validate_positive_finite(v, "movement.dash.boostSpeed")
    })?;
    let momentum_retention = read_dash_number_lua(
        table,
        "momentumRetention",
        "movement.dash.momentumRetention",
        |v| validate_in_range_finite(v, 0.0, 1.0, "movement.dash.momentumRetention"),
    )?;
    let steer_control =
        read_dash_number_lua(table, "steerControl", "movement.dash.steerControl", |v| {
            validate_in_range_finite(v, 0.0, 1.0, "movement.dash.steerControl")
        })?;
    let dash_drag = read_dash_number_lua(table, "dashDrag", "movement.dash.dashDrag", |v| {
        validate_non_negative_finite(v, "movement.dash.dashDrag")
    })?;
    let cooldown_ms = read_dash_number_lua(table, "cooldownMs", "movement.dash.cooldownMs", |v| {
        validate_non_negative_finite(v, "movement.dash.cooldownMs")
    })?;
    let air_dashes = get_required_u32_lua(table, "airDashes")?;
    let preserve_vertical = read_dash_bool_lua(table, "preserveVertical")?;
    Ok(DashParams {
        boost_speed,
        momentum_retention,
        steer_control,
        dash_drag,
        cooldown_ms,
        air_dashes,
        preserve_vertical,
    })
}

/// Read a dash numeric field (Luau): a table value converts through the conv
/// bridge to JSON, deserializes to an [`IrNode`], and is validated as a
/// `Number`-typed expression; a plain number takes the literal path with its
/// existing range validator. Mirror of [`read_dash_number_js`] — the
/// missing-Luau-arm parity trap is avoided by keeping the two symmetric.
pub(crate) fn read_dash_number_lua(
    table: &Table,
    field: &'static str,
    path: &str,
    validate: impl FnOnce(f32) -> Result<f32, DescriptorError>,
) -> Result<NumberOrIr, DescriptorError> {
    if !table.contains_key(field).map_err(lua_err)? {
        return Err(DescriptorError::MissingField { field });
    }
    let raw: LuaValue = table.get(field).map_err(lua_err)?;
    match raw {
        LuaValue::Nil => Err(DescriptorError::MissingField { field }),
        LuaValue::Integer(i) => Ok(NumberOrIr::Literal(validate(i as f32)?)),
        LuaValue::Number(f) => Ok(NumberOrIr::Literal(validate(f as f32)?)),
        LuaValue::Table(_) => {
            let json = conv::lua_to_json(raw).map_err(lua_err)?;
            let node = ir_node_from_json(json, path)?;
            Ok(NumberOrIr::Ir(validate_dash_expr(
                node,
                IrType::Number,
                path,
            )?))
        }
        other => Err(DescriptorError::InvalidShape {
            reason: format!(
                "'{field}' must be a number or a runtime expression, got {}",
                other.type_name()
            ),
        }),
    }
}

/// Read a dash boolean field (Luau): a table value parses as a `Bool`-typed
/// expression; a plain boolean takes the literal path. Mirror of
/// [`read_dash_bool_js`].
pub(crate) fn read_dash_bool_lua(
    table: &Table,
    field: &'static str,
) -> Result<BoolOrIr, DescriptorError> {
    let path = "movement.dash.preserveVertical";
    if !table.contains_key(field).map_err(lua_err)? {
        return Err(DescriptorError::MissingField { field });
    }
    let raw: LuaValue = table.get(field).map_err(lua_err)?;
    match raw {
        LuaValue::Nil => Err(DescriptorError::MissingField { field }),
        LuaValue::Boolean(b) => Ok(BoolOrIr::Literal(b)),
        LuaValue::Table(_) => {
            let json = conv::lua_to_json(raw).map_err(lua_err)?;
            let node = ir_node_from_json(json, path)?;
            Ok(BoolOrIr::Ir(validate_dash_expr(node, IrType::Bool, path)?))
        }
        other => Err(DescriptorError::InvalidShape {
            reason: format!(
                "'{field}' must be a boolean or a runtime expression, got {}",
                other.type_name()
            ),
        }),
    }
}

/// Mirror of [`crouch_params_from_js`] for Luau tables. `radius` is the standing
/// capsule radius, used to bound the crouched `eyeHeight` against the crouched
/// capsule extent (`half_height + radius`).
pub(crate) fn crouch_params_from_lua(
    table: &Table,
    radius: f32,
) -> Result<CrouchParams, DescriptorError> {
    let half_height = validate_positive_finite(
        get_required_f32_lua(table, "halfHeight")?,
        "movement.crouch.halfHeight",
    )?;
    let eye_height = validate_in_range_finite_exclusive_min(
        get_required_f32_lua(table, "eyeHeight")?,
        0.0,
        half_height + radius,
        "movement.crouch.eyeHeight",
    )?;
    let transition_rate = validate_positive_finite(
        get_required_f32_lua(table, "transitionRate")?,
        "movement.crouch.transitionRate",
    )?;
    Ok(CrouchParams {
        half_height,
        eye_height,
        transition_rate,
    })
}

pub(crate) fn get_required_table_lua(
    table: &Table,
    field: &'static str,
) -> Result<Table, DescriptorError> {
    if !table.contains_key(field).map_err(lua_err)? {
        return Err(DescriptorError::MissingField { field });
    }
    let raw: LuaValue = table.get(field).map_err(lua_err)?;
    match raw {
        LuaValue::Nil => Err(DescriptorError::MissingField { field }),
        LuaValue::Table(t) => Ok(t),
        other => Err(DescriptorError::InvalidShape {
            reason: format!("'{field}' must be a table, got {}", other.type_name()),
        }),
    }
}

pub(crate) fn get_required_bool_lua(
    table: &Table,
    field: &'static str,
) -> Result<bool, DescriptorError> {
    if !table.contains_key(field).map_err(lua_err)? {
        return Err(DescriptorError::MissingField { field });
    }
    let raw: LuaValue = table.get(field).map_err(lua_err)?;
    match raw {
        LuaValue::Nil => Err(DescriptorError::MissingField { field }),
        LuaValue::Boolean(b) => Ok(b),
        other => Err(DescriptorError::InvalidShape {
            reason: format!("'{field}' must be a boolean, got {}", other.type_name()),
        }),
    }
}

pub(crate) fn get_required_string_lua(
    table: &Table,
    field: &'static str,
) -> Result<String, DescriptorError> {
    if !table.contains_key(field).map_err(lua_err)? {
        return Err(DescriptorError::MissingField { field });
    }
    let raw: LuaValue = table.get(field).map_err(lua_err)?;
    match raw {
        LuaValue::Nil => Err(DescriptorError::MissingField { field }),
        LuaValue::String(s) => Ok(s.to_str().map_err(lua_err)?.to_string()),
        other => Err(DescriptorError::InvalidShape {
            reason: format!("'{field}' must be a string, got {}", other.type_name()),
        }),
    }
}

pub(crate) fn get_required_f32_lua(
    table: &Table,
    field: &'static str,
) -> Result<f32, DescriptorError> {
    if !table.contains_key(field).map_err(lua_err)? {
        return Err(DescriptorError::MissingField { field });
    }
    let raw: LuaValue = table.get(field).map_err(lua_err)?;
    match raw {
        LuaValue::Nil => Err(DescriptorError::MissingField { field }),
        LuaValue::Integer(i) => Ok(i as f32),
        LuaValue::Number(f) => Ok(f as f32),
        other => Err(DescriptorError::InvalidShape {
            reason: format!("'{field}' must be a number, got {}", other.type_name()),
        }),
    }
}

/// Optional boolean read for the Luau path: absent/nil → `None`, present
/// non-boolean → `Err`. Mirrors `get_optional_bool_js`.
pub(crate) fn get_optional_bool_lua(
    table: &Table,
    field: &'static str,
) -> Result<Option<bool>, DescriptorError> {
    if !table.contains_key(field).map_err(lua_err)? {
        return Ok(None);
    }
    let raw: LuaValue = table.get(field).map_err(lua_err)?;
    match raw {
        LuaValue::Nil => Ok(None),
        LuaValue::Boolean(b) => Ok(Some(b)),
        other => Err(DescriptorError::InvalidShape {
            reason: format!("'{field}' must be a boolean, got {}", other.type_name()),
        }),
    }
}

/// Optional f32 read for the Luau path: absent/nil → `None`, present
/// non-numeric → `Err`. Range validation is the caller's responsibility.
pub(crate) fn get_optional_f32_lua(
    table: &Table,
    field: &'static str,
) -> Result<Option<f32>, DescriptorError> {
    if !table.contains_key(field).map_err(lua_err)? {
        return Ok(None);
    }
    let raw: LuaValue = table.get(field).map_err(lua_err)?;
    match raw {
        LuaValue::Nil => Ok(None),
        LuaValue::Integer(i) => Ok(Some(i as f32)),
        LuaValue::Number(f) => Ok(Some(f as f32)),
        other => Err(DescriptorError::InvalidShape {
            reason: format!("'{field}' must be a number, got {}", other.type_name()),
        }),
    }
}
