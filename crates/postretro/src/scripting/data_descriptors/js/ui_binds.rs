// Data-context descriptors: JS UI bind/style converters and helpers.
// See: context/lib/scripting.md

use super::super::*;

pub(crate) fn read_f32_pair_js<'js>(
    obj: &Object<'js>,
    field: &'static str,
) -> Result<[f32; 2], DescriptorError> {
    let arr: Array = obj.get(field).map_err(|_| DescriptorError::InvalidShape {
        reason: format!("`{field}` must be a [x, y] array"),
    })?;
    if arr.len() != 2 {
        return Err(DescriptorError::InvalidShape {
            reason: format!(
                "`{field}` must be a length-2 [x, y] array, got {}",
                arr.len()
            ),
        });
    }
    let x: JsValue = arr.get(0).map_err(js_err)?;
    let y: JsValue = arr.get(1).map_err(js_err)?;
    Ok([js_value_as_f32(&x, field)?, js_value_as_f32(&y, field)?])
}

pub(crate) fn read_f32_array_n_js<'js, const N: usize>(
    value: &JsValue<'js>,
    field: &str,
) -> Result<[f32; N], DescriptorError> {
    let arr = value
        .as_array()
        .ok_or_else(|| DescriptorError::InvalidShape {
            reason: format!("`{field}` must be a length-{N} numeric array"),
        })?;
    if arr.len() != N {
        return Err(DescriptorError::InvalidShape {
            reason: format!(
                "`{field}` must be a length-{N} numeric array, got {}",
                arr.len()
            ),
        });
    }
    let mut out = [0.0f32; N];
    for (i, slot) in out.iter_mut().enumerate() {
        let item: JsValue = arr.get(i).map_err(js_err)?;
        *slot = js_value_as_f32(&item, field)?;
    }
    Ok(out)
}

pub(crate) fn js_value_as_f32(value: &JsValue, field: &str) -> Result<f32, DescriptorError> {
    if let Some(i) = value.as_int() {
        return Ok(i as f32);
    }
    if let Some(f) = value.as_float() {
        return Ok(f as f32);
    }
    Err(DescriptorError::InvalidShape {
        reason: format!("`{field}` must be a number"),
    })
}

pub(crate) fn get_optional_string_js<'js>(
    obj: &Object<'js>,
    field: &'static str,
) -> Result<Option<String>, DescriptorError> {
    if !obj.contains_key(field).map_err(js_err)? {
        return Ok(None);
    }
    let raw: JsValue = obj.get(field).map_err(js_err)?;
    if raw.is_null() || raw.is_undefined() {
        return Ok(None);
    }
    Ok(Some(String::from_js_value_required(raw, field)?))
}

/// A color slot: a bare `[r,g,b,a]` array → `Literal`, a bare string → `Token`.
/// Mirrors the untagged `ColorValue` wire form (array-or-string, disjoint).
pub(crate) fn color_value_from_js<'js>(
    obj: &Object<'js>,
    field: &'static str,
) -> Result<ColorValue, DescriptorError> {
    if !obj.contains_key(field).map_err(js_err)? {
        return Err(DescriptorError::MissingField { field });
    }
    let raw: JsValue = obj.get(field).map_err(js_err)?;
    color_value_from_js_value(&raw, field)
}

pub(crate) fn color_value_opt_from_js<'js>(
    obj: &Object<'js>,
    field: &'static str,
) -> Result<Option<ColorValue>, DescriptorError> {
    if !obj.contains_key(field).map_err(js_err)? {
        return Ok(None);
    }
    let raw: JsValue = obj.get(field).map_err(js_err)?;
    if raw.is_null() || raw.is_undefined() {
        return Ok(None);
    }
    Ok(Some(color_value_from_js_value(&raw, field)?))
}

