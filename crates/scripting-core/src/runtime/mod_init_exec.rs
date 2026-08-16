// Short-lived VM execution for mod-init: evaluates the start-script and parses
// the returned `ModManifest` for QuickJS and Luau.
// See: context/lib/scripting.md §2 (Mod-init context lifecycle)

use std::path::Path;

use rquickjs::{Array as JsArray, Context as JsContext, Object as JsObject, Value as JsValue};

use crate::data_descriptors::{
    EntityTypeDescriptor, drain_fonts_js, drain_fonts_lua, drain_frontend_js, drain_frontend_lua,
    drain_global_crossings_js, drain_global_crossings_lua, drain_global_reactions_js,
    drain_global_reactions_lua, drain_impact_events_js, drain_impact_events_lua, drain_maps_js,
    drain_maps_lua, drain_mover_defaults_js, drain_mover_defaults_lua,
    drain_presentation_overlays_js, drain_presentation_overlays_lua,
    drain_presentation_templates_js, drain_presentation_templates_lua, drain_render_profile_js,
    drain_render_profile_lua, drain_switching_js, drain_switching_lua, drain_theme_js,
    drain_theme_lua, drain_trigger_events_js, drain_trigger_events_lua, drain_trigger_pools_js,
    drain_trigger_pools_lua, drain_ui_trees_js, drain_ui_trees_lua, entity_descriptor_from_js,
    entity_descriptor_from_lua,
};
use crate::error::ScriptError;
use crate::primitives_registry::ScriptPrimitive;
use crate::quickjs::{QuickJsSubsystem, run_script};
use crate::store_bridge::{drain_store_declarations_js, drain_store_declarations_lua};

