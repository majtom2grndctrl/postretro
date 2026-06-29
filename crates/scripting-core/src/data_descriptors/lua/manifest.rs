// Data-context descriptors: Lua manifest drains (uiTrees/theme/fonts/maps).
// See: context/lib/scripting.md

use super::super::*;

impl LevelManifest {
    /// Deserialize a top-level `{ reactions, crossings }` table returned from a
    /// Luau `setupLevel()` call. `crossings` is optional.
    pub fn from_lua_value(value: LuaValue) -> Result<Self, DescriptorError> {
        let table = match value {
            LuaValue::Table(t) => t,
            other => {
                return Err(DescriptorError::InvalidShape {
                    reason: format!("setupLevel must return a table, got {}", other.type_name()),
                });
            }
        };

        let reactions = if table.contains_key("reactions").map_err(lua_err)? {
            let arr: Table = table.get("reactions").map_err(lua_err)?;
            let len = validate_dense_lua_array(&arr, "`reactions` field")?;
            let mut out = Vec::with_capacity(len);
            for i in 1..=(len as i64) {
                let item: LuaValue = arr.get(i).map_err(lua_err)?;
                out.push(named_reaction_from_lua(item)?);
            }
            out
        } else {
            Vec::new()
        };

        let crossings = if table.contains_key("crossings").map_err(lua_err)? {
            let arr: Table = table.get("crossings").map_err(lua_err)?;
            let len = validate_dense_lua_array(&arr, "`crossings` field")?;
            let mut out = Vec::with_capacity(len);
            for i in 1..=(len as i64) {
                let item: LuaValue = arr.get(i).map_err(lua_err)?;
                out.push(crossing_descriptor_from_lua(item)?);
            }
            out
        } else {
            Vec::new()
        };

        let ui_trees = drain_ui_trees_lua(&table, "setupLevel")?;

        Ok(Self {
            reactions,
            crossings,
            ui_trees,
        })
    }
}

/// Drain the `uiTrees` array from a Luau manifest table. Mirrors
/// [`drain_ui_trees_js`]: malformed entries are logged and skipped, a non-table
/// `uiTrees` field is logged and yields empty.
pub fn drain_ui_trees_lua(
    table: &Table,
    scope: &str,
) -> Result<Vec<RegisteredUiTree>, DescriptorError> {
    let raw: LuaValue = table.get("uiTrees").map_err(lua_err)?;
    let arr = match raw {
        LuaValue::Nil => return Ok(Vec::new()),
        LuaValue::Table(t) => t,
        _ => {
            log::warn!(
                "[Scripting] {scope}: `uiTrees` must be an array of registered trees; ignoring the field"
            );
            return Ok(Vec::new());
        }
    };
    let len = dense_lua_prefix_len(&arr, "uiTrees", scope)?;
    let mut out = Vec::with_capacity(len);
    for i in 1..=(len as i64) {
        let item: LuaValue = arr.get(i).map_err(lua_err)?;
        match registered_ui_tree_from_lua(item) {
            Ok(tree) => out.push(tree),
            Err(e) => {
                log::warn!("[Scripting] {scope}: `uiTrees[{i}]` is malformed and was skipped: {e}")
            }
        }
    }
    log_lua_array_extras(&arr, len, "uiTrees", scope)?;
    Ok(out)
}

/// Parse a single registered-tree entry (`{ name, tree, alwaysOn? }`) from Luau.
/// The `tree` field is converted via the G1a `anchored_tree_from_lua_value`
/// bridge. Returns a named [`DescriptorError`] (never panics) on malformed input.
pub fn registered_ui_tree_from_lua(value: LuaValue) -> Result<RegisteredUiTree, DescriptorError> {
    let table = lua_table(value, "registered UI tree")?;
    let name = get_required_string_lua(&table, "name")?;
    let tree_val: LuaValue = table.get("tree").map_err(lua_err)?;
    if matches!(tree_val, LuaValue::Nil) {
        return Err(DescriptorError::MissingField { field: "tree" });
    }
    let tree = anchored_tree_from_lua_value(tree_val)?;
    let always_on = get_optional_bool_lua(&table, "alwaysOn")?.unwrap_or(false);
    Ok(RegisteredUiTree {
        name,
        tree,
        always_on,
    })
}

