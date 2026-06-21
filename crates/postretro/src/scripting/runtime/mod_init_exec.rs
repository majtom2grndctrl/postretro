// Short-lived VM execution for mod-init: evaluates the start-script and parses
// the returned `ModManifest` for QuickJS and Luau.
// See: context/lib/scripting.md §2 (Mod-init context lifecycle)

use std::path::Path;

use rquickjs::{Array as JsArray, Context as JsContext, Object as JsObject, Value as JsValue};

use crate::scripting::data_descriptors::{
    EntityTypeDescriptor, drain_fonts_js, drain_fonts_lua, drain_frontend_js, drain_frontend_lua,
    drain_global_crossings_js, drain_global_crossings_lua, drain_global_reactions_js,
    drain_global_reactions_lua, drain_maps_js, drain_maps_lua, drain_theme_js, drain_theme_lua,
    drain_ui_trees_js, drain_ui_trees_lua, entity_descriptor_from_js, entity_descriptor_from_lua,
};
use crate::scripting::error::ScriptError;
use crate::scripting::primitives::store::{
    drain_store_declarations_js, drain_store_declarations_lua,
};
use crate::scripting::primitives_registry::ScriptPrimitive;
use crate::scripting::quickjs::{QuickJsSubsystem, run_script};

use super::types::ModManifestResult;

