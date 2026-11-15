// Runtime installation for the generated engine-owned state reference tree.
// The tree is built from the shared catalog, exposed through a temporary bridge
// before the SDK prelude, then captured by SDK-side `getGameState()`.
// See: context/lib/scripting.md §5

use std::sync::LazyLock;

use postretro_foundation::owner_tokens::{
    by_player_owner_tokens_js_array, by_player_owner_tokens_luau_set,
};
use rquickjs::{Ctx, Function as JsFunction, Object as JsObject, Value as JsValue};

#[cfg(test)]
use super::engine_state_catalog::EngineStateCatalogEntry;
use super::engine_state_catalog::{
    EngineStateCatalog, EngineStateCatalogError, EngineStateTreeNode, EngineStateValueType,
    engine_state_catalog,
};
use super::error::ScriptError;
use postretro_entities::engine_state_collections::EngineStateField;

pub const GAME_STATE_BRIDGE_GLOBAL: &str = "__postretroGameStateRefs";
pub const GET_GAME_STATE_GLOBAL: &str = "getGameState";

// A per-player leaf (`EngineStateCatalogEntry::is_per_player`) gains a
// non-enumerable `byPlayer(owner)`, so the tree still serializes as `{slot,
// kind}` and interior nodes stay method-free. The tree is built before the SDK
// prelude, so `byPlayer` cannot see the SDK's owner tokens; it recognizes
// every owner handle (`on.player`, `impact.source`, `assist.helper`,
// `assist.target`) by the wire spelling each carries as `__wire`, generated
// from `BY_PLAYER_OWNER_TOKENS`, and lowers anything else to `@invalid` for the
// engine to reject.
// script-compiler's `light_membership.rs` mirrors both installers; the drift
// guard in `postretro-sim` compares them, so they are public for it alone.
#[doc(hidden)]
pub static QUICKJS_INSTALL_BY_PLAYER: LazyLock<String> = LazyLock::new(|| {
    QUICKJS_INSTALL_BY_PLAYER_TEMPLATE.replace("__OWNERS__", &by_player_owner_tokens_js_array())
});

const QUICKJS_INSTALL_BY_PLAYER_TEMPLATE: &str = r#"(leaf) => {
  const slot = leaf.slot;
  const kind = leaf.kind;
  const owners = __OWNERS__;
  Object.defineProperty(leaf, "byPlayer", {
    value: function byPlayer(owner) {
      const wire = owner !== null && typeof owner === "object" ? owner.__wire : undefined;
      return Object.freeze({
        slot,
        kind,
        owner: typeof wire === "string" && owners.includes(wire) ? wire : "@invalid",
      });
    },
    enumerable: false,
  });
}"#;

// `pcall`: an opaque SDK target raises on any field it does not serve.
#[doc(hidden)]
pub static LUAU_INSTALL_BY_PLAYER: LazyLock<String> = LazyLock::new(|| {
    LUAU_INSTALL_BY_PLAYER_TEMPLATE.replace("__OWNERS__", &by_player_owner_tokens_luau_set())
});

const LUAU_INSTALL_BY_PLAYER_TEMPLATE: &str = r#"return function(leaf)
  local slot = leaf.slot
  local kind = leaf.kind
  local owners = __OWNERS__
  local function byPlayer(_self, owner)
    local wire = nil
    if type(owner) == "table" then
      local ok, value = pcall(function()
        return owner.__wire
      end)
      wire = if ok then value else nil
    end
    if type(wire) ~= "string" or not owners[wire] then
      wire = "@invalid"
    end
    return table.freeze({ slot = slot, kind = kind, owner = wire })
  end
  setmetatable(leaf, table.freeze({ __index = table.freeze({ byPlayer = byPlayer }) }))
end"#;