/// Drain the optional `theme` token maps from a Luau manifest table. A malformed
/// `theme` field is logged and degraded to default (empty) tokens.
pub fn drain_theme_lua(table: &Table, scope: &str) -> Result<ModThemeTokens, DescriptorError> {
    let raw: LuaValue = table.get("theme").map_err(lua_err)?;
    let theme_table = match raw {
        LuaValue::Nil => return Ok(ModThemeTokens::default()),
        LuaValue::Table(t) => t,
        _ => {
            log::warn!("[Scripting] {scope}: `theme` must be a table; ignoring the field");
            return Ok(ModThemeTokens::default());
        }
    };
    Ok(ModThemeTokens {
        colors: f32_array4_map_from_lua(&theme_table, "colors")?,
        fonts: string_map_from_lua(&theme_table, "fonts")?,
        spacing: f32_map_from_lua(&theme_table, "spacing")?,
    })
}

/// Drain the optional mod frontend declaration from a Luau manifest table.
/// Mirrors [`drain_frontend_js`].
pub fn drain_frontend_lua(
    table: &Table,
    _scope: &str,
) -> Result<Option<Frontend>, DescriptorError> {
    let raw: LuaValue = table.get("frontend").map_err(lua_err)?;
    let frontend_table = match raw {
        LuaValue::Nil => return Ok(None),
        LuaValue::Table(t) => t,
        other => {
            return Err(DescriptorError::InvalidShape {
                reason: format!("`frontend` must be a table, got {}", other.type_name()),
            });
        }
    };
    Ok(Some(frontend_from_lua(&frontend_table)?))
}

pub fn frontend_from_lua(table: &Table) -> Result<Frontend, DescriptorError> {
    let menu_tree = get_required_string_lua(table, "menuTree")?;
    let background_level = get_optional_string_lua(table, "backgroundLevel")?;
    let camera = menu_camera_from_lua(table)?;
    Ok(Frontend {
        menu_tree,
        background_level,
        camera,
    })
}

pub fn menu_camera_from_lua(table: &Table) -> Result<MenuCamera, DescriptorError> {
    let raw: LuaValue = table.get("camera").map_err(lua_err)?;
    let camera_table = match raw {
        LuaValue::Nil => return Err(DescriptorError::MissingField { field: "camera" }),
        LuaValue::Table(t) => t,
        other => {
            return Err(DescriptorError::InvalidShape {
                reason: format!(
                    "`frontend.camera` must be a table, got {}",
                    other.type_name()
                ),
            });
        }
    };
    let raw_position: LuaValue = camera_table.get("position").map_err(lua_err)?;
    if matches!(raw_position, LuaValue::Nil) {
        return Err(DescriptorError::MissingField { field: "position" });
    }
    let position = validate_finite_array3(
        read_f32_array_n_lua::<3>(raw_position, "frontend.camera.position")?,
        "frontend.camera.position",
    )?;
    Ok(MenuCamera {
        position,
        yaw: validate_finite_f32(
            get_required_f32_lua(&camera_table, "yaw")?,
            "frontend.camera.yaw",
        )?,
        pitch: validate_finite_f32(
            get_required_f32_lua(&camera_table, "pitch")?,
            "frontend.camera.pitch",
        )?,
    })
}

/// Drain the optional `fonts` (family → TTF path) map from a Luau manifest
/// table. A malformed `fonts` field is logged and degraded to empty.
pub fn drain_fonts_lua(table: &Table, scope: &str) -> Result<ModFontAssets, DescriptorError> {
    let raw: LuaValue = table.get("fonts").map_err(lua_err)?;
    match raw {
        LuaValue::Nil => Ok(ModFontAssets::default()),
        LuaValue::Table(_) => Ok(ModFontAssets {
            families: string_map_from_lua(table, "fonts")?,
        }),
        _ => {
            log::warn!(
                "[Scripting] {scope}: `fonts` must be a family→path table; ignoring the field"
            );
            Ok(ModFontAssets::default())
        }
    }
}

/// Drain the optional mod map catalog from a Luau manifest table. Mirrors
/// [`drain_maps_js`].
pub fn drain_maps_lua(table: &Table, scope: &str) -> Result<Vec<ModMapEntry>, DescriptorError> {
    let raw: LuaValue = table.get("maps").map_err(lua_err)?;
    let arr = match raw {
        LuaValue::Nil => return Ok(Vec::new()),
        LuaValue::Table(t) => t,
        _ => {
            log::warn!(
                "[Scripting] {scope}: `maps` must be an array of map catalog entries; ignoring the field"
            );
            return Ok(Vec::new());
        }
    };

    let len = dense_lua_prefix_len(&arr, "maps", scope)?;
    let mut out = Vec::with_capacity(len);
    let mut seen_ids = BTreeSet::new();
    for i in 1..=(len as i64) {
        let item: LuaValue = arr.get(i).map_err(lua_err)?;
        match mod_map_entry_from_lua(item) {
            Ok(entry) => push_valid_map_entry(entry, &mut seen_ids, &mut out, scope, i as usize),
            Err(e) => {
                log::warn!("[Scripting] {scope}: `maps[{i}]` is malformed and was skipped: {e}")
            }
        }
    }
    log_lua_array_extras(&arr, len, "maps", scope)?;
    Ok(out)
}

