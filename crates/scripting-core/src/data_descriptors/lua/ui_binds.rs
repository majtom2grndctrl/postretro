// Data-context descriptors: Lua UI bind/style converters and helpers.
// See: context/lib/scripting.md

use super::super::*;

pub fn lua_table(value: LuaValue, what: &str) -> Result<Table, DescriptorError> {
    match value {
        LuaValue::Table(t) => Ok(t),
        other => Err(DescriptorError::InvalidShape {
            reason: format!("{what} must be a table, got {}", other.type_name()),
        }),
    }
}

pub fn read_f32_pair_lua(table: &Table, field: &'static str) -> Result<[f32; 2], DescriptorError> {
    let raw: LuaValue = table.get(field).map_err(lua_err)?;
    let arr = lua_table(raw, field)?;
    if arr.raw_len() != 2 {
        return Err(DescriptorError::InvalidShape {
            reason: format!("`{field}` must be a length-2 [x, y] array"),
        });
    }
    Ok([
        lua_value_as_f32(arr.get(1).map_err(lua_err)?, field)?,
        lua_value_as_f32(arr.get(2).map_err(lua_err)?, field)?,
    ])
}

pub fn read_f32_array_n_lua<const N: usize>(
    value: LuaValue,
    field: &str,
) -> Result<[f32; N], DescriptorError> {
    let arr = match value {
        LuaValue::Table(t) => t,
        other => {
            return Err(DescriptorError::InvalidShape {
                reason: format!(
                    "`{field}` must be a length-{N} numeric array, got {}",
                    other.type_name()
                ),
            });
        }
    };
    if arr.raw_len() != N {
        return Err(DescriptorError::InvalidShape {
            reason: format!("`{field}` must be a length-{N} numeric array"),
        });
    }
    let mut out = [0.0f32; N];
    for (i, slot) in out.iter_mut().enumerate() {
        let item: LuaValue = arr.get((i + 1) as i64).map_err(lua_err)?;
        *slot = lua_value_as_f32(item, field)?;
    }
    Ok(out)
}

pub fn lua_value_as_f32(value: LuaValue, field: &str) -> Result<f32, DescriptorError> {
    match value {
        LuaValue::Integer(i) => Ok(i as f32),
        LuaValue::Number(f) => Ok(f as f32),
        other => Err(DescriptorError::InvalidShape {
            reason: format!("`{field}` must be a number, got {}", other.type_name()),
        }),
    }
}

pub fn get_optional_string_lua(
    table: &Table,
    field: &'static str,
) -> Result<Option<String>, DescriptorError> {
    let raw: LuaValue = table.get(field).map_err(lua_err)?;
    match raw {
        LuaValue::Nil => Ok(None),
        LuaValue::String(s) => Ok(Some(s.to_str().map_err(lua_err)?.to_string())),
        other => Err(DescriptorError::InvalidShape {
            reason: format!("`{field}` must be a string, got {}", other.type_name()),
        }),
    }
}

pub fn color_value_from_lua(
    table: &Table,
    field: &'static str,
) -> Result<ColorValue, DescriptorError> {
    let raw: LuaValue = table.get(field).map_err(lua_err)?;
    if matches!(raw, LuaValue::Nil) {
        return Err(DescriptorError::MissingField { field });
    }
    color_value_from_lua_value(raw, field)
}

pub fn color_value_opt_from_lua(
    table: &Table,
    field: &'static str,
) -> Result<Option<ColorValue>, DescriptorError> {
    let raw: LuaValue = table.get(field).map_err(lua_err)?;
    if matches!(raw, LuaValue::Nil) {
        return Ok(None);
    }
    Ok(Some(color_value_from_lua_value(raw, field)?))
}

pub fn color_value_from_lua_value(
    value: LuaValue,
    field: &str,
) -> Result<ColorValue, DescriptorError> {
    match value {
        LuaValue::String(s) => Ok(ColorValue::Token(s.to_str().map_err(lua_err)?.to_string())),
        other => Ok(ColorValue::Literal(read_f32_array_n_lua::<4>(
            other, field,
        )?)),
    }
}