// A family node gains a non-enumerable accessor (`occupancy.tag(key)`)
// returning frozen `{ field: { slot, kind } }` member refs for any string key;
// a list is a frozen array of row refs carrying a non-enumerable `__list` wire
// prefix, so `forEach` can tell an engine list from a plain array. Rows are
// keyed from 0, as their slots are. script-compiler mirrors both installers.
#[doc(hidden)]
pub const QUICKJS_INSTALL_FAMILY: &str = r#"(node, accessor, prefix, fields) => {
  Object.defineProperty(node, accessor, {
    value: function (key) {
      if (typeof key !== "string") {
        throw new TypeError(prefix + "." + accessor + "(key) takes a string key");
      }
      const member = {};
      for (const [name, kind] of fields) {
        member[name] = Object.freeze({ slot: prefix + "." + key + "." + name, kind });
      }
      return Object.freeze(member);
    },
    enumerable: false,
  });
}"#;

#[doc(hidden)]
pub const QUICKJS_BUILD_LIST: &str = r#"(prefix, capacity, fields) => {
  const rows = [];
  for (let row = 0; row < capacity; row++) {
    const refs = {};
    for (const [name, kind] of fields) {
      refs[name] = Object.freeze({ slot: prefix + "." + row + "." + name, kind });
    }
    rows.push(Object.freeze(refs));
  }
  Object.defineProperty(rows, "__list", { value: prefix, enumerable: false });
  return Object.freeze(rows);
}"#;

// Luau reaches the accessor with a colon call (`game.occupancy:tag(key)`);
// list rows sit at 1..capacity, row `i` holding slots keyed `i - 1`.
#[doc(hidden)]
pub const LUAU_INSTALL_FAMILY: &str = r#"return function(node, accessor, prefix, fields)
  local function access(_self, key)
    if type(key) ~= "string" then
      error(prefix .. "." .. accessor .. "(key) takes a string key", 2)
    end
    local member = {}
    for _, field in fields do
      member[field[1]] = table.freeze({ slot = prefix .. "." .. key .. "." .. field[1], kind = field[2] })
    end
    return table.freeze(member)
  end
  setmetatable(node, table.freeze({ __index = table.freeze({ [accessor] = access }) }))
end"#;

#[doc(hidden)]
pub const LUAU_BUILD_LIST: &str = r#"return function(prefix, capacity, fields)
  local rows = {}
  for row = 0, capacity - 1 do
    local refs = {}
    for _, field in fields do
      refs[field[1]] = table.freeze({ slot = prefix .. "." .. row .. "." .. field[1], kind = field[2] })
    end
    rows[row + 1] = table.freeze(refs)
  end
  setmetatable(rows, table.freeze({ __index = table.freeze({ __list = prefix }) }))
  return table.freeze(rows)
end"#;

/// `(name, kind)` for each field a family member or list row carries.
/// Public only for the build-time mirror's drift guard.
#[doc(hidden)]
pub fn collection_field_kinds(fields: &[EngineStateField<'_>]) -> Vec<(String, &'static str)> {
    fields
        .iter()
        .map(|field| (field.name.to_string(), state_ref_kind(field.value_type)))
        .collect()
}

struct QuickJsInstallers<'js> {
    by_player: JsFunction<'js>,
    family: JsFunction<'js>,
    list: JsFunction<'js>,
}

struct LuauInstallers {
    by_player: mlua::Function,
    family: mlua::Function,
    list: mlua::Function,
}

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

    let installers = QuickJsInstallers {
        by_player: ctx
            .eval(QUICKJS_INSTALL_BY_PLAYER.as_str())
            .map_err(|e| host_error("failed to build QuickJS byPlayer installer", e))?,
        family: ctx
            .eval(QUICKJS_INSTALL_FAMILY)
            .map_err(|e| host_error("failed to build QuickJS family installer", e))?,
        list: ctx
            .eval(QUICKJS_BUILD_LIST)
            .map_err(|e| host_error("failed to build QuickJS list builder", e))?,
    };
    let root = build_quickjs_object(ctx, catalog.tree().root(), catalog, &installers)?;
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

