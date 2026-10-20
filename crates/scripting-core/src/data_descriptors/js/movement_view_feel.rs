// Render-only movement view-feel descriptor parsing.
// See: context/lib/scripting.md

use super::super::*;
use rquickjs::{Atom, object::Filter};

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
    for moved in ["slide", "vault"] {
        if obj.contains_key(moved).map_err(js_err)? {
            let raw: JsValue = obj.get(moved).map_err(js_err)?;
            if !raw.is_null() && !raw.is_undefined() {
                return Err(moved_view_block(moved));
            }
        }
    }
    let pose = pose_params_from_value(pose_from_js(obj)?)?;
    Ok(ViewFeelParams {
        bob,
        tilt,
        sway,
        impulse,
        pose,
    })
}

/// Read `viewFeel.pose` for the shared validator. Only an own `pose` counts;
/// one `viewFeel` inherits is rejected rather than read.
fn pose_from_js(view_feel: &Object<'_>) -> Result<Option<PoseValue>, DescriptorError> {
    let raw: JsValue = view_feel.get("pose").map_err(js_err)?;
    if raw.is_null() || raw.is_undefined() {
        return Ok(None);
    }
    if !has_own_string_key_js(view_feel, "pose")? {
        return Err(inherited_pose_block());
    }
    pose_value_from_js(raw, POSE_BLOCK_DEPTH)
}

/// Classify one pose value. `blocks_left` counts the blocks that may still
/// nest here; an object past it is not read.
fn pose_value_from_js(
    raw: JsValue<'_>,
    blocks_left: u8,
) -> Result<Option<PoseValue>, DescriptorError> {
    if raw.is_null() || raw.is_undefined() {
        return Ok(None);
    }
    if let Some(value) = raw.as_number() {
        return Ok(Some(PoseValue::Number(value)));
    }
    // Arrays and functions carry the object tag in QuickJS but are not a
    // block of fields; Luau refuses a table with array entries likewise.
    if blocks_left == 0 || raw.is_array() || raw.is_function() {
        return Ok(Some(PoseValue::Other));
    }
    let Some(obj) = raw.into_object() else {
        return Ok(Some(PoseValue::Other));
    };
    // A symbol key is the JS twin of a Luau non-string key.
    if let Some(key) = obj.own_keys::<Atom>(Filter::new().symbol()).next() {
        key.map_err(js_err)?;
        return Ok(Some(PoseValue::Other));
    }
    let mut block = PoseBlock::default();
    for key in obj.own_keys::<String>(Filter::new().string()) {
        let key = key.map_err(js_err)?;
        let value: JsValue = obj.get(key.as_str()).map_err(js_err)?;
        if let Some(value) = pose_value_from_js(value, blocks_left - 1)? {
            block.fields.insert(key, value);
        }
    }
    // Enumerable, non-nullish prototype entries are what `for…in` and a
    // property read would see; built-in prototypes hold none.
    let mut proto = obj.get_prototype();
    for _ in 0..MAX_INHERITANCE_WALK {
        let Some(current) = proto else {
            break;
        };
        for key in current.own_keys::<String>(Filter::new().string().enum_only()) {
            let key = key.map_err(js_err)?;
            let value: JsValue = current.get(key.as_str()).map_err(js_err)?;
            if !value.is_null() && !value.is_undefined() {
                block.inherited.insert(key);
            }
        }
        proto = current.get_prototype();
    }
    block.inherits_unnamed = proto.is_some();
    Ok(Some(PoseValue::Block(block)))
}

fn has_own_string_key_js(obj: &Object<'_>, wanted: &str) -> Result<bool, DescriptorError> {
    obj.own_keys::<String>(Filter::new().string())
        .try_fold(false, |found, key| {
            Ok(found || key.map_err(js_err)? == wanted)
        })
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
        vault: optional_impulse_state_from_js(obj, "vault")?,
    })
}

fn validate_impulse_state_keys_js(obj: &Object<'_>) -> Result<(), DescriptorError> {
    if let Some(key) = obj.own_keys::<Atom>(Filter::new().symbol()).next() {
        key.map_err(js_err)?;
        return Err(DescriptorError::InvalidShape {
            reason: "`movement.viewFeel.impulse.states` keys must be strings, got symbol"
                .to_string(),
        });
    }
    for key in obj.own_keys::<String>(Filter::new().string()) {
        let key = key.map_err(js_err)?;
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

fn optional_impulse_state_from_js<'js>(
    obj: &Object<'js>,
    field: &'static str,
) -> Result<Option<ImpulseStateParams>, DescriptorError> {
    // State rows are authored descriptor content only when they are own
    // properties. Prototype rows must not bypass the closed state vocabulary.
    if !has_own_string_key_js(obj, field)? {
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