pub(crate) fn color_value_from_js_value(
    value: &JsValue,
    field: &str,
) -> Result<ColorValue, DescriptorError> {
    if let Some(s) = value.as_string() {
        return Ok(ColorValue::Token(s.to_string().map_err(js_err)?));
    }
    Ok(ColorValue::Literal(read_f32_array_n_js::<4>(value, field)?))
}

/// A spacing slot: a bare number → `Literal`, a bare string → `Token`.
pub(crate) fn spacing_value_from_js<'js>(
    obj: &Object<'js>,
    field: &'static str,
) -> Result<SpacingValue, DescriptorError> {
    if !obj.contains_key(field).map_err(js_err)? {
        return Err(DescriptorError::MissingField { field });
    }
    let raw: JsValue = obj.get(field).map_err(js_err)?;
    if let Some(s) = raw.as_string() {
        return Ok(SpacingValue::Token(s.to_string().map_err(js_err)?));
    }
    Ok(SpacingValue::Literal(js_value_as_f32(&raw, field)?))
}

pub(crate) fn focus_neighbors_from_js<'js>(
    obj: &Object<'js>,
) -> Result<FocusNeighbors, DescriptorError> {
    if !obj.contains_key("focusNeighbors").map_err(js_err)? {
        return Ok(FocusNeighbors::default());
    }
    let raw: JsValue = obj.get("focusNeighbors").map_err(js_err)?;
    if raw.is_null() || raw.is_undefined() {
        return Ok(FocusNeighbors::default());
    }
    let n = Object::from_value(raw).map_err(|_| DescriptorError::InvalidShape {
        reason: "`focusNeighbors` must be an object".to_string(),
    })?;
    Ok(FocusNeighbors {
        up: get_optional_string_js(&n, "up")?,
        down: get_optional_string_js(&n, "down")?,
        left: get_optional_string_js(&n, "left")?,
        right: get_optional_string_js(&n, "right")?,
    })
}

pub(crate) fn focus_policy_from_js<'js>(
    obj: &Object<'js>,
) -> Result<Option<FocusPolicy>, DescriptorError> {
    if !obj.contains_key("focus").map_err(js_err)? {
        return Ok(None);
    }
    let raw: JsValue = obj.get("focus").map_err(js_err)?;
    if raw.is_null() || raw.is_undefined() {
        return Ok(None);
    }
    if let Some(s) = raw.as_string() {
        return Ok(Some(FocusPolicy::Shorthand(parse_focus_kind(
            &s.to_string().map_err(js_err)?,
        )?)));
    }
    let o = Object::from_value(raw).map_err(|_| DescriptorError::InvalidShape {
        reason: "`focus` must be a string or an object".to_string(),
    })?;
    let policy = parse_focus_kind(&get_required_string_js(&o, "policy")?)?;
    let wrap = get_optional_bool_js(&o, "wrap")?.unwrap_or(true);
    let repeat = repeat_policy_opt_from_js(&o, "repeat")?;
    Ok(Some(FocusPolicy::Detailed {
        policy,
        wrap,
        repeat,
    }))
}

pub(crate) fn repeat_policy_opt_from_js<'js>(
    obj: &Object<'js>,
    field: &'static str,
) -> Result<Option<RepeatPolicy>, DescriptorError> {
    if !obj.contains_key(field).map_err(js_err)? {
        return Ok(None);
    }
    let raw: JsValue = obj.get(field).map_err(js_err)?;
    if raw.is_null() || raw.is_undefined() {
        return Ok(None);
    }
    let o = Object::from_value(raw).map_err(|_| DescriptorError::InvalidShape {
        reason: format!("`{field}` must be an object"),
    })?;
    Ok(Some(RepeatPolicy {
        initial_delay_ms: get_required_f32_js(&o, "initialDelayMs")?,
        interval_ms: get_required_f32_js(&o, "intervalMs")?,
    }))
}

