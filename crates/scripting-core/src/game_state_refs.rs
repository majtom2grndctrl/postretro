// Runtime installation for the generated engine-owned state reference tree.
// The tree is built from the shared catalog, exposed through a temporary bridge
// before the SDK prelude, then captured by SDK-side `getGameState()`.
// See: context/lib/scripting.md §5

use rquickjs::{Ctx, Function as JsFunction, Object as JsObject, Value as JsValue};

use super::engine_state_catalog::{
    EngineStateCatalog, EngineStateCatalogEntry, EngineStateCatalogError, EngineStateTreeNode,
    EngineStateValueType, engine_state_catalog,
};
use super::error::ScriptError;

pub const GAME_STATE_BRIDGE_GLOBAL: &str = "__postretroGameStateRefs";
pub const GET_GAME_STATE_GLOBAL: &str = "getGameState";

// A per-player leaf (`EngineStateCatalogEntry::is_per_player`) gains a
// non-enumerable `byPlayer(owner)`, so the tree still serializes as `{slot,
// kind}` and interior nodes stay method-free. The tree is built before the SDK
// prelude, so `byPlayer` cannot see the SDK's owner tokens; it recognizes
// `on.player` and `impact.source` by the wire spelling each carries as
// `__wire`, and lowers anything else to `@invalid` for the engine to reject.
// script-compiler's `light_membership.rs` mirrors both installers; the drift
// guard in `postretro-sim` compares them, so they are public for it alone.
#[doc(hidden)]
pub const QUICKJS_INSTALL_BY_PLAYER: &str = r#"(leaf) => {
  const slot = leaf.slot;
  const kind = leaf.kind;
  Object.defineProperty(leaf, "byPlayer", {
    value: function byPlayer(owner) {
      const wire = owner !== null && typeof owner === "object" ? owner.__wire : undefined;
      return Object.freeze({
        slot,
        kind,
        owner: wire === "@player" || wire === "@impact.source" ? wire : "@invalid",
      });
    },
    enumerable: false,
  });
}"#;

// `pcall`: an opaque SDK target raises on any field it does not serve.
#[doc(hidden)]
pub const LUAU_INSTALL_BY_PLAYER: &str = r#"return function(leaf)
  local slot = leaf.slot
  local kind = leaf.kind
  local function byPlayer(_self, owner)
    local wire = nil
    if type(owner) == "table" then
      local ok, value = pcall(function()
        return owner.__wire
      end)
      wire = if ok then value else nil
    end
    if wire ~= "@player" and wire ~= "@impact.source" then
      wire = "@invalid"
    end
    return table.freeze({ slot = slot, kind = kind, owner = wire })
  end
  setmetatable(leaf, table.freeze({ __index = table.freeze({ byPlayer = byPlayer }) }))
end"#;

pub fn install_quickjs_bridge(ctx: &Ctx<'_>) -> Result<(), ScriptError> {
    let catalog = engine_state_catalog().map_err(catalog_error)?;
    install_quickjs_bridge_from_catalog(ctx, &catalog)
}

pub fn install_luau_bridge(lua: &mlua::Lua) -> Result<(), ScriptError> {
    let catalog = engine_state_catalog().map_err(catalog_error)?;
    install_luau_bridge_from_catalog(lua, &catalog)
}

fn catalog_error(error: EngineStateCatalogError) -> ScriptError {
    ScriptError::InvalidArgument {
        reason: format!("engine-state catalog invalid for getGameState bridge: {error}"),
    }
}

fn collision_error(name: &str) -> ScriptError {
    ScriptError::InvalidArgument {
        reason: format!("getGameState bridge global collision: `{name}` already exists"),
    }
}

fn host_error(action: &str, error: impl std::fmt::Display) -> ScriptError {
    ScriptError::InvalidArgument {
        reason: format!("getGameState bridge: {action}: {error}"),
    }
}