pub fn spacing_value_from_lua(
    table: &Table,
    field: &'static str,
) -> Result<SpacingValue, DescriptorError> {
    let raw: LuaValue = table.get(field).map_err(lua_err)?;
    match raw {
        LuaValue::Nil => Err(DescriptorError::MissingField { field }),
        LuaValue::String(s) => Ok(SpacingValue::Token(
            s.to_str().map_err(lua_err)?.to_string(),
        )),
        LuaValue::Integer(i) => Ok(SpacingValue::Literal(i as f32)),
        LuaValue::Number(f) => Ok(SpacingValue::Literal(f as f32)),
        other => Err(DescriptorError::InvalidShape {
            reason: format!(
                "`{field}` must be a number or token string, got {}",
                other.type_name()
            ),
        }),
    }
}

pub fn focus_neighbors_from_lua(table: &Table) -> Result<FocusNeighbors, DescriptorError> {
    let raw: LuaValue = table.get("focusNeighbors").map_err(lua_err)?;
    let n = match raw {
        LuaValue::Nil => return Ok(FocusNeighbors::default()),
        LuaValue::Table(t) => t,
        other => {
            return Err(DescriptorError::InvalidShape {
                reason: format!(
                    "`focusNeighbors` must be a table, got {}",
                    other.type_name()
                ),
            });
        }
    };
    Ok(FocusNeighbors {
        up: get_optional_string_lua(&n, "up")?,
        down: get_optional_string_lua(&n, "down")?,
        left: get_optional_string_lua(&n, "left")?,
        right: get_optional_string_lua(&n, "right")?,
    })
}

pub fn focus_policy_from_lua(table: &Table) -> Result<Option<FocusPolicy>, DescriptorError> {
    let raw: LuaValue = table.get("focus").map_err(lua_err)?;
    match raw {
        LuaValue::Nil => Ok(None),
        LuaValue::String(s) => Ok(Some(FocusPolicy::Shorthand(parse_focus_kind(
            s.to_str().map_err(lua_err)?.as_ref(),
        )?))),
        LuaValue::Table(o) => {
            let policy = parse_focus_kind(&get_required_string_lua(&o, "policy")?)?;
            let wrap = get_optional_bool_lua(&o, "wrap")?.unwrap_or(true);
            let repeat = repeat_policy_opt_from_lua(&o, "repeat")?;
            Ok(Some(FocusPolicy::Detailed {
                policy,
                wrap,
                repeat,
            }))
        }
        other => Err(DescriptorError::InvalidShape {
            reason: format!(
                "`focus` must be a string or table, got {}",
                other.type_name()
            ),
        }),
    }
}

pub fn repeat_policy_opt_from_lua(
    table: &Table,
    field: &'static str,
) -> Result<Option<RepeatPolicy>, DescriptorError> {
    let raw: LuaValue = table.get(field).map_err(lua_err)?;
    match raw {
        LuaValue::Nil => Ok(None),
        LuaValue::Table(o) => Ok(Some(RepeatPolicy {
            initial_delay_ms: get_required_f32_lua(&o, "initialDelayMs")?,
            interval_ms: get_required_f32_lua(&o, "intervalMs")?,
        })),
        other => Err(DescriptorError::InvalidShape {
            reason: format!("`{field}` must be a table, got {}", other.type_name()),
        }),
    }
}

pub fn border_from_lua(
    table: &Table,
    field: &'static str,
) -> Result<Option<Border>, DescriptorError> {
    let raw: LuaValue = table.get(field).map_err(lua_err)?;
    let o = match raw {
        LuaValue::Nil => return Ok(None),
        LuaValue::Table(t) => t,
        other => {
            return Err(DescriptorError::InvalidShape {
                reason: format!("`{field}` must be a table, got {}", other.type_name()),
            });
        }
    };
    let slice_val: LuaValue = o.get("slice").map_err(lua_err)?;
    Ok(Some(Border {
        texture: get_required_string_lua(&o, "texture")?,
        slice: read_f32_array_n_lua::<4>(slice_val, "border.slice")?,
        tint: color_value_from_lua(&o, "tint")?,
    }))
}

