// mlua/Luau subsystem: one sandboxed `mlua::Lua` definition state, driven by
// the shared primitive registry.
// See: context/lib/scripting.md
//
// Mirrors `QuickJsSubsystem` so `ScriptRuntime` can fan out symmetrically by
// file extension.

use std::cell::RefCell;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;
use std::rc::Rc;

use mlua::{Compiler, Function, Lua, Table};

use super::error::ScriptError;
use super::luau_require::{LuauRequireTracker, install_require_resolver};
use super::luau_virtual_modules::LuauVirtualModuleRegistry;
use super::primitives_registry::{PrimitiveRegistry, ScriptPrimitive};
use super::quickjs::{ArchetypeAccumulator, ArchetypeDescriptor};

/// Engine-internal sink function installed into the definition Lua state.
/// Leading underscore: the type-def generator skips names starting with `_`.
const COLLECT_FN_NAME: &str = "__collect_definitions";

/// Deny-list: global names (and `os.<sub>` fields) we clear before any script
/// runs. `sandbox(true)` makes `_G` read-only but does NOT remove these
/// entries — the sandbox is about immutability, not capabilities. These APIs
/// are blocked because they break sandboxing (filesystem, process, or
/// wall-clock access that bypasses engine control).
const DENIED_GLOBALS: &[&str] = &["io", "package", "require", "dofile", "loadfile", "load"];
/// Sub-fields of the `os` table we nil out. `os.time` and `os.clock` expose
/// a free-running wall clock; `os.date` exposes the same surface in string
/// form. All are blocked to keep scripts sandboxed.
const DENIED_OS_FIELDS: &[&str] = &["execute", "exit", "getenv", "time", "clock", "date"];

/// Configuration for a [`LuauSubsystem`].
#[derive(Clone, Copy, Debug, Default)]
pub struct LuauConfig {}

/// Luau subsystem: one `Lua` definition state, plus the primitive snapshot.
/// Fields mirror `QuickJsSubsystem` one-for-one.
pub struct LuauSubsystem {
    definition_lua: Lua,
    primitives: Vec<ScriptPrimitive>,
    archetypes: ArchetypeAccumulator,
}

/// Scope selector passed to `run_source` and to the top-level dispatcher.
/// Lives here rather than in `runtime.rs` so the subsystem can be exercised
/// in isolation from tests.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Which {
    Definition,
}

impl LuauSubsystem {
    /// Construct a subsystem: build the definition Lua state, install the
    /// primitive set, and install the archetype sink. Order is load-bearing:
    ///
    ///   1. `Lua::new()` (luau feature active).
    ///   2. Scrub deny-list globals (write `nil` into `_G` entries; clear
    ///      listed sub-fields of `os`).
    ///   3. Install the `print` redirect forwarding to `log::info!` with the
    ///      `[Script/Luau]` prefix. Must be before `sandbox(true)` — once
    ///      sandboxed, `_G` is read-only.
    ///   4. Install primitives.
    ///   5. `lua.sandbox(true)?` — freezes `_G` and moves subsequent globals
    ///      to a per-thread sandbox table.
    ///
    /// # Sandboxing caveats
    ///
    /// * `sandbox(true)` makes `_G` read-only but does NOT prevent
    ///   script-owned table mutation, field writes on script-created tables,
    ///   or coroutine-local writes. Scripts that want to stash state just
    ///   create their own tables.
    /// * Deny-list removal is what actually prevents filesystem / process
    ///   access. `sandbox(true)` alone would still leave `io.open`
    ///   available.
    /// * Coroutines are permitted. Cross-frame suspension is UNDEFINED —
    ///   there is no Rust-side scheduler to resume a yielded coroutine on a
    ///   later frame. Coroutines that start and finish within one primitive
    ///   call are safe.
    pub fn new(registry: &PrimitiveRegistry, _cfg: &LuauConfig) -> Result<Self, ScriptError> {
        let archetypes: ArchetypeAccumulator = Rc::new(RefCell::new(Vec::new()));
        let primitives_snapshot: Vec<ScriptPrimitive> = registry.iter().cloned().collect();

        // The long-lived definition state has no mod root — `require` stays
        // nil'd-out by the deny-list. Mod-init and per-level data-context VMs
        // are short-lived and built separately with their own mod root.
        let definition_lua = build_lua_state(&primitives_snapshot, Some(&archetypes), None)?;

        Ok(Self {
            definition_lua,
            primitives: primitives_snapshot,
            archetypes,
        })
    }