/// SDK-only state-ref value tag. Consumers project refs to their stable wire
/// identity through `.slot`, so this metadata never reaches descriptors.
/// Public only for the build-time mirror's drift guard.
#[doc(hidden)]
pub fn state_ref_kind(value_type: EngineStateValueType<'_>) -> &'static str {
    match value_type {
        EngineStateValueType::Number => "number",
        EngineStateValueType::Boolean => "boolean",
        EngineStateValueType::String => "string",
        EngineStateValueType::Enum { .. } => "enum",
        EngineStateValueType::Array => "array",
    }
}

fn install_quickjs_bridge_from_catalog(
    ctx: &Ctx<'_>,
    catalog: &EngineStateCatalog,
) -> Result<(), ScriptError> {
    reject_quickjs_collision(ctx, GAME_STATE_BRIDGE_GLOBAL)?;
    reject_quickjs_collision(ctx, GET_GAME_STATE_GLOBAL)?;

    let install_by_player: JsFunction = ctx
        .eval(QUICKJS_INSTALL_BY_PLAYER)
        .map_err(|e| host_error("failed to build QuickJS byPlayer installer", e))?;
    let root = build_quickjs_object(
        ctx,
        catalog.tree().root(),
        catalog.entries(),
        &install_by_player,
    )?;
    ctx.globals()
        .set(GAME_STATE_BRIDGE_GLOBAL, root)
        .map_err(|e| host_error("failed to install QuickJS bridge", e))?;
    Ok(())
}

fn reject_quickjs_collision(ctx: &Ctx<'_>, name: &str) -> Result<(), ScriptError> {
    let exists = ctx
        .globals()
        .contains_key(name)
        .map_err(|e| host_error("failed to inspect QuickJS globals", e))?;
    if exists {
        return Err(collision_error(name));
    }
    Ok(())
}

fn build_quickjs_object<'js>(
    ctx: &Ctx<'js>,
    children: &std::collections::BTreeMap<String, EngineStateTreeNode>,
    entries: &[EngineStateCatalogEntry<'static>],
    install_by_player: &JsFunction<'js>,
) -> Result<JsObject<'js>, ScriptError> {
    let object = JsObject::new(ctx.clone())
        .map_err(|e| host_error("failed to allocate QuickJS state object", e))?;
    for (segment, node) in children {
        match node {
            EngineStateTreeNode::Object(grandchildren) => {
                let child = build_quickjs_object(ctx, grandchildren, entries, install_by_player)?;
                object
                    .set(segment.as_str(), child)
                    .map_err(|e| host_error("failed to set QuickJS state object field", e))?;
            }
            EngineStateTreeNode::Leaf { entry_index } => {
                let entry = entries.get(*entry_index).ok_or_else(|| ScriptError::InvalidArgument {
                    reason: format!(
                        "getGameState bridge: catalog tree leaf index {entry_index} is out of range"
                    ),
                })?;
                let leaf = JsObject::new(ctx.clone())
                    .map_err(|e| host_error("failed to allocate QuickJS state leaf", e))?;
                leaf.set("slot", entry.wire_name)
                    .map_err(|e| host_error("failed to set QuickJS state leaf slot", e))?;
                leaf.set("kind", state_ref_kind(entry.value_type))
                    .map_err(|e| host_error("failed to set QuickJS state leaf kind", e))?;
                if entry.is_per_player() {
                    install_by_player
                        .call::<_, ()>((leaf.clone(),))
                        .map_err(|e| host_error("failed to install QuickJS byPlayer", e))?;
                }
                freeze_quickjs_object(ctx, &leaf)?;
                object
                    .set(segment.as_str(), leaf)
                    .map_err(|e| host_error("failed to set QuickJS state leaf field", e))?;
            }
        }
    }
    freeze_quickjs_object(ctx, &object)?;
    Ok(object)
}

fn freeze_quickjs_object<'js>(ctx: &Ctx<'js>, object: &JsObject<'js>) -> Result<(), ScriptError> {
    let object_ctor: JsObject = ctx
        .globals()
        .get("Object")
        .map_err(|e| host_error("failed to read QuickJS Object constructor", e))?;
    let freeze: JsFunction = object_ctor
        .get("freeze")
        .map_err(|e| host_error("failed to read QuickJS Object.freeze", e))?;
    let _: JsValue = freeze
        .call((object.clone(),))
        .map_err(|e| host_error("failed to freeze QuickJS state object", e))?;
    Ok(())
}

fn install_luau_bridge_from_catalog(
    lua: &mlua::Lua,
    catalog: &EngineStateCatalog,
) -> Result<(), ScriptError> {
    reject_luau_collision(lua, GAME_STATE_BRIDGE_GLOBAL)?;
    reject_luau_collision(lua, GET_GAME_STATE_GLOBAL)?;

    let install_by_player: mlua::Function = lua
        .load(LUAU_INSTALL_BY_PLAYER)
        .set_name("postretro/game_state_refs/byPlayer")
        .eval()
        .map_err(|e| host_error("failed to build Luau byPlayer installer", e))?;
    let root = build_luau_table(
        lua,
        catalog.tree().root(),
        catalog.entries(),
        &install_by_player,
    )?;
    lua.globals()
        .set(GAME_STATE_BRIDGE_GLOBAL, root)
        .map_err(|e| host_error("failed to install Luau bridge", e))?;
    Ok(())
}

