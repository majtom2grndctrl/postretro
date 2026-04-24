// mlua/Luau subsystem: two sandboxed `mlua::Lua` states (definition and
// behavior), driven by the shared primitive registry.
// See: context/lib/scripting.md
//
// Mirrors `QuickJsSubsystem` so `ScriptRuntime` can fan out symmetrically by
// file extension. Two `Lua` states enforce the definition/behavior split; real
// installers only land in the correct state.

use std::cell::RefCell;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::rc::Rc;

use mlua::{Compiler, Function, Lua, Table};

use super::error::ScriptError;
use super::primitives_registry::{ContextScope, PrimitiveRegistry, ScriptPrimitive};
use super::quickjs::{ArchetypeAccumulator, ArchetypeDescriptor};

/// Engine-internal sink function installed into the definition Lua state.
/// Leading underscore: the type-def generator skips names starting with `_`.
const COLLECT_FN_NAME: &str = "__collect_definitions";

/// Deny-list: global names (and `os.<sub>` fields) we clear on both Lua states
/// before any script runs. `sandbox(true)` makes `_G` read-only but does NOT
/// remove these entries — the sandbox is about immutability, not capabilities.
const DENIED_GLOBALS: &[&str] = &["io", "package", "require", "dofile", "loadfile", "load"];
/// Sub-fields of the `os` table we nil out. `os.time` and `os.clock` are wall-
/// clock sources — handlers must take their timing from `ScriptCallContext`
/// (see: context/plans/ready/scripting-foundation/plan-2-light-entity.md
/// §Sub-plan 5), not from a free-running clock. `os.date` is denied alongside
/// them because it exposes the same wall-clock surface in string form.
const DENIED_OS_FIELDS: &[&str] = &[
    "execute", "exit", "getenv", "time", "clock", "date",
];

/// Configuration for a [`LuauSubsystem`]. `pool_size` tunes the ephemeral-
/// context pool. Does NOT affect the shared behavior `Lua` state, which is
/// never pooled.
#[derive(Clone, Copy, Debug)]
pub(crate) struct LuauConfig {
    pub(crate) pool_size: usize,
}

impl Default for LuauConfig {
    fn default() -> Self {
        Self {
            pool_size: super::pool::DEFAULT_POOL_SIZE,
        }
    }
}

/// Luau subsystem: one `Lua` per scope, plus the primitive snapshot used on
/// reload. Fields mirror `QuickJsSubsystem` one-for-one.
pub(crate) struct LuauSubsystem {
    definition_lua: Lua,
    behavior_lua: Lua,
    primitives: Vec<ScriptPrimitive>,
    archetypes: ArchetypeAccumulator,
}

/// Scope selector passed to `run_source` and to the top-level dispatcher.
/// Lives here rather than in `runtime.rs` so the subsystem can be exercised
/// in isolation from tests.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Which {
    Definition,
    Behavior,
}

impl LuauSubsystem {
    /// Construct a subsystem: build both Lua states, install the primitive
    /// set with correct scope partitioning, and install the archetype sink
    /// into the definition state. Order within each state is load-bearing:
    ///
    ///   1. `Lua::new()` (luau feature active).
    ///   2. Scrub deny-list globals (write `nil` into `_G` entries; clear
    ///      listed sub-fields of `os`).
    ///   3. Install the `print` redirect forwarding to `log::info!` with the
    ///      `[Script/Luau]` prefix. Must be before `sandbox(true)` — once
    ///      sandboxed, `_G` is read-only.
    ///   4. Install real/stub primitives, scope-partitioned.
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

        let definition_lua = build_lua_state(
            &primitives_snapshot,
            ContextScope::DefinitionOnly,
            Some(&archetypes),
        )?;
        let behavior_lua = build_lua_state(&primitives_snapshot, ContextScope::BehaviorOnly, None)?;

