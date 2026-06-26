// Data-context descriptors: shared validators and enum-string parsers.
// See: context/lib/scripting.md

use super::*;

// --- shared validation ------------------------------------------------------

pub(crate) fn validate_at(value: f32) -> Result<f32, DescriptorError> {
    if !(0.0..=1.0).contains(&value) {
        return Err(DescriptorError::AtThresholdOutOfRange { value });
    }
    Ok(value)
}

pub(crate) fn validate_primitive_name(name: String) -> Result<String, DescriptorError> {
    if name.is_empty() {
        return Err(DescriptorError::EmptyPrimitiveName);
    }
    Ok(name)
}

pub(crate) fn validate_dense_lua_array(
    table: &Table,
    field_name: &str,
) -> Result<usize, DescriptorError> {
    let mut keys = BTreeSet::new();
    let mut max_index = 0_i64;

    for pair in table.clone().pairs::<LuaValue, LuaValue>() {
        let (key, _) = pair.map_err(lua_err)?;
        let LuaValue::Integer(index) = key else {
            return Err(DescriptorError::InvalidShape {
                reason: format!(
                    "{field_name} must be a dense array; found {} key",
                    key.type_name()
                ),
            });
        };
        if index < 1 {
            return Err(DescriptorError::InvalidShape {
                reason: format!(
                    "{field_name} must be a dense array; index {index} is out of range"
                ),
            });
        }
        keys.insert(index);
        max_index = max_index.max(index);
    }

    if keys.len() != max_index as usize {
        return Err(DescriptorError::InvalidShape {
            reason: format!("{field_name} must be a dense array; holes are not allowed"),
        });
    }

    Ok(max_index as usize)
}

/// Build a [`CrossingDescriptor`] from the raw fields gathered by either FFI
/// path. Shared so JS and Luau enforce identical rules: a non-empty `slot`,
/// exactly one of `below`/`above` (the threshold value, raw), a finite default
/// `max` of `1.0`, and a `fire` list of event names (empty is permitted — the
/// watcher fires nothing, a no-op). `raw_threshold` is divided by `max` here so
/// the stored threshold is already a fraction of `max`, matching the value the
/// detector compares against (`current / max`).
pub(crate) fn build_crossing(
    slot: String,
    below: Option<f32>,
    above: Option<f32>,
    max: Option<f32>,
    fire: Vec<String>,
) -> Result<CrossingDescriptor, DescriptorError> {
    if slot.is_empty() {
        return Err(DescriptorError::InvalidShape {
            reason: "crossing entry `slot` must be a non-empty string".to_string(),
        });
    }
    let max = max.unwrap_or(1.0);
    if !max.is_finite() || max <= 0.0 {
        return Err(DescriptorError::InvalidShape {
            reason: format!("crossing entry `max` must be a finite value > 0.0, got {max}"),
        });
    }
    let condition = match (below, above) {
        (Some(below), None) => {
            if !below.is_finite() {
                return Err(DescriptorError::InvalidShape {
                    reason: format!("crossing entry `below` must be finite, got {below}"),
                });
            }
            CrossingCondition::Below {
                threshold: below / max,
            }
        }
        (None, Some(above)) => {
            if !above.is_finite() {
                return Err(DescriptorError::InvalidShape {
                    reason: format!("crossing entry `above` must be finite, got {above}"),
                });
            }
            CrossingCondition::Above {
                threshold: above / max,
            }
        }
        (None, None) => return Err(DescriptorError::CrossingCondition { count: 0 }),
        (Some(_), Some(_)) => return Err(DescriptorError::CrossingCondition { count: 2 }),
    };
    Ok(CrossingDescriptor {
        slot,
        condition,
        max,
        fire,
    })
}

/// Validate a dash expression node at declaration: wrap it in a read-only
/// [`BakedIr`] envelope and `bind` it against [`MovementScope::for_validation`],
/// then require the bound program's root type to match the field's expected
/// type. Any `BindError` (unknown input, type-table violation) and any root-type
/// mismatch map to [`DescriptorError::InvalidShape`].
///
/// The explicit root-type check is load-bearing: `bind` with no `output` never
/// checks the root's type, so without it a bool-rooted expression in a number
/// field would silently bind and evaluate as a type-zero value.
pub(crate) fn validate_dash_expr(
    node: IrNode,
    expected: IrType,
    field: &str,
) -> Result<IrNode, DescriptorError> {
    let baked = BakedIr {
        version: CURRENT_IR_VERSION,
        output: None,
        root: node,
    };
    let scope = MovementScope::for_validation();
    let program = bind(&baked, &scope).map_err(|e| DescriptorError::InvalidShape {
        reason: format!("`{field}` expression is invalid: {e}"),
    })?;
    if program.root_type != expected {
        return Err(DescriptorError::InvalidShape {
            reason: format!(
                "`{field}` expression must produce a {}, but its root produces a {}",
                ir_type_label(expected),
                ir_type_label(program.root_type)
            ),
        });
    }
    Ok(baked.root)
}

