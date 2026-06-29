// Data-context descriptors: JS manifest drains (uiTrees/theme/fonts/maps).
// See: context/lib/scripting.md

use super::super::*;

impl LevelManifest {
    /// Deserialize a top-level `{ reactions, crossings }` object returned from
    /// a QuickJS `setupLevel()` call. `crossings` is optional.
    pub fn from_js_value<'js>(
        ctx: &Ctx<'js>,
        value: JsValue<'js>,
    ) -> Result<Self, DescriptorError> {
        let obj = Object::from_value(value).map_err(|_| DescriptorError::InvalidShape {
            reason: "setupLevel must return an object".to_string(),
        })?;

        let reactions = if obj.contains_key("reactions").map_err(js_err)? {
            let arr: Array = obj.get("reactions").map_err(js_err)?;
            let mut out = Vec::with_capacity(arr.len());
            for i in 0..arr.len() {
                let item: JsValue = arr.get(i).map_err(js_err)?;
                out.push(named_reaction_from_js(ctx, item)?);
            }
            out
        } else {
            Vec::new()
        };

        let crossings = if obj.contains_key("crossings").map_err(js_err)? {
            let arr: Array = obj.get("crossings").map_err(js_err)?;
            let mut out = Vec::with_capacity(arr.len());
            for i in 0..arr.len() {
                let item: JsValue = arr.get(i).map_err(js_err)?;
                out.push(crossing_descriptor_from_js(&item)?);
            }
            out
        } else {
            Vec::new()
        };

        let ui_trees = drain_ui_trees_js(ctx, &obj, "setupLevel")?;

        Ok(Self {
            reactions,
            crossings,
            ui_trees,
        })
    }
}

// ===========================================================================
// Manifest-level UI field drains for ModManifest/setupLevel().
//
// `uiTrees` / `theme` / `fonts` are optional fields on the mod manifest
// (mod scope); `uiTrees` is also optional on `setupLevel()` (level scope). Each
// drain reads the field straight off the returned object/table via the per-
// runtime field readers, building typed values held on the manifest result.
//
// Degradation contract (ui.md §1.1): a malformed UI *registration* (a tree entry
// that fails its own parse, including the `anchored_tree_from_*` bridge)
// produces a named load-time diagnostic and is SKIPPED — it never aborts the
// boot / level-load pass and never panics. A malformed *container* (the
// `uiTrees`/`theme`/`fonts` field itself not being the expected shape) is also
// logged and degraded to "no UI from this field" rather than failing the parse,
// for the same reason: a bad UI field must not take down mod-init.
// ===========================================================================

/// Drain the `uiTrees` array from a QuickJS manifest object. `scope` is a short
/// label ("ModManifest" / "setupLevel") used in diagnostics. Malformed entries are
/// logged and skipped; a non-array `uiTrees` field is logged and yields empty.
pub fn drain_ui_trees_js<'js>(
    ctx: &Ctx<'js>,
    obj: &Object<'js>,
    scope: &str,
) -> Result<Vec<RegisteredUiTree>, DescriptorError> {
    if !obj.contains_key("uiTrees").map_err(js_err)? {
        return Ok(Vec::new());
    }
    let raw: JsValue = obj.get("uiTrees").map_err(js_err)?;
    if raw.is_null() || raw.is_undefined() {
        return Ok(Vec::new());
    }
    let Some(arr) = raw.as_array() else {
        log::warn!(
            "[Scripting] {scope}: `uiTrees` must be an array of registered trees; ignoring the field"
        );
        return Ok(Vec::new());
    };
    let mut out = Vec::with_capacity(arr.len());
    for i in 0..arr.len() {
        let item: JsValue = arr.get(i).map_err(js_err)?;
        match registered_ui_tree_from_js(ctx, item) {
            Ok(tree) => out.push(tree),
            Err(e) => {
                log::warn!("[Scripting] {scope}: `uiTrees[{i}]` is malformed and was skipped: {e}")
            }
        }
    }
    Ok(out)
}