use super::types::{ModManifestResult, validate_mod_manifest_id, validate_mod_manifest_version};

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

        if let Err(e) = crate::quickjs::evaluate_prelude(&ctx) {
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
        let id: String = match obj.get("id") {
            Ok(value) => value,
            Err(error) => {
                out = Err(ScriptError::InvalidArgument {
                    reason: format!(
                        "mod-init: `{source_path}` default mod manifest export missing `id`: {error}"
                    ),
                });
                return;
            }
        };
        if let Err(error) = validate_mod_manifest_id(&id) {
            out = Err(ScriptError::InvalidArgument {
                reason: format!(
                    "mod-init: `{source_path}` default mod manifest export `id` invalid: {error}"
                ),
            });
            return;
        }
        let version: String = match obj.get("version") {
            Ok(value) => value,
            Err(error) => {
                out = Err(ScriptError::InvalidArgument {
                    reason: format!(
                        "mod-init: `{source_path}` default mod manifest export missing `version`: {error}"
                    ),
                });
                return;
            }
        };
        if let Err(error) = validate_mod_manifest_version(&version) {
            out = Err(ScriptError::InvalidArgument {
                reason: format!(
                    "mod-init: `{source_path}` default mod manifest export `version` invalid: {error}"
                ),
            });
            return;
        }

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
        let presentation_templates = match drain_presentation_templates_js(
            &ctx,
            &obj,
            "default mod manifest export",
        ) {
            Ok(templates) => templates,
            Err(e) => {
                out = Err(ScriptError::InvalidArgument {
                    reason: format!(
                        "mod-init: `{source_path}` default mod manifest export `presentationTemplates` invalid: {e}"
                    ),
                });
                return;
            }
        };
        let presentation_overlays = match drain_presentation_overlays_js(
            &ctx,
            &obj,
            "default mod manifest export",
        ) {
            Ok(overlays) => overlays,
            Err(e) => {
                out = Err(ScriptError::InvalidArgument {
                    reason: format!(
                        "mod-init: `{source_path}` default mod manifest export `presentationOverlays` invalid: {e}"
                    ),
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
        let render = match drain_render_profile_js(&obj, "default mod manifest export") {
            Ok(profile) => profile,
            Err(e) => {
                out = Err(ScriptError::InvalidArgument {
                    reason: format!(
                        "mod-init: `{source_path}` default mod manifest export `render` invalid: {e}"
                    ),
                });
                return;
            }
        };
        let movers = match drain_mover_defaults_js(&obj, "default mod manifest export") {
            Ok(defaults) => defaults,
            Err(e) => {
                out = Err(ScriptError::InvalidArgument {
                    reason: format!(
                        "mod-init: `{source_path}` default mod manifest export `movers` invalid: {e}"
                    ),
                });
                return;
            }
        };
        let switching = match drain_switching_js(&obj, "default mod manifest export") {
            Ok(switching) => switching,
            Err(e) => {
                out = Err(ScriptError::InvalidArgument {
                    reason: format!(
                        "mod-init: `{source_path}` default mod manifest export `switching` invalid: {e}"
                    ),
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
        let crossings = match drain_global_crossings_js(&ctx, &obj, "default mod manifest export") {
            Ok(c) => c,
            Err(e) => {
                out = Err(ScriptError::InvalidArgument {
                    reason: format!("mod-init: `{source_path}` default mod manifest export `crossings` invalid: {e}"),
                });
                return;
            }
        };
        let events = match drain_impact_events_js(&ctx, &obj, "default mod manifest export") {
            Ok(events) => events,
            Err(e) => {
                out = Err(ScriptError::InvalidArgument {
                    reason: format!(
                        "mod-init: `{source_path}` default mod manifest export `events` invalid: {e}"
                    ),
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
        let trigger_events = match drain_trigger_events_js(&obj, "default mod manifest export") {
            Ok(v) => v,
            Err(e) => {
                out = Err(ScriptError::InvalidArgument { reason: format!("mod-init: `{source_path}` triggerEvents invalid: {e}") });
                return;
            }
        };
        let trigger_pools = match drain_trigger_pools_js(&obj, "default mod manifest export") {
            Ok(v) => v,
            Err(e) => {
                out = Err(ScriptError::InvalidArgument { reason: format!("mod-init: `{source_path}` triggerPools invalid: {e}") });
                return;
            }
        };

        out = Ok(ModManifestResult {
            name,
            id,
            version,
            render,
            movers,
            switching,
            entities,
            ui_trees,
            presentation_templates,
            presentation_overlays,
            theme,
            frontend,
            fonts,
            maps,
            reactions,
            crossings,
            events,
            trigger_events,
            trigger_pools,
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
    let lua = crate::luau::build_lua_state(primitives, None, Some(mod_root))?;

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
    let id: String = table
        .get("id")
        .map_err(|error| ScriptError::InvalidArgument {
            reason: format!(
                "mod-init: `{source_path}` returned mod manifest missing `id`: {error}"
            ),
        })?;
    validate_mod_manifest_id(&id).map_err(|error| ScriptError::InvalidArgument {
        reason: format!("mod-init: `{source_path}` returned mod manifest `id` invalid: {error}"),
    })?;
    let version: String = table
        .get("version")
        .map_err(|error| ScriptError::InvalidArgument {
            reason: format!(
                "mod-init: `{source_path}` returned mod manifest missing `version`: {error}"
            ),
        })?;
    validate_mod_manifest_version(&version).map_err(|error| ScriptError::InvalidArgument {
        reason: format!(
            "mod-init: `{source_path}` returned mod manifest `version` invalid: {error}"
        ),
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
    let presentation_templates =
        drain_presentation_templates_lua(&table, "returned mod manifest").map_err(|e| {
            ScriptError::InvalidArgument {
                reason: format!(
                    "mod-init: `{source_path}` returned mod manifest `presentationTemplates` invalid: {e}"
                ),
            }
        })?;
    let presentation_overlays =
        drain_presentation_overlays_lua(&table, "returned mod manifest").map_err(|e| {
            ScriptError::InvalidArgument {
                reason: format!(
                    "mod-init: `{source_path}` returned mod manifest `presentationOverlays` invalid: {e}"
                ),
            }
        })?;
    let theme = drain_theme_lua(&table, "returned mod manifest").map_err(|e| {
        ScriptError::InvalidArgument {
            reason: format!("mod-init: `{source_path}` returned mod manifest `theme` invalid: {e}"),
        }
    })?;
    let render = drain_render_profile_lua(&table, "returned mod manifest").map_err(|e| {
        ScriptError::InvalidArgument {
            reason: format!(
                "mod-init: `{source_path}` returned mod manifest `render` invalid: {e}"
            ),
        }
    })?;
    let movers = drain_mover_defaults_lua(&table, "returned mod manifest").map_err(|e| {
        ScriptError::InvalidArgument {
            reason: format!(
                "mod-init: `{source_path}` returned mod manifest `movers` invalid: {e}"
            ),
        }
    })?;
    let switching = drain_switching_lua(&table, "returned mod manifest").map_err(|e| {
        ScriptError::InvalidArgument {
            reason: format!(
                "mod-init: `{source_path}` returned mod manifest `switching` invalid: {e}"
            ),
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
    let events = drain_impact_events_lua(&table, "returned mod manifest").map_err(|e| {
        ScriptError::InvalidArgument {
            reason: format!(
                "mod-init: `{source_path}` returned mod manifest `events` invalid: {e}"
            ),
        }
    })?;
    let store_declarations =
        drain_store_declarations_lua(&table).map_err(|e| ScriptError::InvalidArgument {
            reason: format!(
                "mod-init: `{source_path}` returned mod manifest `stores` invalid: {e}"
            ),
        })?;
    let trigger_events =
        drain_trigger_events_lua(&table, "returned mod manifest").map_err(|e| {
            ScriptError::InvalidArgument {
                reason: format!("mod-init: `{source_path}` returned triggerEvents invalid: {e}"),
            }
        })?;
    let trigger_pools = drain_trigger_pools_lua(&table, "returned mod manifest").map_err(|e| {
        ScriptError::InvalidArgument {
            reason: format!("mod-init: `{source_path}` returned triggerPools invalid: {e}"),
        }
    })?;

    Ok(ModManifestResult {
        name,
        id,
        version,
        render,
        movers,
        switching,
        entities,
        ui_trees,
        presentation_templates,
        presentation_overlays,
        theme,
        frontend,
        fonts,
        maps,
        reactions,
        crossings,
        events,
        trigger_events,
        trigger_pools,
        store_declarations,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data_descriptors::{SwitchingDescriptor, TriggerPoolArm};
    use crate::primitives_registry::PrimitiveRegistry;
    use crate::runtime::{ModBloomProfile, ModBloomResolution, ModRenderProfile};

    fn cold_render_profiles(
        js_render: &str,
        luau_render: &str,
    ) -> (ModRenderProfile, ModRenderProfile) {
        let registry = PrimitiveRegistry::new();
        let quickjs = QuickJsSubsystem::new(&registry, &crate::quickjs::QuickJsConfig::default())
            .expect("QuickJS subsystem should initialize");
        let js_source = format!(
            "globalThis.__postretroModManifest = {{ name: 'RenderMod', id: 'render-mod', version: 'any display value', render: {js_render} }};"
        );
        let luau_source = format!(
            "return {{ name = 'RenderMod', id = 'render-mod', version = 'any display value', render = {luau_render} }}"
        );

        let js = run_mod_init_quickjs(&quickjs, &js_source, "render-mod.js")
            .expect("optional QuickJS render profile must not reject the manifest");
        let luau = run_mod_init_luau(&[], &luau_source, "render-mod.luau", Path::new("."))
            .expect("optional Luau render profile must not reject the manifest");
        (js.render, luau.render)
    }

    #[test]
    fn mod_init_render_profile_matches_in_both_runtimes() {
        let (js, luau) = cold_render_profiles(
            "{ bloom: { resolution: 'quarter', pixelated: true } }",
            "{ bloom = { resolution = 'quarter', pixelated = true } }",
        );
        let expected = ModRenderProfile {
            bloom: ModBloomProfile {
                resolution: ModBloomResolution::Quarter,
                pixelated: true,
            },
        };
        assert_eq!(js, expected);
        assert_eq!(luau, expected);
    }

    #[test]
    fn mod_init_render_profile_malformed_fields_degrade_equally() {
        let cases = [
            ("7", "7", ModRenderProfile::default()),
            ("{ bloom: 7 }", "{ bloom = 7 }", ModRenderProfile::default()),
            (
                "{ bloom: { resolution: 'third', pixelated: true } }",
                "{ bloom = { resolution = 'third', pixelated = true } }",
                ModRenderProfile {
                    bloom: ModBloomProfile {
                        resolution: ModBloomResolution::Half,
                        pixelated: true,
                    },
                },
            ),
            (
                "{ bloom: { resolution: 'eighth', pixelated: 'yes' } }",
                "{ bloom = { resolution = 'eighth', pixelated = 'yes' } }",
                ModRenderProfile {
                    bloom: ModBloomProfile {
                        resolution: ModBloomResolution::Eighth,
                        pixelated: false,
                    },
                },
            ),
        ];

        for (js_source, luau_source, expected) in cases {
            let (js, luau) = cold_render_profiles(js_source, luau_source);
            assert_eq!(js, expected);
            assert_eq!(luau, expected);
        }
    }

    #[test]
    fn mod_init_mover_defaults_match_in_both_runtimes_and_degrade_to_off() {
        let registry = PrimitiveRegistry::new();
        let quickjs = QuickJsSubsystem::new(&registry, &crate::quickjs::QuickJsConfig::default())
            .expect("QuickJS subsystem should initialize");
        let js = run_mod_init_quickjs(
            &quickjs,
            "globalThis.__postretroModManifest = { name: 'Mover', id: 'mover', version: '1', movers: { autoCloseMs: 250 } };",
            "mover.js",
        )
        .expect("QuickJS mover defaults should parse");
        let luau = run_mod_init_luau(
            &[],
            "return { name = 'Mover', id = 'mover', version = '1', movers = { autoCloseMs = 250 } }",
            "mover.luau",
            Path::new("."),
        )
        .expect("Luau mover defaults should parse");
        assert_eq!(js.movers.auto_close_ms, 250.0);
        assert_eq!(luau.movers, js.movers);

        let malformed_js = run_mod_init_quickjs(
            &quickjs,
            "globalThis.__postretroModManifest = { name: 'Mover', id: 'mover', version: '1', movers: { autoCloseMs: -1 } };",
            "mover-malformed.js",
        )
        .expect("malformed optional QuickJS mover defaults should degrade");
        let malformed_luau = run_mod_init_luau(
            &[],
            "return { name = 'Mover', id = 'mover', version = '1', movers = { autoCloseMs = -1 } }",
            "mover-malformed.luau",
            Path::new("."),
        )
        .expect("malformed optional Luau mover defaults should degrade");
        assert_eq!(malformed_js.movers.auto_close_ms, 0.0);
        assert_eq!(malformed_luau.movers, malformed_js.movers);
    }

    #[test]
    fn mod_init_switching_matches_in_both_runtimes_and_omits_to_defaults() {
        let registry = PrimitiveRegistry::new();
        let quickjs = QuickJsSubsystem::new(&registry, &crate::quickjs::QuickJsConfig::default())
            .expect("QuickJS subsystem should initialize");
        let js = run_mod_init_quickjs(
            &quickjs,
            "globalThis.__postretroModManifest = { name: 'Switch', id: 'switch', version: '1', switching: { commitOnDirectSelect: false, cycleCommitDwellMs: 125, blockDuringReload: true } };",
            "switch.js",
        )
        .expect("QuickJS switching manifest should parse");
        let luau = run_mod_init_luau(
            &[],
            "return { name = 'Switch', id = 'switch', version = '1', switching = { commitOnDirectSelect = false, cycleCommitDwellMs = 125, blockDuringReload = true } }",
            "switch.luau",
            Path::new("."),
        )
        .expect("Luau switching manifest should parse");
        let expected = SwitchingDescriptor {
            commit_on_direct_select: false,
            cycle_commit_dwell_ms: 125.0,
            block_during_reload: true,
        };
        assert_eq!(js.switching, expected);
        assert_eq!(luau.switching, expected);

        let js_default = run_mod_init_quickjs(
            &quickjs,
            "globalThis.__postretroModManifest = { name: 'Default', id: 'default', version: '1' };",
            "default.js",
        )
        .expect("absent QuickJS switching block should default");
        let luau_default = run_mod_init_luau(
            &[],
            "return { name = 'Default', id = 'default', version = '1' }",
            "default.luau",
            Path::new("."),
        )
        .expect("absent Luau switching block should default");
        assert_eq!(js_default.switching, SwitchingDescriptor::default());
        assert_eq!(luau_default.switching, SwitchingDescriptor::default());
    }

    #[test]
    fn mod_init_switching_rejects_invalid_fields_in_both_runtimes() {
        let registry = PrimitiveRegistry::new();
        let quickjs = QuickJsSubsystem::new(&registry, &crate::quickjs::QuickJsConfig::default())
            .expect("QuickJS subsystem should initialize");
        for (label, js_switching, luau_switching, field) in [
            (
                "missing required boolean",
                "{ cycleCommitDwellMs: 10, blockDuringReload: true }",
                "{ cycleCommitDwellMs = 10, blockDuringReload = true }",
                "commitOnDirectSelect",
            ),
            (
                "negative dwell",
                "{ commitOnDirectSelect: true, cycleCommitDwellMs: -1, blockDuringReload: false }",
                "{ commitOnDirectSelect = true, cycleCommitDwellMs = -1, blockDuringReload = false }",
                "cycleCommitDwellMs",
            ),
            (
                "nonfinite dwell",
                "{ commitOnDirectSelect: true, cycleCommitDwellMs: NaN, blockDuringReload: false }",
                "{ commitOnDirectSelect = true, cycleCommitDwellMs = 0/0, blockDuringReload = false }",
                "cycleCommitDwellMs",
            ),
        ] {
            let js_source = format!(
                "globalThis.__postretroModManifest = {{ name: 'Bad', id: 'bad', version: '1', switching: {js_switching} }};"
            );
            let js_error = run_mod_init_quickjs(&quickjs, &js_source, "bad.js")
                .expect_err("invalid QuickJS switching field must abort mod init");
            let luau_source = format!(
                "return {{ name = 'Bad', id = 'bad', version = '1', switching = {luau_switching} }}"
            );
            let luau_error = run_mod_init_luau(&[], &luau_source, "bad.luau", Path::new("."))
                .expect_err("invalid Luau switching field must abort mod init");
            assert!(
                js_error.to_string().contains(field),
                "{label}: QuickJS diagnostic must name {field}: {js_error}"
            );
            assert!(
                luau_error.to_string().contains(field),
                "{label}: Luau diagnostic must name {field}: {luau_error}"
            );
        }
    }

    #[test]
    fn mod_init_manifest_identity_accepts_bare_id_and_arbitrary_nonempty_version_in_both_runtimes()
    {
        let registry = PrimitiveRegistry::new();
        let quickjs = QuickJsSubsystem::new(&registry, &crate::quickjs::QuickJsConfig::default())
            .expect("QuickJS subsystem should initialize");
        let js = run_mod_init_quickjs(
            &quickjs,
            "globalThis.__postretroModManifest = { name: 'Dev', id: 'dev', version: 'build 4 / not semver' };",
            "identity.js",
        )
        .expect("bare id and arbitrary non-empty version must parse in QuickJS");
        let luau = run_mod_init_luau(
            &[],
            "return { name = 'Dev', id = 'dev', version = 'build 4 / not semver' }",
            "identity.luau",
            Path::new("."),
        )
        .expect("bare id and arbitrary non-empty version must parse in Luau");

        assert_eq!(
            (js.id.as_str(), js.version.as_str()),
            ("dev", "build 4 / not semver")
        );
        assert_eq!(
            (luau.id.as_str(), luau.version.as_str()),
            ("dev", "build 4 / not semver")
        );
    }

    #[test]
    fn mod_init_manifest_identity_rejects_missing_or_invalid_fields_in_both_runtimes() {
        let registry = PrimitiveRegistry::new();
        let quickjs = QuickJsSubsystem::new(&registry, &crate::quickjs::QuickJsConfig::default())
            .expect("QuickJS subsystem should initialize");

        for (label, js_source, luau_source, expected_field) in [
            (
                "missing id",
                "globalThis.__postretroModManifest = { name: 'Dev', version: '1' };".to_string(),
                "return { name = 'Dev', version = '1' }".to_string(),
                "missing `id`",
            ),
            (
                "missing version",
                "globalThis.__postretroModManifest = { name: 'Dev', id: 'dev' };".to_string(),
                "return { name = 'Dev', id = 'dev' }".to_string(),
                "missing `version`",
            ),
            (
                "empty id",
                "globalThis.__postretroModManifest = { name: 'Dev', id: '', version: '1' };"
                    .to_string(),
                "return { name = 'Dev', id = '', version = '1' }".to_string(),
                "`id` invalid",
            ),
            (
                "65-byte id",
                format!(
                    "globalThis.__postretroModManifest = {{ name: 'Dev', id: '{}', version: '1' }};",
                    "a".repeat(65)
                ),
                format!(
                    "return {{ name = 'Dev', id = '{}', version = '1' }}",
                    "a".repeat(65)
                ),
                "`id` invalid",
            ),
            (
                "non-ascii id",
                "globalThis.__postretroModManifest = { name: 'Dev', id: 'déf', version: '1' };"
                    .to_string(),
                "return { name = 'Dev', id = 'déf', version = '1' }".to_string(),
                "`id` invalid",
            ),
            (
                "disallowed id character",
                "globalThis.__postretroModManifest = { name: 'Dev', id: 'bad/id', version: '1' };"
                    .to_string(),
                "return { name = 'Dev', id = 'bad/id', version = '1' }".to_string(),
                "`id` invalid",
            ),
            (
                "colon id",
                "globalThis.__postretroModManifest = { name: 'Dev', id: 'mod:dev', version: '1' };"
                    .to_string(),
                "return { name = 'Dev', id = 'mod:dev', version = '1' }".to_string(),
                "`id` invalid",
            ),
            (
                "single-dot id",
                "globalThis.__postretroModManifest = { name: 'Dev', id: '.', version: '1' };"
                    .to_string(),
                "return { name = 'Dev', id = '.', version = '1' }".to_string(),
                "`id` invalid",
            ),
            (
                "double-dot id",
                "globalThis.__postretroModManifest = { name: 'Dev', id: '..', version: '1' };"
                    .to_string(),
                "return { name = 'Dev', id = '..', version = '1' }".to_string(),
                "`id` invalid",
            ),
            (
                "three-dot id",
                "globalThis.__postretroModManifest = { name: 'Dev', id: '...', version: '1' };"
                    .to_string(),
                "return { name = 'Dev', id = '...', version = '1' }".to_string(),
                "`id` invalid",
            ),
            (
                "empty version",
                "globalThis.__postretroModManifest = { name: 'Dev', id: 'dev', version: '' };"
                    .to_string(),
                "return { name = 'Dev', id = 'dev', version = '' }".to_string(),
                "`version` invalid",
            ),
        ] {
            let js_error = run_mod_init_quickjs(&quickjs, &js_source, "identity.js")
                .expect_err("invalid QuickJS identity must reject");
            assert!(
                matches!(&js_error, ScriptError::InvalidArgument { .. })
                    && js_error.to_string().contains("identity.js")
                    && js_error.to_string().contains(expected_field),
                "{label} QuickJS error must name source and field: {js_error}"
            );
            let luau_error = run_mod_init_luau(&[], &luau_source, "identity.luau", Path::new("."))
                .expect_err("invalid Luau identity must reject");
            assert!(
                matches!(&luau_error, ScriptError::InvalidArgument { .. })
                    && luau_error.to_string().contains("identity.luau")
                    && luau_error.to_string().contains(expected_field),
                "{label} Luau error must name source and field: {luau_error}"
            );
        }
    }

    #[test]
    fn mod_init_trigger_pools_skip_malformed_entries_in_both_runtimes() {
        let registry = PrimitiveRegistry::new();
        let quickjs = QuickJsSubsystem::new(&registry, &crate::quickjs::QuickJsConfig::default())
            .expect("QuickJS subsystem should initialize");
        let js_manifest = run_mod_init_quickjs(
            &quickjs,
            r#"
                globalThis.__postretroModManifest = {
                    name: "PoolMod",
                    id: "pool-mod",
                    version: "1",
                    triggerPools: [
                        { tag: "valid_pool", arm: 2, levels: ["campaign"] },
                        { tag: "invalid_pool", arm: -1 },
                    ],
                };
            "#,
            "pool-mod.js",
        )
        .expect("malformed pool entry should not abort QuickJS mod init");
        let luau_manifest = run_mod_init_luau(
            &[],
            r#"
                return {
                    name = "PoolMod",
                    id = "pool-mod",
                    version = "1",
                    triggerPools = {
                        { tag = "valid_pool", arm = 2, levels = { "campaign" } },
                        { tag = "invalid_pool", arm = -1 },
                    },
                }
            "#,
            "pool-mod.luau",
            Path::new("."),
        )
        .expect("malformed pool entry should not abort Luau mod init");

        assert_eq!(js_manifest.render, ModRenderProfile::default());
        assert_eq!(luau_manifest.render, ModRenderProfile::default());
        assert_eq!(js_manifest.trigger_pools, luau_manifest.trigger_pools);
        assert_eq!(js_manifest.trigger_pools.len(), 1);
        let pool = &js_manifest.trigger_pools[0];
        assert_eq!(pool.tag, "valid_pool");
        assert_eq!(pool.arm, TriggerPoolArm::Count(2));
        assert_eq!(pool.levels, ["campaign"]);
    }

    #[test]
    fn mod_init_preserves_impact_event_descriptors_in_both_runtimes() {
        let registry = PrimitiveRegistry::new();
        let quickjs = QuickJsSubsystem::new(&registry, &crate::quickjs::QuickJsConfig::default())
            .expect("QuickJS subsystem should initialize");
        let js_manifest = run_mod_init_quickjs(
            &quickjs,
            r#"
                globalThis.__postretroModManifest = {
                    name: "Impact Mod",
                    id: "impact-mod",
                    version: "1",
                    events: [{
                        kind: "impact",
                        id: "crate-break",
                        isOverride: false,
                        levels: ["campaign"],
                        filter: { tag: "crate" },
                        policy: [{
                            primitive: "setState",
                            target: "@impact.target",
                            args: { name: "hits", value: { op: "input", name: "@state.hits" } },
                        }],
                    }],
                };
            "#,
            "impact-mod.js",
        )
        .expect("QuickJS impact event manifest should parse");
        let luau_manifest = run_mod_init_luau(
            &[],
            r#"
                return {
                    name = "Impact Mod",
                    id = "impact-mod",
                    version = "1",
                    events = {{
                        kind = "impact",
                        id = "crate-break",
                        isOverride = false,
                        levels = { "campaign" },
                        filter = { tag = "crate" },
                        policy = {{
                            primitive = "setState",
                            target = "@impact.target",
                            args = { name = "hits", value = { op = "input", name = "@state.hits" } },
                        }},
                    }},
                }
            "#,
            "impact-mod.luau",
            Path::new("."),
        )
        .expect("Luau impact event manifest should parse");

        assert_eq!(js_manifest.events, luau_manifest.events);
        assert_eq!(js_manifest.events.len(), 1);
        let event = &js_manifest.events[0];
        assert_eq!(event.id, "crate-break");
        assert!(!event.is_override);
        assert_eq!(event.levels, ["campaign"]);
        assert_eq!(event.filter_tag.as_deref(), Some("crate"));
        assert_eq!(
            event.policy,
            vec![serde_json::json!({
                "primitive": "setState",
                "target": "@impact.target",
                "args": {
                    "name": "hits",
                    "value": { "op": "input", "name": "@state.hits" },
                },
            })]
        );
    }
}
