// mlua/Luau subsystem: one sandboxed `mlua::Lua` definition state, driven by
// the shared primitive registry.
// See: context/lib/scripting.md
//
// Mirrors `QuickJsSubsystem` so `ScriptRuntime` can fan out symmetrically by
// file extension.

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use mlua::{Compiler, Function, Lua, Table};

use super::error::ScriptError;
use super::primitives_registry::{PrimitiveRegistry, ScriptPrimitive};
use super::quickjs::{ArchetypeAccumulator, ArchetypeDescriptor};

/// Engine-internal sink function installed into the definition Lua state.
/// Leading underscore: the type-def generator skips names starting with `_`.
const COLLECT_FN_NAME: &str = "__collect_definitions";

/// SDK library prelude — `world.luau` returns the `world` table; we promote
/// it to global `world`. Embedded at compile time; SDK changes require an
/// engine rebuild.
const WORLD_LUAU_SRC: &str = include_str!("../../../../sdk/lib/world.luau");

/// SDK library prelude — `entities/lights.luau` returns a table whose only
/// promoted field is `wrapLightEntity`, installed as a temporary global for
/// `world.luau` to capture and then nil'd out before the sandbox freezes.
/// Capability methods (`pulse`, `fade`, `flicker`, `colorShift`, `sweep`)
/// live on the handle returned from `wrapLightEntity`; no bare globals.
const LIGHTS_LUAU_SRC: &str = include_str!("../../../../sdk/lib/entities/lights.luau");

/// SDK library prelude — `util/keyframes.luau` returns a table whose fields
/// (`timeline`, `sequence`) are destructured into globals.
const KEYFRAMES_LUAU_SRC: &str = include_str!("../../../../sdk/lib/util/keyframes.luau");

/// SDK library prelude — `entities/emitters.luau` returns a table whose fields
/// are destructured into globals so authors can call them by bare name.
const EMITTERS_LUAU_SRC: &str = include_str!("../../../../sdk/lib/entities/emitters.luau");

/// SDK library prelude — `entities/fog_volumes.luau` returns a table whose
/// only promoted field is `wrapFogVolumeEntity`, installed as a temporary
/// global for `world.luau` to capture and then nil'd out before the sandbox
/// freezes. Capability methods (`pulse`, `fade`, `flicker`,
/// `pulseSaturation`, `fadeSaturation`) live on the handle returned from
/// `wrapFogVolumeEntity`; no bare globals.
const FOG_VOLUMES_LUAU_SRC: &str = include_str!("../../../../sdk/lib/entities/fog_volumes.luau");

/// SDK library prelude — `data_script.luau` returns a table whose fields
/// (`defineReaction`, `defineEntity`) are destructured into globals so
/// data-script authors call them by bare name. Pure descriptor builders;
/// no FFI happens until `setupMod` or `setupLevel` returns.
const DATA_SCRIPT_LUAU_SRC: &str = include_str!("../../../../sdk/lib/data_script.luau");

/// SDK library prelude — `runtime.luau` returns the runtime-value builder table,
/// promoted whole to global `runtime` (mirroring `world`). Pure data assembly: a
/// builder assembles a `RuntimeValue` table and never calls back into Rust.
const RUNTIME_LUAU_SRC: &str = include_str!("../../../../sdk/lib/runtime.luau");

/// SDK library prelude — `ui/reactions.luau` returns a table whose field
/// (`onStateCrossing`) is destructured into a global so data-script authors
/// call it by bare name. A pure descriptor builder; its result lands in
/// `setupLevel`'s `crossings` array — no FFI until the script returns.
const UI_REACTIONS_LUAU_SRC: &str = include_str!("../../../../sdk/lib/ui/reactions.luau");

/// Lights SDK fields lifted to globals after evaluating
/// `entities/lights.luau`. Empty: the public vocabulary lives on the handle
/// returned from `wrapLightEntity`, which is itself installed as a
/// temporary bridge (not a bare global) before `world.luau` evaluates and
/// nil'd out afterward.
const LIGHTS_LUAU_FIELDS: &[&str] = &[];

/// Keyframe-utility SDK fields lifted to globals after evaluating
/// `util/keyframes.luau`.
const KEYFRAMES_LUAU_FIELDS: &[&str] = &["timeline", "sequence"];

/// Emitter SDK fields lifted to globals after evaluating
/// `entities/emitters.luau`.
const EMITTERS_LUAU_FIELDS: &[&str] = &["emitter", "smokeEmitter", "sparkEmitter", "dustEmitter"];

/// Fog-volume SDK fields lifted to globals after evaluating
/// `entities/fog_volumes.luau`. Empty: the public vocabulary lives on the
/// handle returned from `wrapFogVolumeEntity`, which is itself installed
/// as a temporary bridge (not a bare global) before `world.luau`
/// evaluates and nil'd out afterward.
const FOG_VOLUMES_LUAU_FIELDS: &[&str] = &[];

/// Data-script SDK fields lifted to globals after evaluating
/// `data_script.luau`.
const DATA_SCRIPT_FIELDS: &[&str] = &["defineReaction", "defineEntity"];

/// UI-reactions SDK fields lifted to globals after evaluating
/// `ui/reactions.luau`. `onStateCrossing` builds a state-crossing watcher; the
/// rest are system-reaction body constructors that pair with `defineReaction` to
/// emit `playSound` / `rumble` / `flashScreen`, the UI-stack (`showDialog` /
/// `openMenu` / `closeDialog`) primitives, the `setState` slot write (Goal F),
/// the text-entry helpers (`openTextEntry` wraps `showDialog` for the engine
/// keyboard; `KEYBOARD_TREE` is its registry name constant), and the text-edit
/// reactions (`appendText` / `backspaceText` / `clearText`, M13 Text Entry).
const UI_REACTIONS_FIELDS: &[&str] = &[
    "onStateCrossing",
    "playSound",
    "rumble",
    "flashScreen",
    "showDialog",
    "openMenu",
    "closeDialog",
    "openTextEntry",
    "KEYBOARD_TREE",
    "setState",
    "appendText",
    "backspaceText",
    "clearText",
];