    /// Borrow the definition Lua state.
    pub fn definition_lua(&self) -> &Lua {
        &self.definition_lua
    }

    /// Borrow the primitive snapshot.
    pub fn primitives(&self) -> &[ScriptPrimitive] {
        &self.primitives
    }

    /// Shared handle to the archetype accumulator. Exposed for tests and for
    /// the caller that drains it after evaluating definition scripts.
    pub fn archetypes(&self) -> &ArchetypeAccumulator {
        &self.archetypes
    }

    /// Compile and evaluate `source` inside the chosen state. Compile errors
    /// surface as `ScriptError::ScriptThrew` (same variant as runtime errors).
    /// Runtime errors include mlua's source-line traceback because
    /// `mlua::Error`'s `Display` impl embeds it.
    pub fn run_source<T>(&self, which: Which, source: &str, name: &str) -> Result<T, ScriptError>
    where
        T: mlua::FromLuaMulti,
    {
        let lua = match which {
            Which::Definition => &self.definition_lua,
        };

        // Compile step — surfaces as SyntaxError (or InvalidArgument for the
        // rare internal failure). Logged at `error!` before we throw away the
        // mlua error type.
        let bytecode = Compiler::new().compile(source).map_err(|e| {
            let msg = e.to_string();
            log::error!(
                target: "script/luau",
                "failed to compile `{name}`: {msg}",
            );
            ScriptError::ScriptThrew {
                msg,
                source_name: name.to_string(),
            }
        })?;

        // Run the compiled bytecode. `set_name` gives us a useful traceback
        // prefix even though we're loading binary chunks.
        lua.load(&bytecode)
            .set_name(name)
            .set_mode(mlua::ChunkMode::Binary)
            .eval::<T>()
            .map_err(|e| {
                // mlua's Display impl for CallbackError / RuntimeError already
                // embeds the traceback — just format the error and go.
                let msg = e.to_string();
                log::error!(
                    target: "script/luau",
                    "script `{name}` threw: {msg}",
                );
                ScriptError::ScriptThrew {
                    msg,
                    source_name: name.to_string(),
                }
            })
    }
}

// ---------------------------------------------------------------------------
// Construction helpers.

/// Construct an mlua Lua state with the deny-list applied, the `print` redirect
/// installed, the SDK prelude evaluated, and primitives installed.
///
/// `mod_root`, when `Some`, installs a `require` global resolved against the
/// mod root (see [`install_require_resolver`]). When `None`, `require` stays
/// nil'd-out by the deny-list — appropriate for the long-lived definition
/// state and for any helper VM that has no associated mod.
pub fn build_lua_state(
    primitives: &[ScriptPrimitive],
    archetypes: Option<&ArchetypeAccumulator>,
    mod_root: Option<&Path>,
) -> Result<Lua, ScriptError> {
    build_lua_state_with_require_tracking(primitives, archetypes, mod_root, None)
}

pub fn build_lua_state_with_require_tracking(
    primitives: &[ScriptPrimitive],
    archetypes: Option<&ArchetypeAccumulator>,
    mod_root: Option<&Path>,
    require_tracker: Option<&LuauRequireTracker>,
) -> Result<Lua, ScriptError> {
    let lua = Lua::new();
    let virtual_modules = if mod_root.is_some() {
        Some(LuauVirtualModuleRegistry::new())
    } else {
        None
    };

    // 1. Deny-list scrub.
    apply_denylist(&lua)?;

    // 2. `print` redirect — MUST happen before `sandbox(true)`, because
    // sandbox freezes `_G` and any subsequent `globals().set` would fail.
    install_print_redirect(&lua)?;

    // 3. Install primitives.
    install_primitives(&lua, primitives)?;

    // 4. Archetype sink into the definition state only.
    if let Some(accum) = archetypes {
        install_collect_definitions(&lua, accum.clone())?;
    }

    // 5. Mod-rooted `require` resolver. Installed after the deny-list scrub
    //    overwrites the inherited `require` slot with `nil`, and before the SDK
    //    prelude (step 6) so that any prelude chunk that calls `require` resolves
    //    correctly. Without a mod root, `require` stays nil — matching the
    //    definition-context contract.
    if let Some(root) = mod_root {
        let virtual_modules = virtual_modules
            .as_ref()
            .expect("mod-rooted Lua states always allocate virtual modules");
        install_require_resolver(&lua, root, require_tracker, virtual_modules)?;
    }

    // 6. SDK prelude — installs `world`, `timeline`, `sequence`,
    //    `defineReaction`, and emitter constructors as bare globals.
    //    Capability methods (pulse, fade, flicker, etc.) live on handles
    //    returned by `world:query`; they are not bare globals.
    //    Must run before `sandbox(true)` because the prelude writes to `_G`,
    //    and after primitive install because the prelude calls them.
    super::luau_prelude::evaluate_prelude(&lua, virtual_modules.as_ref())?;

    // 7. Freeze `_G`.
    lua.sandbox(true)
        .map_err(|e| ScriptError::InvalidArgument {
            reason: e.to_string(),
        })?;

    Ok(lua)
}