/// Deserialize a JSON value (produced from the conv bridge) into an [`IrNode`],
/// reporting a malformed node object as [`DescriptorError::InvalidShape`].
pub(crate) fn ir_node_from_json(
    value: serde_json::Value,
    field: &str,
) -> Result<IrNode, DescriptorError> {
    serde_json::from_value(value).map_err(|e| DescriptorError::InvalidShape {
        reason: format!("`{field}` is not a recognizable runtime expression: {e}"),
    })
}

pub(crate) fn ir_type_label(ty: IrType) -> &'static str {
    match ty {
        IrType::Number => "number",
        IrType::Bool => "boolean",
    }
}

pub(crate) fn validate_positive_finite(value: f32, field: &str) -> Result<f32, DescriptorError> {
    if !value.is_finite() || value <= 0.0 {
        return Err(DescriptorError::InvalidShape {
            reason: format!("`{field}` must be a finite value > 0.0, got {value}"),
        });
    }
    Ok(value)
}

pub(crate) fn validate_non_negative_finite(
    value: f32,
    field: &str,
) -> Result<f32, DescriptorError> {
    if !value.is_finite() || value < 0.0 {
        return Err(DescriptorError::InvalidShape {
            reason: format!("`{field}` must be a finite value >= 0.0, got {value}"),
        });
    }
    Ok(value)
}

pub(crate) fn validate_in_range_finite(
    value: f32,
    min: f32,
    max: f32,
    field: &str,
) -> Result<f32, DescriptorError> {
    if !value.is_finite() || value < min || value > max {
        return Err(DescriptorError::InvalidShape {
            reason: format!("`{field}` must be a finite value in [{min}, {max}], got {value}"),
        });
    }
    Ok(value)
}

/// Validate a finite value in `(min, max]` — strictly greater than `min`, at
/// most `max`. Used by `eyeHeight` which must be > 0 and at most the capsule
/// top (`half_height + radius`).
pub(crate) fn validate_in_range_finite_exclusive_min(
    value: f32,
    min: f32,
    max: f32,
    field: &str,
) -> Result<f32, DescriptorError> {
    if !value.is_finite() || value <= min || value > max {
        return Err(DescriptorError::InvalidShape {
            reason: format!("`{field}` must be a finite value in ({min}, {max}], got {value}"),
        });
    }
    Ok(value)
}

// --- shared enum-string parsers (runtime-agnostic) --------------------------

pub(crate) fn parse_anchor(s: &str) -> Result<Anchor, DescriptorError> {
    Ok(match s {
        "topLeft" => Anchor::TopLeft,
        "top" => Anchor::Top,
        "topRight" => Anchor::TopRight,
        "left" => Anchor::Left,
        "center" => Anchor::Center,
        "right" => Anchor::Right,
        "bottomLeft" => Anchor::BottomLeft,
        "bottom" => Anchor::Bottom,
        "bottomRight" => Anchor::BottomRight,
        other => {
            return Err(DescriptorError::InvalidShape {
                reason: format!("`anchor` must be a placement anchor, got \"{other}\""),
            });
        }
    })
}

pub(crate) fn parse_align(s: &str) -> Result<Align, DescriptorError> {
    Ok(match s {
        "start" => Align::Start,
        "center" => Align::Center,
        "end" => Align::End,
        "stretch" => Align::Stretch,
        other => {
            return Err(DescriptorError::InvalidShape {
                reason: format!(
                    "`align` must be \"start\"|\"center\"|\"end\"|\"stretch\", got \"{other}\""
                ),
            });
        }
    })
}

pub(crate) fn parse_capture_mode(s: &str) -> Result<CaptureMode, DescriptorError> {
    Ok(match s {
        "capture" => CaptureMode::Capture,
        "passthrough" => CaptureMode::Passthrough,
        other => {
            return Err(DescriptorError::InvalidShape {
                reason: format!(
                    "`captureMode` must be \"capture\"|\"passthrough\", got \"{other}\""
                ),
            });
        }
    })
}

pub(crate) fn parse_easing(s: &str) -> Result<Easing, DescriptorError> {
    Ok(match s {
        "linear" => Easing::Linear,
        "easeIn" => Easing::EaseIn,
        "easeOut" => Easing::EaseOut,
        "easeInOut" => Easing::EaseInOut,
        other => {
            return Err(DescriptorError::InvalidShape {
                reason: format!(
                    "`easing` must be \"linear\"|\"easeIn\"|\"easeOut\"|\"easeInOut\", got \"{other}\""
                ),
            });
        }
    })
}