/// Evaluate the Luau SDK prelude in `lua` and promote the return values to
/// globals. Must be called after primitives are installed and before
/// `sandbox(true)` (which freezes `_G`). The primitive dependency applies
/// to `entities/lights.luau`, `world.luau`, and `fog_volumes.luau` — they
/// reference primitives like `worldQuery` and `setLightAnimation`.
/// `data_script.luau` is also evaluated as a prelude step but has no
/// primitive dependencies; it's pure data builders (`defineReaction`,
/// `defineEntity`).
/// The prelude source uses type annotations declared in postretro.d.luau (luau-lsp only); the runtime evaluates the .luau source without loading the declaration file.
pub(crate) fn evaluate_prelude(lua: &Lua) -> Result<(), ScriptError> {
    // Step 1: evaluate `entities/lights.luau`. The only exported field is
    // the `wrapLightEntity` bridge — capability methods (`pulse`, `fade`,
    // `flicker`, `colorShift`, `sweep`) live on the handle it produces,
    // not as bare globals. `wrapLightEntity` itself is installed below as
    // a temporary global so `world.luau` can capture it as an upvalue,
    // then nil'd out in step 4.
    let lights_sdk: Table = lua
        .load(LIGHTS_LUAU_SRC)
        .set_name("postretro/sdk/entities/lights.luau")
        .eval()
        .map_err(|e| ScriptError::ScriptThrew {
            msg: format!("failed to evaluate SDK prelude `entities/lights.luau`: {e}"),
            source_name: "sdk/lib/entities/lights.luau".to_string(),
        })?;
    let globals = lua.globals();
    let wrap_light_entity: mlua::Value =
        lights_sdk
            .get("wrapLightEntity")
            .map_err(|e| ScriptError::InvalidArgument {
                reason: format!("entities/lights.luau missing `wrapLightEntity`: {e}"),
            })?;
    globals
        .set("wrapLightEntity", wrap_light_entity)
        .map_err(|e| ScriptError::InvalidArgument {
            reason: format!("failed to install temporary global `wrapLightEntity`: {e}"),
        })?;

    // Step 2: install the public lights fields as globals.
    // `LIGHTS_LUAU_FIELDS` is empty in the capability-handle world; the
    // loop is retained so adding a future bare global is a one-line
    // change in the slice declaration.
    for field in LIGHTS_LUAU_FIELDS {
        let value: mlua::Value =
            lights_sdk
                .get(*field)
                .map_err(|e| ScriptError::InvalidArgument {
                    reason: format!("entities/lights.luau missing `{field}`: {e}"),
                })?;
        globals
            .set(*field, value)
            .map_err(|e| ScriptError::InvalidArgument {
                reason: format!("failed to install global `{field}`: {e}"),
            })?;
    }

    // Step 2b: evaluate `entities/fog_volumes.luau`. Mirrors lights.luau:
    // the only exported field is `wrapFogVolumeEntity`. Capability
    // methods (`pulse`, `fade`, `flicker`, `pulseSaturation`,
    // `fadeSaturation`) live on the handle, not as bare globals.
    let fog_volumes_sdk: Table = lua
        .load(FOG_VOLUMES_LUAU_SRC)
        .set_name("postretro/sdk/entities/fog_volumes.luau")
        .eval()
        .map_err(|e| ScriptError::ScriptThrew {
            msg: format!("failed to evaluate SDK prelude `entities/fog_volumes.luau`: {e}"),
            source_name: "sdk/lib/entities/fog_volumes.luau".to_string(),
        })?;
    let wrap_fog_volume_entity: mlua::Value =
        fog_volumes_sdk
            .get("wrapFogVolumeEntity")
            .map_err(|e| ScriptError::InvalidArgument {
                reason: format!("entities/fog_volumes.luau missing `wrapFogVolumeEntity`: {e}"),
            })?;
    globals
        .set("wrapFogVolumeEntity", wrap_fog_volume_entity)
        .map_err(|e| ScriptError::InvalidArgument {
            reason: format!("failed to install temporary global `wrapFogVolumeEntity`: {e}"),
        })?;
    for field in FOG_VOLUMES_LUAU_FIELDS {
        let value: mlua::Value =
            fog_volumes_sdk
                .get(*field)
                .map_err(|e| ScriptError::InvalidArgument {
                    reason: format!("entities/fog_volumes.luau missing `{field}`: {e}"),
                })?;
        globals
            .set(*field, value)
            .map_err(|e| ScriptError::InvalidArgument {
                reason: format!("failed to install global `{field}`: {e}"),
            })?;
    }

    // Step 3: evaluate `world.luau`. Its `query` closure captures
    // `wrapLightEntity` and `wrapFogVolumeEntity` as upvalues at evaluation
    // time, so step 4's nil-out does not break the closure.
    let world: mlua::Value = lua
        .load(WORLD_LUAU_SRC)
        .set_name("postretro/sdk/world.luau")
        .eval()
        .map_err(|e| ScriptError::ScriptThrew {
            msg: format!("failed to evaluate SDK prelude `world.luau`: {e}"),
            source_name: "sdk/lib/world.luau".to_string(),
        })?;
    globals
        .set("world", world)
        .map_err(|e| ScriptError::InvalidArgument {
            reason: format!("failed to install global `world`: {e}"),
        })?;

    // Step 4: nil out the temporary `wrapLightEntity` / `wrapFogVolumeEntity`
    // bridges so author scripts never see them as bare globals once
    // `sandbox(true)` freezes `_G`.
    globals
        .set("wrapLightEntity", mlua::Value::Nil)
        .map_err(|e| ScriptError::InvalidArgument {
            reason: format!("failed to clear temporary global `wrapLightEntity`: {e}"),
        })?;
    globals
        .set("wrapFogVolumeEntity", mlua::Value::Nil)
        .map_err(|e| ScriptError::InvalidArgument {
            reason: format!("failed to clear temporary global `wrapFogVolumeEntity`: {e}"),
        })?;

    // Step 5: evaluate `util/keyframes.luau` and lift its fields to globals.
    let keyframes_sdk: Table = lua
        .load(KEYFRAMES_LUAU_SRC)
        .set_name("postretro/sdk/util/keyframes.luau")
        .eval()
        .map_err(|e| ScriptError::ScriptThrew {
            msg: format!("failed to evaluate SDK prelude `util/keyframes.luau`: {e}"),
            source_name: "sdk/lib/util/keyframes.luau".to_string(),
        })?;
    for field in KEYFRAMES_LUAU_FIELDS {
        let value: mlua::Value =
            keyframes_sdk
                .get(*field)
                .map_err(|e| ScriptError::InvalidArgument {
                    reason: format!("util/keyframes.luau missing `{field}`: {e}"),
                })?;
        globals
            .set(*field, value)
            .map_err(|e| ScriptError::InvalidArgument {
                reason: format!("failed to install global `{field}`: {e}"),
            })?;
    }

    // Step 6: evaluate `entities/emitters.luau` and lift its fields to globals.
    let emitters_sdk: Table = lua
        .load(EMITTERS_LUAU_SRC)
        .set_name("postretro/sdk/entities/emitters.luau")
        .eval()
        .map_err(|e| ScriptError::ScriptThrew {
            msg: format!("failed to evaluate SDK prelude `entities/emitters.luau`: {e}"),
            source_name: "sdk/lib/entities/emitters.luau".to_string(),
        })?;
    for field in EMITTERS_LUAU_FIELDS {
        let value: mlua::Value =
            emitters_sdk
                .get(*field)
                .map_err(|e| ScriptError::InvalidArgument {
                    reason: format!("entities/emitters.luau missing `{field}`: {e}"),
                })?;
        globals
            .set(*field, value)
            .map_err(|e| ScriptError::InvalidArgument {
                reason: format!("failed to install global `{field}`: {e}"),
            })?;
    }

    // Step 7: evaluate `data_script.luau` and lift its fields to globals.
    let data_sdk: Table = lua
        .load(DATA_SCRIPT_LUAU_SRC)
        .set_name("postretro/sdk/data_script.luau")
        .eval()
        .map_err(|e| ScriptError::ScriptThrew {
            msg: format!("failed to evaluate SDK prelude `data_script.luau`: {e}"),
            source_name: "sdk/lib/data_script.luau".to_string(),
        })?;
    for field in DATA_SCRIPT_FIELDS {
        let value: mlua::Value =
            data_sdk
                .get(*field)
                .map_err(|e| ScriptError::InvalidArgument {
                    reason: format!("data_script.luau missing `{field}`: {e}"),
                })?;
        globals
            .set(*field, value)
            .map_err(|e| ScriptError::InvalidArgument {
                reason: format!("failed to install global `{field}`: {e}"),
            })?;
    }

    // Step 7b: evaluate `ui/reactions.luau` and lift its fields to globals.
    // Pure builders (no primitive dependency); ordering relative to the other
    // steps is irrelevant.
    let ui_reactions_sdk: Table = lua
        .load(UI_REACTIONS_LUAU_SRC)
        .set_name("postretro/sdk/ui/reactions.luau")
        .eval()
        .map_err(|e| ScriptError::ScriptThrew {
            msg: format!("failed to evaluate SDK prelude `ui/reactions.luau`: {e}"),
            source_name: "sdk/lib/ui/reactions.luau".to_string(),
        })?;
    for field in UI_REACTIONS_FIELDS {
        let value: mlua::Value =
            ui_reactions_sdk
                .get(*field)
                .map_err(|e| ScriptError::InvalidArgument {
                    reason: format!("ui/reactions.luau missing `{field}`: {e}"),
                })?;
        globals
            .set(*field, value)
            .map_err(|e| ScriptError::InvalidArgument {
                reason: format!("failed to install global `{field}`: {e}"),
            })?;
    }

    // Step 8: evaluate `runtime.luau` and promote its table to global `runtime`.
    // The builders are pure (no primitive dependency), so ordering relative to
    // the other steps is irrelevant.
    let runtime: mlua::Value = lua
        .load(RUNTIME_LUAU_SRC)
        .set_name("postretro/sdk/runtime.luau")
        .eval()
        .map_err(|e| ScriptError::ScriptThrew {
            msg: format!("failed to evaluate SDK prelude `runtime.luau`: {e}"),
            source_name: "sdk/lib/runtime.luau".to_string(),
        })?;
    globals
        .set("runtime", runtime)
        .map_err(|e| ScriptError::InvalidArgument {
            reason: format!("failed to install global `runtime`: {e}"),
        })?;

    Ok(())
}

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
pub(crate) struct LuauConfig {}