fn apply_denylist(lua: &Lua) -> Result<(), ScriptError> {
    let globals = lua.globals();
    for name in DENIED_GLOBALS {
        globals
            .set(*name, mlua::Value::Nil)
            .map_err(|e| ScriptError::InvalidArgument {
                reason: e.to_string(),
            })?;
    }
    // `os` stays, but unsafe sub-fields go. If the `os` table is somehow
    // missing (custom builds), that's fine — there's nothing to clear.
    if let Ok(os_table) = globals.get::<Table>("os") {
        for field in DENIED_OS_FIELDS {
            os_table
                .set(*field, mlua::Value::Nil)
                .map_err(|e| ScriptError::InvalidArgument {
                    reason: e.to_string(),
                })?;
        }
    }
    Ok(())
}

fn install_print_redirect(lua: &Lua) -> Result<(), ScriptError> {
    let f = lua
        .create_function(|_lua, args: mlua::MultiValue| {
            const NAME: &str = "print";
            let result = catch_unwind(AssertUnwindSafe(|| {
                // Lua's `print` separates values with tabs. Mirror that here so
                // existing debug habits transfer cleanly to the log line.
                let mut out = String::new();
                for (i, v) in args.iter().enumerate() {
                    if i > 0 {
                        out.push('\t');
                    }
                    match v.to_string() {
                        Ok(s) => out.push_str(&s),
                        Err(_) => out.push_str("<unprintable>"),
                    }
                }
                log::info!(target: "script/luau", "[Script/Luau] {out}");
            }));
            match result {
                Ok(()) => Ok(()),
                Err(_) => {
                    log::error!(target: "script/luau", "[Scripting] print closure panicked: {NAME}");
                    Err(mlua::Error::RuntimeError(format!("panic in print: {NAME}")))
                }
            }
        })
        .map_err(|e| ScriptError::InvalidArgument {
            reason: e.to_string(),
        })?;
    lua.globals()
        .set("print", f)
        .map_err(|e| ScriptError::InvalidArgument {
            reason: e.to_string(),
        })?;
    Ok(())
}

fn install_primitives(lua: &Lua, primitives: &[ScriptPrimitive]) -> Result<(), ScriptError> {
    for p in primitives {
        (p.luau_installer)(lua).map_err(|e| ScriptError::InvalidArgument {
            reason: e.to_string(),
        })?;
    }
    Ok(())
}

fn install_collect_definitions(
    lua: &Lua,
    archetypes: ArchetypeAccumulator,
) -> Result<(), ScriptError> {
    let f: Function = lua
        .create_function(move |lua, ()| {
            let result = catch_unwind(AssertUnwindSafe(|| {
                archetypes
                    .borrow_mut()
                    .drain(..)
                    .collect::<Vec<ArchetypeDescriptor>>()
            }));
            match result {
                Ok(drained) => {
                    let t = lua.create_table()?;
                    for (i, d) in drained.into_iter().enumerate() {
                        let row = lua.create_table()?;
                        row.set("name", d.name)?;
                        // Lua is 1-indexed.
                        t.set(i + 1, row)?;
                    }
                    Ok(t)
                }
                Err(_) => {
                    let err = ScriptError::Panicked {
                        name: COLLECT_FN_NAME,
                    };
                    Err(mlua::Error::RuntimeError(err.to_string()))
                }
            }
        })
        .map_err(|e| ScriptError::InvalidArgument {
            reason: e.to_string(),
        })?;
    lua.globals()
        .set(COLLECT_FN_NAME, f)
        .map_err(|e| ScriptError::InvalidArgument {
            reason: e.to_string(),
        })?;
    Ok(())
}
