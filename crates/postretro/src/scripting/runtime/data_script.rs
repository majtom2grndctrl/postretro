// Per-level data-script execution: short-lived QuickJS/Luau contexts that
// produce a `LevelManifest`, plus the hot-reload health-range follow hook.
// See: context/lib/scripting.md §2 (Data context lifecycle)

use std::path::Path;

use postretro_level_format::data_script::DataScriptSection;
use rquickjs::{
    CatchResultExt, Context as JsContext, Function as JsFunction, Object as JsObject,
    Value as JsValue,
};

use crate::scripting::data_descriptors::LevelManifest;
use crate::scripting::error::ScriptError;
use crate::scripting::primitives_registry::ScriptPrimitive;
use crate::scripting::quickjs::{QuickJsSubsystem, run_script};
#[cfg(debug_assertions)]
use crate::scripting::refresh_plan::{DescriptorRefreshAction, DescriptorRefreshPlan};

use super::types::ScriptRuntime;

impl ScriptRuntime {
    /// Evaluate a level's data script in a short-lived VM context and return
    /// the resulting `LevelManifest`. Errors are logged and converted to an
    /// empty manifest — the level loads with an empty reaction registry
    /// (per-level reactions are absent) rather than failing. The engine-global
    /// entity-type registry, populated at mod-init from the mod manifest's
    /// `entities` field, is unaffected.
    ///
    /// `mod_root` is forwarded to the Luau VM so `require("./shared/loot")`
    /// inside data scripts resolves against the mod root, matching the
    /// mod-init VM's resolver wiring. For `.js` scripts, `mod_root` is not
    /// used — the QuickJS data context has no `require` resolver.
    ///
    /// The context is created and dropped within this call.
    /// See: context/lib/scripting.md §2 (Data context lifecycle)
    pub(crate) fn run_data_script(
        &self,
        section: &DataScriptSection,
        mod_root: &Path,
    ) -> LevelManifest {
        // Anything that isn't `.luau` runs through QuickJS, mirroring
        // `run_script_file`'s policy: prl-build emits `.js` from `.ts`, so the
        // on-disk extension is the only signal available at runtime.
        let is_luau = Path::new(&section.source_path)
            .extension()
            .and_then(|s| s.to_str())
            .map(|e| e.eq_ignore_ascii_case("luau"))
            .unwrap_or(false);

        let result = if is_luau {
            run_data_script_luau(
                self.luau.primitives(),
                &section.compiled_bytes,
                &section.source_path,
                mod_root,
            )
        } else {
            run_data_script_quickjs(&self.quickjs, &section.compiled_bytes, &section.source_path)
        };

        match result {
            Ok(manifest) => manifest,
            Err(err) => {
                log::warn!(
                    "[Scripting] data script failed for `{}`: {err}",
                    section.source_path,
                );
                LevelManifest::default()
            }
        }
    }
}

/// Re-attach the `player.health` slot range `[0, max]` after a descriptor
/// hot-reload, when (and only when) the refresh replaced the pawn's `Health`
/// component.
///
/// "The pawn" is the marked local player pawn, falling back to the first entity
/// carrying `PlayerMovement` for older fixtures/maps (entity_model.md).
/// The function inspects `plan` for a `Replace` action carrying a `Health`
/// component on that pawn's entity; if found, it reads the post-apply `max`
/// from the live component and re-sets the range unconditionally (idempotent —
/// no `max`-delta detection). A plan that did not touch the pawn's health (or a
/// world with no resolved pawn / no pawn health) leaves the range unchanged.
///
/// Factored as a standalone function taking only the pieces it needs — the
/// plan, a read-only registry, and a mutable slot table — so the range-follow
/// is unit-testable without the file watcher or a `ScriptCtx`.
#[cfg(debug_assertions)]
pub(super) fn follow_pawn_health_range_after_refresh(
    plan: &DescriptorRefreshPlan,
    registry: &crate::scripting::registry::EntityRegistry,
    slot_table: &mut crate::scripting::slot_table::SlotTable,
) {
    use crate::scripting::components::health::pawn_with_health;
    use crate::scripting::registry::ComponentValue;
    use crate::scripting::slot_table::NumericRange;

    let Some((pawn, health)) = pawn_with_health(registry) else {
        // No pawn or no pawn health: nothing to follow. The slot retains its
        // current range and value (slot-staleness contract).
        return;
    };

    let pawn_health_replaced = plan.actions.iter().any(|action| {
        matches!(
            action,
            DescriptorRefreshAction::Replace {
                entity,
                component: ComponentValue::Health(_),
            } if *entity == pawn
        )
    });
    if !pawn_health_replaced {
        return;
    }

    if let Err(err) = slot_table.set_engine_numeric_range(
        "player.health",
        NumericRange {
            min: 0.0,
            max: health.max,
        },
    ) {
        log::warn!("[Scripting] failed to follow player.health range on hot reload: {err}");
    }
}

// A short-lived data context is built fresh for each level. It uses the same
// primitive scope as the definition context.