/// Parse a single registered-tree entry (`{ name, tree, alwaysOn? }`) from JS.
/// The `tree` field is converted via the G1a `anchored_tree_from_js_value`
/// bridge. Returns a named [`DescriptorError`] (never panics) on malformed input.
pub fn registered_ui_tree_from_js<'js>(
    ctx: &Ctx<'js>,
    value: JsValue<'js>,
) -> Result<RegisteredUiTree, DescriptorError> {
    let obj = Object::from_value(value).map_err(|_| DescriptorError::InvalidShape {
        reason: "registered UI tree must be an object".to_string(),
    })?;
    let name = get_required_string_js(&obj, "name")?;
    if !obj.contains_key("tree").map_err(js_err)? {
        return Err(DescriptorError::MissingField { field: "tree" });
    }
    let tree_val: JsValue = obj.get("tree").map_err(js_err)?;
    let tree = anchored_tree_from_js_value(ctx, tree_val)?;
    let always_on = get_optional_bool_js(&obj, "alwaysOn")?.unwrap_or(false);
    Ok(RegisteredUiTree {
        name,
        tree,
        always_on,
    })
}

/// Drain the optional `theme` token maps from a QuickJS manifest object. A
/// malformed `theme` field is logged and degraded to default (empty) tokens.
pub fn drain_theme_js<'js>(
    obj: &Object<'js>,
    scope: &str,
) -> Result<ModThemeTokens, DescriptorError> {
    if !obj.contains_key("theme").map_err(js_err)? {
        return Ok(ModThemeTokens::default());
    }
    let raw: JsValue = obj.get("theme").map_err(js_err)?;
    if raw.is_null() || raw.is_undefined() {
        return Ok(ModThemeTokens::default());
    }
    let Ok(theme_obj) = Object::from_value(raw) else {
        log::warn!("[Scripting] {scope}: `theme` must be an object; ignoring the field");
        return Ok(ModThemeTokens::default());
    };
    let colors = match theme_obj.contains_key("colors").map_err(js_err)? {
        true => f32_array4_map_from_js(&theme_obj, "colors")?,
        false => HashMap::new(),
    };
    let fonts = match theme_obj.contains_key("fonts").map_err(js_err)? {
        true => string_map_from_js(&theme_obj, "fonts")?,
        false => HashMap::new(),
    };
    let spacing = match theme_obj.contains_key("spacing").map_err(js_err)? {
        true => f32_map_from_js(&theme_obj, "spacing")?,
        false => HashMap::new(),
    };
    Ok(ModThemeTokens {
        colors,
        fonts,
        spacing,
    })
}

/// Drain the optional mod frontend declaration from a QuickJS manifest object.
/// Missing/null normalizes to `None`; a present malformed object is fatal so a
/// bad frontend cannot partially replace the committed app-side snapshot.
pub fn drain_frontend_js<'js>(
    obj: &Object<'js>,
    _scope: &str,
) -> Result<Option<Frontend>, DescriptorError> {
    if !obj.contains_key("frontend").map_err(js_err)? {
        return Ok(None);
    }
    let raw: JsValue = obj.get("frontend").map_err(js_err)?;
    if raw.is_null() || raw.is_undefined() {
        return Ok(None);
    }
    let frontend_obj = Object::from_value(raw).map_err(|_| DescriptorError::InvalidShape {
        reason: "`frontend` must be an object".to_string(),
    })?;
    Ok(Some(frontend_from_js(&frontend_obj)?))
}

pub fn frontend_from_js<'js>(obj: &Object<'js>) -> Result<Frontend, DescriptorError> {
    let menu_tree = get_required_string_js(obj, "menuTree")?;
    let background_level = get_optional_string_js(obj, "backgroundLevel")?;
    let camera = menu_camera_from_js(obj)?;
    Ok(Frontend {
        menu_tree,
        background_level,
        camera,
    })
}