fn reject_luau_collision(lua: &mlua::Lua, name: &str) -> Result<(), ScriptError> {
    let exists = lua
        .globals()
        .contains_key(name)
        .map_err(|e| host_error("failed to inspect Luau globals", e))?;
    if exists {
        return Err(collision_error(name));
    }
    Ok(())
}

fn build_luau_table(
    lua: &mlua::Lua,
    children: &std::collections::BTreeMap<String, EngineStateTreeNode>,
    entries: &[EngineStateCatalogEntry<'static>],
    install_by_player: &mlua::Function,
) -> Result<mlua::Table, ScriptError> {
    let table = lua
        .create_table()
        .map_err(|e| host_error("failed to allocate Luau state table", e))?;
    for (segment, node) in children {
        match node {
            EngineStateTreeNode::Object(grandchildren) => {
                let child = build_luau_table(lua, grandchildren, entries, install_by_player)?;
                table
                    .set(segment.as_str(), child)
                    .map_err(|e| host_error("failed to set Luau state table field", e))?;
            }
            EngineStateTreeNode::Leaf { entry_index } => {
                let entry = entries.get(*entry_index).ok_or_else(|| ScriptError::InvalidArgument {
                    reason: format!(
                        "getGameState bridge: catalog tree leaf index {entry_index} is out of range"
                    ),
                })?;
                let leaf = lua
                    .create_table()
                    .map_err(|e| host_error("failed to allocate Luau state leaf", e))?;
                leaf.set("slot", entry.wire_name)
                    .map_err(|e| host_error("failed to set Luau state leaf slot", e))?;
                leaf.set("kind", state_ref_kind(entry.value_type))
                    .map_err(|e| host_error("failed to set Luau state leaf kind", e))?;
                if entry.is_per_player() {
                    install_by_player
                        .call::<()>(leaf.clone())
                        .map_err(|e| host_error("failed to install Luau byPlayer", e))?;
                }
                leaf.set_readonly(true);
                table
                    .set(segment.as_str(), leaf)
                    .map_err(|e| host_error("failed to set Luau state leaf field", e))?;
            }
        }
    }
    table.set_readonly(true);
    Ok(table)
}

#[cfg(test)]
pub fn install_quickjs_bridge_from_entries(
    ctx: &Ctx<'_>,
    entries: &[EngineStateCatalogEntry<'static>],
) -> Result<(), ScriptError> {
    let catalog = EngineStateCatalog::from_entries(entries).map_err(catalog_error)?;
    install_quickjs_bridge_from_catalog(ctx, &catalog)
}

#[cfg(test)]
pub fn install_luau_bridge_from_entries(
    lua: &mlua::Lua,
    entries: &[EngineStateCatalogEntry<'static>],
) -> Result<(), ScriptError> {
    let catalog = EngineStateCatalog::from_entries(entries).map_err(catalog_error)?;
    install_luau_bridge_from_catalog(lua, &catalog)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine_state_catalog::{
        EngineStateCapability, EngineStateDefault, EngineStateValueType,
    };
    use crate::slot_table::ReplicationScope;
    use std::collections::BTreeMap;

    const BASE: EngineStateCatalogEntry<'static> = EngineStateCatalogEntry {
        wire_name: "player.health",
        sdk_path: &["player", "health"],
        value_type: EngineStateValueType::Number,
        default: EngineStateDefault::None,
        range: None,
        persist: false,
        capability: EngineStateCapability::Readonly,
        network: ReplicationScope::None,
    };

    fn expected_catalog_path_slots() -> BTreeMap<String, String> {
        engine_state_catalog()
            .unwrap()
            .entries()
            .iter()
            .map(|entry| (entry.sdk_path.join("."), entry.wire_name.to_string()))
            .collect()
    }

    fn expected_catalog_path_kinds() -> BTreeMap<String, String> {
        engine_state_catalog()
            .unwrap()
            .entries()
            .iter()
            .map(|entry| {
                (
                    entry.sdk_path.join("."),
                    state_ref_kind(entry.value_type).to_string(),
                )
            })
            .collect()
    }

    fn collect_luau_slots(
        table: mlua::Table,
        prefix: &mut Vec<String>,
        out: &mut BTreeMap<String, String>,
    ) {
        if let Ok(slot) = table.get::<String>("slot") {
            out.insert(prefix.join("."), slot);
            return;
        }

        for pair in table.pairs::<String, mlua::Value>() {
            let (key, value) = pair.unwrap();
            if let mlua::Value::Table(child) = value {
                prefix.push(key);
                collect_luau_slots(child, prefix, out);
                prefix.pop();
            }
        }
    }

    fn collect_luau_kinds(
        table: mlua::Table,
        prefix: &mut Vec<String>,
        out: &mut BTreeMap<String, String>,
    ) {
        if let Ok(kind) = table.get::<String>("kind") {
            out.insert(prefix.join("."), kind);
            return;
        }

        for pair in table.pairs::<String, mlua::Value>() {
            let (key, value) = pair.unwrap();
            if let mlua::Value::Table(child) = value {
                prefix.push(key);
                collect_luau_kinds(child, prefix, out);
                prefix.pop();
            }
        }
    }

    #[test]
    fn quickjs_bridge_runtime_tree_matches_catalog_paths_and_slots() {
        let runtime = rquickjs::Runtime::new().unwrap();
        let ctx = rquickjs::Context::full(&runtime).unwrap();
        ctx.with(|ctx| {
            install_quickjs_bridge(&ctx).unwrap();
            let json: String = ctx
                .eval(
                    r#"
                    (() => {
                      const out = {};
                      function walk(node, path) {
                        if (node && typeof node.slot === "string") {
                          out[path] = node.slot;
                          return;
                        }
                        for (const key of Object.keys(node).sort()) {
                          walk(node[key], path ? `${path}.${key}` : key);
                        }
                      }
                      walk(globalThis.__postretroGameStateRefs, "");
                      return JSON.stringify(out);
                    })()
                    "#,
                )
                .unwrap();
            let got: BTreeMap<String, String> = serde_json::from_str(&json).unwrap();
            assert_eq!(got, expected_catalog_path_slots());
        });
    }

    #[test]
    fn luau_bridge_runtime_tree_matches_catalog_paths_and_slots() {
        let lua = mlua::Lua::new();
        install_luau_bridge(&lua).unwrap();
        let root: mlua::Table = lua.globals().get(GAME_STATE_BRIDGE_GLOBAL).unwrap();
        let mut got = BTreeMap::new();
        collect_luau_slots(root, &mut Vec::new(), &mut got);
        assert_eq!(got, expected_catalog_path_slots());
    }

    #[test]
    fn quickjs_bridge_stamps_catalog_value_kinds() {
        let runtime = rquickjs::Runtime::new().unwrap();
        let ctx = rquickjs::Context::full(&runtime).unwrap();
        ctx.with(|ctx| {
            install_quickjs_bridge(&ctx).unwrap();
            let json: String = ctx
                .eval(
                    r#"
                    (() => {
                      const out = {};
                      function walk(node, path) {
                        if (node && typeof node.kind === "string") {
                          out[path] = node.kind;
                          return;
                        }
                        for (const key of Object.keys(node).sort()) {
                          walk(node[key], path ? `${path}.${key}` : key);
                        }
                      }
                      walk(globalThis.__postretroGameStateRefs, "");
                      return JSON.stringify(out);
                    })()
                    "#,
                )
                .unwrap();
            let got: BTreeMap<String, String> = serde_json::from_str(&json).unwrap();
            assert_eq!(got, expected_catalog_path_kinds());
        });
    }

    #[test]
    fn luau_bridge_stamps_catalog_value_kinds() {
        let lua = mlua::Lua::new();
        install_luau_bridge(&lua).unwrap();
        let root: mlua::Table = lua.globals().get(GAME_STATE_BRIDGE_GLOBAL).unwrap();
        let mut got = BTreeMap::new();
        collect_luau_kinds(root, &mut Vec::new(), &mut got);
        assert_eq!(got, expected_catalog_path_kinds());
    }

    #[test]
    fn per_player_leaves_lower_by_player_owner_tokens_in_both_runtimes() {
        // Only the catalog's per-player leaves carry `byPlayer`; it accepts
        // the two owner tokens by wire spelling and lowers anything else to
        // `@invalid` for the engine to reject.
        let runtime = rquickjs::Runtime::new().unwrap();
        let ctx = rquickjs::Context::full(&runtime).unwrap();
        ctx.with(|ctx| {
            install_quickjs_bridge(&ctx).unwrap();
            let json: String = ctx
                .eval(
                    r#"
                    (() => {
                      const p = globalThis.__postretroGameStateRefs.player;
                      const player = {};
                      Object.defineProperty(player, "__wire", { value: "@player" });
                      Object.freeze(player);
                      return JSON.stringify([
                        p.health.byPlayer(player),
                        p.overheated.byPlayer({ __wire: "@impact.source" }),
                        p.heat.byPlayer({ __wire: "@emitter" }),
                        typeof p.weapon.current.byPlayer,
                        Object.keys(p.health),
                      ]);
                    })()
                    "#,
                )
                .unwrap();
            let got: serde_json::Value = serde_json::from_str(&json).unwrap();
            assert_eq!(
                got,
                serde_json::json!([
                    { "slot": "player.health", "kind": "number", "owner": "@player" },
                    { "slot": "player.overheated", "kind": "boolean", "owner": "@impact.source" },
                    { "slot": "player.heat", "kind": "number", "owner": "@invalid" },
                    "undefined",
                    ["slot", "kind"],
                ])
            );
        });

        let lua = mlua::Lua::new();
        install_luau_bridge(&lua).unwrap();
        let got: (String, String, String, bool) = lua
            .load(
                r#"
                local p = __postretroGameStateRefs.player
                local opaque = setmetatable({}, { __index = function() error("opaque") end })
                return p.health:byPlayer({ __wire = "@player" }).owner,
                  p.overheated:byPlayer({ __wire = "@impact.source" }).owner,
                  p.heat:byPlayer(opaque).owner,
                  p.weapon.current.byPlayer == nil
                "#,
            )
            .eval()
            .unwrap();
        assert_eq!(
            got,
            (
                "@player".to_string(),
                "@impact.source".to_string(),
                "@invalid".to_string(),
                true
            )
        );
    }

    #[test]
    fn malformed_catalog_rejects_quickjs_bridge_without_partial_global() {
        let runtime = rquickjs::Runtime::new().unwrap();
        let ctx = rquickjs::Context::full(&runtime).unwrap();
        ctx.with(|ctx| {
            let bad = EngineStateCatalogEntry {
                sdk_path: &["player", "bad-name"],
                ..BASE
            };

            let err = install_quickjs_bridge_from_entries(&ctx, &[bad])
                .expect_err("malformed path must reject bridge construction");
            match err {
                ScriptError::InvalidArgument { reason } => {
                    assert!(reason.contains("engine-state catalog invalid"), "{reason}");
                    assert!(reason.contains("bad-name"), "{reason}");
                }
                other => panic!("expected InvalidArgument, got {other:?}"),
            }
            assert!(
                !ctx.globals()
                    .contains_key(GAME_STATE_BRIDGE_GLOBAL)
                    .unwrap(),
                "malformed catalog must not expose a partial bridge"
            );
        });
    }

    #[test]
    fn malformed_catalog_rejects_luau_bridge_without_partial_global() {
        let lua = mlua::Lua::new();
        let bad = EngineStateCatalogEntry {
            sdk_path: &["player", "bad-name"],
            ..BASE
        };

        let err = install_luau_bridge_from_entries(&lua, &[bad])
            .expect_err("malformed path must reject bridge construction");
        match err {
            ScriptError::InvalidArgument { reason } => {
                assert!(reason.contains("engine-state catalog invalid"), "{reason}");
                assert!(reason.contains("bad-name"), "{reason}");
            }
            other => panic!("expected InvalidArgument, got {other:?}"),
        }
        assert!(
            !lua.globals()
                .contains_key(GAME_STATE_BRIDGE_GLOBAL)
                .unwrap(),
            "malformed catalog must not expose a partial bridge"
        );
    }

    #[test]
    fn existing_get_game_state_global_rejects_quickjs_install() {
        let runtime = rquickjs::Runtime::new().unwrap();
        let ctx = rquickjs::Context::full(&runtime).unwrap();
        ctx.with(|ctx| {
            ctx.globals().set(GET_GAME_STATE_GLOBAL, 1).unwrap();
            let err = install_quickjs_bridge_from_entries(&ctx, &[BASE])
                .expect_err("global collision must reject bridge construction");
            match err {
                ScriptError::InvalidArgument { reason } => {
                    assert!(reason.contains("global collision"), "{reason}");
                    assert!(reason.contains(GET_GAME_STATE_GLOBAL), "{reason}");
                }
                other => panic!("expected InvalidArgument, got {other:?}"),
            }
        });
    }

    #[test]
    fn existing_bridge_global_rejects_luau_install() {
        let lua = mlua::Lua::new();
        lua.globals().set(GAME_STATE_BRIDGE_GLOBAL, true).unwrap();
        let err = install_luau_bridge_from_entries(&lua, &[BASE])
            .expect_err("bridge collision must reject bridge construction");
        match err {
            ScriptError::InvalidArgument { reason } => {
                assert!(reason.contains("global collision"), "{reason}");
                assert!(reason.contains(GAME_STATE_BRIDGE_GLOBAL), "{reason}");
            }
            other => panic!("expected InvalidArgument, got {other:?}"),
        }
    }
}
