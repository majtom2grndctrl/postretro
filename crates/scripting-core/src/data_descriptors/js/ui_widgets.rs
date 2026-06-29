// Data-context descriptors: JS UI widget-tree converters.
// See: context/lib/scripting.md

use super::super::*;

// --- JS UI deserialization --------------------------------------------------

/// Convert a QuickJS descriptor value (the object returned by the `tree`
/// factory) into a typed [`AnchoredTree`]. Mirrors [`entity_descriptor_from_js`]:
/// a hand-written field reader that builds the typed tree directly (no
/// `serde_json::Value` lowering), returning a named [`DescriptorError`] on
/// malformed input and never panicking.
pub fn anchored_tree_from_js_value<'js>(
    ctx: &Ctx<'js>,
    value: JsValue<'js>,
) -> Result<AnchoredTree, DescriptorError> {
    let obj = Object::from_value(value).map_err(|_| DescriptorError::InvalidShape {
        reason: "anchored tree must be an object".to_string(),
    })?;

    let anchor = parse_anchor(&get_required_string_js(&obj, "anchor")?)?;
    let offset = read_f32_pair_js(&obj, "offset")?;

    if !obj.contains_key("root").map_err(js_err)? {
        return Err(DescriptorError::MissingField { field: "root" });
    }
    let root_val: JsValue = obj.get("root").map_err(js_err)?;
    let root = widget_from_js(ctx, root_val)?;

    let capture_mode = match get_optional_string_js(&obj, "captureMode")? {
        Some(s) => parse_capture_mode(&s)?,
        None => CaptureMode::Passthrough,
    };
    let initial_focus = get_optional_string_js(&obj, "initialFocus")?;
    let text_entry_target = get_optional_string_js(&obj, "textEntryTarget")?;
    let accessible_name = get_optional_string_js(&obj, "accessibleName")?;
    let role = role_opt_from_js(&obj)?;

    Ok(AnchoredTree {
        anchor,
        offset,
        root,
        capture_mode,
        initial_focus,
        text_entry_target,
        accessible_name,
        role,
    })
}

pub fn widget_from_js<'js>(ctx: &Ctx<'js>, value: JsValue<'js>) -> Result<Widget, DescriptorError> {
    let obj = Object::from_value(value).map_err(|_| DescriptorError::InvalidShape {
        reason: "widget must be an object".to_string(),
    })?;
    let kind = get_required_string_js(&obj, "kind")?;
    Ok(match kind.as_str() {
        "text" => Widget::Text(text_widget_from_js(ctx, &obj)?),
        "panel" => Widget::Panel(panel_widget_from_js(ctx, &obj)?),
        "image" => Widget::Image(image_widget_from_js(&obj)?),
        "vstack" => Widget::VStack(container_widget_from_js(ctx, &obj)?),
        "hstack" => Widget::HStack(container_widget_from_js(ctx, &obj)?),
        "grid" => Widget::Grid(grid_widget_from_js(ctx, &obj)?),
        "spacer" => Widget::Spacer(spacer_widget_from_js(&obj)?),
        "button" => Widget::Button(button_widget_from_js(ctx, &obj)?),
        "slider" => Widget::Slider(slider_widget_from_js(ctx, &obj)?),
        "bar" => Widget::Bar(bar_widget_from_js(ctx, &obj)?),
        "announce" => Widget::Announce(announce_widget_from_js(&obj)?),
        other => {
            return Err(DescriptorError::InvalidShape {
                reason: format!("unknown widget `kind` \"{other}\""),
            });
        }
    })
}

pub fn text_widget_from_js<'js>(
    ctx: &Ctx<'js>,
    obj: &Object<'js>,
) -> Result<TextWidget, DescriptorError> {
    Ok(TextWidget {
        content: get_required_string_js(obj, "content")?,
        font_size: get_required_f32_js(obj, "fontSize")?,
        color: color_value_from_js(obj, "color")?,
        id: get_optional_string_js(obj, "id")?,
        focus_neighbors: focus_neighbors_from_js(obj)?,
        font: get_optional_string_js(obj, "font")?,
        bind: text_bind_from_js(ctx, obj)?,
        style_ranges: style_ranges_from_js(ctx, obj)?,
        visible_when: predicate_opt_from_js(obj, "visibleWhen")?,
        role: role_opt_from_js(obj)?,
    })
}

pub fn panel_widget_from_js<'js>(
    ctx: &Ctx<'js>,
    obj: &Object<'js>,
) -> Result<PanelWidget, DescriptorError> {
    Ok(PanelWidget {
        fill: color_value_from_js(obj, "fill")?,
        border: border_from_js(obj, "border")?,
        id: get_optional_string_js(obj, "id")?,
        focus_neighbors: focus_neighbors_from_js(obj)?,
        bind: panel_bind_from_js(ctx, obj)?,
        style_ranges: style_ranges_from_js(ctx, obj)?,
        visible_when: predicate_opt_from_js(obj, "visibleWhen")?,
        role: role_opt_from_js(obj)?,
    })
}

