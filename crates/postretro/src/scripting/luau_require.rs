// Luau require resolver: mod-rooted file loading for short-lived setup VMs.
// See: context/lib/scripting.md

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use mlua::{Compiler, Lua};

use super::error::ScriptError;
use super::luau_virtual_modules::LuauVirtualModuleRegistry;

/// Opt-in dependency recorder for short-lived mod-init Luau builds.
///
/// `build_lua_state` (startup/data-script path) uses no tracker.
/// `build_lua_state_with_require_tracking` (staged manifest builds) accepts
/// one so only those `require` calls contribute to the hot-reload dependency
/// set.
#[derive(Clone, Debug)]
pub(crate) struct LuauRequireTracker {
    mod_root: PathBuf,
    paths: Rc<RefCell<BTreeSet<PathBuf>>>,
}

impl LuauRequireTracker {
    pub(crate) fn new(mod_root: &Path) -> Result<Self, ScriptError> {
        let mod_root = mod_root
            .canonicalize()
            .map_err(|e| ScriptError::InvalidArgument {
                reason: format!(
                    "mod-init: failed to canonicalize `{}`: {e}",
                    mod_root.display()
                ),
            })?;
        Ok(Self {
            mod_root,
            paths: Rc::new(RefCell::new(BTreeSet::new())),
        })
    }

    pub(crate) fn dependency_paths(&self) -> Vec<PathBuf> {
        self.paths.borrow().iter().cloned().collect()
    }
}

/// Install a `require` global rooted at `mod_root`.
///
/// # Resolution rules
///
/// - The path argument is treated as a relative path. A leading `./` is
///   stripped; `../` segments are rejected (mods must not escape their root).
/// - Absolute paths are rejected.
/// - If the resolved path lacks a `.luau` extension, one is appended.
/// - The resolved file is read, compiled with `mlua::Compiler`, and executed
///   in the same Lua state. Its return value (typically a table) is the
///   value of the `require` call.
/// - File-not-found, IO failure, compile failure, and runtime error all
///   surface as `mlua::Error::RuntimeError` so scripts can `pcall` them.
///
/// This is intentionally simpler than Luau's full `require()` semantics: no
/// module caching, no upward path search, no init-file convention. It exists
/// to wire `start-script.luau` to its sibling domain scripts. Richer semantics
/// (caching, upward search) can be added when mods require them — see
/// `context/lib/scripting.md` §2.
pub(super) fn install_require_resolver(
    lua: &Lua,
    mod_root: &Path,
    require_tracker: Option<&LuauRequireTracker>,
    virtual_modules: &LuauVirtualModuleRegistry,
) -> Result<(), ScriptError> {
    let mod_root: PathBuf = mod_root.to_path_buf();
    let tracker = require_tracker.cloned();
    let virtual_modules = virtual_modules.clone();
    let f = lua
        .create_function(move |lua, path: String| -> mlua::Result<mlua::Value> {
            if let Some(module) = virtual_modules.get(&path) {
                return Ok(mlua::Value::Table(module));
            }

            let resolved =
                resolve_require_path(&mod_root, &path).map_err(mlua::Error::RuntimeError)?;
            if let Some(tracker) = &tracker {
                let canonical = canonical_require_dependency(&tracker.mod_root, &resolved, &path)
                    .map_err(mlua::Error::RuntimeError)?;
                tracker.paths.borrow_mut().insert(canonical);
            }
            let source = std::fs::read_to_string(&resolved).map_err(|e| {
                mlua::Error::RuntimeError(format!(
                    "require(`{path}`): failed to read `{}`: {e}",
                    resolved.display()
                ))
            })?;
            let bytecode = Compiler::new().compile(&source).map_err(|e| {
                mlua::Error::RuntimeError(format!("require(`{path}`): compile failed: {e}"))
            })?;
            lua.load(&bytecode)
                .set_name(resolved.to_string_lossy().as_ref())
                .set_mode(mlua::ChunkMode::Binary)
                .eval::<mlua::Value>()
        })
        .map_err(|e| ScriptError::InvalidArgument {
            reason: e.to_string(),
        })?;
    lua.globals()
        .set("require", f)
        .map_err(|e| ScriptError::InvalidArgument {
            reason: e.to_string(),
        })?;
    Ok(())
}

