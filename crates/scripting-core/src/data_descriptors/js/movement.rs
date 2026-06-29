// Data-context descriptors: JS movement-descriptor converters.
// See: context/lib/scripting.md

use super::super::*;

pub fn movement_descriptor_from_js<'js>(
    ctx: &Ctx<'js>,
    obj: &Object<'js>,
) -> Result<PlayerMovementDescriptor, DescriptorError> {
    let capsule_obj: Object = get_required_object_js(obj, "capsule")?;
    let radius = validate_positive_finite(
        get_required_f32_js(&capsule_obj, "radius")?,
        "movement.capsule.radius",
    )?;
    let half_height = validate_positive_finite(
        get_required_f32_js(&capsule_obj, "halfHeight")?,
        "movement.capsule.halfHeight",
    )?;
    let eye_height = validate_in_range_finite_exclusive_min(
        get_required_f32_js(&capsule_obj, "eyeHeight")?,
        0.0,
        half_height + radius,
        "movement.capsule.eyeHeight",
    )?;
    let capsule = CapsuleParams {
        radius,
        half_height,
        eye_height,
    };

    let ground_obj: Object = get_required_object_js(obj, "ground")?;
    let speed_obj: Object = get_required_object_js(&ground_obj, "speed")?;
    let speed = SpeedParams {
        walk: validate_non_negative_finite(
            get_required_f32_js(&speed_obj, "walk")?,
            "movement.ground.speed.walk",
        )?,
        run: validate_non_negative_finite(
            get_required_f32_js(&speed_obj, "run")?,
            "movement.ground.speed.run",
        )?,
        crouch: validate_non_negative_finite(
            get_required_f32_js(&speed_obj, "crouch")?,
            "movement.ground.speed.crouch",
        )?,
    };
    let ground = GroundParams {
        speed,
        accel: validate_non_negative_finite(
            get_required_f32_js(&ground_obj, "accel")?,
            "movement.ground.accel",
        )?,
        step_height: validate_non_negative_finite(
            get_required_f32_js(&ground_obj, "stepHeight")?,
            "movement.ground.stepHeight",
        )?,
        max_slope: validate_in_range_finite(
            get_required_f32_js(&ground_obj, "maxSlope")?,
            0.0,
            90.0,
            "movement.ground.maxSlope",
        )?,
    };

    let air_obj: Object = get_required_object_js(obj, "air")?;
    let jumps_value = get_required_f32_js(&air_obj, "jumps")?;
    if !jumps_value.is_finite() || jumps_value < 0.0 || jumps_value.fract() != 0.0 {
        return Err(DescriptorError::InvalidShape {
            reason: format!(
                "`movement.air.jumps` must be a non-negative integer, got {jumps_value}"
            ),
        });
    }
    let jumps = jumps_value as u32;
    let jump_ceiling_present = air_obj.contains_key("jumpCeiling").map_err(js_err)?;
    if jumps > 0 && !jump_ceiling_present {
        return Err(DescriptorError::MissingField {
            field: "jumpCeiling",
        });
    }
    let air = AirParams {
        forward_steer: validate_in_range_finite(
            get_required_f32_js(&air_obj, "forwardSteer")?,
            0.0,
            1.0,
            "movement.air.forwardSteer",
        )?,
        accel: validate_non_negative_finite(
            get_required_f32_js(&air_obj, "accel")?,
            "movement.air.accel",
        )?,
        max_control_speed: validate_non_negative_finite(
            get_required_f32_js(&air_obj, "maxControlSpeed")?,
            "movement.air.maxControlSpeed",
        )?,
        bunny_hop: get_required_bool_js(&air_obj, "bunnyHop")?,
        jumps,
        jump_velocity: validate_non_negative_finite(
            get_required_f32_js(&air_obj, "jumpVelocity")?,
            "movement.air.jumpVelocity",
        )?,
        jump_ceiling: if jumps > 0 || jump_ceiling_present {
            get_required_f32_js(&air_obj, "jumpCeiling")?
        } else {
            0.0
        },
    };

    let fall_obj: Object = get_required_object_js(obj, "fall")?;
    let fall = FallParams {
        terminal_velocity: validate_positive_finite(
            get_required_f32_js(&fall_obj, "terminalVelocity")?,
            "movement.fall.terminalVelocity",
        )?,
    };

    // Stuck-stop deadzone fields are optional at the wire layer: omitting
    // them yields the canonical defaults so descriptors authored before the
    // deadzone shipped continue to parse.
    let stuck_stop_enabled = get_optional_bool_js(obj, "stuckStopEnabled")?
        .unwrap_or(PlayerMovementDescriptor::DEFAULT_STUCK_STOP_ENABLED);
    let stuck_stop_threshold = match get_optional_f32_js(obj, "stuckStopThreshold")? {
        Some(v) => validate_non_negative_finite(v, "movement.stuckStopThreshold")?,
        None => PlayerMovementDescriptor::DEFAULT_STUCK_STOP_THRESHOLD,
    };

    // `dash` is optional: absence disables dash. When present, every field is
    // required and validated, matching the ground/air/fall discipline.
    let dash = if obj.contains_key("dash").map_err(js_err)? {
        let raw: JsValue = obj.get("dash").map_err(js_err)?;
        if raw.is_null() || raw.is_undefined() {
            None
        } else {
            let dash_obj = Object::from_value(raw).map_err(|_| DescriptorError::InvalidShape {
                reason: "`movement.dash` must be an object".to_string(),
            })?;
            Some(dash_params_from_js(ctx, &dash_obj)?)
        }
    } else {
        None
    };

    // `forgiveness` is an optional sub-object: absence applies the engine
    // defaults at materialization. When present, each field is itself optional
    // and falls back to its engine default; an explicit 0 disables that grace.
    let forgiveness = if obj.contains_key("forgiveness").map_err(js_err)? {
        let raw: JsValue = obj.get("forgiveness").map_err(js_err)?;
        if raw.is_null() || raw.is_undefined() {
            None
        } else {
            let f_obj = Object::from_value(raw).map_err(|_| DescriptorError::InvalidShape {
                reason: "`movement.forgiveness` must be an object".to_string(),
            })?;
            Some(forgiveness_params_from_js(&f_obj)?)
        }
    } else {
        None
    };

    // `crouch` is optional: absence disables crouch. When present, every field
    // is required and validated. The crouched `eyeHeight` bound is computed
    // against the crouched capsule extent (`crouch.halfHeight + capsule.radius`).
    let crouch = if obj.contains_key("crouch").map_err(js_err)? {
        let raw: JsValue = obj.get("crouch").map_err(js_err)?;
        if raw.is_null() || raw.is_undefined() {
            None
        } else {
            let crouch_obj =
                Object::from_value(raw).map_err(|_| DescriptorError::InvalidShape {
                    reason: "`movement.crouch` must be an object".to_string(),
                })?;
            Some(crouch_params_from_js(&crouch_obj, radius)?)
        }
    } else {
        None
    };

    // `viewFeel` is optional: absence disables view feel. When present, each of
    // `bob`/`tilt`/`sway` is independently optional; an absent sub-object
    // disables that motion. Within a present sub-object, all tuning fields are
    // required except the optional `groundedOnly` gate (two-level
    // present-then-all-required, mirroring the `dash`/`crouch` discipline).
    let view_feel = if obj.contains_key("viewFeel").map_err(js_err)? {
        let raw: JsValue = obj.get("viewFeel").map_err(js_err)?;
        if raw.is_null() || raw.is_undefined() {
            None
        } else {
            let vf_obj = Object::from_value(raw).map_err(|_| DescriptorError::InvalidShape {
                reason: "`movement.viewFeel` must be an object".to_string(),
            })?;
            Some(view_feel_params_from_js(&vf_obj)?)
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

pub fn view_feel_params_from_js<'js>(obj: &Object<'js>) -> Result<ViewFeelParams, DescriptorError> {
    let bob = if obj.contains_key("bob").map_err(js_err)? {
        let raw: JsValue = obj.get("bob").map_err(js_err)?;
        if raw.is_null() || raw.is_undefined() {
            None
        } else {
            let bob_obj = Object::from_value(raw).map_err(|_| DescriptorError::InvalidShape {
                reason: "`movement.viewFeel.bob` must be an object".to_string(),
            })?;
            Some(bob_params_from_js(&bob_obj)?)
        }
    } else {
        None
    };
    let tilt = if obj.contains_key("tilt").map_err(js_err)? {
        let raw: JsValue = obj.get("tilt").map_err(js_err)?;
        if raw.is_null() || raw.is_undefined() {
            None
        } else {
            let tilt_obj = Object::from_value(raw).map_err(|_| DescriptorError::InvalidShape {
                reason: "`movement.viewFeel.tilt` must be an object".to_string(),
            })?;
            Some(tilt_params_from_js(&tilt_obj)?)
        }
    } else {
        None
    };
    let sway = if obj.contains_key("sway").map_err(js_err)? {
        let raw: JsValue = obj.get("sway").map_err(js_err)?;
        if raw.is_null() || raw.is_undefined() {
            None
        } else {
            let sway_obj = Object::from_value(raw).map_err(|_| DescriptorError::InvalidShape {
                reason: "`movement.viewFeel.sway` must be an object".to_string(),
            })?;
            Some(sway_params_from_js(&sway_obj)?)
        }
    } else {
        None
    };
    Ok(ViewFeelParams { bob, tilt, sway })
}

pub fn bob_params_from_js<'js>(obj: &Object<'js>) -> Result<BobParams, DescriptorError> {
    let vertical_frequency = validate_positive_finite(
        get_required_f32_js(obj, "verticalFrequency")?,
        "movement.viewFeel.bob.verticalFrequency",
    )?;
    let lateral_frequency = validate_positive_finite(
        get_required_f32_js(obj, "lateralFrequency")?,
        "movement.viewFeel.bob.lateralFrequency",
    )?;
    let vertical_amplitude = validate_non_negative_finite(
        get_required_f32_js(obj, "verticalAmplitude")?,
        "movement.viewFeel.bob.verticalAmplitude",
    )?;
    let lateral_amplitude = validate_non_negative_finite(
        get_required_f32_js(obj, "lateralAmplitude")?,
        "movement.viewFeel.bob.lateralAmplitude",
    )?;
    let speed_threshold = validate_non_negative_finite(
        get_required_f32_js(obj, "speedThreshold")?,
        "movement.viewFeel.bob.speedThreshold",
    )?;
    let grounded_only =
        get_optional_bool_js(obj, "groundedOnly")?.unwrap_or(BobParams::DEFAULT_GROUNDED_ONLY);
    Ok(BobParams {
        vertical_frequency,
        lateral_frequency,
        vertical_amplitude,
        lateral_amplitude,
        speed_threshold,
        grounded_only,
    })
}

pub fn tilt_params_from_js<'js>(obj: &Object<'js>) -> Result<TiltParams, DescriptorError> {
    let max_angle = validate_in_range_finite(
        get_required_f32_js(obj, "maxAngle")?,
        0.0,
        90.0,
        "movement.viewFeel.tilt.maxAngle",
    )?;
    let speed_reference = validate_positive_finite(
        get_required_f32_js(obj, "speedReference")?,
        "movement.viewFeel.tilt.speedReference",
    )?;
    let tension = validate_positive_finite(
        get_required_f32_js(obj, "tension")?,
        "movement.viewFeel.tilt.tension",
    )?;
    let grounded_only =
        get_optional_bool_js(obj, "groundedOnly")?.unwrap_or(TiltParams::DEFAULT_GROUNDED_ONLY);
    Ok(TiltParams {
        max_angle,
        speed_reference,
        tension,
        grounded_only,
    })
}

