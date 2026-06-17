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
pub(crate) struct LuauConfig {}

/// Luau subsystem: one `Lua` definition state, plus the primitive snapshot.
/// Fields mirror `QuickJsSubsystem` one-for-one.
pub(crate) struct LuauSubsystem {
    definition_lua: Lua,
    primitives: Vec<ScriptPrimitive>,
    archetypes: ArchetypeAccumulator,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scripting::ctx::ScriptCtx;
    use crate::scripting::luau_prelude::{FOG_VOLUMES_LUAU_SRC, UI_REACTIONS_FIELDS};
    use crate::scripting::primitives::register_all;
    use crate::scripting::primitives_registry::ContextScope;

    // 16 `type(...)` strings returned by `sdk_prelude_installs_globals`.
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
        String,
    );

    fn setup() -> (LuauSubsystem, ScriptCtx) {
        let ctx = ScriptCtx::new();
        let mut registry = PrimitiveRegistry::new();
        register_all(&mut registry, ctx.clone());
        let subsys = LuauSubsystem::new(&registry, &LuauConfig::default()).unwrap();
        (subsys, ctx)
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
                define_theme_ty,
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
                      type(defineTheme),
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
            assert_eq!(define_theme_ty, "function", "{which:?}: defineTheme");
            // Temporary bridges nil'd out before author scripts run.
            assert_eq!(wrap_light_ty, "nil", "{which:?}: wrapLightEntity");
            assert_eq!(wrap_fog_ty, "nil", "{which:?}: wrapFogVolumeEntity");
            let (get_game_state_ty, game_state_bridge_ty): (String, String) = subsys
                .run_source(
                    which,
                    "return type(getGameState), type(__postretroGameStateRefs)",
                    "game_state_prelude.luau",
                )
                .unwrap();
            assert_eq!(get_game_state_ty, "function", "{which:?}: getGameState");
            assert_eq!(
                game_state_bridge_ty, "nil",
                "{which:?}: __postretroGameStateRefs"
            );
        }
    }

    #[test]
    fn get_game_state_returns_same_frozen_hidden_reference_tree() {
        let (subsys, _ctx) = setup();
        let (
            health_slot,
            max_health_slot,
            has_ammo,
            same_root,
            same_leaf,
            bridge_ty,
            root_mutates,
            leaf_mutates,
        ): (String, String, bool, bool, bool, String, bool, bool) = subsys
            .run_source(
                Which::Definition,
                r#"
                local first = getGameState()
                local second = getGameState()
                local rootOk = pcall(function()
                    first.player = {}
                end)
                local leafOk = pcall(function()
                    first.player.health.slot = "mutated"
                end)
                return
                  first.player.health.slot,
                  first.player.maxHealth.slot,
                  first.player.ammo ~= nil,
                  first == second,
                  first.player.health == second.player.health,
                  type(__postretroGameStateRefs),
                  rootOk,
                  leafOk
                "#,
                "game_state_refs.luau",
            )
            .unwrap();

        assert_eq!(health_slot, "player.health");
        assert_eq!(max_health_slot, "player.maxHealth");
        assert!(!has_ammo, "removed player.ammo path must not be exposed");
        assert!(same_root, "getGameState must return the captured singleton");
        assert!(same_leaf, "leaf identity must be stable across calls");
        assert_eq!(bridge_ty, "nil", "bridge global must be hidden");
        assert!(!root_mutates, "nested object mutation must fail");
        assert!(!leaf_mutates, "leaf mutation must fail");
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
            "vignette",
            "screenShake",
            "showDialog",
            "openMenu",
            "closeDialog",
            "openTextEntry",
            "KEYBOARD_TREE",
            "CLOSE_DIALOG_ACTION",
            "EXIT_TO_DESKTOP_ACTION",
            "updateState",
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

    #[test]
    fn close_dialog_action_global_emits_reserved_wire_name() {
        let lua = build_lua_state(&[], None, None).expect("lua state");
        let value: String = lua
            .load("return CLOSE_DIALOG_ACTION")
            .eval()
            .expect("CLOSE_DIALOG_ACTION global");
        assert_eq!(value, "ui.closeDialog");
    }

    #[test]
    fn exit_to_desktop_action_global_emits_reserved_wire_name() {
        let lua = build_lua_state(&[], None, None).expect("lua state");
        let value: String = lua
            .load("return EXIT_TO_DESKTOP_ACTION")
            .eval()
            .expect("EXIT_TO_DESKTOP_ACTION global");
        assert_eq!(value, "ui.exitToDesktop");
    }

    #[test]
    fn sdk_prelude_installs_ui_factory_globals() {
        // M13 G1a Task 6: the widget/layout/tree/state factory modules must be
        // wired into the Luau prelude so authors call them by bare name. Without
        // the wiring (the precedent the Task 3/4 notes flag), a Luau author hits a
        // nil global at runtime while a TS author works (TS lifts via the
        // globalThis rewrite). Asserts every lifted field is a callable global and
        // that the internal `validateBorder`/`resolveReactionName` helpers stay
        // OFF the global table.
        let (subsys, _ctx) = setup();
        let src = r#"
            local function isfn(n) return type(_G[n]) == "function" end
            return
              isfn("Text"), isfn("Panel"), isfn("Image"), isfn("Spacer"),
              isfn("Button"), isfn("Slider"), isfn("Bar"),
              isfn("VStack"), isfn("HStack"), isfn("Grid"),
              isfn("Tree"), isfn("bindState"), isfn("stateEquals"),
              isfn("defineTheme"),
              type(validateBorder), type(resolveReactionName)
        "#;
        let results: mlua::MultiValue = subsys
            .run_source(Which::Definition, src, "ui_prelude.luau")
            .unwrap();
        let vals: Vec<mlua::Value> = results.into_iter().collect();
        // First 14 are the factory/helper globals: must all be functions (true).
        for (i, v) in vals.iter().take(14).enumerate() {
            assert_eq!(
                v.as_boolean(),
                Some(true),
                "UI factory global #{i} must be installed as a function"
            );
        }
        // Last 2 are the internal helpers: must be `nil` (not leaked).
        let validate_border_ty = vals[14]
            .as_string()
            .and_then(|s| s.to_str().ok().map(|s| s.to_string()));
        assert_eq!(
            validate_border_ty.as_deref(),
            Some("nil"),
            "validateBorder must NOT be a bare global"
        );
        let resolve_reaction_name_ty = vals[15]
            .as_string()
            .and_then(|s| s.to_str().ok().map(|s| s.to_string()));
        assert_eq!(
            resolve_reaction_name_ty.as_deref(),
            Some("nil"),
            "resolveReactionName must NOT be a bare global"
        );
    }

    /// M13 G1a Task 6: `defineReaction` auto-id determinism + byte-identical
    /// `onPress` (handle vs. bare string) + bare-string round-trip. Drives the
    /// ACTUAL `data_script.luau` factory and `widgets.luau`'s `Button` under a raw
    /// mlua VM, then converts via the engine's `lua_to_json` walker so the
    /// assertions are on the real wire JSON.
    #[test]
    fn define_reaction_auto_id_is_deterministic_and_on_press_is_byte_identical() {
        const DATA_SCRIPT_SRC: &str = include_str!("../../../../sdk/lib/data_script.luau");
        const WIDGETS_SRC: &str = include_str!("../../../../sdk/lib/ui/widgets.luau");
        let lua = mlua::Lua::new();
        let data_script: mlua::Table = lua
            .load(DATA_SCRIPT_SRC)
            .set_name("data_script.luau")
            .eval()
            .unwrap();
        let widgets: mlua::Table = lua
            .load(WIDGETS_SRC)
            .set_name("widgets.luau")
            .eval()
            .unwrap();
        lua.globals().set("D", data_script).unwrap();
        lua.globals().set("W", widgets).unwrap();

        // (1) The same body run twice yields the same content-derived auto-id.
        let (id_a, id_b): (String, String) = lua
            .load(
                r#"
                local body = { primitive = "playSound", args = { sound = "click" } }
                local a = D.defineReaction(body)
                local b = D.defineReaction({ primitive = "playSound", args = { sound = "click" } })
                return a.name, b.name
                "#,
            )
            .eval()
            .unwrap();
        assert_eq!(id_a, id_b, "auto-id must be deterministic across runs");
        assert!(
            id_a.starts_with("reaction_"),
            "auto-id must carry the `reaction_` prefix, got {id_a}"
        );

        // (2) `Button({ onPress = handle })` emits `onPress: "<id>"` byte-identical
        // to `Button({ onPress = "<id>" })`. Compare through lua_to_json.
        let handle_btn: mlua::Value = lua
            .load(
                r#"
                local handle = D.defineReaction({ primitive = "playSound", args = { sound = "click" } })
                return W.Button({ id = "a", label = "A", onPress = handle })
                "#,
            )
            .eval()
            .unwrap();
        let handle_json = super::super::conv::lua_to_json(handle_btn).unwrap();
        let on_press = handle_json
            .get("onPress")
            .and_then(|v| v.as_str())
            .expect("onPress must be a string")
            .to_string();
        assert_eq!(
            on_press, id_a,
            "Button onPress from a handle must equal the handle's auto-id"
        );

        let string_btn: mlua::Value = lua
            .load(&format!(
                r#"return W.Button({{ id = "a", label = "A", onPress = "{on_press}" }})"#
            ))
            .eval()
            .unwrap();
        let string_json = super::super::conv::lua_to_json(string_btn).unwrap();
        assert_eq!(
            handle_json, string_json,
            "Button({{onPress: handle}}) must be byte-identical to Button({{onPress: \"<id>\"}})"
        );

        // (3) A bare-string onPress round-trips unchanged (the shipped path).
        let bare: mlua::Value = lua
            .load(r#"return W.Button({ id = "x", label = "X", onPress = "resumeGame" })"#)
            .eval()
            .unwrap();
        let bare_json = super::super::conv::lua_to_json(bare).unwrap();
        assert_eq!(
            bare_json.get("onPress").and_then(|v| v.as_str()),
            Some("resumeGame"),
            "a bare-string onPress must pass through unchanged"
        );
    }

    /// M13 G1b: `ui.createLocalState` is promoted to the `ui` global after the
    /// prelude step that evaluates `ui/state.luau`. Without `"ui"` in
    /// `UI_STATE_FIELDS`, a Luau author calling `ui.createLocalState(...)` gets
    /// "attempt to index nil value 'ui'". This test verifies:
    ///   1. `ui` is a table (not nil).
    ///   2. `ui.createLocalState` is a function.
    ///   3. Calling `ui.createLocalState({ hp = 100 })` returns a bundle whose
    ///      `cells.hp:get()` is a `{ local = ... }` bind ref.
    ///   4. `cells.hp:set(42)` returns a `cellWrite` reaction descriptor
    ///      (primitive = "cellWrite", args.cell = "hp", args.value = 42).
    #[test]
    fn sdk_prelude_installs_ui_create_local_state() {
        let (subsys, _ctx) = setup();
        let src = r#"
            -- 1. ui is a table, not nil
            assert(type(ui) == "table",
                "ui must be a table, got: " .. type(ui))
            -- 2. ui.createLocalState is a function
            assert(type(ui.createLocalState) == "function",
                "ui.createLocalState must be a function, got: " .. type(ui.createLocalState))
            -- 3. Call createLocalState and inspect the bundle
            local bundle = ui.createLocalState({ hp = 100 })
            assert(type(bundle) == "table", "bundle must be a table")
            assert(type(bundle.cells) == "table", "bundle.cells must be a table")
            local cell = bundle.cells.hp
            assert(type(cell) == "table", "cells.hp must be a table")
            -- 4. :get() yields the { local } bind ref
            local ref_ = cell:get()
            assert(type(ref_) == "table", "get() must return a table")
            local localField = ref_["local"]
            assert(type(localField) == "string",
                "bind ref must have a string 'local' field, got: " .. type(localField))
            -- 5. :set(v) emits a cellWrite reaction descriptor
            local desc = cell:set(42)
            assert(type(desc) == "table", "set() must return a table")
            assert(desc.primitive == "cellWrite",
                "set() primitive must be 'cellWrite', got: " .. tostring(desc.primitive))
            assert(type(desc.args) == "table", "set() must have args table")
            assert(desc.args.cell == "hp",
                "args.cell must be 'hp', got: " .. tostring(desc.args.cell))
            assert(desc.args.value == 42,
                "args.value must be 42, got: " .. tostring(desc.args.value))
            -- 6. args.scope is present and is the stable "localState.N" id.
            -- The SDK's monotonic counter starts at 0; the first createLocalState
            -- call in this script yields "localState.0".
            assert(type(desc.args.scope) == "string" and #desc.args.scope > 0,
                "args.scope must be a non-empty string, got: " .. tostring(desc.args.scope))
            assert(desc.args.scope == "localState.0",
                "args.scope must be 'localState.0', got: " .. tostring(desc.args.scope))
            return true
        "#;
        let ok: bool = subsys
            .run_source(Which::Definition, src, "ui_create_local_state.luau")
            .unwrap();
        assert!(ok, "ui.createLocalState round-trip failed");
    }

    #[test]
    fn luau_update_state_emits_existing_set_state_wire_descriptor() {
        const REACTIONS_SRC: &str = include_str!("../../../../sdk/lib/ui/reactions.luau");

        let lua = mlua::Lua::new();
        let reactions: mlua::Table = lua
            .load(REACTIONS_SRC)
            .set_name("reactions.luau")
            .eval()
            .unwrap();
        lua.globals().set("R", reactions).unwrap();

        let value: mlua::Value = lua
            .load(r#"return R.updateState({ slot = "audio.master" }, 0.5)"#)
            .set_name("update_state")
            .eval()
            .unwrap_or_else(|e| panic!("updateState call failed:\n{e}"));
        let got = super::super::conv::lua_to_json(value).expect("lua_to_json");
        let expected = serde_json::json!({
            "primitive": "setState",
            "args": {
                "slot": "audio.master",
                "value": 0.5,
            },
        });
        assert_eq!(got, expected);
    }

    #[test]
    fn luau_bind_state_emits_json_identical_to_typescript() {
        const STATE_SRC: &str = include_str!("../../../../sdk/lib/ui/state.luau");

        let lua = mlua::Lua::new();
        let state: mlua::Table = lua
            .load(STATE_SRC)
            .set_name("state.luau")
            .eval()
            .expect("state.luau must evaluate to a module table");
        lua.globals().set("S", state).unwrap();

        let cases: &[(&str, &str)] = &[
            (
                r#"S.bindState({ slot = "player.health" }, { format = "HP {}", tween = { durationMs = 120, easing = "easeOut", from = 0 } })"#,
                r#"{"slot":"player.health","format":"HP {}","tween":{"durationMs":120,"easing":"easeOut","from":0}}"#,
            ),
            (
                r#"S.bindState({ slot = "screen.flash" }, { tween = { durationMs = 80, easing = "linear", from = {0, 0, 0, 0} } })"#,
                r#"{"slot":"screen.flash","tween":{"durationMs":80,"easing":"linear","from":[0,0,0,0]}}"#,
            ),
        ];

        for (expr, expected_ts) in cases {
            let value: mlua::Value = lua
                .load(&format!("return {expr}"))
                .set_name("bind_state_case")
                .eval()
                .unwrap_or_else(|e| panic!("bindState call failed: {expr}\n{e}"));
            let got = super::super::conv::lua_to_json(value)
                .unwrap_or_else(|e| panic!("lua_to_json failed for {expr}: {e}"));
            let expected: serde_json::Value =
                serde_json::from_str(expected_ts).expect("TS expected JSON parses");
            assert_eq!(
                got, expected,
                "Luau bindState output differs from TS for `{expr}`:\nluau: {got}\nts:   {expected}"
            );
        }
    }

    #[test]
    fn luau_define_theme_returns_token_strings_without_retaining_helper_metadata() {
        const THEME_SRC: &str = include_str!("../../../../sdk/lib/ui/theme.luau");
        const WIDGETS_SRC: &str = include_str!("../../../../sdk/lib/ui/widgets.luau");
        const LAYOUT_SRC: &str = include_str!("../../../../sdk/lib/ui/layout.luau");

        let lua = mlua::Lua::new();
        let theme_sdk: mlua::Table = lua
            .load(THEME_SRC)
            .set_name("theme.luau")
            .eval()
            .expect("theme.luau must evaluate to a module table");
        lua.globals().set("T", theme_sdk).unwrap();
        let widgets_sdk: mlua::Table = lua
            .load(WIDGETS_SRC)
            .set_name("widgets.luau")
            .eval()
            .expect("widgets.luau must evaluate to a module table");
        lua.globals().set("W", widgets_sdk).unwrap();
        let layout_sdk: mlua::Table = lua
            .load(LAYOUT_SRC)
            .set_name("layout.luau")
            .eval()
            .expect("layout.luau must evaluate to a module table");
        lua.globals().set("L", layout_sdk).unwrap();

        let (
            color,
            font,
            spacing,
            unknown,
            empty,
            invalid,
            text_color,
            text_font,
            stack_gap,
            stack_padding,
            has_raw_tokens,
            pair_count,
            theme,
        ): (
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
            bool,
            i64,
            mlua::Value,
        ) = lua
            .load(
                r#"
                local theme = T.defineTheme({
                  colors = { ["hud.text"] = {1, 1, 1, 1} },
                  fonts = { ["hud.status"] = "JetBrains Mono" },
                  spacing = { ["hud.gap"] = 8 },
                })
                theme.colors["late.color"] = {0, 1, 0, 1}
                theme.colors["hud.text"] = nil
                local pairCount = 0
                for _ in pairs(theme) do
                  pairCount += 1
                end
                local text = W.Text({
                  content = "empty token",
                  color = theme.tokens.color(""),
                  font = theme.tokens.font(""),
                })
                local stack = L.VStack({
                  gap = theme.tokens.spacing(""),
                  padding = theme.tokens.spacing(""),
                }, { text })
                return
                  theme.tokens.color("hud.text"),
                  theme.tokens.font("hud.status"),
                  theme.tokens.spacing("hud.gap"),
                  theme.tokens.color("late.color"),
                  theme.tokens.spacing(""),
                  theme.tokens.font(42),
                  text.color,
                  text.font,
                  stack.gap,
                  stack.padding,
                  rawget(theme, "tokens") ~= nil,
                  pairCount,
                  theme
                "#,
            )
            .set_name("define_theme_case")
            .eval()
            .expect("defineTheme should return token strings");

        assert_eq!(color, "hud.text");
        assert_eq!(font, "hud.status");
        assert_eq!(spacing, "hud.gap");
        assert_eq!(unknown, "late.color");
        assert_eq!(empty, "");
        assert_eq!(invalid, "42");
        assert_eq!(text_color, "");
        assert_eq!(text_font, "");
        assert_eq!(stack_gap, "");
        assert_eq!(stack_padding, "");
        assert!(
            !has_raw_tokens,
            "tokens helper must not be a retained table field"
        );
        assert_eq!(
            pair_count, 3,
            "pairs(theme) must see only theme category maps"
        );
        let json = super::super::conv::lua_to_json(theme).expect("theme converts to JSON");
        assert_eq!(
            json.get("tokens"),
            None,
            "defineTheme helper metadata must not enter generic theme data"
        );
        let object = json.as_object().expect("theme serializes as object");
        assert_eq!(object.len(), 3);
        for key in ["colors", "fonts", "spacing"] {
            assert!(object.contains_key(key), "theme JSON missing {key}");
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
                r#"W.Image({ asset = "ui/logo", decorative = true })"#,
                r#"{"kind":"image","asset":"ui/logo","decorative":true}"#,
            ),
            (
                r#"W.Image({ asset = "ui/portrait", label = "Hero portrait" })"#,
                r#"{"kind":"image","asset":"ui/portrait","label":"Hero portrait"}"#,
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
                r#"L.Grid({ gap = 1, padding = 3, align = "stretch", cols = 2 }, { W.Image({ asset = "ui/icon", decorative = true }) })"#,
                r#"{"kind":"grid","gap":1,"padding":3,"align":"stretch","cols":2,"children":[{"kind":"image","asset":"ui/icon","decorative":true}]}"#,
            ),
            (
                // Detailed focus policy (wrap:false + repeat) + a child. (A child
                // is present so `children` is an unambiguous array under the generic
                // `lua_to_json` walker — an EMPTY Lua table is `{}`, not `[]`, a
                // limitation the Task 5 bridge resolves by deserializing straight
                // into the typed `ContainerWidget` rather than a generic `Value`.)
                r#"L.Grid({ cols = 2, focus = { policy = "spatial", wrap = false, ["repeat"] = { initialDelayMs = 300, intervalMs = 80 } } }, { W.Image({ asset = "x", decorative = true }) })"#,
                r#"{"kind":"grid","gap":0,"padding":0,"align":"start","cols":2,"focus":{"policy":"spatial","wrap":false,"repeat":{"initialDelayMs":300,"intervalMs":80}},"children":[{"kind":"image","asset":"x","decorative":true}]}"#,
            ),
            // M13 G2: name-XOR via labelledBy, reactive predicates, disabled, role,
            // visibleWhen, and the Announce widget — each emits the camelCase wire
            // form the SE/G2 Rust descriptor round-trips.
            (
                r#"W.Button({ id = "tab1", labelledBy = "tab1Label", onPress = "selectTab", selected = { ["local"] = "tab", equals = "stats" }, role = "tab" })"#,
                r#"{"kind":"button","id":"tab1","labelledBy":"tab1Label","onPress":"selectTab","selected":{"local":"tab","equals":"stats"},"role":"tab"}"#,
            ),
            (
                r#"W.Button({ id = "mute", label = "Mute", onPress = "toggleMute", checked = { slot = "audio.muted", equals = true }, disabled = true })"#,
                r#"{"kind":"button","id":"mute","label":"Mute","onPress":"toggleMute","checked":{"slot":"audio.muted","equals":true},"disabled":true}"#,
            ),
            (
                r#"W.Slider({ id = "vol", labelledBy = "volLabel", bind = { slot = "audio.master" }, min = 0, max = 1, step = 0.1, visibleWhen = { ["local"] = "open" } })"#,
                r#"{"kind":"slider","id":"vol","labelledBy":"volLabel","bind":{"slot":"audio.master"},"min":0,"max":1,"step":0.1,"visibleWhen":{"local":"open"}}"#,
            ),
            (
                r#"W.Announce({ priority = "assertive" }, "Wave incoming")"#,
                r#"{"kind":"announce","text":"Wave incoming","priority":"assertive"}"#,
            ),
            (
                r#"W.Announce({}, "Saved")"#,
                r#"{"kind":"announce","text":"Saved"}"#,
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
            // M13 G2 name-XOR preconditions: neither or both is an error.
            (r#"W.Button({ id = "b", onPress = "go" })"#, "label"),
            (
                r#"W.Button({ id = "b", label = "B", labelledBy = "x", onPress = "go" })"#,
                "labelledBy",
            ),
            (
                r#"W.Slider({ id = "s", bind = { slot = "a.b" }, min = 0, max = 1, step = 1 })"#,
                "label",
            ),
            (r#"W.Image({ asset = "x" })"#, "label"),
            (
                r#"W.Image({ asset = "x", label = "L", decorative = true })"#,
                "decorative",
            ),
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

    /// M13 G2 Task 4: `Switch(cell, map)` expands a string-valued cell's case map
    /// into an array of subtrees, each carrying an injected
    /// `visibleWhen = cell:is(key)`, in LEXICOGRAPHICALLY-SORTED key order. The
    /// sort is load-bearing: Luau pair iteration is undefined, so without it the
    /// Luau array order would diverge from TS. The expected JSON is the TS `Switch`
    /// contract (the cross-runtime parity anchor — `widgets.luau` parity test
    /// pattern); a 3-key cell here proves the order and per-case injection.
    #[test]
    fn luau_switch_injects_sorted_visiblewhen_predicates() {
        const WIDGETS_SRC: &str = include_str!("../../../../sdk/lib/ui/widgets.luau");
        const STATE_SRC: &str = include_str!("../../../../sdk/lib/ui/state.luau");

        let lua = mlua::Lua::new();
        let widgets: mlua::Table = lua
            .load(WIDGETS_SRC)
            .set_name("widgets.luau")
            .eval()
            .unwrap();
        let state: mlua::Table = lua.load(STATE_SRC).set_name("state.luau").eval().unwrap();
        lua.globals().set("W", widgets).unwrap();
        lua.globals().set("S", state).unwrap();

        // A 3-key map authored OUT of sorted order ("stats"/"gear"/"map") to prove
        // the factory sorts rather than preserving authoring order.
        let expr = r#"
            local bundle = S.createLocalState({ tab = "map" })
            local tab = bundle.cells.tab
            return S.Switch(tab, {
              stats = W.Text({ content = "Stats" }),
              gear = W.Text({ content = "Gear" }),
              map = W.Text({ content = "Map" }),
            })
        "#;
        let value: mlua::Value = lua
            .load(expr)
            .set_name("switch_case")
            .eval()
            .unwrap_or_else(|e| panic!("Switch call failed:\n{e}"));
        let got = super::super::conv::lua_to_json(value).expect("lua_to_json");

        // The byte-identical TS contract: sorted "gear" < "map" < "stats", each
        // subtree carrying `visibleWhen = { local = "tab", equals = <key> }`.
        let expected: serde_json::Value = serde_json::from_str(
            r#"[
              {"kind":"text","content":"Gear","fontSize":12,"color":[1,1,1,1],"visibleWhen":{"local":"tab","equals":"gear"}},
              {"kind":"text","content":"Map","fontSize":12,"color":[1,1,1,1],"visibleWhen":{"local":"tab","equals":"map"}},
              {"kind":"text","content":"Stats","fontSize":12,"color":[1,1,1,1],"visibleWhen":{"local":"tab","equals":"stats"}}
            ]"#,
        )
        .unwrap();
        assert_eq!(
            got, expected,
            "Switch output diverges from the TS contract:\n{got}"
        );

        // The sorted-key order is positional, so assert it explicitly too (a
        // value-compare would not catch a reordering if the objects were equal).
        let arr = got.as_array().expect("Switch returns an array");
        let order: Vec<&str> = arr
            .iter()
            .map(|e| e["visibleWhen"]["equals"].as_str().unwrap())
            .collect();
        assert_eq!(
            order,
            ["gear", "map", "stats"],
            "Switch keys must be lexicographically sorted"
        );
    }
}