pub fn menu_camera_from_js<'js>(obj: &Object<'js>) -> Result<MenuCamera, DescriptorError> {
    if !obj.contains_key("camera").map_err(js_err)? {
        return Err(DescriptorError::MissingField { field: "camera" });
    }
    let raw: JsValue = obj.get("camera").map_err(js_err)?;
    if raw.is_null() || raw.is_undefined() {
        return Err(DescriptorError::MissingField { field: "camera" });
    }
    let camera_obj = Object::from_value(raw).map_err(|_| DescriptorError::InvalidShape {
        reason: "`frontend.camera` must be an object".to_string(),
    })?;
    if !camera_obj.contains_key("position").map_err(js_err)? {
        return Err(DescriptorError::MissingField { field: "position" });
    }
    let raw_position: JsValue = camera_obj.get("position").map_err(js_err)?;
    let position = validate_finite_array3(
        read_f32_array_n_js::<3>(&raw_position, "frontend.camera.position")?,
        "frontend.camera.position",
    )?;
    Ok(MenuCamera {
        position,
        yaw: validate_finite_f32(
            get_required_f32_js(&camera_obj, "yaw")?,
            "frontend.camera.yaw",
        )?,
        pitch: validate_finite_f32(
            get_required_f32_js(&camera_obj, "pitch")?,
            "frontend.camera.pitch",
        )?,
    })
}

/// Drain the optional `fonts` (family → TTF path) map from a QuickJS manifest
/// object. A malformed `fonts` field is logged and degraded to empty.
pub fn drain_fonts_js<'js>(
    obj: &Object<'js>,
    scope: &str,
) -> Result<ModFontAssets, DescriptorError> {
    if !obj.contains_key("fonts").map_err(js_err)? {
        return Ok(ModFontAssets::default());
    }
    let raw: JsValue = obj.get("fonts").map_err(js_err)?;
    if raw.is_null() || raw.is_undefined() {
        return Ok(ModFontAssets::default());
    }
    if raw.as_object().is_none() {
        log::warn!("[Scripting] {scope}: `fonts` must be a family→path object; ignoring the field");
        return Ok(ModFontAssets::default());
    }
    Ok(ModFontAssets {
        families: string_map_from_js(obj, "fonts")?,
    })
}

/// Drain the optional mod map catalog from a QuickJS manifest object. Malformed
/// entries, duplicate ids, entries with empty ids, and entries with invalid
/// paths are logged and skipped.
pub fn drain_maps_js<'js>(
    obj: &Object<'js>,
    scope: &str,
) -> Result<Vec<ModMapEntry>, DescriptorError> {
    if !obj.contains_key("maps").map_err(js_err)? {
        return Ok(Vec::new());
    }
    let raw: JsValue = obj.get("maps").map_err(js_err)?;
    if raw.is_null() || raw.is_undefined() {
        return Ok(Vec::new());
    }
    let Some(arr) = raw.as_array() else {
        log::warn!(
            "[Scripting] {scope}: `maps` must be an array of map catalog entries; ignoring the field"
        );
        return Ok(Vec::new());
    };

    let mut out = Vec::with_capacity(arr.len());
    let mut seen_ids = BTreeSet::new();
    for i in 0..arr.len() {
        let item: JsValue = arr.get(i).map_err(js_err)?;
        match mod_map_entry_from_js(item) {
            Ok(entry) => push_valid_map_entry(entry, &mut seen_ids, &mut out, scope, i),
            Err(e) => {
                log::warn!("[Scripting] {scope}: `maps[{i}]` is malformed and was skipped: {e}")
            }
        }
    }
    Ok(out)
}