pub(crate) fn parse_focus_kind(s: &str) -> Result<FocusKind, DescriptorError> {
    Ok(match s {
        "linear" => FocusKind::Linear,
        "spatial" => FocusKind::Spatial,
        other => {
            return Err(DescriptorError::InvalidShape {
                reason: format!("`focus.policy` must be \"linear\"|\"spatial\", got \"{other}\""),
            });
        }
    })
}

/// Parse a widget `role` override (M13 G2) from its camelCase wire literal. Shared
/// by the JS and Lua bridges so both reject the same unknown roles.
pub(crate) fn parse_role(s: &str) -> Result<Role, DescriptorError> {
    Ok(match s {
        "tab" => Role::Tab,
        "tablist" => Role::Tablist,
        "checkbox" => Role::Checkbox,
        "radio" => Role::Radio,
        "listitem" => Role::Listitem,
        "button" => Role::Button,
        "slider" => Role::Slider,
        "progressbar" => Role::Progressbar,
        "image" => Role::Image,
        "group" => Role::Group,
        "none" => Role::None,
        other => {
            return Err(DescriptorError::InvalidShape {
                reason: format!("unknown widget `role` \"{other}\""),
            });
        }
    })
}

/// Parse an `announce` widget `priority` (M13 G2). Shared by both bridges.
pub(crate) fn parse_priority(s: &str) -> Result<Priority, DescriptorError> {
    Ok(match s {
        "polite" => Priority::Polite,
        "assertive" => Priority::Assertive,
        other => {
            return Err(DescriptorError::InvalidShape {
                reason: format!(
                    "`announce.priority` must be \"polite\"|\"assertive\", got \"{other}\""
                ),
            });
        }
    })
}

/// Accessible-name precondition for an interactive widget (M13 G2): EXACTLY one of
/// `label` / `labelledBy` must be present. Neither or both is a named load-time
/// error (no panic). Shared by the JS and Lua bridges. `kind` names the widget for
/// diagnostics ("button"/"slider").
pub(crate) fn validate_interactive_name(
    kind: &str,
    has_label: bool,
    has_labelled_by: bool,
) -> Result<(), DescriptorError> {
    match (has_label, has_labelled_by) {
        (true, false) | (false, true) => Ok(()),
        (false, false) => Err(DescriptorError::InvalidShape {
            reason: format!(
                "a `{kind}` needs an accessible name: set exactly one of `label` or `labelledBy` (got neither)"
            ),
        }),
        (true, true) => Err(DescriptorError::InvalidShape {
            reason: format!("a `{kind}` must set exactly one of `label` or `labelledBy`, not both"),
        }),
    }
}

/// Accessible-name precondition for an `image` (M13 G2): EXACTLY one of `label` /
/// `decorative: true` must be present. Neither or both is a named load-time error.
/// Shared by both bridges.
pub(crate) fn validate_image_name(
    has_label: bool,
    decorative: bool,
) -> Result<(), DescriptorError> {
    match (has_label, decorative) {
        (true, false) | (false, true) => Ok(()),
        (false, false) => Err(DescriptorError::InvalidShape {
            reason: "an `image` needs an accessible name: set exactly one of `label` or `decorative: true` (got neither)".to_string(),
        }),
        (true, true) => Err(DescriptorError::InvalidShape {
            reason: "an `image` must set exactly one of `label` or `decorative: true`, not both".to_string(),
        }),
    }
}

pub(crate) fn is_catalog_path_relative_to_content_root(path: &str) -> bool {
    let path = Path::new(path);
    if path.is_absolute() {
        return false;
    }
    path.components().all(|component| {
        !matches!(
            component,
            Component::ParentDir | Component::Prefix(_) | Component::RootDir
        )
    })
}

pub(crate) fn validate_finite_f32(value: f32, field: &str) -> Result<f32, DescriptorError> {
    if value.is_finite() {
        Ok(value)
    } else {
        Err(DescriptorError::InvalidShape {
            reason: format!("`{field}` must be finite, got {value}"),
        })
    }
}

pub(crate) fn validate_finite_array3(
    value: [f32; 3],
    field: &str,
) -> Result<[f32; 3], DescriptorError> {
    for (index, item) in value.iter().enumerate() {
        if !item.is_finite() {
            return Err(DescriptorError::InvalidShape {
                reason: format!("`{field}[{index}]` must be finite, got {item}"),
            });
        }
    }
    Ok(value)
}