pub(super) fn run_mod_init_quickjs(
    subsys: &QuickJsSubsystem,
    source: &str,
    source_path: &str,
) -> Result<ModManifestResult, ScriptError> {
    let ctx = JsContext::full(subsys.runtime()).map_err(|e| ScriptError::InvalidArgument {
        reason: format!("mod-init: failed to create context: {e}"),
    })?;

    let primitives = subsys.primitives();
    let mut out: Result<ModManifestResult, ScriptError> = Err(ScriptError::InvalidArgument {
        reason: "mod-init: default mod manifest export did not produce a manifest".to_string(),
    });

    ctx.with(|ctx| {
        for p in primitives {
            if let Err(e) = (p.quickjs_installer)(&ctx) {
                out = Err(ScriptError::InvalidArgument {
                    reason: format!("mod-init: failed to install primitive `{}`: {e}", p.name),
                });
                return;
            }
        }

        if let Err(e) = crate::scripting::quickjs::evaluate_prelude(&ctx) {
            out = Err(e);
            return;
        }

        let globals = ctx.globals();
        if let Err(e) = globals.remove("__postretroModManifest") {
            out = Err(ScriptError::InvalidArgument {
                reason: format!(
                    "mod-init: failed to clear default mod manifest export slot: {e}"
                ),
            });
            return;
        }

        if let Err(e) = run_script::<()>(&ctx, source, source_path) {
            out = Err(match e {
                ScriptError::ScriptThrew { msg, source_name } => ScriptError::ScriptThrew {
                    msg: format!("default mod manifest export initialization failed: {msg}"),
                    source_name,
                },
                other => other,
            });
            return;
        }

        match globals.contains_key("__postretroModManifest") {
            Ok(false) => {
                out = Err(ScriptError::InvalidArgument {
                    reason: format!(
                        "mod-init: `{source_path}` missing default mod manifest export"
                    ),
                });
                return;
            }
            Ok(true) => {}
            Err(e) => {
                out = Err(ScriptError::InvalidArgument {
                    reason: format!(
                        "mod-init: `{source_path}` default mod manifest export presence check failed: {e}"
                    ),
                });
                return;
            }
        }

        let manifest: JsValue = match globals.get::<_, JsValue>("__postretroModManifest") {
            Ok(value) => value,
            Err(e) => {
                out = Err(ScriptError::InvalidArgument {
                    reason: format!(
                        "mod-init: `{source_path}` default mod manifest export lookup failed: {e}"
                    ),
                });
                return;
            }
        };

        let obj = match JsObject::from_value(manifest) {
            Ok(o) => o,
            Err(_) => {
                out = Err(ScriptError::InvalidArgument {
                    reason: format!(
                        "mod-init: `{source_path}` default mod manifest export must be an object"
                    ),
                });
                return;
            }
        };

        let name: String = match obj.get("name") {
            Ok(s) => s,
            Err(e) => {
                out = Err(ScriptError::InvalidArgument {
                    reason: format!(
                        "mod-init: `{source_path}` default mod manifest export missing `name`: {e}"
                    ),
                });
                return;
            }
        };

        // Optional `entities` array. Missing key → empty Vec. Present-but-not-
        // array → InvalidArgument. Each element parses via the shared
        // descriptor reader (`entity_descriptor_from_js`).
        let entities: Vec<EntityTypeDescriptor> = match obj.contains_key("entities") {
            Ok(false) => Vec::new(),
            Ok(true) => match obj.get::<_, JsArray>("entities") {
                Ok(arr) => {
                    let mut parsed = Vec::with_capacity(arr.len());
                    let mut err: Option<ScriptError> = None;
                    for i in 0..arr.len() {
                        let v: JsValue = match arr.get(i) {
                            Ok(v) => v,
                            Err(e) => {
                                err = Some(ScriptError::InvalidArgument {
                                    reason: format!(
                                        "mod-init: `{source_path}` default mod manifest export `entities[{i}]` could not be read: {e}"
                                    ),
                                });
                                break;
                            }
                        };
                        match entity_descriptor_from_js(&ctx, v) {
                            Ok(d) => parsed.push(d),
                            Err(e) => {
                                err = Some(ScriptError::InvalidArgument {
                                    reason: format!(
                                        "mod-init: `{source_path}` default mod manifest export `entities[{i}]` invalid: {e}"
                                    ),
                                });
                                break;
                            }
                        }
                    }
                    if let Some(e) = err {
                        out = Err(e);
                        return;
                    }
                    parsed
                }
                Err(e) => {
                    out = Err(ScriptError::InvalidArgument {
                        reason: format!(
                            "mod-init: `{source_path}` default mod manifest export `entities` field must be an array: {e}"
                        ),
                    });
                    return;
                }
            },
            Err(e) => {
                out = Err(ScriptError::InvalidArgument {
                    reason: format!(
                        "mod-init: `{source_path}` default mod manifest export `entities` lookup failed: {e}"
                    ),
                });
                return;
            }
        };

        // UI fields drain via the G1a bridge fns. Malformed entries are logged
        // and skipped inside the drains — a bad UI field never aborts mod-init
        // (ui.md §1.1). A structurally broken read still surfaces as InvalidArgument.
        let ui_trees = match drain_ui_trees_js(&ctx, &obj, "default mod manifest export") {
            Ok(t) => t,
            Err(e) => {
                out = Err(ScriptError::InvalidArgument {
                    reason: format!("mod-init: `{source_path}` default mod manifest export `uiTrees` invalid: {e}"),
                });
                return;
            }
        };
        let theme = match drain_theme_js(&obj, "default mod manifest export") {
            Ok(t) => t,
            Err(e) => {
                out = Err(ScriptError::InvalidArgument {
                    reason: format!("mod-init: `{source_path}` default mod manifest export `theme` invalid: {e}"),
                });
                return;
            }
        };
        let frontend = match drain_frontend_js(&obj, "default mod manifest export") {
            Ok(frontend) => frontend,
            Err(e) => {
                out = Err(ScriptError::InvalidArgument {
                    reason: format!("mod-init: `{source_path}` default mod manifest export `frontend` invalid: {e}"),
                });
                return;
            }
        };
        let fonts = match drain_fonts_js(&obj, "default mod manifest export") {
            Ok(f) => f,
            Err(e) => {
                out = Err(ScriptError::InvalidArgument {
                    reason: format!("mod-init: `{source_path}` default mod manifest export `fonts` invalid: {e}"),
                });
                return;
            }
        };
        let maps = match drain_maps_js(&obj, "default mod manifest export") {
            Ok(m) => m,
            Err(e) => {
                out = Err(ScriptError::InvalidArgument {
                    reason: format!("mod-init: `{source_path}` default mod manifest export `maps` invalid: {e}"),
                });
                return;
            }
        };
        let reactions = match drain_global_reactions_js(&ctx, &obj, "default mod manifest export") {
            Ok(r) => r,
            Err(e) => {
                out = Err(ScriptError::InvalidArgument {
                    reason: format!("mod-init: `{source_path}` default mod manifest export `reactions` invalid: {e}"),
                });
                return;
            }
        };
        let crossings = match drain_global_crossings_js(&obj, "default mod manifest export") {
            Ok(c) => c,
            Err(e) => {
                out = Err(ScriptError::InvalidArgument {
                    reason: format!("mod-init: `{source_path}` default mod manifest export `crossings` invalid: {e}"),
                });
                return;
            }
        };
        let store_declarations = match drain_store_declarations_js(&ctx, &obj) {
            Ok(stores) => stores,
            Err(e) => {
                out = Err(ScriptError::InvalidArgument {
                    reason: format!("mod-init: `{source_path}` default mod manifest export `stores` invalid: {e}"),
                });
                return;
            }
        };

        out = Ok(ModManifestResult {
            name,
            entities,
            ui_trees,
            theme,
            frontend,
            fonts,
            maps,
            reactions,
            crossings,
            store_declarations,
        });
    });

    out
}