pub fn string_array_from_lua(
    table: &Table,
    field: &'static str,
) -> Result<Vec<String>, DescriptorError> {
    let raw: LuaValue = table.get(field).map_err(lua_err)?;
    let arr = match raw {
        LuaValue::Nil => return Ok(Vec::new()),
        LuaValue::Table(t) => t,
        other => {
            return Err(DescriptorError::InvalidShape {
                reason: format!(
                    "`{field}` must be a string array, got {}",
                    other.type_name()
                ),
            });
        }
    };
    let len = validate_dense_lua_array(&arr, &format!("`{field}`"))?;
    let mut out = Vec::with_capacity(len);
    for i in 1..=(len as i64) {
        match arr.get::<LuaValue>(i).map_err(lua_err)? {
            LuaValue::String(s) => out.push(s.to_str().map_err(lua_err)?.to_string()),
            other => {
                return Err(DescriptorError::InvalidShape {
                    reason: format!(
                        "`{field}` elements must be strings, got {}",
                        other.type_name()
                    ),
                });
            }
        }
    }
    Ok(out)
}

pub fn text_bind_from_lua(table: &Table) -> Result<Option<TextBind>, DescriptorError> {
    let Some(bind) = optional_table_lua(table, "bind")? else {
        return Ok(None);
    };
    Ok(Some(TextBind {
        source: bind_source_from_lua(&bind)?,
        format: get_optional_string_lua(&bind, "format")?,
        tween: text_tween_from_lua(&bind)?,
    }))
}

pub fn panel_bind_from_lua(table: &Table) -> Result<Option<PanelBind>, DescriptorError> {
    let Some(bind) = optional_table_lua(table, "bind")? else {
        return Ok(None);
    };
    Ok(Some(PanelBind {
        source: bind_source_from_lua(&bind)?,
        tween: panel_tween_from_lua(&bind)?,
    }))
}

pub fn slider_bind_from_lua(
    table: &Table,
    field: &'static str,
) -> Result<Option<SliderBind>, DescriptorError> {
    let Some(bind) = optional_table_lua(table, field)? else {
        return Ok(None);
    };
    Ok(Some(SliderBind {
        source: bind_source_from_lua(&bind)?,
        tween: text_tween_from_lua(&bind)?,
    }))
}

/// Lua twin of [`bind_source_from_js`]: read a bind's `{ slot }` vs `{ local }`
/// source. `slot` wins when present; else `local`; neither present is rejected: a
/// bind must reference a slot or a local cell.
pub fn bind_source_from_lua(bind: &Table) -> Result<BindSource, DescriptorError> {
    if let Some(slot) = get_optional_string_lua(bind, "slot")? {
        return Ok(BindSource::Slot { slot });
    }
    if let Some(local) = get_optional_string_lua(bind, "local")? {
        return Ok(BindSource::Local { local });
    }
    Err(DescriptorError::InvalidShape {
        reason: "a widget `bind` must carry either `slot` or `local`".to_string(),
    })
}

pub fn bar_max_from_lua(table: &Table) -> Result<BarMax, DescriptorError> {
    if !table.contains_key("max").map_err(lua_err)? {
        return Err(DescriptorError::MissingField { field: "max" });
    }
    let raw: LuaValue = table.get("max").map_err(lua_err)?;
    match raw {
        LuaValue::Integer(value) => finite_bar_max_literal(value as f64),
        LuaValue::Number(value) => finite_bar_max_literal(value),
        LuaValue::Table(reference) => {
            let slot = get_optional_string_lua(&reference, "slot")?.ok_or_else(|| {
                DescriptorError::InvalidShape {
                    reason: "`bar.max` state reference must carry `slot`".to_string(),
                }
            })?;
            Ok(BarMax::State(BarMaxStateRef {
                slot: validate_bar_max_slot(slot)?,
            }))
        }
        _ => Err(DescriptorError::InvalidShape {
            reason: "`bar.max` must be a number or a `{ slot }` state reference".to_string(),
        }),
    }
}

/// Lua twin of [`predicate_opt_from_js`]: read an optional [`Predicate`] field.
pub fn predicate_opt_from_lua(
    table: &Table,
    field: &'static str,
) -> Result<Option<Predicate>, DescriptorError> {
    let Some(pred) = optional_table_lua(table, field)? else {
        return Ok(None);
    };
    let source = bind_source_from_lua(&pred)?;
    let equals = predicate_value_opt_from_lua(&pred, field)?;
    Ok(Some(Predicate { source, equals }))
}