pub(crate) fn border_from_js<'js>(
    obj: &Object<'js>,
    field: &'static str,
) -> Result<Option<Border>, DescriptorError> {
    if !obj.contains_key(field).map_err(js_err)? {
        return Ok(None);
    }
    let raw: JsValue = obj.get(field).map_err(js_err)?;
    if raw.is_null() || raw.is_undefined() {
        return Ok(None);
    }
    let o = Object::from_value(raw).map_err(|_| DescriptorError::InvalidShape {
        reason: format!("`{field}` must be an object"),
    })?;
    let slice_val: JsValue = o.get("slice").map_err(|_| DescriptorError::InvalidShape {
        reason: "`border.slice` must be a length-4 array".to_string(),
    })?;
    Ok(Some(Border {
        texture: get_required_string_js(&o, "texture")?,
        slice: read_f32_array_n_js::<4>(&slice_val, "border.slice")?,
        tint: color_value_from_js(&o, "tint")?,
    }))
}

pub(crate) fn string_array_from_js<'js>(
    obj: &Object<'js>,
    field: &'static str,
) -> Result<Vec<String>, DescriptorError> {
    if !obj.contains_key(field).map_err(js_err)? {
        return Ok(Vec::new());
    }
    let raw: JsValue = obj.get(field).map_err(js_err)?;
    if raw.is_null() || raw.is_undefined() {
        return Ok(Vec::new());
    }
    let arr: Array = obj.get(field).map_err(|_| DescriptorError::InvalidShape {
        reason: format!("`{field}` must be a string array"),
    })?;
    let mut out = Vec::with_capacity(arr.len());
    for i in 0..arr.len() {
        let item: JsValue = arr.get(i).map_err(js_err)?;
        out.push(String::from_js_value_required(item, field)?);
    }
    Ok(out)
}

pub(crate) fn text_bind_from_js<'js>(
    ctx: &Ctx<'js>,
    obj: &Object<'js>,
) -> Result<Option<TextBind>, DescriptorError> {
    let Some(bind_obj) = optional_object_js(obj, "bind")? else {
        return Ok(None);
    };
    Ok(Some(TextBind {
        source: bind_source_from_js(&bind_obj)?,
        format: get_optional_string_js(&bind_obj, "format")?,
        tween: text_tween_from_js(ctx, &bind_obj)?,
    }))
}

pub(crate) fn panel_bind_from_js<'js>(
    _ctx: &Ctx<'js>,
    obj: &Object<'js>,
) -> Result<Option<PanelBind>, DescriptorError> {
    let Some(bind_obj) = optional_object_js(obj, "bind")? else {
        return Ok(None);
    };
    Ok(Some(PanelBind {
        source: bind_source_from_js(&bind_obj)?,
        tween: panel_tween_from_js(&bind_obj)?,
    }))
}

pub(crate) fn slider_bind_from_js<'js>(
    ctx: &Ctx<'js>,
    obj: &Object<'js>,
    field: &'static str,
) -> Result<Option<SliderBind>, DescriptorError> {
    let Some(bind_obj) = optional_object_js(obj, field)? else {
        return Ok(None);
    };
    Ok(Some(SliderBind {
        source: bind_source_from_js(&bind_obj)?,
        tween: text_tween_from_js(ctx, &bind_obj)?,
    }))
}

/// Read a bind's `{ slot }` vs `{ local }` source. `slot` wins when present (a
/// store binding); else `local` (a presentation-cell binding). Neither present is
/// rejected: a bind must reference a slot or a local cell.
pub(crate) fn bind_source_from_js<'js>(
    bind_obj: &Object<'js>,
) -> Result<BindSource, DescriptorError> {
    if let Some(slot) = get_optional_string_js(bind_obj, "slot")? {
        return Ok(BindSource::Slot { slot });
    }
    if let Some(local) = get_optional_string_js(bind_obj, "local")? {
        return Ok(BindSource::Local { local });
    }
    Err(DescriptorError::InvalidShape {
        reason: "a widget `bind` must carry either `slot` or `local`".to_string(),
    })
}