fn quickjs_fields<'js>(
    ctx: &Ctx<'js>,
    fields: &[EngineStateField<'_>],
) -> Result<rquickjs::Array<'js>, ScriptError> {
    let array = rquickjs::Array::new(ctx.clone())
        .map_err(|e| host_error("failed to allocate QuickJS field list", e))?;
    for (index, (name, kind)) in collection_field_kinds(fields).into_iter().enumerate() {
        let pair = rquickjs::Array::new(ctx.clone())
            .map_err(|e| host_error("failed to allocate QuickJS field pair", e))?;
        pair.set(0, name)
            .and_then(|()| pair.set(1, kind))
            .and_then(|()| array.set(index, pair))
            .map_err(|e| host_error("failed to set QuickJS field pair", e))?;
    }
    Ok(array)
}

fn build_quickjs_object<'js>(
    ctx: &Ctx<'js>,
    children: &std::collections::BTreeMap<String, EngineStateTreeNode>,
    catalog: &EngineStateCatalog,
    installers: &QuickJsInstallers<'js>,
) -> Result<JsObject<'js>, ScriptError> {
    let entries = catalog.entries();
    let object = JsObject::new(ctx.clone())
        .map_err(|e| host_error("failed to allocate QuickJS state object", e))?;
    for (segment, node) in children {
        match node {
            EngineStateTreeNode::Object(grandchildren) => {
                let child = build_quickjs_object(ctx, grandchildren, catalog, installers)?;
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
                    installers
                        .by_player
                        .call::<_, ()>((leaf.clone(),))
                        .map_err(|e| host_error("failed to install QuickJS byPlayer", e))?;
                }
                freeze_quickjs_object(ctx, &leaf)?;
                object
                    .set(segment.as_str(), leaf)
                    .map_err(|e| host_error("failed to set QuickJS state leaf field", e))?;
            }
            EngineStateTreeNode::Family { family_index } => {
                let family = &catalog.families()[*family_index];
                let node = JsObject::new(ctx.clone())
                    .map_err(|e| host_error("failed to allocate QuickJS family node", e))?;
                installers
                    .family
                    .call::<_, ()>((
                        node.clone(),
                        family.accessor,
                        family.wire_prefix(),
                        quickjs_fields(ctx, family.fields)?,
                    ))
                    .map_err(|e| host_error("failed to install QuickJS family accessor", e))?;
                freeze_quickjs_object(ctx, &node)?;
                object
                    .set(segment.as_str(), node)
                    .map_err(|e| host_error("failed to set QuickJS family node", e))?;
            }
            EngineStateTreeNode::List { list_index } => {
                let list = &catalog.lists()[*list_index];
                let rows: JsValue = installers
                    .list
                    .call((
                        list.wire_prefix(),
                        list.capacity,
                        quickjs_fields(ctx, list.fields)?,
                    ))
                    .map_err(|e| host_error("failed to build QuickJS list rows", e))?;
                object
                    .set(segment.as_str(), rows)
                    .map_err(|e| host_error("failed to set QuickJS list", e))?;
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

    let load = |source: &str, name: &str| -> Result<mlua::Function, ScriptError> {
        lua.load(source)
            .set_name(format!("postretro/game_state_refs/{name}"))
            .eval()
            .map_err(|e| host_error(&format!("failed to build Luau {name} installer"), e))
    };
    let installers = LuauInstallers {
        by_player: load(LUAU_INSTALL_BY_PLAYER.as_str(), "byPlayer")?,
        family: load(LUAU_INSTALL_FAMILY, "family")?,
        list: load(LUAU_BUILD_LIST, "list")?,
    };
    let root = build_luau_table(lua, catalog.tree().root(), catalog, &installers)?;
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

fn luau_fields(
    lua: &mlua::Lua,
    fields: &[EngineStateField<'_>],
) -> Result<mlua::Table, ScriptError> {
    let pairs = collection_field_kinds(fields)
        .into_iter()
        .map(|(name, kind)| lua.create_sequence_from([name, kind.to_string()]))
        .collect::<mlua::Result<Vec<_>>>()
        .map_err(|e| host_error("failed to allocate Luau field pair", e))?;
    lua.create_sequence_from(pairs)
        .map_err(|e| host_error("failed to allocate Luau field list", e))
}

fn build_luau_table(
    lua: &mlua::Lua,
    children: &std::collections::BTreeMap<String, EngineStateTreeNode>,
    catalog: &EngineStateCatalog,
    installers: &LuauInstallers,
) -> Result<mlua::Table, ScriptError> {
    let entries = catalog.entries();
    let table = lua
        .create_table()
        .map_err(|e| host_error("failed to allocate Luau state table", e))?;
    for (segment, node) in children {
        match node {
            EngineStateTreeNode::Object(grandchildren) => {
                let child = build_luau_table(lua, grandchildren, catalog, installers)?;
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
                    installers
                        .by_player
                        .call::<()>(leaf.clone())
                        .map_err(|e| host_error("failed to install Luau byPlayer", e))?;
                }
                leaf.set_readonly(true);
                table
                    .set(segment.as_str(), leaf)
                    .map_err(|e| host_error("failed to set Luau state leaf field", e))?;
            }
            EngineStateTreeNode::Family { family_index } => {
                let family = &catalog.families()[*family_index];
                let node = lua
                    .create_table()
                    .map_err(|e| host_error("failed to allocate Luau family node", e))?;
                installers
                    .family
                    .call::<()>((
                        node.clone(),
                        family.accessor,
                        family.wire_prefix(),
                        luau_fields(lua, family.fields)?,
                    ))
                    .map_err(|e| host_error("failed to install Luau family accessor", e))?;
                node.set_readonly(true);
                table
                    .set(segment.as_str(), node)
                    .map_err(|e| host_error("failed to set Luau family node", e))?;
            }
            EngineStateTreeNode::List { list_index } => {
                let list = &catalog.lists()[*list_index];
                let rows: mlua::Table = installers
                    .list
                    .call((
                        list.wire_prefix(),
                        list.capacity,
                        luau_fields(lua, list.fields)?,
                    ))
                    .map_err(|e| host_error("failed to build Luau list rows", e))?;
                table
                    .set(segment.as_str(), rows)
                    .map_err(|e| host_error("failed to set Luau list", e))?;
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

    /// Every enumerable leaf path → (slot, kind). List rows enumerate from
    /// `row_base` (0 in QuickJS arrays, 1 in Luau sequences) while their slots
    /// keep 0-based row keys; a family's members are reached only through its
    /// accessor, so it contributes no enumerable leaf.
    fn expected_catalog_leaves(row_base: usize) -> BTreeMap<String, (String, String)> {
        let catalog = engine_state_catalog().unwrap();
        let mut leaves: BTreeMap<String, (String, String)> = catalog
            .entries()
            .iter()
            .map(|entry| {
                (
                    entry.sdk_path.join("."),
                    (
                        entry.wire_name.to_string(),
                        state_ref_kind(entry.value_type).to_string(),
                    ),
                )
            })
            .collect();
        for list in catalog.lists() {
            for row in 0..list.capacity {
                for (field, kind) in collection_field_kinds(list.fields) {
                    leaves.insert(
                        format!("{}.{}.{field}", list.wire_prefix(), row + row_base),
                        (list.slot_name(row, &field), kind.to_string()),
                    );
                }
            }
        }
        leaves
    }

    fn expected_catalog_path_slots(row_base: usize) -> BTreeMap<String, String> {
        expected_catalog_leaves(row_base)
            .into_iter()
            .map(|(path, (slot, _))| (path, slot))
            .collect()
    }

    fn expected_catalog_path_kinds(row_base: usize) -> BTreeMap<String, String> {
        expected_catalog_leaves(row_base)
            .into_iter()
            .map(|(path, (_, kind))| (path, kind))
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
            assert_eq!(got, expected_catalog_path_slots(0));
        });
    }

    #[test]
    fn luau_bridge_runtime_tree_matches_catalog_paths_and_slots() {
        let lua = mlua::Lua::new();
        install_luau_bridge(&lua).unwrap();
        let root: mlua::Table = lua.globals().get(GAME_STATE_BRIDGE_GLOBAL).unwrap();
        let mut got = BTreeMap::new();
        collect_luau_slots(root, &mut Vec::new(), &mut got);
        assert_eq!(got, expected_catalog_path_slots(1));
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
            assert_eq!(got, expected_catalog_path_kinds(0));
        });
    }

    #[test]
    fn luau_bridge_stamps_catalog_value_kinds() {
        let lua = mlua::Lua::new();
        install_luau_bridge(&lua).unwrap();
        let root: mlua::Table = lua.globals().get(GAME_STATE_BRIDGE_GLOBAL).unwrap();
        let mut got = BTreeMap::new();
        collect_luau_kinds(root, &mut Vec::new(), &mut got);
        assert_eq!(got, expected_catalog_path_kinds(1));
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

    // A family accessor builds member refs for any string key, rejects a
    // non-string, and stays off the enumerable tree; a list is a frozen row
    // array marked as an engine list.
    #[test]
    fn family_accessor_and_list_rows_serve_slot_refs_in_both_runtimes() {
        let runtime = rquickjs::Runtime::new().unwrap();
        let ctx = rquickjs::Context::full(&runtime).unwrap();
        ctx.with(|ctx| {
            install_quickjs_bridge(&ctx).unwrap();
            let json: String = ctx
                .eval(
                    r#"
                    (() => {
                      const g = globalThis.__postretroGameStateRefs;
                      const hill = g.occupancy.tag("hill");
                      let threw = false;
                      try { g.occupancy.tag(3); } catch (e) { threw = e instanceof TypeError; }
                      const players = g.session.players;
                      return JSON.stringify([
                        hill,
                        Object.isFrozen(hill),
                        Object.keys(g.occupancy),
                        threw,
                        players.length,
                        players.__list,
                        Object.isFrozen(players),
                        players[2].status,
                        Object.keys(players[0]),
                      ]);
                    })()
                    "#,
                )
                .unwrap();
            let got: serde_json::Value = serde_json::from_str(&json).unwrap();
            assert_eq!(
                got,
                serde_json::json!([
                    {
                        "occupiedCount": { "slot": "occupancy.hill.occupiedCount", "kind": "number" },
                        "occupants": { "slot": "occupancy.hill.occupants", "kind": "number" },
                    },
                    true,
                    [],
                    true,
                    postretro_foundation::seat::SESSION_PLAYER_CAPACITY,
                    "session.players",
                    true,
                    { "slot": "session.players.2.status", "kind": "enum" },
                    ["present", "name", "status", "isLocal"],
                ])
            );
        });

        let lua = mlua::Lua::new();
        install_luau_bridge(&lua).unwrap();
        let got: (String, String, bool, usize, String, String, bool) = lua
            .load(
                r#"
                local g = __postretroGameStateRefs
                local hill = g.occupancy:tag("hill")
                local ok = pcall(function() return g.occupancy:tag(3) end)
                local players = g.session.players
                return hill.occupants.slot, hill.occupiedCount.kind, ok, #players,
                  players.__list, players[1].present.slot, table.isfrozen(players)
                "#,
            )
            .eval()
            .unwrap();
        assert_eq!(
            got,
            (
                "occupancy.hill.occupants".to_string(),
                "number".to_string(),
                false,
                postretro_foundation::seat::SESSION_PLAYER_CAPACITY,
                "session.players".to_string(),
                "session.players.0.present".to_string(),
                true,
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