pub fn sway_params_from_js<'js>(obj: &Object<'js>) -> Result<SwayParams, DescriptorError> {
    let amplitude = validate_non_negative_finite(
        get_required_f32_js(obj, "amplitude")?,
        "movement.viewFeel.sway.amplitude",
    )?;
    let frequency = validate_positive_finite(
        get_required_f32_js(obj, "frequency")?,
        "movement.viewFeel.sway.frequency",
    )?;
    let speed_scale = validate_non_negative_finite(
        get_required_f32_js(obj, "speedScale")?,
        "movement.viewFeel.sway.speedScale",
    )?;
    let grounded_only =
        get_optional_bool_js(obj, "groundedOnly")?.unwrap_or(SwayParams::DEFAULT_GROUNDED_ONLY);
    Ok(SwayParams {
        amplitude,
        frequency,
        speed_scale,
        grounded_only,
    })
}

pub fn forgiveness_params_from_js<'js>(
    obj: &Object<'js>,
) -> Result<ForgivenessParams, DescriptorError> {
    let coyote_ms = match get_optional_f32_js(obj, "coyoteMs")? {
        Some(v) => validate_non_negative_finite(v, "movement.forgiveness.coyoteMs")?,
        None => ForgivenessParams::DEFAULT_COYOTE_MS,
    };
    let jump_buffer_ms = match get_optional_f32_js(obj, "jumpBufferMs")? {
        Some(v) => validate_non_negative_finite(v, "movement.forgiveness.jumpBufferMs")?,
        None => ForgivenessParams::DEFAULT_JUMP_BUFFER_MS,
    };
    Ok(ForgivenessParams {
        coyote_ms,
        jump_buffer_ms,
    })
}