pub(crate) fn bar_max_from_js<'js>(obj: &Object<'js>) -> Result<BarMax, DescriptorError> {
    if !obj.contains_key("max").map_err(js_err)? {
        return Err(DescriptorError::MissingField { field: "max" });
    }
    let raw: JsValue = obj.get("max").map_err(js_err)?;
    if raw.is_null() || raw.is_undefined() {
        return Err(DescriptorError::MissingField { field: "max" });
    }
    if let Some(i) = raw.as_int() {
        return finite_bar_max_literal(f64::from(i));
    }
    if let Some(f) = raw.as_float() {
        return finite_bar_max_literal(f);
    }
    let reference = Object::from_value(raw).map_err(|_| DescriptorError::InvalidShape {
        reason: "`bar.max` must be a number or a `{ slot }` state reference".to_string(),
    })?;
    let slot = get_optional_string_js(&reference, "slot")?.ok_or_else(|| {
        DescriptorError::InvalidShape {
            reason: "`bar.max` state reference must carry `slot`".to_string(),
        }
    })?;
    Ok(BarMax::State(BarMaxStateRef {
        slot: validate_bar_max_slot(slot)?,
    }))
}

pub(crate) fn finite_bar_max_literal(value: f64) -> Result<BarMax, DescriptorError> {
    let narrowed = value as f32;
    if !value.is_finite() || !narrowed.is_finite() {
        return Err(DescriptorError::InvalidShape {
            reason: format!("`bar.max` must be a finite number, got {value}"),
        });
    }
    Ok(BarMax::Literal(narrowed))
}

pub(crate) fn validate_bar_max_slot(slot: String) -> Result<String, DescriptorError> {
    if slot.is_empty() {
        return Err(DescriptorError::InvalidShape {
            reason: "`bar.max` state reference must carry a non-empty `slot`".to_string(),
        });
    }
    Ok(slot)
}

/// Read an optional [`Predicate`] field (M13 G2). Absent/null yields `None`. The
/// predicate carries a `{ slot }`/`{ local }` source flattened beside an optional
/// `equals` comparand whose value must be a number, boolean, or string — an
/// rgba/array comparand is a named load-time error (mirrors the serde reject of
/// the array form). `field` names the source key for diagnostics.
pub(crate) fn predicate_opt_from_js<'js>(
    obj: &Object<'js>,
    field: &'static str,
) -> Result<Option<Predicate>, DescriptorError> {
    let Some(pred_obj) = optional_object_js(obj, field)? else {
        return Ok(None);
    };
    let source = bind_source_from_js(&pred_obj)?;
    let equals = predicate_value_opt_from_js(&pred_obj, field)?;
    Ok(Some(Predicate { source, equals }))
}

/// Read a predicate's optional `equals` comparand (M13 G2). Number/bool/string
/// only; an array (rgba) or any other shape is a named load-time error.
pub(crate) fn predicate_value_opt_from_js<'js>(
    obj: &Object<'js>,
    field: &str,
) -> Result<Option<PredicateValue>, DescriptorError> {
    if !obj.contains_key("equals").map_err(js_err)? {
        return Ok(None);
    }
    let raw: JsValue = obj.get("equals").map_err(js_err)?;
    if raw.is_null() || raw.is_undefined() {
        return Ok(None);
    }
    if let Some(b) = raw.as_bool() {
        return Ok(Some(PredicateValue::Boolean(b)));
    }
    if let Some(n) = raw.as_number() {
        return Ok(Some(PredicateValue::Number(n)));
    }
    if let Some(s) = raw.as_string() {
        return Ok(Some(PredicateValue::String(s.to_string().map_err(js_err)?)));
    }
    Err(DescriptorError::InvalidShape {
        reason: format!(
            "`{field}.equals` must be a number, boolean, or string (rgba/array comparands are unsupported)"
        ),
    })
}

