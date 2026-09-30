// Render-only movement view-feel descriptor parsing.
// See: context/lib/scripting.md

use super::super::*;
use rquickjs::object::Filter;

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
    let impulse = if obj.contains_key("impulse").map_err(js_err)? {
        let raw: JsValue = obj.get("impulse").map_err(js_err)?;
        if raw.is_null() || raw.is_undefined() {
            None
        } else {
            let impulse_obj =
                Object::from_value(raw).map_err(|_| DescriptorError::InvalidShape {
                    reason: "`movement.viewFeel.impulse` must be an object".to_string(),
                })?;
            Some(impulse_params_from_js(&impulse_obj)?)
        }
    } else {
        None
    };
    let slide = if obj.contains_key("slide").map_err(js_err)? {
        let raw: JsValue = obj.get("slide").map_err(js_err)?;
        if raw.is_null() || raw.is_undefined() {
            None
        } else {
            let slide_obj = Object::from_value(raw).map_err(|_| DescriptorError::InvalidShape {
                reason: "`movement.viewFeel.slide` must be an object".to_string(),
            })?;
            Some(slide_view_params_from_js(&slide_obj)?)
        }
    } else {
        None
    };
    Ok(ViewFeelParams {
        bob,
        tilt,
        sway,
        impulse,
        slide,
    })
}

pub fn slide_view_params_from_js<'js>(
    obj: &Object<'js>,
) -> Result<SlideViewParams, DescriptorError> {
    Ok(SlideViewParams {
        eye_drop: slide_view_number_from_js(obj, "eyeDrop", 0.0, f64::from(f32::MAX))?,
        fov_increase: slide_view_number_from_js(
            obj,
            "fovIncrease",
            0.0,
            f64::from(SlideViewParams::MAX_FOV_INCREASE),
        )?,
        enter_rate: slide_view_number_from_js(
            obj,
            "enterRate",
            0.1,
            f64::from(SlideViewParams::MAX_RATE),
        )?,
        exit_rate: slide_view_number_from_js(
            obj,
            "exitRate",
            0.1,
            f64::from(SlideViewParams::MAX_RATE),
        )?,
    })
}

// Check the authored number before f32 narrowing can round an invalid value
// onto a valid boundary (or a tiny negative eye drop to zero).
fn slide_view_number_from_js(
    obj: &Object<'_>,
    field: &'static str,
    min: f64,
    max: f64,
) -> Result<f32, DescriptorError> {
    if !obj.contains_key(field).map_err(js_err)? {
        return Err(DescriptorError::MissingField { field });
    }
    let raw: JsValue = obj.get(field).map_err(js_err)?;
    if raw.is_null() || raw.is_undefined() {
        return Err(DescriptorError::MissingField { field });
    }
    let value = raw
        .as_int()
        .map(f64::from)
        .or_else(|| raw.as_float())
        .ok_or_else(|| DescriptorError::InvalidShape {
            reason: format!("`movement.viewFeel.slide.{field}` must be a number"),
        })?;
    if !value.is_finite() || !(min..=max).contains(&value) {
        return Err(DescriptorError::InvalidShape {
            reason: format!(
                "`movement.viewFeel.slide.{field}` must be finite in [{min}, {max}], got {value}"
            ),
        });
    }
    Ok(value as f32)
}

pub fn impulse_params_from_js<'js>(obj: &Object<'js>) -> Result<ImpulseParams, DescriptorError> {
    let tension = validate_in_range_finite(
        get_required_f32_js(obj, "tension")?,
        ImpulseParams::MIN_TENSION,
        ImpulseParams::MAX_TENSION,
        "movement.viewFeel.impulse.tension",
    )?;
    let max: Object = get_required_object_js(obj, "max")?;
    let states: Object = get_required_object_js(obj, "states")?;
    Ok(ImpulseParams {
        tension,
        max: impulse_max_from_js(&max)?,
        states: impulse_states_from_js(&states)?,
    })
}

fn impulse_max_from_js<'js>(obj: &Object<'js>) -> Result<ImpulseChannels, DescriptorError> {
    Ok(ImpulseChannels {
        fov: validate_in_range_finite(
            get_required_f32_js(obj, "fov")?,
            0.0,
            ImpulseParams::MAX_CHANNEL_MAGNITUDE,
            "movement.viewFeel.impulse.max.fov",
        )?,
        pitch: validate_in_range_finite(
            get_required_f32_js(obj, "pitch")?,
            0.0,
            ImpulseParams::MAX_CHANNEL_MAGNITUDE,
            "movement.viewFeel.impulse.max.pitch",
        )?,
        roll: validate_in_range_finite(
            get_required_f32_js(obj, "roll")?,
            0.0,
            ImpulseParams::MAX_CHANNEL_MAGNITUDE,
            "movement.viewFeel.impulse.max.roll",
        )?,
    })
}