/// Lua twin of [`predicate_value_opt_from_js`]: read a predicate's optional
/// `equals` comparand. Number/bool/string only; an array (rgba) or any other
/// shape is a named load-time error.
pub fn predicate_value_opt_from_lua(
    table: &Table,
    field: &str,
) -> Result<Option<PredicateValue>, DescriptorError> {
    let raw: LuaValue = table.get("equals").map_err(lua_err)?;
    match raw {
        LuaValue::Nil => Ok(None),
        LuaValue::Boolean(b) => Ok(Some(PredicateValue::Boolean(b))),
        LuaValue::Integer(i) => Ok(Some(PredicateValue::Number(i as f64))),
        LuaValue::Number(n) => Ok(Some(PredicateValue::Number(n))),
        LuaValue::String(s) => Ok(Some(PredicateValue::String(
            s.to_str().map_err(lua_err)?.to_string(),
        ))),
        _ => Err(DescriptorError::InvalidShape {
            reason: format!(
                "`{field}.equals` must be a number, boolean, or string (rgba/array comparands are unsupported)"
            ),
        }),
    }
}

/// Lua twin of [`role_opt_from_js`]: read an optional widget `role` override.
pub fn role_opt_from_lua(table: &Table) -> Result<Option<Role>, DescriptorError> {
    match get_optional_string_lua(table, "role")? {
        Some(s) => Ok(Some(parse_role(&s)?)),
        None => Ok(None),
    }
}

pub fn text_tween_from_lua(table: &Table) -> Result<Option<TextTween>, DescriptorError> {
    let Some(tw) = optional_table_lua(table, "tween")? else {
        return Ok(None);
    };
    Ok(Some(TextTween {
        duration_ms: get_required_f32_lua(&tw, "durationMs")?,
        easing: parse_easing(&get_required_string_lua(&tw, "easing")?)?,
        from: get_optional_f32_lua(&tw, "from")?,
    }))
}

pub fn panel_tween_from_lua(table: &Table) -> Result<Option<PanelTween>, DescriptorError> {
    let Some(tw) = optional_table_lua(table, "tween")? else {
        return Ok(None);
    };
    let from_raw: LuaValue = tw.get("from").map_err(lua_err)?;
    let from = match from_raw {
        LuaValue::Nil => None,
        other => Some(read_f32_array_n_lua::<4>(other, "tween.from")?),
    };
    Ok(Some(PanelTween {
        duration_ms: get_required_f32_lua(&tw, "durationMs")?,
        easing: parse_easing(&get_required_string_lua(&tw, "easing")?)?,
        from,
    }))
}

pub fn style_ranges_from_lua(table: &Table) -> Result<Option<StyleRanges>, DescriptorError> {
    let Some(sr) = optional_table_lua(table, "styleRanges")? else {
        return Ok(None);
    };
    let max = get_required_f32_lua(&sr, "max")?;
    let entries_raw: LuaValue = sr.get("entries").map_err(lua_err)?;
    let entries_arr = lua_table(entries_raw, "styleRanges.entries")?;
    let len = entries_arr.raw_len();
    let mut entries = Vec::with_capacity(len);
    for i in 1..=(len as i64) {
        let e = lua_table(
            entries_arr.get(i).map_err(lua_err)?,
            "styleRanges.entries[]",
        )?;
        entries.push(StyleEntry {
            up_to: get_optional_f32_lua(&e, "upTo")?,
            color: color_value_opt_from_lua(&e, "color")?,
            pulse: match optional_table_lua(&e, "pulse")? {
                Some(p) => Some(Pulse {
                    period_ms: get_required_f32_lua(&p, "periodMs")?,
                }),
                None => None,
            },
            flash: match optional_table_lua(&e, "flash")? {
                Some(f) => Some(Flash {
                    duration_ms: get_required_f32_lua(&f, "durationMs")?,
                }),
                None => None,
            },
        });
    }
    Ok(Some(StyleRanges { max, entries }))
}

pub fn optional_table_lua(
    table: &Table,
    field: &'static str,
) -> Result<Option<Table>, DescriptorError> {
    let raw: LuaValue = table.get(field).map_err(lua_err)?;
    match raw {
        LuaValue::Nil => Ok(None),
        LuaValue::Table(t) => Ok(Some(t)),
        other => Err(DescriptorError::InvalidShape {
            reason: format!("`{field}` must be a table, got {}", other.type_name()),
        }),
    }
}