/// Drain mod-global reaction definitions from a Luau manifest table. Mirrors
/// [`drain_global_reactions_js`].
pub fn drain_global_reactions_lua(
    table: &Table,
    scope: &str,
) -> Result<Vec<ScopedReaction>, DescriptorError> {
    let Some(arr) = optional_manifest_array_lua(table, "reactions", scope)? else {
        return Ok(Vec::new());
    };
    let len = validate_dense_lua_array(&arr, "`reactions` field")?;
    let mut out = Vec::with_capacity(len);
    for i in 1..=(len as i64) {
        let item: LuaValue = arr.get(i).map_err(lua_err)?;
        let item_table = lua_table(item.clone(), "reaction entry")?;
        out.push(ScopedReaction {
            reaction: named_reaction_from_lua(item)?,
            levels: string_array_from_lua(&item_table, "levels")?,
        });
    }
    Ok(out)
}

/// Drain mod-global crossing definitions from a Luau manifest table. Mirrors
/// [`drain_global_crossings_js`].
pub fn drain_global_crossings_lua(
    table: &Table,
    scope: &str,
) -> Result<Vec<ScopedCrossing>, DescriptorError> {
    let Some(arr) = optional_manifest_array_lua(table, "crossings", scope)? else {
        return Ok(Vec::new());
    };
    let len = validate_dense_lua_array(&arr, "`crossings` field")?;
    let mut out = Vec::with_capacity(len);
    for i in 1..=(len as i64) {
        let item: LuaValue = arr.get(i).map_err(lua_err)?;
        let item_table = lua_table(item.clone(), "crossing entry")?;
        out.push(ScopedCrossing {
            crossing: crossing_descriptor_from_lua(item)?,
            levels: string_array_from_lua(&item_table, "levels")?,
        });
    }
    Ok(out)
}

pub fn optional_manifest_array_lua(
    table: &Table,
    field: &'static str,
    scope: &str,
) -> Result<Option<Table>, DescriptorError> {
    let raw: LuaValue = table.get(field).map_err(lua_err)?;
    match raw {
        LuaValue::Nil => Ok(None),
        LuaValue::Table(t) => Ok(Some(t)),
        other => Err(DescriptorError::InvalidShape {
            reason: format!(
                "{scope}: `{field}` must be an array, got {}",
                other.type_name()
            ),
        }),
    }
}

pub fn mod_map_entry_from_lua(value: LuaValue) -> Result<ModMapEntry, DescriptorError> {
    let table = lua_table(value, "map catalog entry")?;
    Ok(ModMapEntry {
        id: get_required_string_lua(&table, "id")?,
        path: get_required_string_lua(&table, "path")?,
        name: get_required_string_lua(&table, "name")?,
        tags: string_array_from_lua(&table, "tags")?,
    })
}

fn dense_lua_prefix_len(
    arr: &Table,
    field: &'static str,
    scope: &str,
) -> Result<usize, DescriptorError> {
    let mut indices = BTreeSet::new();
    for pair in arr.clone().pairs::<LuaValue, LuaValue>() {
        let (key, _) = pair.map_err(lua_err)?;
        if let LuaValue::Integer(index) = key {
            if index >= 1 {
                indices.insert(index);
            }
        }
    }

    let mut len = 0usize;
    while indices.contains(&((len + 1) as i64)) {
        len += 1;
    }

    if len == 0 && !indices.is_empty() {
        log::warn!(
            "[Scripting] {scope}: `{field}` has no dense prefix; non-prefix entries were skipped"
        );
    }

    Ok(len)
}

fn log_lua_array_extras(
    arr: &Table,
    prefix_len: usize,
    field: &'static str,
    scope: &str,
) -> Result<(), DescriptorError> {
    for pair in arr.clone().pairs::<LuaValue, LuaValue>() {
        let (key, _) = pair.map_err(lua_err)?;
        let in_prefix =
            matches!(&key, LuaValue::Integer(index) if *index >= 1 && *index <= prefix_len as i64);
        if !in_prefix {
            log::warn!(
                "[Scripting] {scope}: `{field}` entry with {} key was skipped because `{field}` must be a dense array",
                key.type_name()
            );
        }
    }
    Ok(())
}