fn impulse_states_from_js<'js>(obj: &Object<'js>) -> Result<ImpulseStates, DescriptorError> {
    validate_impulse_state_keys_js(obj)?;
    Ok(ImpulseStates {
        normal: optional_impulse_state_from_js(obj, "normal")?,
        dash: optional_impulse_state_from_js(obj, "dash")?,
        crouch: optional_impulse_state_from_js(obj, "crouch")?,
        slide: optional_impulse_state_from_js(obj, "slide")?,
    })
}

fn validate_impulse_state_keys_js(obj: &Object<'_>) -> Result<(), DescriptorError> {
    if let Some(key) = obj
        .own_keys::<rquickjs::Atom>(Filter::new().symbol())
        .next()
    {
        key.map_err(js_err)?;
        return Err(DescriptorError::InvalidShape {
            reason: "`movement.viewFeel.impulse.states` keys must be strings, got symbol"
                .to_string(),
        });
    }
    for key in obj.own_keys::<String>(Filter::new().string()) {
        let key = key.map_err(js_err)?;
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

fn optional_impulse_state_from_js<'js>(
    obj: &Object<'js>,
    field: &'static str,
) -> Result<Option<ImpulseStateParams>, DescriptorError> {
    if !has_own_impulse_state_key_js(obj, field)? {
        return Ok(None);
    }
    let raw: JsValue = obj.get(field).map_err(js_err)?;
    if raw.is_null() || raw.is_undefined() {
        return Ok(None);
    }
    let state = Object::from_value(raw).map_err(|_| DescriptorError::InvalidShape {
        reason: format!("`movement.viewFeel.impulse.states.{field}` must be an object"),
    })?;
    let tension = match get_optional_f32_js(&state, "tension")? {
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
        enter: optional_impulse_channels_from_js(&state, "enter", field)?,
        exit: optional_impulse_channels_from_js(&state, "exit", field)?,
    }))
}

/// State rows are authored descriptor content only when they are own
/// properties. Prototype rows must not bypass the closed state vocabulary.
fn has_own_impulse_state_key_js(obj: &Object<'_>, wanted: &str) -> Result<bool, DescriptorError> {
    obj.own_keys::<String>(Filter::new().string())
        .try_fold(false, |found, key| {
            Ok(found || key.map_err(js_err)? == wanted)
        })
}

fn optional_impulse_channels_from_js<'js>(
    state: &Object<'js>,
    field: &'static str,
    state_name: &'static str,
) -> Result<Option<ImpulseChannels>, DescriptorError> {
    if !state.contains_key(field).map_err(js_err)? {
        return Ok(None);
    }
    let raw: JsValue = state.get(field).map_err(js_err)?;
    if raw.is_null() || raw.is_undefined() {
        return Ok(None);
    }
    let channels = Object::from_value(raw).map_err(|_| DescriptorError::InvalidShape {
        reason: format!(
            "`movement.viewFeel.impulse.states.{state_name}.{field}` must be an object"
        ),
    })?;
    let path =
        |channel: &str| format!("movement.viewFeel.impulse.states.{state_name}.{field}.{channel}");
    Ok(Some(ImpulseChannels {
        fov: validate_in_range_finite(
            get_required_f32_js(&channels, "fov")?,
            -ImpulseParams::MAX_CHANNEL_MAGNITUDE,
            ImpulseParams::MAX_CHANNEL_MAGNITUDE,
            &path("fov"),
        )?,
        pitch: validate_in_range_finite(
            get_required_f32_js(&channels, "pitch")?,
            -ImpulseParams::MAX_CHANNEL_MAGNITUDE,
            ImpulseParams::MAX_CHANNEL_MAGNITUDE,
            &path("pitch"),
        )?,
        roll: validate_in_range_finite(
            get_required_f32_js(&channels, "roll")?,
            -ImpulseParams::MAX_CHANNEL_MAGNITUDE,
            ImpulseParams::MAX_CHANNEL_MAGNITUDE,
            &path("roll"),
        )?,
    }))
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