fn run_data_script_quickjs(
    subsys: &QuickJsSubsystem,
    compiled_bytes: &[u8],
    source_path: &str,
) -> Result<LevelManifest, ScriptError> {
    let source = std::str::from_utf8(compiled_bytes).map_err(|e| ScriptError::InvalidArgument {
        reason: format!("data script `{source_path}` is not valid UTF-8: {e}"),
    })?;

    // Fresh context against the existing runtime — shares the GC heap and
    // memory limit with the long-lived contexts. Dropped at the end of this
    // function via RAII when `ctx` goes out of scope.
    let ctx = JsContext::full(subsys.runtime()).map_err(|e| ScriptError::InvalidArgument {
        reason: format!("failed to create data context: {e}"),
    })?;

    let primitives = subsys.primitives();

    let mut manifest_out: Result<LevelManifest, ScriptError> = Err(ScriptError::InvalidArgument {
        reason: "data script did not produce a manifest".to_string(),
    });

    ctx.with(|ctx| {
        for p in primitives {
            if let Err(e) = (p.quickjs_installer)(&ctx) {
                manifest_out = Err(ScriptError::InvalidArgument {
                    reason: format!("failed to install primitive `{}`: {e}", p.name),
                });
                return;
            }
        }

        if let Err(e) = crate::scripting::quickjs::evaluate_prelude(&ctx) {
            manifest_out = Err(e);
            return;
        }

        if let Err(e) = run_script::<()>(&ctx, source, source_path) {
            manifest_out = Err(e);
            return;
        }

        let globals = ctx.globals();
        let func: JsFunction = match globals.get("setupLevel") {
            Ok(f) => f,
            Err(e) => {
                manifest_out = Err(ScriptError::InvalidArgument {
                    reason: format!("data script `{source_path}` did not export `setupLevel`: {e}"),
                });
                return;
            }
        };

        // Pass an empty object as the context argument — descriptor-API
        // builders read no fields from it today; the parameter is reserved
        // for forward-compat (see scripting.md §2).
        let arg = match JsObject::new(ctx.clone()) {
            Ok(o) => o,
            Err(e) => {
                manifest_out = Err(ScriptError::InvalidArgument {
                    reason: format!("failed to allocate ctx argument: {e}"),
                });
                return;
            }
        };

        let returned: JsValue = match func.call((arg,)).catch(&ctx) {
            Ok(v) => v,
            Err(caught) => {
                let msg = caught.to_string();
                log::error!(
                    target: "script/quickjs",
                    "data script `{source_path}` setupLevel threw: {msg}",
                );
                manifest_out = Err(ScriptError::ScriptThrew {
                    msg,
                    source_name: source_path.to_string(),
                });
                return;
            }
        };

        match LevelManifest::from_js_value(&ctx, returned) {
            Ok(m) => manifest_out = Ok(m),
            Err(e) => {
                manifest_out = Err(ScriptError::InvalidArgument {
                    reason: e.to_string(),
                });
            }
        }
    });

    manifest_out
}

fn run_data_script_luau(
    primitives: &[ScriptPrimitive],
    compiled_bytes: &[u8],
    source_path: &str,
    mod_root: &Path,
) -> Result<LevelManifest, ScriptError> {
    let source = std::str::from_utf8(compiled_bytes).map_err(|e| ScriptError::InvalidArgument {
        reason: format!("data script `{source_path}` is not valid UTF-8: {e}"),
    })?;

    // Fresh `mlua::Lua`, dropped on return. Routed through `build_lua_state`
    // so the deny-list, print redirect, SDK prelude, primitives, and
    // mod-rooted `require` resolver match the mod-init VM. The archetype
    // sink is intentionally not installed here — data scripts don't drive
    // it. See: context/lib/scripting.md §2 (Luau `require` resolver)
    let lua = crate::scripting::luau::build_lua_state(primitives, None, Some(mod_root))?;

    // Mirror `LuauSubsystem::run_source`'s compile+load shape so traceback
    // formatting stays consistent.
    let bytecode = mlua::Compiler::new()
        .compile(source)
        .map_err(|e| ScriptError::ScriptThrew {
            msg: e.to_string(),
            source_name: source_path.to_string(),
        })?;
    lua.load(&bytecode)
        .set_name(source_path)
        .set_mode(mlua::ChunkMode::Binary)
        .exec()
        .map_err(|e| ScriptError::ScriptThrew {
            msg: e.to_string(),
            source_name: source_path.to_string(),
        })?;

    let func: mlua::Function =
        lua.globals()
            .get("setupLevel")
            .map_err(|e| ScriptError::InvalidArgument {
                reason: format!("data script `{source_path}` did not export `setupLevel`: {e}"),
            })?;

    let arg = lua
        .create_table()
        .map_err(|e| ScriptError::InvalidArgument {
            reason: format!("failed to allocate ctx argument: {e}"),
        })?;

    let returned: mlua::Value = func.call(arg).map_err(|e| ScriptError::ScriptThrew {
        msg: e.to_string(),
        source_name: source_path.to_string(),
    })?;

    LevelManifest::from_lua_value(returned).map_err(|e| ScriptError::InvalidArgument {
        reason: e.to_string(),
    })
}