/// Luau subsystem: one `Lua` definition state, plus the primitive snapshot.
/// Fields mirror `QuickJsSubsystem` one-for-one.
pub(crate) struct LuauSubsystem {
    definition_lua: Lua,
    primitives: Vec<ScriptPrimitive>,
    archetypes: ArchetypeAccumulator,
}

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

/// Scope selector passed to `run_source` and to the top-level dispatcher.
/// Lives here rather than in `runtime.rs` so the subsystem can be exercised
/// in isolation from tests.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Which {
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
    pub(crate) fn new(
        registry: &PrimitiveRegistry,
        _cfg: &LuauConfig,
    ) -> Result<Self, ScriptError> {
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
    pub(crate) fn definition_lua(&self) -> &Lua {
        &self.definition_lua
    }

    /// Borrow the primitive snapshot.
    pub(crate) fn primitives(&self) -> &[ScriptPrimitive] {
        &self.primitives
    }

    /// Shared handle to the archetype accumulator. Exposed for tests and for
    /// the caller that drains it after evaluating definition scripts.
    pub(crate) fn archetypes(&self) -> &ArchetypeAccumulator {
        &self.archetypes
    }

    /// Compile and evaluate `source` inside the chosen state. Compile errors
    /// surface as `ScriptError::ScriptThrew` (same variant as runtime errors).
    /// Runtime errors include mlua's source-line traceback because
    /// `mlua::Error`'s `Display` impl embeds it.
    pub(crate) fn run_source<T>(
        &self,
        which: Which,
        source: &str,
        name: &str,
    ) -> Result<T, ScriptError>
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
pub(crate) fn build_lua_state(
    primitives: &[ScriptPrimitive],
    archetypes: Option<&ArchetypeAccumulator>,
    mod_root: Option<&Path>,
) -> Result<Lua, ScriptError> {
    build_lua_state_with_require_tracking(primitives, archetypes, mod_root, None)
}

pub(crate) fn build_lua_state_with_require_tracking(
    primitives: &[ScriptPrimitive],
    archetypes: Option<&ArchetypeAccumulator>,
    mod_root: Option<&Path>,
    require_tracker: Option<&LuauRequireTracker>,
) -> Result<Lua, ScriptError> {
    let lua = Lua::new();

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
        install_require_resolver(&lua, root, require_tracker)?;
    }

    // 6. SDK prelude — installs `world`, `timeline`, `sequence`,
    //    `defineReaction`, and emitter constructors as bare globals.
    //    Capability methods (pulse, fade, flicker, etc.) live on handles
    //    returned by `world:query`; they are not bare globals.
    //    Must run before `sandbox(true)` because the prelude writes to `_G`,
    //    and after primitive install because the prelude calls them.
    evaluate_prelude(&lua)?;

    // 7. Freeze `_G`.
    lua.sandbox(true)
        .map_err(|e| ScriptError::InvalidArgument {
            reason: e.to_string(),
        })?;

    Ok(lua)
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
fn install_require_resolver(
    lua: &Lua,
    mod_root: &Path,
    require_tracker: Option<&LuauRequireTracker>,
) -> Result<(), ScriptError> {
    let mod_root: PathBuf = mod_root.to_path_buf();
    let tracker = require_tracker.cloned();
    let f = lua
        .create_function(move |lua, path: String| -> mlua::Result<mlua::Value> {
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
    // TODO: symlink traversal is not checked — a symlink planted at the mod
    // root (e.g. `<mod_root>/link -> /etc`) would still pass the checks above
    // and resolve to an arbitrary path. Add `canonicalize` + `starts_with(mod_root)`
    // after canonicalization when symlink-safe require is needed.
    Ok(joined)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scripting::ctx::ScriptCtx;
    use crate::scripting::primitives::register_all;
    use crate::scripting::primitives_registry::ContextScope;
    use std::fs;

    // 15 `type(...)` strings returned by `sdk_prelude_installs_globals`.
    type PreludeTypeNames = (
        String,
        String,
        String,
        String,
        String,
        String,
        String,
        String,
        String,
        String,
        String,
        String,
        String,
        String,
        String,
    );

    fn setup() -> (LuauSubsystem, ScriptCtx) {
        let ctx = ScriptCtx::new();
        let mut registry = PrimitiveRegistry::new();
        register_all(&mut registry, ctx.clone());
        let subsys = LuauSubsystem::new(&registry, &LuauConfig::default()).unwrap();
        (subsys, ctx)
    }

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

    #[test]
    fn new_constructs_definition_state() {
        let (subsys, _ctx) = setup();
        let v: u32 = subsys
            .run_source(Which::Definition, "return 1 + 2", "def.luau")
            .unwrap();
        assert_eq!(v, 3);
    }

    #[test]
    fn print_redirect_is_installed() {
        // We don't have log-capture plumbed in the test harness; the contract
        // here is that the function is bound. A human can run with
        // `RUST_LOG=info cargo test -- --nocapture` to see the output.
        let (subsys, _ctx) = setup();
        subsys
            .run_source::<()>(
                Which::Definition,
                r#"
                assert(type(print) == "function", "print must be a function")
                print("hello from luau")
                return
                "#,
                "print.luau",
            )
            .unwrap();
    }

    #[test]
    fn denylist_covers_all_names() {
        // `io`, `os.execute`, `os.exit`, `os.getenv`, `os.time`, `os.clock`,
        // `os.date`, `package`, `require`, `dofile`, `loadfile`, `load`.
        // All must be nil when accessed from a script.
        let (subsys, _ctx) = setup();
        let results: mlua::MultiValue = subsys
            .run_source(
                Which::Definition,
                r#"
                return
                  io == nil,
                  os.execute == nil,
                  os.exit == nil,
                  os.getenv == nil,
                  os.time == nil,
                  os.clock == nil,
                  os.date == nil,
                  package == nil,
                  require == nil,
                  dofile == nil,
                  loadfile == nil,
                  load == nil
                "#,
                "denylist.luau",
            )
            .unwrap();
        let flags: Vec<bool> = results
            .into_iter()
            .map(|v| match v {
                mlua::Value::Boolean(b) => b,
                other => panic!("expected boolean, got {other:?}"),
            })
            .collect();
        assert_eq!(flags.len(), 12, "expected 12 denylist checks");
        for (i, f) in flags.iter().enumerate() {
            assert!(f, "denylist entry {i} is still reachable");
        }
    }

    #[test]
    fn run_source_returns_script_threw_on_runtime_error() {
        let (subsys, _ctx) = setup();
        let err = subsys
            .run_source::<()>(Which::Definition, "error('boom')", "test.luau")
            .expect_err("script should error");
        match err {
            ScriptError::ScriptThrew { msg, source_name } => {
                assert_eq!(source_name, "test.luau");
                assert!(msg.contains("boom"), "msg: {msg}");
            }
            other => panic!("expected ScriptThrew, got {other:?}"),
        }
        // State must still be usable.
        let v: u32 = subsys
            .run_source(Which::Definition, "return 1 + 1", "after.luau")
            .unwrap();
        assert_eq!(v, 2);
    }

    #[test]
    fn run_source_returns_script_threw_on_compile_error() {
        let (subsys, _ctx) = setup();
        let err = subsys
            .run_source::<()>(Which::Definition, "this is not valid luau ===", "bad.luau")
            .expect_err("compile should fail");
        match err {
            ScriptError::ScriptThrew { source_name, .. } => {
                assert_eq!(source_name, "bad.luau");
            }
            other => panic!("expected ScriptThrew, got {other:?}"),
        }
    }

    #[test]
    fn panicking_primitive_does_not_unwind_past_ffi_boundary() {
        let mut registry = PrimitiveRegistry::new();
        registry
            .register("boom", || -> Result<u32, ScriptError> {
                panic!("intentional");
            })
            .scope(ContextScope::Both)
            .finish();
        let subsys = LuauSubsystem::new(&registry, &LuauConfig::default()).unwrap();
        let (ok, msg): (bool, String) = subsys
            .run_source(
                Which::Definition,
                r#"
                local ok, err = pcall(boom)
                return ok, tostring(err)
                "#,
                "panic.luau",
            )
            .unwrap();
        assert!(!ok);
        assert!(msg.contains("panicked"), "got: {msg}");
    }

    #[test]
    fn collect_definitions_round_trips_through_accumulator() {
        let (subsys, _ctx) = setup();
        let archetypes = subsys.archetypes().clone();

        // Push test-only defineEntity stub that writes into the same
        // accumulator. Done before any script runs and before sandbox would
        // have frozen _G... but sandbox is already active on `definition_lua`.
        // To inject a test-only function, we install it through the sandbox
        // by mutating the thread-local globals table directly — mlua permits
        // host-side `globals().set` post-sandbox; it's script writes that
        // are frozen.
        let accum = archetypes.clone();
        let lua = subsys.definition_lua();
        let define = lua
            .create_function(move |_, name: String| {
                accum.borrow_mut().push(ArchetypeDescriptor { name });
                Ok(())
            })
            .unwrap();
        lua.globals().set("defineEntity", define).unwrap();

        let names: Vec<String> = subsys
            .run_source(
                Which::Definition,
                r#"
                defineEntity("goblin")
                defineEntity("orc")
                defineEntity("troll")
                local out = __collect_definitions()
                local names = {}
                for i, d in ipairs(out) do
                    names[i] = d.name
                end
                return names
                "#,
                "collect.luau",
            )
            .unwrap();
        assert_eq!(names, vec!["goblin", "orc", "troll"]);
        assert!(archetypes.borrow().is_empty());
    }

    #[test]
    fn sdk_prelude_installs_globals() {
        // Verifies the SDK prelude shape after the capability-handle refactor:
        // - `world`, `timeline`, `sequence`, and the emitter constructors stay
        //   as bare globals;
        // - `flicker`, `pulse`, `colorShift`, `sweep`, `fogPulse`, `fogFade`
        //   are NO LONGER bare globals — they are handle methods now;
        // - the temporary bridges (`wrapLightEntity`, `wrapFogVolumeEntity`)
        //   are nil by the time author scripts run.
        let (subsys, _ctx) = setup();
        {
            let which = Which::Definition;
            let (
                world_ty,
                flicker_ty,
                pulse_ty,
                color_ty,
                sweep_ty,
                fog_pulse_ty,
                fog_fade_ty,
                timeline_ty,
                sequence_ty,
                emitter_ty,
                smoke_ty,
                spark_ty,
                dust_ty,
                wrap_light_ty,
                wrap_fog_ty,
            ): PreludeTypeNames = subsys
                .run_source(
                    which,
                    r#"
                    return
                      type(world),
                      type(flicker),
                      type(pulse),
                      type(colorShift),
                      type(sweep),
                      type(fogPulse),
                      type(fogFade),
                      type(timeline),
                      type(sequence),
                      type(emitter),
                      type(smokeEmitter),
                      type(sparkEmitter),
                      type(dustEmitter),
                      type(wrapLightEntity),
                      type(wrapFogVolumeEntity)
                    "#,
                    "prelude.luau",
                )
                .unwrap();
            assert_eq!(world_ty, "table", "{which:?}: world");
            // Capability methods — not bare globals anymore.
            assert_eq!(flicker_ty, "nil", "{which:?}: flicker");
            assert_eq!(pulse_ty, "nil", "{which:?}: pulse");
            assert_eq!(color_ty, "nil", "{which:?}: colorShift");
            assert_eq!(sweep_ty, "nil", "{which:?}: sweep");
            assert_eq!(fog_pulse_ty, "nil", "{which:?}: fogPulse");
            assert_eq!(fog_fade_ty, "nil", "{which:?}: fogFade");
            assert_eq!(timeline_ty, "function", "{which:?}: timeline");
            assert_eq!(sequence_ty, "function", "{which:?}: sequence");
            assert_eq!(emitter_ty, "function", "{which:?}: emitter");
            assert_eq!(smoke_ty, "function", "{which:?}: smokeEmitter");
            assert_eq!(spark_ty, "function", "{which:?}: sparkEmitter");
            assert_eq!(dust_ty, "function", "{which:?}: dustEmitter");
            // Temporary bridges nil'd out before author scripts run.
            assert_eq!(wrap_light_ty, "nil", "{which:?}: wrapLightEntity");
            assert_eq!(wrap_fog_ty, "nil", "{which:?}: wrapFogVolumeEntity");
        }
    }

    /// Re-evaluate `entities/fog_volumes.luau` against the live definition
    /// state and install its `wrapFogVolumeEntity` function as a test-only
    /// global named `__test_wrapFogVolume`. The prelude pass nil'd out the
    /// authoritative `wrapFogVolumeEntity` before sandbox; re-evaluating here
    /// is a host-side ergonomic for unit tests that need to construct a
    /// `FogVolumeHandle` without a real fog entity in the registry.
    fn install_test_fog_wrapper(subsys: &LuauSubsystem) {
        let lua = subsys.definition_lua();
        let fog_sdk: Table = lua
            .load(FOG_VOLUMES_LUAU_SRC)
            .set_name("test/fog_volumes.luau")
            .eval()
            .unwrap();
        let wrap: mlua::Function = fog_sdk.get("wrapFogVolumeEntity").unwrap();
        lua.globals().set("__test_wrapFogVolume", wrap).unwrap();
    }

    /// Build a synthetic FogVolumeHandle inline. Lua snippet shared by the
    /// pulse / fade tests; assumes `__test_wrapFogVolume` has been installed.
    const FOG_HANDLE_FIXTURE: &str = r#"
        local snapshot = {
            id = ID,
            position = { x = 0, y = 0, z = 0 },
            tags = {},
            component = {
                density = 1.0, glow = 0.5, edgeSoftness = 0,
                falloff = 1.0, tint = {1, 1, 1},
                saturation = 1.0, minBrightness = 0.0, lightRange = 1.0,
                animation = nil,
            },
        }
        local fog = __test_wrapFogVolume(snapshot)
    "#;

    #[test]
    fn fog_handle_pulse_returns_single_step_set_fog_animation() {
        // `fog:pulse({ ... })` produces a single-element step array of
        // `{ id, primitive = "setFogAnimation", args = FogAnimation }` with a
        // 17-sample sine `density` curve (16 intervals + wrap sample) and
        // `playCount = nil` (loop forever). The 17th sample equals the 1st so
        // the linear sampler interpolates cleanly at the period boundary.
        // Replaces the previous test against the old free `fogPulse` global —
        // capability methods now own the curve construction.
        let (subsys, _ctx) = setup();
        install_test_fog_wrapper(&subsys);
        let src = format!(
            r#"
            local ID = 7
            {fixture}
            local steps = fog:pulse({{ min = 0.2, max = 1.0, periodMs = 1500 }})
            assert(#steps == 1, "expected 1 step, got " .. tostring(#steps))
            local s = steps[1]
            assert(s.id == 7, "expected id == 7")
            assert(s.primitive == "setFogAnimation",
                "expected primitive == setFogAnimation, got " .. tostring(s.primitive))
            assert(s.args ~= nil, "expected args ~= nil")
            assert(s.args.periodMs == 1500, "expected periodMs == 1500")
            assert(s.args.phase == nil, "expected phase == nil")
            assert(s.args.playCount == nil, "expected playCount == nil (loop forever)")
            local out = {{}}
            for i, d in ipairs(s.args.density) do
                out[i] = d
            end
            return out
            "#,
            fixture = FOG_HANDLE_FIXTURE,
        );
        let densities: Vec<f64> = subsys
            .run_source(Which::Definition, &src, "fog_pulse_unit.luau")
            .unwrap();
        assert_eq!(densities.len(), 17);
        let lo = 0.2_f64;
        let hi = 1.0_f64;
        let mid = (lo + hi) * 0.5;
        let amp = (hi - lo) * 0.5;
        for (i, &got) in densities.iter().enumerate() {
            let theta = (i as f64 / 16.0) * std::f64::consts::PI * 2.0;
            let expected = mid + amp * theta.sin();
            assert!(
                (got - expected).abs() < 1e-5,
                "sample {i}: expected {expected}, got {got}"
            );
        }
        // Wrap sample: sample[16] must equal sample[0].
        assert!(
            (densities[16] - densities[0]).abs() < 1e-5,
            "wrap sample[16] must equal sample[0]; got {} vs {}",
            densities[16],
            densities[0]
        );
    }

    #[test]
    fn fog_handle_fade_returns_single_step_one_shot_set_fog_animation() {
        // `fog:fade({ ... })` emits one `setFogAnimation` step whose `density`
        // curve is a 16-sample linear ramp from `from` to `to`, with
        // `playCount = 1` (one-shot). See `fog_handle_pulse_...` for the
        // shape rationale.
        let (subsys, _ctx) = setup();
        install_test_fog_wrapper(&subsys);
        let src = format!(
            r#"
            local ID = 11
            {fixture}
            local steps = fog:fade({{ from = 0.0, to = 4.0, periodMs = 750 }})
            assert(#steps == 1, "expected 1 step, got " .. tostring(#steps))
            local s = steps[1]
            assert(s.id == 11, "expected id == 11")
            assert(s.primitive == "setFogAnimation",
                "expected primitive == setFogAnimation, got " .. tostring(s.primitive))
            assert(s.args ~= nil, "expected args ~= nil")
            assert(s.args.periodMs == 750, "expected periodMs == 750")
            assert(s.args.phase == nil, "expected phase == nil")
            assert(s.args.playCount == 1, "expected playCount == 1 (one-shot)")
            local out = {{}}
            for i, d in ipairs(s.args.density) do
                out[i] = d
            end
            return out
            "#,
            fixture = FOG_HANDLE_FIXTURE,
        );
        let densities: Vec<f64> = subsys
            .run_source(Which::Definition, &src, "fog_fade_unit.luau")
            .unwrap();
        assert_eq!(densities.len(), 16);
        let from = 0.0_f64;
        let to = 4.0_f64;
        for (i, &got) in densities.iter().enumerate() {
            let t = i as f64 / 15.0;
            let expected = from + (to - from) * t;
            assert!(
                (got - expected).abs() < 1e-5,
                "sample {i}: expected {expected}, got {got}"
            );
        }
        assert!((densities[0] - from).abs() < 1e-6);
        assert!((densities[15] - to).abs() < 1e-6);
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

    #[test]
    fn ui_reactions_fields_contains_complete_sdk_surface() {
        // Regression: openTextEntry/KEYBOARD_TREE were declared in the typedef
        // + SDK but not lifted, so Luau callers hit a nil global at runtime
        // while TS callers worked fine (TS installs them via globalThis rewrite).
        //
        // This test guards against the same gap recurring: every name exposed by
        // `sdk/lib/ui/reactions.luau` as a UiReactionsSdk field must appear in
        // UI_REACTIONS_FIELDS so the field is actually promoted to a bare global.
        let expected: &[&str] = &[
            "onStateCrossing",
            "playSound",
            "rumble",
            "flashScreen",
            "showDialog",
            "openMenu",
            "closeDialog",
            "openTextEntry",
            "KEYBOARD_TREE",
            "setState",
            "appendText",
            "backspaceText",
            "clearText",
        ];
        let actual: std::collections::BTreeSet<&str> =
            UI_REACTIONS_FIELDS.iter().copied().collect();
        for name in expected {
            assert!(
                actual.contains(name),
                "UI_REACTIONS_FIELDS is missing `{name}` — it is declared in \
                sdk/lib/ui/reactions.luau and the Luau typedef but was not lifted \
                to a bare global, so Luau callers hit nil at runtime",
            );
        }
    }

    // --- M13 G1a, Task 3: TS/Luau widget-factory JSON parity ---
    //
    // The AC requires the TS and Luau widget/layout factories to emit IDENTICAL
    // JSON for identical inputs. The Luau factories run here under a raw `mlua`
    // VM; each result table is converted with the engine's `lua_to_json` walker
    // (the same conversion the Task 5 bridge will use) and compared — as parsed
    // `serde_json::Value`, so table key order is irrelevant — to the JSON the TS
    // factory emits for the same call (captured by running the TS factories under
    // bun; see the round-trip cases in `render::ui::descriptor`).
    #[test]
    fn luau_widget_factories_emit_json_identical_to_typescript() {
        const WIDGETS_SRC: &str = include_str!("../../../../sdk/lib/ui/widgets.luau");
        const LAYOUT_SRC: &str = include_str!("../../../../sdk/lib/ui/layout.luau");

        let lua = mlua::Lua::new();
        let widgets: mlua::Table = lua
            .load(WIDGETS_SRC)
            .set_name("widgets.luau")
            .eval()
            .expect("widgets.luau must evaluate to a module table");
        let layout: mlua::Table = lua
            .load(LAYOUT_SRC)
            .set_name("layout.luau")
            .eval()
            .expect("layout.luau must evaluate to a module table");
        lua.globals().set("W", widgets).unwrap();
        lua.globals().set("L", layout).unwrap();

        // (lua expression producing a widget table, expected JSON == TS output)
        let cases: &[(&str, &str)] = &[
            (
                r#"W.Text({ content = "hello" })"#,
                r#"{"kind":"text","content":"hello","fontSize":12,"color":[1,1,1,1]}"#,
            ),
            (
                r#"W.Text({ content = "0", fontSize = 18, color = {1,1,1,1}, bind = { slot = "player.health", format = "HP {}" } })"#,
                r#"{"kind":"text","content":"0","fontSize":18,"color":[1,1,1,1],"bind":{"slot":"player.health","format":"HP {}"}}"#,
            ),
            (
                r#"W.Text({ content = "0", fontSize = 18, color = {1,1,1,1}, bind = { slot = "player.health", tween = { durationMs = 1200, easing = "easeOut", from = 0 } } })"#,
                r#"{"kind":"text","content":"0","fontSize":18,"color":[1,1,1,1],"bind":{"slot":"player.health","tween":{"durationMs":1200,"easing":"easeOut","from":0}}}"#,
            ),
            (
                r#"W.Text({ content = "0", fontSize = 18, color = {1,1,1,1}, bind = { slot = "player.health" }, styleRanges = { max = 100, entries = { { upTo = 0.25, color = "critical" }, { color = "ok" } } } })"#,
                r#"{"kind":"text","content":"0","fontSize":18,"color":[1,1,1,1],"bind":{"slot":"player.health"},"styleRanges":{"max":100,"entries":[{"upTo":0.25,"color":"critical"},{"color":"ok"}]}}"#,
            ),
            (
                r#"W.Panel({ fill = {0.1,0.2,0.3,1}, border = { texture = "ui/frame", slice = {8,8,8,8}, tint = {1,1,1,1} } })"#,
                r#"{"kind":"panel","fill":[0.1,0.2,0.3,1],"border":{"texture":"ui/frame","slice":[8,8,8,8],"tint":[1,1,1,1]}}"#,
            ),
            (
                r#"W.Panel({ fill = {0.1,0.2,0.3,1} })"#,
                r#"{"kind":"panel","fill":[0.1,0.2,0.3,1]}"#,
            ),
            (
                r#"W.Panel({ fill = {0,0,0,1}, bind = { slot = "intro.flashColor", tween = { durationMs = 300, easing = "linear", from = {1,0,0,1} } } })"#,
                r#"{"kind":"panel","fill":[0,0,0,1],"bind":{"slot":"intro.flashColor","tween":{"durationMs":300,"easing":"linear","from":[1,0,0,1]}}}"#,
            ),
            (
                r#"W.Image({ asset = "ui/logo" })"#,
                r#"{"kind":"image","asset":"ui/logo"}"#,
            ),
            (
                r#"W.Spacer({ flexGrow = 1 })"#,
                r#"{"kind":"spacer","flexGrow":1}"#,
            ),
            (
                r#"W.Button({ id = "resume", label = "Resume", onPress = "resumeGame" })"#,
                r#"{"kind":"button","id":"resume","label":"Resume","onPress":"resumeGame"}"#,
            ),
            (
                r#"W.Button({ id = "a", label = "A", onPress = { name = "fa" } })"#,
                r#"{"kind":"button","id":"a","label":"A","onPress":"fa"}"#,
            ),
            (
                r#"W.Slider({ id = "vol", label = "Volume", bind = { slot = "audio.master" }, min = 0, max = 1, step = 0.1, capturesNav = {"nav.left","nav.right"} })"#,
                r#"{"kind":"slider","id":"vol","label":"Volume","bind":{"slot":"audio.master"},"min":0,"max":1,"step":0.1,"capturesNav":["nav.left","nav.right"]}"#,
            ),
            (
                r#"W.Bar({ bind = { slot = "player.health" }, max = 100, fill = {0,1,0,1}, background = {0.1,0.1,0.1,1} })"#,
                r#"{"kind":"bar","bind":{"slot":"player.health"},"max":100,"fill":[0,1,0,1],"background":[0.1,0.1,0.1,1]}"#,
            ),
            (
                r#"L.VStack({ gap = 4, padding = 8, align = "start" }, { W.Text({ content = "hi", fontSize = 12 }) })"#,
                r#"{"kind":"vstack","gap":4,"padding":8,"align":"start","children":[{"kind":"text","content":"hi","fontSize":12,"color":[1,1,1,1]}]}"#,
            ),
            (
                r#"L.Grid({ gap = 1, padding = 3, align = "stretch", cols = 2 }, { W.Image({ asset = "ui/icon" }) })"#,
                r#"{"kind":"grid","gap":1,"padding":3,"align":"stretch","cols":2,"children":[{"kind":"image","asset":"ui/icon"}]}"#,
            ),
            (
                // Detailed focus policy (wrap:false + repeat) + a child. (A child
                // is present so `children` is an unambiguous array under the generic
                // `lua_to_json` walker — an EMPTY Lua table is `{}`, not `[]`, a
                // limitation the Task 5 bridge resolves by deserializing straight
                // into the typed `ContainerWidget` rather than a generic `Value`.)
                r#"L.Grid({ cols = 2, focus = { policy = "spatial", wrap = false, ["repeat"] = { initialDelayMs = 300, intervalMs = 80 } } }, { W.Image({ asset = "x" }) })"#,
                r#"{"kind":"grid","gap":0,"padding":0,"align":"start","cols":2,"focus":{"policy":"spatial","wrap":false,"repeat":{"initialDelayMs":300,"intervalMs":80}},"children":[{"kind":"image","asset":"x"}]}"#,
            ),
        ];

        for (expr, expected_ts) in cases {
            let value: mlua::Value = lua
                .load(&format!("return {expr}"))
                .set_name("case")
                .eval()
                .unwrap_or_else(|e| panic!("luau factory call failed: {expr}\n{e}"));
            let got = super::super::conv::lua_to_json(value)
                .unwrap_or_else(|e| panic!("lua_to_json failed for {expr}: {e}"));
            let expected: serde_json::Value =
                serde_json::from_str(expected_ts).expect("TS expected JSON parses");
            assert_eq!(
                got, expected,
                "Luau factory output differs from TS for `{expr}`:\nluau: {got}\nts:   {expected}"
            );
        }
    }

    #[test]
    fn luau_widget_factories_reject_invalid_props_with_field_named_errors() {
        const WIDGETS_SRC: &str = include_str!("../../../../sdk/lib/ui/widgets.luau");
        const LAYOUT_SRC: &str = include_str!("../../../../sdk/lib/ui/layout.luau");
        let lua = mlua::Lua::new();
        let widgets: mlua::Table = lua.load(WIDGETS_SRC).eval().unwrap();
        let layout: mlua::Table = lua.load(LAYOUT_SRC).eval().unwrap();
        lua.globals().set("W", widgets).unwrap();
        lua.globals().set("L", layout).unwrap();

        // (lua call expected to error, substring the error must name)
        let cases: &[(&str, &str)] = &[
            (r#"W.Text({})"#, "content"),
            (r#"W.Image({ asset = "" })"#, "asset"),
            (
                r#"W.Button({ id = "x", label = "X", onPress = 42 })"#,
                "onPress",
            ),
            (
                r#"W.Slider({ id = "v", label = "V", min = 0, max = 1, step = 1 })"#,
                "bind",
            ),
            (r#"L.Grid({ cols = 0 }, {})"#, "cols"),
        ];
        for (expr, field) in cases {
            let err = lua
                .load(&format!("return {expr}"))
                .eval::<mlua::Value>()
                .expect_err(&format!("expected `{expr}` to error"));
            let msg = err.to_string();
            assert!(
                msg.contains(field),
                "error for `{expr}` must name `{field}`, got: {msg}"
            );
        }
    }
}