pub fn image_widget_from_js<'js>(obj: &Object<'js>) -> Result<ImageWidget, DescriptorError> {
    let label = get_optional_string_js(obj, "label")?;
    let decorative = get_optional_bool_js(obj, "decorative")?.unwrap_or(false);
    validate_image_name(label.is_some(), decorative)?;
    Ok(ImageWidget {
        asset: get_required_string_js(obj, "asset")?,
        id: get_optional_string_js(obj, "id")?,
        focus_neighbors: focus_neighbors_from_js(obj)?,
        label,
        decorative,
        visible_when: predicate_opt_from_js(obj, "visibleWhen")?,
        role: role_opt_from_js(obj)?,
    })
}

pub fn container_widget_from_js<'js>(
    ctx: &Ctx<'js>,
    obj: &Object<'js>,
) -> Result<ContainerWidget, DescriptorError> {
    Ok(ContainerWidget {
        gap: spacing_value_from_js(obj, "gap")?,
        padding: spacing_value_from_js(obj, "padding")?,
        align: parse_align(&get_required_string_js(obj, "align")?)?,
        fill: color_value_opt_from_js(obj, "fill")?,
        border: border_from_js(obj, "border")?,
        id: get_optional_string_js(obj, "id")?,
        focus_neighbors: focus_neighbors_from_js(obj)?,
        focus: focus_policy_from_js(obj)?,
        restore_on_return: get_optional_bool_js(obj, "restoreOnReturn")?.unwrap_or(false),
        local_state: local_state_from_js(obj)?,
        visible_when: predicate_opt_from_js(obj, "visibleWhen")?,
        role: role_opt_from_js(obj)?,
        children: children_from_js(ctx, obj)?,
    })
}

/// Read a container's optional `localState` declaration (M13 G1b, Task 5): the
/// stable `scope` id plus the `cells` map of declared initial values. Absent
/// returns `None` (a localState-less container). The cell value shapes mirror the
/// `CellInit` wire form: a bare number/boolean/string or a length-4 RGBA array.
pub fn local_state_from_js<'js>(obj: &Object<'js>) -> Result<Option<LocalState>, DescriptorError> {
    let Some(ls) = optional_object_js(obj, "localState")? else {
        return Ok(None);
    };
    let scope = get_required_string_js(&ls, "scope")?;
    let cells_obj = optional_object_js(&ls, "cells")?.ok_or(DescriptorError::InvalidShape {
        reason: "`localState.cells` must be an object".to_string(),
    })?;
    let mut cells = std::collections::BTreeMap::new();
    for key in cells_obj.keys::<String>() {
        let key = key.map_err(js_err)?;
        let value: JsValue = cells_obj.get(&*key).map_err(js_err)?;
        cells.insert(key, cell_init_from_js(value)?);
    }
    Ok(Some(LocalState { scope, cells }))
}

/// Read one declared cell initial value (M13 G1b, Task 5) from a JS value: a
/// number, boolean, string, or length-4 numeric array. Any other shape is a hard
/// error (a cell must seed a usable value).
pub fn cell_init_from_js(value: JsValue) -> Result<CellInit, DescriptorError> {
    if let Some(b) = value.as_bool() {
        return Ok(CellInit::Boolean(b));
    }
    if let Some(n) = value.as_number() {
        return Ok(CellInit::Number(n));
    }
    if let Some(s) = value.as_string() {
        return Ok(CellInit::String(s.to_string().map_err(js_err)?));
    }
    if value.is_array() {
        let arr = read_f32_array_n_js::<4>(&value, "localState.cells[*]")?;
        return Ok(CellInit::Array(arr));
    }
    Err(DescriptorError::InvalidShape {
        reason: "a `localState` cell must be a number, boolean, string, or length-4 array"
            .to_string(),
    })
}

pub fn grid_widget_from_js<'js>(
    ctx: &Ctx<'js>,
    obj: &Object<'js>,
) -> Result<GridWidget, DescriptorError> {
    Ok(GridWidget {
        gap: spacing_value_from_js(obj, "gap")?,
        padding: spacing_value_from_js(obj, "padding")?,
        align: parse_align(&get_required_string_js(obj, "align")?)?,
        cols: get_required_u32_js(obj, "cols")?,
        id: get_optional_string_js(obj, "id")?,
        focus_neighbors: focus_neighbors_from_js(obj)?,
        focus: focus_policy_from_js(obj)?,
        restore_on_return: get_optional_bool_js(obj, "restoreOnReturn")?.unwrap_or(false),
        visible_when: predicate_opt_from_js(obj, "visibleWhen")?,
        role: role_opt_from_js(obj)?,
        children: children_from_js(ctx, obj)?,
    })
}

pub fn spacer_widget_from_js<'js>(obj: &Object<'js>) -> Result<SpacerWidget, DescriptorError> {
    Ok(SpacerWidget {
        flex_grow: get_required_f32_js(obj, "flexGrow")?,
        id: get_optional_string_js(obj, "id")?,
        visible_when: predicate_opt_from_js(obj, "visibleWhen")?,
        role: role_opt_from_js(obj)?,
    })
}

