// Weapon-only raw VM guards and descriptor lowering.
use super::super::*;
use super::entity::validate_optional_string_field_lua;

pub(super) fn weapon_descriptor_from_lua(
    raw: LuaValue,
) -> Result<WeaponDescriptor, DescriptorError> {
    if let LuaValue::Table(weapon) = &raw {
        validate_optional_weapon_model_paths_lua(weapon)?;
        validate_optional_weapon_placement_shape_lua(weapon)?;
        validate_optional_projectile_shapes_lua(weapon)?;
        validate_optional_weapon_sound_keys_lua(weapon)?;
        validate_activation_shapes(weapon)?;
    }
    let json = conv::lua_to_json(raw).map_err(lua_err)?;
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

/// Reject unsupported VM values before the JSON bridge can treat them as omission.
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

/// Preserve malformed sound-key evidence before the JSON bridge coerces it to null.
fn validate_optional_weapon_sound_keys_lua(weapon: &Table) -> Result<(), DescriptorError> {
    let Some(sounds) =
        optional_table_field_lua(weapon, "sounds", "components.weapon.sounds", true)?
    else {
        return Ok(());
    };
    for field in [
        "fire",
        "dryFire",
        "impact",
        "reloadStart",
        "reloadShell",
        "reloadComplete",
        "overheat",
    ] {
        validate_optional_string_field_lua(
            &sounds,
            field,
            &format!("components.weapon.sounds.{field}"),
        )?;
    }
    Ok(())
}

/// Authored placement must not disappear when unsupported VM values become JSON null.
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

fn validate_activation_shapes(weapon: &Table) -> Result<(), DescriptorError> {
    for lane in ["primary", "secondary"] {
        let path = format!("components.weapon.{lane}");
        let Some(action) = optional_table_field_lua(weapon, lane, &path, true)? else {
            continue;
        };
        for child in ["charge", "sounds", "emits"] {
            if let Some(object) =
                optional_table_field_lua(&action, child, &format!("{path}.{child}"), true)?
                && child != "charge"
            {
                for field in if child == "sounds" {
                    ["fire", "impact"]
                } else {
                    ["activate", "impact"]
                } {
                    let value: LuaValue = object.get(field).map_err(lua_err)?;
                    if !matches!(value, LuaValue::Nil | LuaValue::String(_)) {
                        return Err(DescriptorError::InvalidShape {
                            reason: format!(
                                "`{path}.{child}.{field}` must be a string when supplied"
                            ),
                        });
                    }
                }
            }
        }
        let raw: LuaValue = action.get("steps").map_err(lua_err)?;
        let LuaValue::Table(steps) = raw else {
            return Err(DescriptorError::InvalidShape {
                reason: format!("`{path}.steps` must be an array"),
            });
        };
        let length = steps.raw_len();
        if length > postretro_foundation::MAX_ACTIVATION_STEPS {
            return Err(DescriptorError::InvalidShape {
                reason: format!("`{path}.steps` must contain at most 64 steps"),
            });
        }
        for entry in steps.clone().pairs::<LuaValue, LuaValue>() {
            let (key, _) = entry.map_err(lua_err)?;
            if !matches!(key, LuaValue::Integer(index) if index > 0 && index as usize <= length) {
                return Err(DescriptorError::InvalidShape {
                    reason: format!("`{path}.steps` must be a dense array"),
                });
            }
        }
        for index in 1..=length {
            let raw: LuaValue = steps.get(index).map_err(lua_err)?;
            let LuaValue::Table(step) = raw else {
                return Err(DescriptorError::InvalidShape {
                    reason: format!("`{path}.steps[{}]` must be an object", index - 1),
                });
            };
            optional_table_field_lua(
                &step,
                "scale",
                &format!("{path}.steps[{}].scale", index - 1),
                true,
            )?;
        }
    }
    Ok(())
}