pub(super) fn run_mod_init_luau(
    primitives: &[ScriptPrimitive],
    source: &str,
    source_path: &str,
    mod_root: &Path,
) -> Result<ModManifestResult, ScriptError> {
    // The mod-init Luau VM gets a working `require` resolver rooted at the
    // mod root so start-script can pull in domain scripts.
    let lua = crate::scripting::luau::build_lua_state(primitives, None, Some(mod_root))?;

    let bytecode = mlua::Compiler::new()
        .compile(source)
        .map_err(|e| ScriptError::ScriptThrew {
            msg: e.to_string(),
            source_name: source_path.to_string(),
        })?;
    let returned = lua
        .load(&bytecode)
        .set_name(source_path)
        .set_mode(mlua::ChunkMode::Binary)
        .eval::<mlua::Value>()
        .map_err(|e| ScriptError::ScriptThrew {
            msg: format!("returned mod manifest initialization failed: {e}"),
            source_name: source_path.to_string(),
        })?;

    let table = match returned {
        mlua::Value::Table(t) => t,
        mlua::Value::Nil => {
            return Err(ScriptError::InvalidArgument {
                reason: format!("mod-init: `{source_path}` missing returned mod manifest"),
            });
        }
        other => {
            return Err(ScriptError::InvalidArgument {
                reason: format!(
                    "mod-init: `{source_path}` returned mod manifest must be a table, got {}",
                    other.type_name()
                ),
            });
        }
    };

    let name: String = table
        .get("name")
        .map_err(|e| ScriptError::InvalidArgument {
            reason: format!("mod-init: `{source_path}` returned mod manifest missing `name`: {e}"),
        })?;

    // Optional `entities` array. Missing key → empty Vec. Present-but-not-table
    // → InvalidArgument. Each element parses via the shared descriptor reader
    // (`entity_descriptor_from_lua`).
    let entities: Vec<EntityTypeDescriptor> = if table.contains_key("entities").map_err(|e| {
        ScriptError::InvalidArgument {
            reason: format!(
                "mod-init: `{source_path}` returned mod manifest `entities` lookup failed: {e}"
            ),
        }
    })? {
        let raw: mlua::Value = table
            .get("entities")
            .map_err(|e| ScriptError::InvalidArgument {
                reason: format!(
                    "mod-init: `{source_path}` returned mod manifest `entities` field could not be read: {e}"
                ),
            })?;
        match raw {
            mlua::Value::Nil => Vec::new(),
            mlua::Value::Table(arr) => {
                let len = arr.raw_len();
                let mut out = Vec::with_capacity(len);
                for i in 1..=(len as i64) {
                    let item: mlua::Value =
                        arr.get(i).map_err(|e| ScriptError::InvalidArgument {
                            reason: format!(
                                "mod-init: `{source_path}` returned mod manifest `entities[{i}]` could not be read: {e}"
                            ),
                        })?;
                    let descriptor = entity_descriptor_from_lua(item).map_err(|e| {
                        ScriptError::InvalidArgument {
                            reason: format!(
                                "mod-init: `{source_path}` returned mod manifest `entities[{i}]` invalid: {e}"
                            ),
                        }
                    })?;
                    out.push(descriptor);
                }
                out
            }
            other => {
                return Err(ScriptError::InvalidArgument {
                    reason: format!(
                        "mod-init: `{source_path}` returned mod manifest `entities` field must be an array, got {}",
                        other.type_name()
                    ),
                });
            }
        }
    } else {
        Vec::new()
    };

    // UI fields drain via the G1a bridge fns; malformed entries log+skip inside
    // the drains (ui.md §1.1). Errors here are structural read failures only.
    let ui_trees = drain_ui_trees_lua(&table, "returned mod manifest").map_err(|e| {
        ScriptError::InvalidArgument {
            reason: format!(
                "mod-init: `{source_path}` returned mod manifest `uiTrees` invalid: {e}"
            ),
        }
    })?;
    let theme = drain_theme_lua(&table, "returned mod manifest").map_err(|e| {
        ScriptError::InvalidArgument {
            reason: format!("mod-init: `{source_path}` returned mod manifest `theme` invalid: {e}"),
        }
    })?;
    let frontend = drain_frontend_lua(&table, "returned mod manifest").map_err(|e| {
        ScriptError::InvalidArgument {
            reason: format!(
                "mod-init: `{source_path}` returned mod manifest `frontend` invalid: {e}"
            ),
        }
    })?;
    let fonts = drain_fonts_lua(&table, "returned mod manifest").map_err(|e| {
        ScriptError::InvalidArgument {
            reason: format!("mod-init: `{source_path}` returned mod manifest `fonts` invalid: {e}"),
        }
    })?;
    let maps = drain_maps_lua(&table, "returned mod manifest").map_err(|e| {
        ScriptError::InvalidArgument {
            reason: format!("mod-init: `{source_path}` returned mod manifest `maps` invalid: {e}"),
        }
    })?;
    let reactions = drain_global_reactions_lua(&table, "returned mod manifest").map_err(|e| {
        ScriptError::InvalidArgument {
            reason: format!(
                "mod-init: `{source_path}` returned mod manifest `reactions` invalid: {e}"
            ),
        }
    })?;
    let crossings = drain_global_crossings_lua(&table, "returned mod manifest").map_err(|e| {
        ScriptError::InvalidArgument {
            reason: format!(
                "mod-init: `{source_path}` returned mod manifest `crossings` invalid: {e}"
            ),
        }
    })?;
    let store_declarations =
        drain_store_declarations_lua(&table).map_err(|e| ScriptError::InvalidArgument {
            reason: format!(
                "mod-init: `{source_path}` returned mod manifest `stores` invalid: {e}"
            ),
        })?;

    Ok(ModManifestResult {
        name,
        entities,
        ui_trees,
        theme,
        frontend,
        fonts,
        maps,
        reactions,
        crossings,
        store_declarations,
    })
}
