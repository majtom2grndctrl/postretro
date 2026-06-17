// Luau virtual module registry: engine-owned modules resolved before mod files.
// See: context/lib/scripting.md

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use mlua::{Lua, Table, Value};

use super::error::ScriptError;

const MAX_VIRTUAL_MODULE_COPY_DEPTH: usize = 32;

/// VM-local registry for engine-owned Luau modules resolved before mod files.
///
/// Modules are stored as Lua table handles tied to the state that created this
/// registry. Registering from an export table deep-copies table values before
/// marking the owned copy read-only, so freezing a virtual module never mutates
/// the bare globals or SDK return tables it was built from.
#[derive(Clone, Debug, Default)]
pub(crate) struct LuauVirtualModuleRegistry {
    modules: Rc<RefCell<BTreeMap<String, Table>>>,
}

impl LuauVirtualModuleRegistry {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn register_from_table(
        &self,
        lua: &Lua,
        name: &str,
        exports: Table,
    ) -> Result<(), ScriptError> {
        if name.is_empty() {
            return Err(ScriptError::InvalidArgument {
                reason: "virtual Luau module name must not be empty".to_string(),
            });
        }

        let module = copy_owned_table(lua, exports, 0)?;
        let mut modules = self.modules.borrow_mut();
        if modules.contains_key(name) {
            return Err(ScriptError::InvalidArgument {
                reason: format!("virtual Luau module `{name}` is already registered"),
            });
        }
        modules.insert(name.to_string(), module);
        Ok(())
    }

    pub(crate) fn get(&self, name: &str) -> Option<Table> {
        self.modules.borrow().get(name).cloned()
    }
}

fn copy_owned_value(lua: &Lua, value: Value, depth: usize) -> Result<Value, ScriptError> {
    match value {
        Value::Table(table) => copy_owned_table(lua, table, depth + 1).map(Value::Table),
        other => Ok(other),
    }
}

fn copy_owned_table(lua: &Lua, source: Table, depth: usize) -> Result<Table, ScriptError> {
    if depth > MAX_VIRTUAL_MODULE_COPY_DEPTH {
        return Err(ScriptError::InvalidArgument {
            reason: format!(
                "virtual Luau module table nesting exceeds {MAX_VIRTUAL_MODULE_COPY_DEPTH} levels"
            ),
        });
    }

    let table = lua
        .create_table()
        .map_err(|e| invalid_argument("failed to allocate virtual Luau module table", e))?;
    for pair in source.pairs::<Value, Value>() {
        let (key, value) =
            pair.map_err(|e| invalid_argument("failed to read virtual Luau module export", e))?;
        let key = copy_owned_value(lua, key, depth)?;
        let value = copy_owned_value(lua, value, depth)?;
        table
            .set(key, value)
            .map_err(|e| invalid_argument("failed to set virtual Luau module export", e))?;
    }
    table.set_readonly(true);
    Ok(table)
}

fn invalid_argument(action: &str, error: mlua::Error) -> ScriptError {
    ScriptError::InvalidArgument {
        reason: format!("{action}: {error}"),
    }
}