/// Drain mod-global reaction definitions from a QuickJS manifest object.
/// Missing/null `reactions` normalizes to empty; present entries use the same
/// descriptor parser as level-local reactions plus an optional `levels` scope.
pub fn drain_global_reactions_js<'js>(
    ctx: &Ctx<'js>,
    obj: &Object<'js>,
    scope: &str,
) -> Result<Vec<ScopedReaction>, DescriptorError> {
    let Some(arr) = optional_manifest_array_js(obj, "reactions", scope)? else {
        return Ok(Vec::new());
    };

    let mut out = Vec::with_capacity(arr.len());
    for i in 0..arr.len() {
        let item: JsValue = arr.get(i).map_err(js_err)?;
        let item_obj =
            Object::from_value(item.clone()).map_err(|_| DescriptorError::InvalidShape {
                reason: "reaction entry must be an object".to_string(),
            })?;
        out.push(ScopedReaction {
            reaction: named_reaction_from_js(ctx, item)?,
            levels: string_array_from_js(&item_obj, "levels")?,
        });
    }
    Ok(out)
}

/// Drain mod-global crossing definitions from a QuickJS manifest object.
/// Missing/null `crossings` normalizes to empty; present entries use the same
/// descriptor parser as level-local crossings plus an optional `levels` scope.
pub fn drain_global_crossings_js<'js>(
    obj: &Object<'js>,
    scope: &str,
) -> Result<Vec<ScopedCrossing>, DescriptorError> {
    let Some(arr) = optional_manifest_array_js(obj, "crossings", scope)? else {
        return Ok(Vec::new());
    };

    let mut out = Vec::with_capacity(arr.len());
    for i in 0..arr.len() {
        let item: JsValue = arr.get(i).map_err(js_err)?;
        let item_obj =
            Object::from_value(item.clone()).map_err(|_| DescriptorError::InvalidShape {
                reason: "crossing entry must be an object".to_string(),
            })?;
        out.push(ScopedCrossing {
            crossing: crossing_descriptor_from_js(&item)?,
            levels: string_array_from_js(&item_obj, "levels")?,
        });
    }
    Ok(out)
}

pub fn optional_manifest_array_js<'js>(
    obj: &Object<'js>,
    field: &'static str,
    scope: &str,
) -> Result<Option<Array<'js>>, DescriptorError> {
    if !obj.contains_key(field).map_err(js_err)? {
        return Ok(None);
    }
    let raw: JsValue = obj.get(field).map_err(js_err)?;
    if raw.is_null() || raw.is_undefined() {
        return Ok(None);
    }
    let Some(arr) = raw.as_array() else {
        return Err(DescriptorError::InvalidShape {
            reason: format!("{scope}: `{field}` must be an array"),
        });
    };
    Ok(Some(arr.clone()))
}

pub fn mod_map_entry_from_js<'js>(value: JsValue<'js>) -> Result<ModMapEntry, DescriptorError> {
    let obj = Object::from_value(value).map_err(|_| DescriptorError::InvalidShape {
        reason: "map catalog entry must be an object".to_string(),
    })?;
    Ok(ModMapEntry {
        id: get_required_string_js(&obj, "id")?,
        path: get_required_string_js(&obj, "path")?,
        name: get_required_string_js(&obj, "name")?,
        tags: string_array_from_js(&obj, "tags")?,
    })
}

pub fn push_valid_map_entry(
    entry: ModMapEntry,
    seen_ids: &mut BTreeSet<String>,
    out: &mut Vec<ModMapEntry>,
    scope: &str,
    index: usize,
) {
    if entry.id.is_empty() {
        log::warn!("[Scripting] {scope}: `maps[{index}]` has an empty `id` and was skipped");
        return;
    }
    if entry.path.is_empty() {
        log::warn!("[Scripting] {scope}: `maps[{index}]` has an empty `path` and was skipped");
        return;
    }
    if !is_catalog_path_relative_to_content_root(&entry.path) {
        log::warn!(
            "[Scripting] {scope}: `maps[{index}]` path `{}` escapes the content root and was skipped",
            entry.path,
        );
        return;
    }
    if !seen_ids.insert(entry.id.clone()) {
        log::warn!(
            "[Scripting] {scope}: duplicate map catalog id `{}` at `maps[{index}]`; keeping the first entry",
            entry.id,
        );
        return;
    }
    out.push(entry);
}

