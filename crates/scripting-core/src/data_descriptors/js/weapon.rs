// Weapon-only raw VM guards and descriptor lowering.
use super::super::*;

pub(super) fn weapon_descriptor_from_js<'js>(
    ctx: &Ctx<'js>,
    raw: JsValue<'js>,
) -> Result<WeaponDescriptor, DescriptorError> {
    if let Some(weapon) = raw.as_object() {
        validate_optional_weapon_model_paths_js(weapon)?;
        validate_optional_weapon_placement_shape_js(weapon)?;
        validate_optional_projectile_shapes_js(weapon)?;
        validate_activation_shapes(weapon)?;
    }
    let json = conv::js_to_json(ctx, raw).map_err(js_err)?;
    validate_optional_knockback_object(&json, "components.weapon.knockback")?;
    if let Some(splash) = json.get("splash") {
        validate_optional_knockback_object(splash, "components.weapon.splash.knockback")?;
    }
    let descriptor: WeaponDescriptor =
        serde_json::from_value(json).map_err(|e| DescriptorError::InvalidShape {
            reason: format!("`components.weapon` invalid: {e}"),
        })?;
    descriptor.validate()
}

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

/// Placement is an authored presentation contract, so a supplied unsupported
/// VM value must not cross the JSON bridge as `null` and silently become an
/// omitted placement.
fn validate_optional_weapon_placement_shape_js<'js>(
    weapon: &Object<'js>,
) -> Result<(), DescriptorError> {
    optional_object_field_js(weapon, "placement", "components.weapon.placement", true)?;
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

fn validate_activation_shapes(weapon: &Object<'_>) -> Result<(), DescriptorError> {
    for lane in ["primary", "secondary"] {
        let path = format!("components.weapon.{lane}");
        let Some(action) = optional_object_field_js(weapon, lane, &path, true)? else {
            continue;
        };
        for child in ["charge", "sounds", "emits"] {
            if let Some(object) =
                optional_object_field_js(&action, child, &format!("{path}.{child}"), true)?
            {
                if child != "charge" {
                    for field in if child == "sounds" {
                        ["fire", "impact"]
                    } else {
                        ["activate", "impact"]
                    } {
                        let value: JsValue = object.get(field).map_err(js_err)?;
                        if !value.is_null() && !value.is_undefined() && value.as_string().is_none()
                        {
                            return Err(DescriptorError::InvalidShape {
                                reason: format!(
                                    "`{path}.{child}.{field}` must be a string when supplied"
                                ),
                            });
                        }
                    }
                }
            }
        }
        let raw: JsValue = action.get("steps").map_err(js_err)?;
        let Some(steps) = raw.as_array() else {
            return Err(DescriptorError::InvalidShape {
                reason: format!("`{path}.steps` must be an array"),
            });
        };
        if steps.len() > postretro_foundation::MAX_ACTIVATION_STEPS {
            return Err(DescriptorError::InvalidShape {
                reason: format!("`{path}.steps` must contain at most 64 steps"),
            });
        }
        for index in 0..steps.len() {
            let raw: JsValue = steps.get(index).map_err(js_err)?;
            if raw.type_of() != rquickjs::Type::Object {
                return Err(DescriptorError::InvalidShape {
                    reason: format!("`{path}.steps[{index}]` must be an object"),
                });
            }
            if let Some(step) = raw.as_object() {
                optional_object_field_js(
                    step,
                    "scale",
                    &format!("{path}.steps[{index}].scale"),
                    true,
                )?;
            }
        }
    }
    Ok(())
}
