// Data-context descriptors: Lua UI widget-tree converters.
// See: context/lib/scripting.md

use super::super::*;

// --- Lua UI deserialization -------------------------------------------------

/// Mirror of [`anchored_tree_from_js_value`] for Luau tables. Builds the typed
/// [`AnchoredTree`] directly: reading `children` straight into a `Vec<Widget>`
/// is what makes an EMPTY Luau container (an empty `children` table, which the
/// generic `lua_to_json` walker would emit as `{}` not `[]`) deserialize
/// cleanly. Returns a named [`DescriptorError`] on malformed input; never panics.
pub(crate) fn anchored_tree_from_lua_value(
    value: LuaValue,
) -> Result<AnchoredTree, DescriptorError> {
    let table = lua_table(value, "anchored tree")?;

    let anchor = parse_anchor(&get_required_string_lua(&table, "anchor")?)?;
    let offset = read_f32_pair_lua(&table, "offset")?;

    let root_val: LuaValue = table.get("root").map_err(lua_err)?;
    if matches!(root_val, LuaValue::Nil) {
        return Err(DescriptorError::MissingField { field: "root" });
    }
    let root = widget_from_lua(root_val)?;

    let capture_mode = match get_optional_string_lua(&table, "captureMode")? {
        Some(s) => parse_capture_mode(&s)?,
        None => CaptureMode::Passthrough,
    };
    let initial_focus = get_optional_string_lua(&table, "initialFocus")?;
    let text_entry_target = get_optional_string_lua(&table, "textEntryTarget")?;
    let accessible_name = get_optional_string_lua(&table, "accessibleName")?;
    let role = role_opt_from_lua(&table)?;

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

pub(crate) fn widget_from_lua(value: LuaValue) -> Result<Widget, DescriptorError> {
    let table = lua_table(value, "widget")?;
    let kind = get_required_string_lua(&table, "kind")?;
    Ok(match kind.as_str() {
        "text" => Widget::Text(text_widget_from_lua(&table)?),
        "panel" => Widget::Panel(panel_widget_from_lua(&table)?),
        "image" => Widget::Image(image_widget_from_lua(&table)?),
        "vstack" => Widget::VStack(container_widget_from_lua(&table)?),
        "hstack" => Widget::HStack(container_widget_from_lua(&table)?),
        "grid" => Widget::Grid(grid_widget_from_lua(&table)?),
        "spacer" => Widget::Spacer(spacer_widget_from_lua(&table)?),
        "button" => Widget::Button(button_widget_from_lua(&table)?),
        "slider" => Widget::Slider(slider_widget_from_lua(&table)?),
        "bar" => Widget::Bar(bar_widget_from_lua(&table)?),
        "announce" => Widget::Announce(announce_widget_from_lua(&table)?),
        other => {
            return Err(DescriptorError::InvalidShape {
                reason: format!("unknown widget `kind` \"{other}\""),
            });
        }
    })
}

pub(crate) fn text_widget_from_lua(table: &Table) -> Result<TextWidget, DescriptorError> {
    Ok(TextWidget {
        content: get_required_string_lua(table, "content")?,
        font_size: get_required_f32_lua(table, "fontSize")?,
        color: color_value_from_lua(table, "color")?,
        id: get_optional_string_lua(table, "id")?,
        focus_neighbors: focus_neighbors_from_lua(table)?,
        font: get_optional_string_lua(table, "font")?,
        bind: text_bind_from_lua(table)?,
        style_ranges: style_ranges_from_lua(table)?,
        visible_when: predicate_opt_from_lua(table, "visibleWhen")?,
        role: role_opt_from_lua(table)?,
    })
}

pub(crate) fn panel_widget_from_lua(table: &Table) -> Result<PanelWidget, DescriptorError> {
    Ok(PanelWidget {
        fill: color_value_from_lua(table, "fill")?,
        border: border_from_lua(table, "border")?,
        id: get_optional_string_lua(table, "id")?,
        focus_neighbors: focus_neighbors_from_lua(table)?,
        bind: panel_bind_from_lua(table)?,
        style_ranges: style_ranges_from_lua(table)?,
        visible_when: predicate_opt_from_lua(table, "visibleWhen")?,
        role: role_opt_from_lua(table)?,
    })
}

pub(crate) fn image_widget_from_lua(table: &Table) -> Result<ImageWidget, DescriptorError> {
    let label = get_optional_string_lua(table, "label")?;
    let decorative = get_optional_bool_lua(table, "decorative")?.unwrap_or(false);
    validate_image_name(label.is_some(), decorative)?;
    Ok(ImageWidget {
        asset: get_required_string_lua(table, "asset")?,
        id: get_optional_string_lua(table, "id")?,
        focus_neighbors: focus_neighbors_from_lua(table)?,
        label,
        decorative,
        visible_when: predicate_opt_from_lua(table, "visibleWhen")?,
        role: role_opt_from_lua(table)?,
    })
}

pub(crate) fn container_widget_from_lua(table: &Table) -> Result<ContainerWidget, DescriptorError> {
    Ok(ContainerWidget {
        gap: spacing_value_from_lua(table, "gap")?,
        padding: spacing_value_from_lua(table, "padding")?,
        align: parse_align(&get_required_string_lua(table, "align")?)?,
        fill: color_value_opt_from_lua(table, "fill")?,
        border: border_from_lua(table, "border")?,
        id: get_optional_string_lua(table, "id")?,
        focus_neighbors: focus_neighbors_from_lua(table)?,
        focus: focus_policy_from_lua(table)?,
        restore_on_return: get_optional_bool_lua(table, "restoreOnReturn")?.unwrap_or(false),
        local_state: local_state_from_lua(table)?,
        visible_when: predicate_opt_from_lua(table, "visibleWhen")?,
        role: role_opt_from_lua(table)?,
        children: children_from_lua(table)?,
    })
}

/// Lua twin of [`local_state_from_js`]: read a container's optional `localState`.
pub(crate) fn local_state_from_lua(table: &Table) -> Result<Option<LocalState>, DescriptorError> {
    let Some(ls) = optional_table_lua(table, "localState")? else {
        return Ok(None);
    };
    let scope = get_required_string_lua(&ls, "scope")?;
    let cells_table = optional_table_lua(&ls, "cells")?.ok_or(DescriptorError::InvalidShape {
        reason: "`localState.cells` must be a table".to_string(),
    })?;
    let mut cells = std::collections::BTreeMap::new();
    for pair in cells_table.clone().pairs::<String, LuaValue>() {
        let (key, value) = pair.map_err(lua_err)?;
        cells.insert(key, cell_init_from_lua(value)?);
    }
    Ok(Some(LocalState { scope, cells }))
}

/// Lua twin of [`cell_init_from_js`]: read one declared cell initial value.
pub(crate) fn cell_init_from_lua(value: LuaValue) -> Result<CellInit, DescriptorError> {
    match value {
        LuaValue::Boolean(b) => Ok(CellInit::Boolean(b)),
        LuaValue::Integer(i) => Ok(CellInit::Number(i as f64)),
        LuaValue::Number(n) => Ok(CellInit::Number(n)),
        LuaValue::String(ref s) => Ok(CellInit::String(s.to_str().map_err(lua_err)?.to_string())),
        LuaValue::Table(_) => {
            let arr = read_f32_array_n_lua::<4>(value, "localState.cells[*]")?;
            Ok(CellInit::Array(arr))
        }
        _ => Err(DescriptorError::InvalidShape {
            reason: "a `localState` cell must be a number, boolean, string, or length-4 array"
                .to_string(),
        }),
    }
}

pub(crate) fn grid_widget_from_lua(table: &Table) -> Result<GridWidget, DescriptorError> {
    Ok(GridWidget {
        gap: spacing_value_from_lua(table, "gap")?,
        padding: spacing_value_from_lua(table, "padding")?,
        align: parse_align(&get_required_string_lua(table, "align")?)?,
        cols: get_required_u32_lua(table, "cols")?,
        id: get_optional_string_lua(table, "id")?,
        focus_neighbors: focus_neighbors_from_lua(table)?,
        focus: focus_policy_from_lua(table)?,
        restore_on_return: get_optional_bool_lua(table, "restoreOnReturn")?.unwrap_or(false),
        visible_when: predicate_opt_from_lua(table, "visibleWhen")?,
        role: role_opt_from_lua(table)?,
        children: children_from_lua(table)?,
    })
}

pub(crate) fn spacer_widget_from_lua(table: &Table) -> Result<SpacerWidget, DescriptorError> {
    Ok(SpacerWidget {
        flex_grow: get_required_f32_lua(table, "flexGrow")?,
        id: get_optional_string_lua(table, "id")?,
        visible_when: predicate_opt_from_lua(table, "visibleWhen")?,
        role: role_opt_from_lua(table)?,
    })
}

pub(crate) fn button_widget_from_lua(table: &Table) -> Result<ButtonWidget, DescriptorError> {
    let label = get_optional_string_lua(table, "label")?;
    let labelled_by = get_optional_string_lua(table, "labelledBy")?;
    validate_interactive_name("button", label.is_some(), labelled_by.is_some())?;
    Ok(ButtonWidget {
        id: get_required_string_lua(table, "id")?,
        label,
        labelled_by,
        on_press: get_required_string_lua(table, "onPress")?,
        focus_neighbors: focus_neighbors_from_lua(table)?,
        repeat_on_hold: repeat_policy_opt_from_lua(table, "repeatOnHold")?,
        selected: predicate_opt_from_lua(table, "selected")?,
        checked: predicate_opt_from_lua(table, "checked")?,
        bind: predicate_opt_from_lua(table, "bind")?,
        style_ranges: style_ranges_from_lua(table)?,
        disabled: get_optional_bool_lua(table, "disabled")?.unwrap_or(false),
        visible_when: predicate_opt_from_lua(table, "visibleWhen")?,
        role: role_opt_from_lua(table)?,
    })
}

pub(crate) fn slider_widget_from_lua(table: &Table) -> Result<SliderWidget, DescriptorError> {
    let label = get_optional_string_lua(table, "label")?;
    let labelled_by = get_optional_string_lua(table, "labelledBy")?;
    validate_interactive_name("slider", label.is_some(), labelled_by.is_some())?;
    Ok(SliderWidget {
        id: get_required_string_lua(table, "id")?,
        label,
        labelled_by,
        bind: slider_bind_from_lua(table, "bind")?
            .ok_or(DescriptorError::MissingField { field: "bind" })?,
        min: get_required_f32_lua(table, "min")?,
        max: get_required_f32_lua(table, "max")?,
        step: get_required_f32_lua(table, "step")?,
        captures_nav: string_array_from_lua(table, "capturesNav")?,
        focus_neighbors: focus_neighbors_from_lua(table)?,
        disabled: get_optional_bool_lua(table, "disabled")?.unwrap_or(false),
        visible_when: predicate_opt_from_lua(table, "visibleWhen")?,
        role: role_opt_from_lua(table)?,
    })
}

pub(crate) fn bar_widget_from_lua(table: &Table) -> Result<BarWidget, DescriptorError> {
    Ok(BarWidget {
        bind: slider_bind_from_lua(table, "bind")?
            .ok_or(DescriptorError::MissingField { field: "bind" })?,
        max: bar_max_from_lua(table)?,
        fill: color_value_from_lua(table, "fill")?,
        background: color_value_from_lua(table, "background")?,
        id: get_optional_string_lua(table, "id")?,
        style_ranges: style_ranges_from_lua(table)?,
        visible_when: predicate_opt_from_lua(table, "visibleWhen")?,
        role: role_opt_from_lua(table)?,
    })
}

/// Lua twin of [`announce_widget_from_js`]: required non-empty `text`, optional
/// `priority`. An empty `text` is a named load-time error (same condition as the
/// JS path).
pub(crate) fn announce_widget_from_lua(table: &Table) -> Result<AnnounceWidget, DescriptorError> {
    let text = get_required_string_lua(table, "text")?;
    if text.is_empty() {
        return Err(DescriptorError::InvalidShape {
            reason: "`announce.text` must be a non-empty string".to_string(),
        });
    }
    Ok(AnnounceWidget {
        text,
        priority: match get_optional_string_lua(table, "priority")? {
            Some(s) => parse_priority(&s)?,
            None => Priority::Polite,
        },
        visible_when: predicate_opt_from_lua(table, "visibleWhen")?,
    })
}

// --- Lua leaf-field readers -------------------------------------------------

/// Read `children` straight into a `Vec<Widget>`. An absent or empty `children`
/// table yields the empty vec — the `children: []` form — so an empty Luau
/// container parses cleanly (the empty-table → `{}` ambiguity of the generic
/// `lua_to_json` walker never enters this path).
pub(crate) fn children_from_lua(table: &Table) -> Result<Vec<Widget>, DescriptorError> {
    let raw: LuaValue = table.get("children").map_err(lua_err)?;
    let arr = match raw {
        LuaValue::Nil => return Ok(Vec::new()),
        LuaValue::Table(t) => t,
        other => {
            return Err(DescriptorError::InvalidShape {
                reason: format!(
                    "`children` must be an array of widgets, got {}",
                    other.type_name()
                ),
            });
        }
    };
    let len = arr.raw_len();
    let mut out = Vec::with_capacity(len);
    for i in 1..=(len as i64) {
        let item: LuaValue = arr.get(i).map_err(lua_err)?;
        out.push(widget_from_lua(item)?);
    }
    Ok(out)
}