        Ok(Self {
            definition_lua,
            behavior_lua,
            primitives: primitives_snapshot,
            archetypes,
        })
    }

    /// Borrow the definition Lua state.
    pub(crate) fn definition_lua(&self) -> &Lua {
        &self.definition_lua
    }

    /// Borrow the behavior Lua state.
    pub(crate) fn behavior_lua(&self) -> &Lua {
        &self.behavior_lua
    }

    /// Borrow the primitive snapshot. Used by the context pool to pre-warm
    /// Lua states with the same primitive set.
    pub(crate) fn primitives(&self) -> &[ScriptPrimitive] {
        &self.primitives
    }

    /// Shared handle to the archetype accumulator. Exposed for tests and for
    /// the caller that drains it after evaluating definition scripts.
    pub(crate) fn archetypes(&self) -> &ArchetypeAccumulator {
        &self.archetypes
    }

    /// Drop the current definition state and rebuild it. Dev-mode hot-reload
    /// path. The archetype `Rc` is cleared in place so outside handles remain
    /// valid.
    pub(crate) fn reload_definition_context(&mut self) -> Result<(), ScriptError> {
        self.archetypes.borrow_mut().clear();
        self.definition_lua = build_lua_state(
            &self.primitives,
            ContextScope::DefinitionOnly,
            Some(&self.archetypes),
        )?;
        Ok(())
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
            Which::Behavior => &self.behavior_lua,
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

fn build_lua_state(
    primitives: &[ScriptPrimitive],
    target: ContextScope,
    archetypes: Option<&ArchetypeAccumulator>,
) -> Result<Lua, ScriptError> {
    let lua = Lua::new();

    // 1. Deny-list scrub.
    apply_denylist(&lua)?;

    // 2. `print` redirect — MUST happen before `sandbox(true)`, because
    // sandbox freezes `_G` and any subsequent `globals().set` would fail.
    install_print_redirect(&lua)?;

    // 3. Install primitives (real + stubs).
    install_primitives(&lua, primitives, target)?;

    // 4. Archetype sink into the definition state only.
    if let Some(accum) = archetypes {
        install_collect_definitions(&lua, accum.clone())?;
    }

    // 5. Freeze `_G`.
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

fn install_primitives(
    lua: &Lua,
    primitives: &[ScriptPrimitive],
    target: ContextScope,
) -> Result<(), ScriptError> {
    debug_assert!(
        matches!(
            target,
            ContextScope::DefinitionOnly | ContextScope::BehaviorOnly
        ),
        "install_primitives target must name a concrete context, not `Both`",
    );
    for p in primitives {
        let use_real = matches!(
            (p.context_scope, target),
            (ContextScope::Both, _)
                | (ContextScope::DefinitionOnly, ContextScope::DefinitionOnly)
                | (ContextScope::BehaviorOnly, ContextScope::BehaviorOnly)
        );
        let installer = if use_real {
            &p.luau_installer
        } else {
            &p.luau_stub_installer
        };
        installer(lua).map_err(|e| ScriptError::InvalidArgument {
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

    fn setup() -> (LuauSubsystem, ScriptCtx) {
        let ctx = ScriptCtx::new();
        let mut registry = PrimitiveRegistry::new();
        register_all(&mut registry, ctx.clone());
        let subsys = LuauSubsystem::new(&registry, &LuauConfig::default()).unwrap();
        (subsys, ctx)
    }

    #[test]
    fn new_constructs_both_states() {
        let (subsys, _ctx) = setup();
        let v: u32 = subsys
            .run_source(Which::Definition, "return 1 + 2", "def.luau")
            .unwrap();
        assert_eq!(v, 3);
        let v: u32 = subsys
            .run_source(Which::Behavior, "return 4 * 5", "beh.luau")
            .unwrap();
        assert_eq!(v, 20);
    }

    #[test]
    fn print_redirect_is_installed() {
        // We don't have log-capture plumbed in the test harness; the contract
        // here is that the function is bound. A human can run with
        // `RUST_LOG=info cargo test -- --nocapture` to see the output.
        let (subsys, _ctx) = setup();
        subsys
            .run_source::<()>(
                Which::Behavior,
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
                Which::Behavior,
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
    fn definition_context_rejects_behavior_only_primitive() {
        // `emit_event` is BehaviorOnly — in the definition state it's a stub.
        let (subsys, _ctx) = setup();
        let (ok, msg): (bool, String) = subsys
            .run_source(
                Which::Definition,
                r#"
                local ok, err = pcall(emit_event, { kind = "boom", payload = {} })
                return ok, tostring(err)
                "#,
                "wc.luau",
            )
            .unwrap();
        assert!(!ok);
        assert!(
            msg.contains("emit_event") && msg.contains("not available"),
            "unexpected: {msg}",
        );
    }

    #[test]
    fn behavior_context_rejects_definition_only_primitive() {
        let script_ctx = ScriptCtx::new();
        let mut registry = PrimitiveRegistry::new();
        register_all(&mut registry, script_ctx.clone());
        registry
            .register("test_def_only", || -> Result<u32, ScriptError> { Ok(7) })
            .scope(ContextScope::DefinitionOnly)
            .finish();

        let subsys = LuauSubsystem::new(&registry, &LuauConfig::default()).unwrap();
        let (ok, msg): (bool, String) = subsys
            .run_source(
                Which::Behavior,
                r#"
                local ok, err = pcall(test_def_only)
                return ok, tostring(err)
                "#,
                "wc2.luau",
            )
            .unwrap();
        assert!(!ok);
        assert!(
            msg.contains("test_def_only") && msg.contains("not available"),
            "unexpected: {msg}",
        );

        // And available in the definition state.
        let v: u32 = subsys
            .run_source(Which::Definition, "return test_def_only()", "def2.luau")
            .unwrap();
        assert_eq!(v, 7);

        // Keep script_ctx alive.
        let _ = script_ctx;
    }

    #[test]
    fn run_source_returns_script_threw_on_runtime_error() {
        let (subsys, _ctx) = setup();
        let err = subsys
            .run_source::<()>(Which::Behavior, "error('boom')", "test.luau")
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
            .run_source(Which::Behavior, "return 1 + 1", "after.luau")
            .unwrap();
        assert_eq!(v, 2);
    }

    #[test]
    fn run_source_returns_script_threw_on_compile_error() {
        let (subsys, _ctx) = setup();
        let err = subsys
            .run_source::<()>(Which::Behavior, "this is not valid luau ===", "bad.luau")
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
                Which::Behavior,
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
    fn end_to_end_transform_component_round_trip() {
        // Mirrors the QuickJS end-to-end test. `ComponentKind` is a bare
        // string (`"Transform"`); the returned `ComponentValue` table has a
        // top-level `kind = "Transform"` plus the Transform fields.
        let (subsys, ctx_handle) = setup();
        let (px, py, pz, pitch, yaw, roll, sx, sy, sz, kind): (
            f32,
            f32,
            f32,
            f32,
            f32,
            f32,
            f32,
            f32,
            f32,
            String,
        ) = subsys
            .run_source(
                Which::Behavior,
                r#"
                local id = spawn_entity({
                    position = { x = 0, y = 0, z = 0 },
                    rotation = { pitch = 0, yaw = 0, roll = 0 },
                    scale    = { x = 1, y = 1, z = 1 },
                })
                local input = {
                    kind = "Transform",
                    position = { x = 1.5,  y = 2.5, z = -3.25 },
                    rotation = { pitch = 15.0, yaw = 45.0, roll = -30.0 },
                    scale    = { x = 2.0, y = 2.0, z = 2.0 },
                }
                set_component(id, "Transform", input)
                local out = get_component(id, "Transform")
                return
                  out.position.x, out.position.y, out.position.z,
                  out.rotation.pitch, out.rotation.yaw, out.rotation.roll,
                  out.scale.x, out.scale.y, out.scale.z,
                  out.kind
                "#,
                "roundtrip.luau",
            )
            .unwrap();

        assert_eq!(kind, "Transform");
        assert!((px - 1.5).abs() < 1e-4);
        assert!((py - 2.5).abs() < 1e-4);
        assert!((pz - (-3.25)).abs() < 1e-4);
        assert!((pitch - 15.0).abs() < 1e-2, "pitch: {pitch}");
        assert!((yaw - 45.0).abs() < 1e-2, "yaw: {yaw}");
        assert!((roll - (-30.0)).abs() < 1e-2, "roll: {roll}");
        assert!((sx - 2.0).abs() < 1e-4);
        assert!((sy - 2.0).abs() < 1e-4);
        assert!((sz - 2.0).abs() < 1e-4);

        assert!(
            ctx_handle
                .registry
                .borrow()
                .exists(crate::scripting::registry::EntityId::from_raw(0))
        );
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
    fn reload_definition_context_rebuilds_and_clears_accumulator() {
        let (mut subsys, _ctx) = setup();
        let archetypes = subsys.archetypes().clone();

        archetypes.borrow_mut().push(ArchetypeDescriptor {
            name: "stale".into(),
        });
        assert_eq!(archetypes.borrow().len(), 1);

        subsys.reload_definition_context().unwrap();
        assert!(
            archetypes.borrow().is_empty(),
            "reload must drain accumulator"
        );

        // Fresh state must still have the sink.
        let len: usize = subsys
            .run_source(
                Which::Definition,
                "return #__collect_definitions()",
                "reload.luau",
            )
            .unwrap();
        assert_eq!(len, 0);
    }
}
