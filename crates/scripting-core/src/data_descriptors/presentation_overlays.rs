// `presentationOverlays` manifest drain for both runtimes. The field holds one
// overlay descriptor or an array of them, at most one per overlay kind.
// See: context/lib/ui.md §1 (presentation layer)

use super::*;

/// Drain `presentationOverlays`: one descriptor, or an array of descriptors.
/// A malformed array entry is skipped and a malformed single descriptor
/// ignores the field, so an optional combat visual cannot reject the whole
/// manifest. A repeated overlay kind keeps its first descriptor.
pub fn drain_presentation_overlays_js<'js>(
    ctx: &Ctx<'js>,
    obj: &Object<'js>,
    scope: &str,
) -> Result<Vec<PresentationOverlay>, DescriptorError> {
    if !obj.contains_key("presentationOverlays").map_err(js_err)? {
        return Ok(Vec::new());
    }
    let value: JsValue = obj.get("presentationOverlays").map_err(js_err)?;
    if value.is_null() || value.is_undefined() {
        return Ok(Vec::new());
    }
    let overlays = if let Some(array) = value.as_array() {
        let mut overlays = Vec::new();
        for index in 0..array.len() {
            let item: JsValue = array.get(index).map_err(js_err)?;
            keep_entry(
                &mut overlays,
                index,
                presentation_overlay_from_js(ctx, item),
                scope,
            );
        }
        overlays
    } else {
        single_descriptor(presentation_overlay_from_js(ctx, value), scope)
    };
    Ok(first_presentation_overlay_per_kind(overlays, scope))
}

/// Luau twin of [`drain_presentation_overlays_js`]. A table carrying `over` is
/// one descriptor; any other table is read as an array of descriptors.
pub fn drain_presentation_overlays_lua(
    table: &Table,
    scope: &str,
) -> Result<Vec<PresentationOverlay>, DescriptorError> {
    let value: LuaValue = table.get("presentationOverlays").map_err(lua_err)?;
    let array = match &value {
        LuaValue::Nil => return Ok(Vec::new()),
        LuaValue::Table(field) if !field.contains_key("over").map_err(lua_err)? => {
            let len = field.raw_len();
            (len > 0 || field.is_empty()).then(|| (field.clone(), len))
        }
        _ => None,
    };
    let overlays = match array {
        Some((array, len)) => {
            let mut overlays = Vec::new();
            for index in 0..len {
                let item: LuaValue = array.get(index as i64 + 1).map_err(lua_err)?;
                keep_entry(
                    &mut overlays,
                    index,
                    presentation_overlay_from_lua(item),
                    scope,
                );
            }
            overlays
        }
        None => single_descriptor(presentation_overlay_from_lua(value), scope),
    };
    Ok(first_presentation_overlay_per_kind(overlays, scope))
}

fn keep_entry(
    overlays: &mut Vec<PresentationOverlay>,
    index: usize,
    entry: Result<PresentationOverlay, DescriptorError>,
    scope: &str,
) {
    match entry {
        Ok(overlay) => overlays.push(overlay),
        Err(error) => log::warn!(
            "[Scripting] {scope}: `presentationOverlays[{index}]` is malformed and was skipped: {error}"
        ),
    }
}

fn single_descriptor(
    entry: Result<PresentationOverlay, DescriptorError>,
    scope: &str,
) -> Vec<PresentationOverlay> {
    match entry {
        Ok(overlay) => vec![overlay],
        Err(error) => {
            log::warn!(
                "[Scripting] {scope}: `presentationOverlays` must be an overlay descriptor or an array of them; the field was ignored: {error}"
            );
            Vec::new()
        }
    }
}