pub fn button_widget_from_js<'js>(
    ctx: &Ctx<'js>,
    obj: &Object<'js>,
) -> Result<ButtonWidget, DescriptorError> {
    let label = get_optional_string_js(obj, "label")?;
    let labelled_by = get_optional_string_js(obj, "labelledBy")?;
    validate_interactive_name("button", label.is_some(), labelled_by.is_some())?;
    Ok(ButtonWidget {
        id: get_required_string_js(obj, "id")?,
        label,
        labelled_by,
        on_press: get_required_string_js(obj, "onPress")?,
        focus_neighbors: focus_neighbors_from_js(obj)?,
        repeat_on_hold: repeat_policy_opt_from_js(obj, "repeatOnHold")?,
        selected: predicate_opt_from_js(obj, "selected")?,
        checked: predicate_opt_from_js(obj, "checked")?,
        bind: predicate_opt_from_js(obj, "bind")?,
        style_ranges: style_ranges_from_js(ctx, obj)?,
        disabled: get_optional_bool_js(obj, "disabled")?.unwrap_or(false),
        visible_when: predicate_opt_from_js(obj, "visibleWhen")?,
        role: role_opt_from_js(obj)?,
    })
}

pub fn slider_widget_from_js<'js>(
    ctx: &Ctx<'js>,
    obj: &Object<'js>,
) -> Result<SliderWidget, DescriptorError> {
    let label = get_optional_string_js(obj, "label")?;
    let labelled_by = get_optional_string_js(obj, "labelledBy")?;
    validate_interactive_name("slider", label.is_some(), labelled_by.is_some())?;
    Ok(SliderWidget {
        id: get_required_string_js(obj, "id")?,
        label,
        labelled_by,
        bind: slider_bind_from_js(ctx, obj, "bind")?
            .ok_or(DescriptorError::MissingField { field: "bind" })?,
        min: get_required_f32_js(obj, "min")?,
        max: get_required_f32_js(obj, "max")?,
        step: get_required_f32_js(obj, "step")?,
        captures_nav: string_array_from_js(obj, "capturesNav")?,
        focus_neighbors: focus_neighbors_from_js(obj)?,
        disabled: get_optional_bool_js(obj, "disabled")?.unwrap_or(false),
        visible_when: predicate_opt_from_js(obj, "visibleWhen")?,
        role: role_opt_from_js(obj)?,
    })
}

pub fn bar_widget_from_js<'js>(
    ctx: &Ctx<'js>,
    obj: &Object<'js>,
) -> Result<BarWidget, DescriptorError> {
    Ok(BarWidget {
        bind: slider_bind_from_js(ctx, obj, "bind")?
            .ok_or(DescriptorError::MissingField { field: "bind" })?,
        max: bar_max_from_js(obj)?,
        fill: color_value_from_js(obj, "fill")?,
        background: color_value_from_js(obj, "background")?,
        id: get_optional_string_js(obj, "id")?,
        style_ranges: style_ranges_from_js(ctx, obj)?,
        visible_when: predicate_opt_from_js(obj, "visibleWhen")?,
        role: role_opt_from_js(obj)?,
    })
}

/// Read an `announce` widget (M13 G2): a required `text` and an optional
/// `priority` (`"polite"`|`"assertive"`, default polite). A garbled shape — a
/// non-string `text`, a missing `text`, an empty `text`, or an unknown
/// `priority` — is a named load-time error.
pub fn announce_widget_from_js<'js>(obj: &Object<'js>) -> Result<AnnounceWidget, DescriptorError> {
    let text = get_required_string_js(obj, "text")?;
    if text.is_empty() {
        return Err(DescriptorError::InvalidShape {
            reason: "`announce.text` must be a non-empty string".to_string(),
        });
    }
    Ok(AnnounceWidget {
        text,
        priority: match get_optional_string_js(obj, "priority")? {
            Some(s) => parse_priority(&s)?,
            None => Priority::Polite,
        },
        visible_when: predicate_opt_from_js(obj, "visibleWhen")?,
    })
}

// --- JS leaf-field readers --------------------------------------------------

/// Read `children` straight into a `Vec<Widget>`. An absent or null array yields
/// the empty vec — the container's `children: []` form — so an empty container
/// parses cleanly without depending on the JSON empty-table/array convention.
pub fn children_from_js<'js>(
    ctx: &Ctx<'js>,
    obj: &Object<'js>,
) -> Result<Vec<Widget>, DescriptorError> {
    if !obj.contains_key("children").map_err(js_err)? {
        return Ok(Vec::new());
    }
    let raw: JsValue = obj.get("children").map_err(js_err)?;
    if raw.is_null() || raw.is_undefined() {
        return Ok(Vec::new());
    }
    let arr: Array = obj
        .get("children")
        .map_err(|_| DescriptorError::InvalidShape {
            reason: "`children` must be an array of widgets".to_string(),
        })?;
    let mut out = Vec::with_capacity(arr.len());
    for i in 0..arr.len() {
        let item: JsValue = arr.get(i).map_err(js_err)?;
        out.push(widget_from_js(ctx, item)?);
    }
    Ok(out)
}