pub fn dash_params_from_js<'js>(
    ctx: &Ctx<'js>,
    obj: &Object<'js>,
) -> Result<DashParams, DescriptorError> {
    let boost_speed =
        read_dash_number_js(ctx, obj, "boostSpeed", "movement.dash.boostSpeed", |v| {
            validate_positive_finite(v, "movement.dash.boostSpeed")
        })?;
    let momentum_retention = read_dash_number_js(
        ctx,
        obj,
        "momentumRetention",
        "movement.dash.momentumRetention",
        |v| validate_in_range_finite(v, 0.0, 1.0, "movement.dash.momentumRetention"),
    )?;
    let steer_control = read_dash_number_js(
        ctx,
        obj,
        "steerControl",
        "movement.dash.steerControl",
        |v| validate_in_range_finite(v, 0.0, 1.0, "movement.dash.steerControl"),
    )?;
    let dash_drag = read_dash_number_js(ctx, obj, "dashDrag", "movement.dash.dashDrag", |v| {
        validate_non_negative_finite(v, "movement.dash.dashDrag")
    })?;
    let cooldown_ms =
        read_dash_number_js(ctx, obj, "cooldownMs", "movement.dash.cooldownMs", |v| {
            validate_non_negative_finite(v, "movement.dash.cooldownMs")
        })?;
    let air_dashes_value = get_required_f32_js(obj, "airDashes")?;
    if !air_dashes_value.is_finite() || air_dashes_value < 0.0 || air_dashes_value.fract() != 0.0 {
        return Err(DescriptorError::InvalidShape {
            reason: format!(
                "`movement.dash.airDashes` must be a non-negative integer, got {air_dashes_value}"
            ),
        });
    }
    let air_dashes = air_dashes_value as u32;
    let preserve_vertical = read_dash_bool_js(ctx, obj, "preserveVertical")?;
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

/// Read a dash numeric field (JS): a value that is an object converts through
/// the conv bridge to JSON, deserializes to an [`IrNode`], and is validated as a
/// `Number`-typed expression. A plain number takes the literal path with its
/// existing range validator `validate`. Absence is a [`DescriptorError::MissingField`].
pub fn read_dash_number_js<'js>(
    ctx: &Ctx<'js>,
    obj: &Object<'js>,
    field: &'static str,
    path: &str,
    validate: impl FnOnce(f32) -> Result<f32, DescriptorError>,
) -> Result<NumberOrIr, DescriptorError> {
    if !obj.contains_key(field).map_err(js_err)? {
        return Err(DescriptorError::MissingField { field });
    }
    let raw: JsValue = obj.get(field).map_err(js_err)?;
    if raw.is_null() || raw.is_undefined() {
        return Err(DescriptorError::MissingField { field });
    }
    // Arrays fall through here (is_array() excludes them) and hit the final
    // InvalidShape below. The Luau reader routes arrays into ir_node_from_json
    // instead — same InvalidShape variant, different message text by design.
    if raw.is_object() && !raw.is_array() {
        let json = conv::js_to_json(ctx, raw).map_err(js_err)?;
        let node = ir_node_from_json(json, path)?;
        return Ok(NumberOrIr::Ir(validate_dash_expr(
            node,
            IrType::Number,
            path,
        )?));
    }
    if let Some(i) = raw.as_int() {
        return Ok(NumberOrIr::Literal(validate(i as f32)?));
    }
    if let Some(f) = raw.as_float() {
        return Ok(NumberOrIr::Literal(validate(f as f32)?));
    }
    Err(DescriptorError::InvalidShape {
        reason: format!("'{field}' must be a number or a runtime expression"),
    })
}