fn canonical_require_dependency(
    canonical_mod_root: &Path,
    resolved: &Path,
    request_path: &str,
) -> Result<PathBuf, String> {
    let canonical = resolved.canonicalize().map_err(|e| {
        format!(
            "require(`{request_path}`): failed to canonicalize `{}`: {e}",
            resolved.display()
        )
    })?;
    if !canonical.starts_with(canonical_mod_root) {
        return Err(format!(
            "require(`{request_path}`): resolved path `{}` is outside mod root `{}`",
            canonical.display(),
            canonical_mod_root.display()
        ));
    }
    Ok(canonical)
}

/// Resolve a `require(...)` argument to an absolute path under `mod_root`.
/// Rejects absolute paths and `..` traversal. Appends `.luau` if missing.
fn resolve_require_path(mod_root: &Path, path: &str) -> Result<PathBuf, String> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err("require: empty path".to_string());
    }
    // Reject backslashes outright. On Unix `Path` treats `\` as an ordinary
    // filename character, so a Windows-style `..\escape` would slip past the
    // `Component::ParentDir` scan below. Rejecting at the string level keeps
    // behavior consistent across platforms.
    if trimmed.contains('\\') {
        return Err(format!(
            "require(`{path}`): backslashes are not permitted in require paths"
        ));
    }
    // Belt-and-suspenders: also scan the raw string for `..` segments. The
    // component-level check below is the canonical guard, but the platform
    // divergence around path separators makes a string-level check cheap
    // insurance against future regressions.
    if trimmed.split('/').any(|seg| seg == "..") {
        return Err(format!(
            "require(`{path}`): `..` segments are not permitted (mod root escape)"
        ));
    }
    let stripped = trimmed.strip_prefix("./").unwrap_or(trimmed);
    let candidate = Path::new(stripped);
    if candidate.is_absolute() {
        return Err(format!(
            "require(`{path}`): absolute paths are not permitted"
        ));
    }
    if candidate
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err(format!(
            "require(`{path}`): `..` segments are not permitted (mod root escape)"
        ));
    }
    let mut joined = mod_root.join(candidate);
    if joined.extension().is_none() {
        joined.set_extension("luau");
    }
    // This resolver is lexical: it rejects direct `..`/absolute traversal but
    // leaves symlink containment to the dependency tracker path, where
    // canonicalization is available for hot-reload dependency validation.
    Ok(joined)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scripting::luau::build_lua_state_with_require_tracking;
    use crate::scripting::luau_prelude::{
        POSTRETRO_ROOT_MODULE_EXPORTS, POSTRETRO_UI_MODULE_EXPORTS,
    };
    use crate::scripting::luau_virtual_modules::LuauVirtualModuleRegistry;
    use mlua::Table;
    use std::collections::BTreeSet;
    use std::fs;

    struct TempModRoot(PathBuf);

    impl std::ops::Deref for TempModRoot {
        type Target = Path;
        fn deref(&self) -> &Self::Target {
            &self.0
        }
    }

    impl Drop for TempModRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn temp_mod_root(name: &str) -> TempModRoot {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let mut p = std::env::temp_dir();
        p.push(format!(
            "postretro_luau_test_{}_{}_{name}",
            std::process::id(),
            n,
        ));
        fs::create_dir_all(&p).unwrap();
        TempModRoot(p)
    }

    fn assert_exact_string_keys(table: &Table, expected: &[&str]) {
        let actual = table
            .pairs::<String, mlua::Value>()
            .map(|pair| pair.map(|(key, _)| key))
            .collect::<mlua::Result<BTreeSet<_>>>()
            .expect("module table must enumerate string keys");
        let expected = expected
            .iter()
            .map(|name| (*name).to_string())
            .collect::<BTreeSet<_>>();
        assert_eq!(actual, expected);
    }

    #[test]
    fn resolve_require_path_rejects_backslash_path() {
        // Backslashes never appear in valid mod-root-relative paths. Rejecting
        // at the string level catches Windows-style `..\escape` traversals
        // that the Unix `Component::ParentDir` scan would silently accept.
        let mod_root = Path::new("/tmp/mod");
        let err = resolve_require_path(mod_root, "..\\escape")
            .expect_err("backslash path must be rejected");
        assert!(err.contains("backslash"), "got: {err}");
    }

    #[test]
    fn resolve_require_path_rejects_literal_dotdot_string() {
        let mod_root = Path::new("/tmp/mod");
        let err = resolve_require_path(mod_root, "../escape")
            .expect_err("`..` traversal must be rejected");
        assert!(err.contains(".."), "got: {err}");
    }

    #[test]
    fn require_tracker_records_unique_sorted_canonical_paths() {
        let dir = temp_mod_root("require_tracker");
        fs::create_dir_all(dir.join("modules")).unwrap();
        fs::write(dir.join("modules/a.luau"), "return { name = 'a' }\n").unwrap();
        fs::write(dir.join("modules/b.luau"), "return { name = 'b' }\n").unwrap();

        let tracker = LuauRequireTracker::new(&dir).unwrap();
        let lua = build_lua_state_with_require_tracking(&[], None, Some(&dir), Some(&tracker))
            .expect("lua state");
        lua.load(
            r#"
            local a1 = require("./modules/a")
            local b = require("./modules/b")
            local a2 = require("./modules/a.luau")
            assert(a1.name == "a")
            assert(a2.name == "a")
            assert(b.name == "b")
            "#,
        )
        .exec()
        .unwrap();

        let expected_a = dir.join("modules/a.luau").canonicalize().unwrap();
        let expected_b = dir.join("modules/b.luau").canonicalize().unwrap();
        assert_eq!(tracker.dependency_paths(), vec![expected_a, expected_b]);
    }

    #[test]
    fn virtual_require_returns_readonly_singleton_before_filesystem_and_tracking() {
        let dir = temp_mod_root("virtual_require");
        fs::create_dir_all(dir.join("engine")).unwrap();
        fs::write(
            dir.join("engine/test.luau"),
            "return { source = 'file', nested = { label = 'file' } }\n",
        )
        .unwrap();

        let lua = Lua::new();
        let registry = LuauVirtualModuleRegistry::new();
        let exports = lua.create_table().unwrap();
        exports.set("source", "virtual").unwrap();
        let nested = lua.create_table().unwrap();
        nested.set("label", "owned").unwrap();
        exports.set("nested", nested.clone()).unwrap();
        lua.globals().set("bareNested", nested.clone()).unwrap();
        registry
            .register_from_table(&lua, "engine/test", exports)
            .expect("register virtual module");

        let tracker = LuauRequireTracker::new(&dir).unwrap();
        install_require_resolver(&lua, &dir, Some(&tracker), &registry).unwrap();

        let (source, same, module_write_ok, nested_write_ok, file_source): (
            String,
            bool,
            bool,
            bool,
            String,
        ) = lua
            .load(
                r#"
                local first = require("engine/test")
                local second = require("engine/test")
                local moduleWriteOk = pcall(function()
                    first.source = "mutated"
                end)
                local nestedWriteOk = pcall(function()
                    first.nested.label = "mutated"
                end)
                local file = require("./engine/test")
                return first.source, first == second, moduleWriteOk, nestedWriteOk, file.source
                "#,
            )
            .eval()
            .unwrap();

        assert_eq!(source, "virtual");
        assert!(same, "repeated virtual require must return the same table");
        assert!(
            !module_write_ok,
            "virtual module table must reject script writes"
        );
        assert!(
            !nested_write_ok,
            "nested virtual module tables must reject script writes"
        );
        assert_eq!(
            file_source, "file",
            "virtual lookup must be exact, not normalized from ./ paths"
        );

        let expected_file = dir.join("engine/test.luau").canonicalize().unwrap();
        assert_eq!(tracker.dependency_paths(), vec![expected_file]);

        nested
            .set("label", "bare-global-still-mutable")
            .expect("registering virtual module must not freeze source tables");
        let bare_label: String = lua.load("return bareNested.label").eval().unwrap();
        assert_eq!(bare_label, "bare-global-still-mutable");
    }

    #[test]
    fn sdk_virtual_modules_expose_runtime_inventory_and_cannot_be_shadowed() {
        let dir = temp_mod_root("sdk_virtual_modules");
        fs::create_dir_all(dir.join("postretro")).unwrap();
        fs::write(dir.join("postretro.luau"), "return { shadow = true }\n").unwrap();
        fs::write(
            dir.join("postretro/ui.luau"),
            "return { Text = function() return { shadow = true } end, shadow = true }\n",
        )
        .unwrap();

        let tracker = LuauRequireTracker::new(&dir).unwrap();
        let lua = build_lua_state_with_require_tracking(&[], None, Some(&dir), Some(&tracker))
            .expect("lua state");

        let (
            root,
            ui,
            same_root,
            same_ui,
            module_write_ok,
            nested_write_ok,
            root_nested_write_ok,
        ): (Table, Table, bool, bool, bool, bool, bool) = lua
            .load(
                r#"
                local root = require("postretro")
                local rootAgain = require("postretro")
                local UI = require("postretro/ui")
                local UIAgain = require("postretro/ui")

                assert(root.shadow == nil, "mod-root postretro.luau shadowed the engine module")
                assert(UI.shadow == nil, "mod-root postretro/ui.luau shadowed the engine module")
                assert(UI.Text({ content = "hello" }).kind == "text", "UI.Text must be the SDK factory")
                assert(root.Text({ content = "hello" }).kind == "text", "root Text must mirror the UI SDK factory")
                assert(UI.getGameState().player.health.slot == "player.health", "UI getGameState must expose refs")
                assert(type(ui) == "table", "bare global ui must remain installed")
                assert(type(ui.createLocalState) == "function", "bare global ui.createLocalState must still work")
                assert(root.ui ~= ui, "root module must own a copy of the ui namespace table")
                assert(UI.ui ~= ui, "ui module must own a copy of the ui namespace table")

                local moduleWriteOk = pcall(function()
                  UI.Text = nil
                end)
                local nestedWriteOk = pcall(function()
                  UI.ui.createLocalState = nil
                end)
                local rootNestedWriteOk = pcall(function()
                  root.world.extra = true
                end)

                return
                  root,
                  UI,
                  root == rootAgain,
                  UI == UIAgain,
                  moduleWriteOk,
                  nestedWriteOk,
                  rootNestedWriteOk
                "#,
            )
            .eval()
            .unwrap();

        assert_exact_string_keys(&root, POSTRETRO_ROOT_MODULE_EXPORTS);
        assert_exact_string_keys(&ui, POSTRETRO_UI_MODULE_EXPORTS);

        assert!(
            same_root,
            "repeated root require must return the same table"
        );
        assert!(same_ui, "repeated UI require must return the same table");
        assert!(
            !module_write_ok,
            "postretro/ui module table must reject writes"
        );
        assert!(
            !nested_write_ok,
            "postretro/ui nested namespace tables must reject writes"
        );
        assert!(
            !root_nested_write_ok,
            "postretro nested namespace tables must reject writes"
        );
        assert_eq!(
            tracker.dependency_paths(),
            Vec::<PathBuf>::new(),
            "virtual SDK requires must not be recorded as file dependencies"
        );
    }

    #[test]
    fn require_tracker_rejects_canonical_path_outside_mod_root() {
        let dir = temp_mod_root("require_tracker_root");
        let outside = temp_mod_root("require_tracker_outside");
        let outside_file = outside.join("module.luau");
        fs::write(&outside_file, "return {}\n").unwrap();

        let err =
            canonical_require_dependency(&dir.canonicalize().unwrap(), &outside_file, "./module")
                .expect_err("outside canonical path must be rejected");
        assert!(err.contains("outside mod root"), "got: {err}");
    }
}