/// Read an optional widget `role` override (M13 G2). Absent/null yields `None`.
pub(crate) fn role_opt_from_js<'js>(obj: &Object<'js>) -> Result<Option<Role>, DescriptorError> {
    match get_optional_string_js(obj, "role")? {
        Some(s) => Ok(Some(parse_role(&s)?)),
        None => Ok(None),
    }
}

pub(crate) fn text_tween_from_js<'js>(
    _ctx: &Ctx<'js>,
    obj: &Object<'js>,
) -> Result<Option<TextTween>, DescriptorError> {
    let Some(tw) = optional_object_js(obj, "tween")? else {
        return Ok(None);
    };
    Ok(Some(TextTween {
        duration_ms: get_required_f32_js(&tw, "durationMs")?,
        easing: parse_easing(&get_required_string_js(&tw, "easing")?)?,
        from: get_optional_f32_js(&tw, "from")?,
    }))
}

pub(crate) fn panel_tween_from_js<'js>(
    obj: &Object<'js>,
) -> Result<Option<PanelTween>, DescriptorError> {
    let Some(tw) = optional_object_js(obj, "tween")? else {
        return Ok(None);
    };
    let from = match optional_value_js(&tw, "from")? {
        Some(v) => Some(read_f32_array_n_js::<4>(&v, "tween.from")?),
        None => None,
    };
    Ok(Some(PanelTween {
        duration_ms: get_required_f32_js(&tw, "durationMs")?,
        easing: parse_easing(&get_required_string_js(&tw, "easing")?)?,
        from,
    }))
}

pub(crate) fn style_ranges_from_js<'js>(
    _ctx: &Ctx<'js>,
    obj: &Object<'js>,
) -> Result<Option<StyleRanges>, DescriptorError> {
    let Some(sr) = optional_object_js(obj, "styleRanges")? else {
        return Ok(None);
    };
    let max = get_required_f32_js(&sr, "max")?;
    let entries_arr: Array = sr
        .get("entries")
        .map_err(|_| DescriptorError::InvalidShape {
            reason: "`styleRanges.entries` must be an array".to_string(),
        })?;
    let mut entries = Vec::with_capacity(entries_arr.len());
    for i in 0..entries_arr.len() {
        let item: JsValue = entries_arr.get(i).map_err(js_err)?;
        let e = Object::from_value(item).map_err(|_| DescriptorError::InvalidShape {
            reason: format!("`styleRanges.entries[{i}]` must be an object"),
        })?;
        entries.push(StyleEntry {
            up_to: get_optional_f32_js(&e, "upTo")?,
            color: color_value_opt_from_js(&e, "color")?,
            pulse: match optional_object_js(&e, "pulse")? {
                Some(p) => Some(Pulse {
                    period_ms: get_required_f32_js(&p, "periodMs")?,
                }),
                None => None,
            },
            flash: match optional_object_js(&e, "flash")? {
                Some(f) => Some(Flash {
                    duration_ms: get_required_f32_js(&f, "durationMs")?,
                }),
                None => None,
            },
        });
    }
    Ok(Some(StyleRanges { max, entries }))
}

pub(crate) fn optional_object_js<'js>(
    obj: &Object<'js>,
    field: &'static str,
) -> Result<Option<Object<'js>>, DescriptorError> {
    let Some(raw) = optional_value_js(obj, field)? else {
        return Ok(None);
    };
    Ok(Some(Object::from_value(raw).map_err(|_| {
        DescriptorError::InvalidShape {
            reason: format!("`{field}` must be an object"),
        }
    })?))
}

pub(crate) fn optional_value_js<'js>(
    obj: &Object<'js>,
    field: &'static str,
) -> Result<Option<JsValue<'js>>, DescriptorError> {
    if !obj.contains_key(field).map_err(js_err)? {
        return Ok(None);
    }
    let raw: JsValue = obj.get(field).map_err(js_err)?;
    if raw.is_null() || raw.is_undefined() {
        return Ok(None);
    }
    Ok(Some(raw))
}