/// Read a dash boolean field (JS): an object value parses as a `Bool`-typed
/// expression; a plain boolean takes the literal path. Mirror of
/// [`read_dash_number_js`] for the single boolean dash field.
pub fn read_dash_bool_js<'js>(
    ctx: &Ctx<'js>,
    obj: &Object<'js>,
    field: &'static str,
) -> Result<BoolOrIr, DescriptorError> {
    let path = "movement.dash.preserveVertical";
    if !obj.contains_key(field).map_err(js_err)? {
        return Err(DescriptorError::MissingField { field });
    }
    let raw: JsValue = obj.get(field).map_err(js_err)?;
    if raw.is_null() || raw.is_undefined() {
        return Err(DescriptorError::MissingField { field });
    }
    if raw.is_object() && !raw.is_array() {
        let json = conv::js_to_json(ctx, raw).map_err(js_err)?;
        let node = ir_node_from_json(json, path)?;
        return Ok(BoolOrIr::Ir(validate_dash_expr(node, IrType::Bool, path)?));
    }
    raw.as_bool()
        .map(BoolOrIr::Literal)
        .ok_or_else(|| DescriptorError::InvalidShape {
            reason: format!("'{field}' must be a boolean or a runtime expression"),
        })
}

/// Parse a `crouch` sub-object. `radius` is the standing capsule radius, used to
/// bound the crouched `eyeHeight` against the crouched capsule extent
/// (`half_height + radius`), mirroring how the standing `eyeHeight` is bounded.
pub fn crouch_params_from_js<'js>(
    obj: &Object<'js>,
    radius: f32,
) -> Result<CrouchParams, DescriptorError> {
    let half_height = validate_positive_finite(
        get_required_f32_js(obj, "halfHeight")?,
        "movement.crouch.halfHeight",
    )?;
    let eye_height = validate_in_range_finite_exclusive_min(
        get_required_f32_js(obj, "eyeHeight")?,
        0.0,
        half_height + radius,
        "movement.crouch.eyeHeight",
    )?;
    let transition_rate = validate_positive_finite(
        get_required_f32_js(obj, "transitionRate")?,
        "movement.crouch.transitionRate",
    )?;
    Ok(CrouchParams {
        half_height,
        eye_height,
        transition_rate,
    })
}

pub fn get_required_object_js<'js>(
    obj: &Object<'js>,
    field: &'static str,
) -> Result<Object<'js>, DescriptorError> {
    if !obj.contains_key(field).map_err(js_err)? {
        return Err(DescriptorError::MissingField { field });
    }
    let raw: JsValue = obj.get(field).map_err(js_err)?;
    if raw.is_null() || raw.is_undefined() {
        return Err(DescriptorError::MissingField { field });
    }
    Object::from_value(raw).map_err(|_| DescriptorError::InvalidShape {
        reason: format!("'{field}' must be an object"),
    })
}

pub fn get_required_bool_js<'js>(
    obj: &Object<'js>,
    field: &'static str,
) -> Result<bool, DescriptorError> {
    if !obj.contains_key(field).map_err(js_err)? {
        return Err(DescriptorError::MissingField { field });
    }
    let raw: JsValue = obj.get(field).map_err(js_err)?;
    if raw.is_null() || raw.is_undefined() {
        return Err(DescriptorError::MissingField { field });
    }
    raw.as_bool().ok_or_else(|| DescriptorError::InvalidShape {
        reason: format!("'{field}' must be a boolean"),
    })
}