/// Read an object-valued field as a `String → String` map. Absent/non-object →
/// empty (with a `log::warn!` when present but not an object). Malformed tokens
/// are logged and skipped (per-token degraded) so a single bad entry does not
/// abort the whole theme drain — mirrors the Luau twin.
pub fn string_map_from_js<'js>(
    obj: &Object<'js>,
    field: &'static str,
) -> Result<HashMap<String, String>, DescriptorError> {
    let raw: JsValue = obj.get(field).map_err(js_err)?;
    let map = match Object::from_value(raw) {
        Ok(o) => o,
        Err(_) => {
            log::warn!("[Scripting] theme `{field}` must be an object; skipping field");
            return Ok(HashMap::new());
        }
    };
    let mut out = HashMap::new();
    for entry in map.props::<String, JsValue>() {
        let (key, value) = entry.map_err(js_err)?;
        match String::from_js_value_required(value, field) {
            Ok(s) => {
                out.insert(key, s);
            }
            Err(e) => {
                log::warn!("[Scripting] theme `{field}.{key}` is malformed and was skipped: {e}");
            }
        }
    }
    Ok(out)
}

/// Read an object-valued field as a `String → f32` map. Absent/non-object →
/// empty (with a `log::warn!` when present but not an object). Malformed tokens
/// are logged and skipped (per-token degraded) so a single bad entry does not
/// abort the whole theme drain — mirrors the Luau twin.
pub fn f32_map_from_js<'js>(
    obj: &Object<'js>,
    field: &'static str,
) -> Result<HashMap<String, f32>, DescriptorError> {
    let raw: JsValue = obj.get(field).map_err(js_err)?;
    let map = match Object::from_value(raw) {
        Ok(o) => o,
        Err(_) => {
            log::warn!("[Scripting] theme `{field}` must be an object; skipping field");
            return Ok(HashMap::new());
        }
    };
    let mut out = HashMap::new();
    for entry in map.props::<String, JsValue>() {
        let (key, value) = entry.map_err(js_err)?;
        match js_value_as_f32(&value, field) {
            Ok(f) => {
                out.insert(key, f);
            }
            Err(e) => {
                log::warn!("[Scripting] theme `{field}.{key}` is malformed and was skipped: {e}");
            }
        }
    }
    Ok(out)
}

/// Read an object-valued field as a `String → [f32; 4]` map (linear-RGBA color
/// tokens). Absent/non-object → empty (with a `log::warn!` when present but not
/// an object). Malformed tokens are logged and skipped (per-token degraded) so
/// a single bad entry does not abort the whole theme drain — mirrors the Luau twin.
pub fn f32_array4_map_from_js<'js>(
    obj: &Object<'js>,
    field: &'static str,
) -> Result<HashMap<String, [f32; 4]>, DescriptorError> {
    let raw: JsValue = obj.get(field).map_err(js_err)?;
    let map = match Object::from_value(raw) {
        Ok(o) => o,
        Err(_) => {
            log::warn!("[Scripting] theme `{field}` must be an object; skipping field");
            return Ok(HashMap::new());
        }
    };
    let mut out = HashMap::new();
    for entry in map.props::<String, JsValue>() {
        let (key, value) = entry.map_err(js_err)?;
        match read_f32_array_n_js::<4>(&value, field) {
            Ok(arr) => {
                out.insert(key, arr);
            }
            Err(e) => {
                log::warn!("[Scripting] theme `{field}.{key}` is malformed and was skipped: {e}");
            }
        }
    }
    Ok(out)
}
