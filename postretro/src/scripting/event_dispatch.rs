// Event dispatch: per-level handler table and `registerHandler` primitive.
// See: context/lib/scripting.md

use std::cell::RefCell;
use std::rc::Rc;

use mlua::Function as LuaFunction;
use rquickjs::{CatchResultExt, Function as JsFunction, Persistent};

use super::call_context::ScriptCallContext;
use super::error::ScriptError;

/// Which engine event a handler subscribes to. `levelLoad` fires once per
/// level; `tick` fires once per frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EventKind {
    LevelLoad,
    Tick,
}

impl EventKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            EventKind::LevelLoad => "levelLoad",
            EventKind::Tick => "tick",
        }
    }

    pub(crate) fn parse(s: &str) -> Option<Self> {
        match s {
            "levelLoad" => Some(EventKind::LevelLoad),
            "tick" => Some(EventKind::Tick),
            _ => None,
        }
    }
}

/// A single registered handler. `source_name` is used for diagnostic logging
/// when a handler throws.
pub(crate) struct Handler {
    pub(crate) event: EventKind,
    pub(crate) source_name: String,
    pub(crate) callable: HandlerCallable,
}

/// Per-VM handler function storage. QuickJS functions must be saved into a
/// `Persistent` so they outlive the `Ctx<'js>` that created them; Luau
/// `Function` has no lifetime parameter and is stored directly.
pub(crate) enum HandlerCallable {
    QuickJs(Persistent<JsFunction<'static>>),
    Luau(LuaFunction),
}

/// The handler table. Fields:
///
/// * `handlers` — appended in registration order; drained on level unload.
/// * `current_source` — the script file currently being loaded. `registerHandler`
///   reads it so thrown-handler log lines cite the author's file. Set by
///   `ScriptRuntime::run_script_file` before evaluation, cleared after.
#[derive(Default)]
pub(crate) struct HandlerTable {
    handlers: Vec<Handler>,
    current_source: Option<String>,
}

impl HandlerTable {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Install the name of the script currently being evaluated. `registerHandler`
    /// reads this at append time and stores it on the handler.
    pub(crate) fn set_current_source(&mut self, name: Option<String>) {
        self.current_source = name;
    }

    /// Append a handler. `source_name` falls back to `"<unknown>"` if no
    /// script is currently being evaluated — the primitive should not be
    /// callable outside a script, but the fallback keeps logging robust.
    pub(crate) fn push(&mut self, event: EventKind, callable: HandlerCallable) {
        let source_name = self
            .current_source
            .clone()
            .unwrap_or_else(|| "<unknown>".to_string());
        self.handlers.push(Handler {
            event,
            source_name,
            callable,
        });
    }

    /// Number of registered handlers. Primarily a test hook.
    pub(crate) fn len(&self) -> usize {
        self.handlers.len()
    }

    /// Drop every handler. Called on level unload.
    pub(crate) fn clear(&mut self) {
        self.handlers.clear();
    }

    fn iter(&self) -> impl Iterator<Item = &Handler> {
        self.handlers.iter()
    }
}

/// Shared handle captured by the `registerHandler` primitive and by the frame
/// loop's fire helpers.
pub(crate) type SharedHandlerTable = Rc<RefCell<HandlerTable>>;

/// Fire the `levelLoad` event. Iterates handlers in registration order,
/// invoking each in its owning VM. A throwing handler is logged and swallowed;
/// the next handler still runs.
///
/// `quickjs_ctx` is the QuickJS behavior context (entered via `ctx.with`);
/// `lua` is the Luau behavior state.
pub(crate) fn fire_level_load(
    handlers: &SharedHandlerTable,
    quickjs_ctx: &rquickjs::Context,
    lua: &mlua::Lua,
) {
    // Snapshot: cloning handler references is awkward because `Persistent` is
    // `Clone` but `mlua::Function` doesn't need a clone for restore. We index
    // by position and re-borrow on each step so a handler calling back into
    // `registerHandler` does not invalidate iteration.
    let len = handlers.borrow().len();
    for i in 0..len {
        // Read handler fields into local variables; release the borrow before
        // calling into JS/Lua so the handler can freely re-enter `registerHandler`.
        let (event, source_name, is_quickjs, js_persistent, lua_fn);
        {
            let table = handlers.borrow();
            let Some(h) = table.handlers.get(i) else {
                continue;
            };
            if h.event != EventKind::LevelLoad {
                continue;
            }
            event = h.event;
            source_name = h.source_name.clone();
            match &h.callable {
                HandlerCallable::QuickJs(p) => {
                    is_quickjs = true;
                    js_persistent = Some(p.clone());
                    lua_fn = None;
                }
                HandlerCallable::Luau(f) => {
                    is_quickjs = false;
                    js_persistent = None;
                    lua_fn = Some(f.clone());
                }
            }
        }

        if is_quickjs {
            let p = js_persistent.expect("QuickJs handler must carry a Persistent");
            quickjs_ctx.with(|ctx| {
                let restored = match p.restore(&ctx) {
                    Ok(f) => f,
                    Err(e) => {
                        log::error!(
                            target: "script/event",
                            "handler for `{event}` in `{source_name}` failed to restore: {e}",
                            event = event.as_str(),
                        );
                        return;
                    }
                };
                let call_result: rquickjs::Result<()> = restored.call(());
                if let Err(e) = call_result.catch(&ctx) {
                    log::error!(
                        target: "script/event",
                        "handler for `{event}` in `{source_name}` threw: {e}",
                        event = event.as_str(),
                    );
                }
            });
        } else {
            let f = lua_fn.expect("Luau handler must carry a Function");
            if let Err(e) = f.call::<()>(()) {
                log::error!(
                    target: "script/event",
                    "handler for `{event}` in `{source_name}` threw: {e}",
                    event = event.as_str(),
                );
            }
        }
        let _ = lua; // unused when only quickjs handlers are present; keeps the param meaningful.
    }
}

/// Fire the `tick` event. Iterates handlers in registration order, invoking
/// each with `ctx`. A throwing handler is logged and swallowed.
pub(crate) fn fire_tick(
    handlers: &SharedHandlerTable,
    quickjs_ctx: &rquickjs::Context,
    lua: &mlua::Lua,
    call_ctx: ScriptCallContext,
) {
    let len = handlers.borrow().len();
    for i in 0..len {
        let (event, source_name, is_quickjs, js_persistent, lua_fn);
        {
            let table = handlers.borrow();
            let Some(h) = table.handlers.get(i) else {
                continue;
            };
            if h.event != EventKind::Tick {
                continue;
            }
            event = h.event;
            source_name = h.source_name.clone();
            match &h.callable {
                HandlerCallable::QuickJs(p) => {
                    is_quickjs = true;
                    js_persistent = Some(p.clone());
                    lua_fn = None;
                }
                HandlerCallable::Luau(f) => {
                    is_quickjs = false;
                    js_persistent = None;
                    lua_fn = Some(f.clone());
                }
            }
        }

        if is_quickjs {
            let p = js_persistent.expect("QuickJs handler must carry a Persistent");
            quickjs_ctx.with(|ctx| {
                let restored = match p.restore(&ctx) {
                    Ok(f) => f,
                    Err(e) => {
                        log::error!(
                            target: "script/event",
                            "handler for `{event}` in `{source_name}` failed to restore: {e}",
                            event = event.as_str(),
                        );
                        return;
                    }
                };
                let call_result: rquickjs::Result<()> = restored.call((call_ctx,));
                if let Err(e) = call_result.catch(&ctx) {
                    log::error!(
                        target: "script/event",
                        "handler for `{event}` in `{source_name}` threw: {e}",
                        event = event.as_str(),
                    );
                }
            });
        } else {
            let f = lua_fn.expect("Luau handler must carry a Function");
            if let Err(e) = f.call::<()>(call_ctx) {
                log::error!(
                    target: "script/event",
                    "handler for `{event}` in `{source_name}` threw: {e}",
                    event = event.as_str(),
                );
            }
        }
        let _ = lua;
    }
}

// ---------------------------------------------------------------------------
// Primitive installers.
//
// `registerHandler` cannot use the generic RegisterablePrimitive path because
// it takes a script function value, which does not implement FromJs / FromLua
// as a plain "value". Instead, we build `ScriptPrimitive` by hand and push it
// into the registry.
//
// `Arc` (rather than `Rc`) is the shape of the registry's installer aliases —
// the generic `RegisterablePrimitive` impls produce `Arc<dyn Fn...>`. Cloning
// closures that capture `Rc<RefCell<_>>` here is fine: scripting is strictly
// single-threaded (see context/lib/scripting.md §1), so the `Send + Sync`
// bound is moot. Matches the `primitives_registry` pattern; see that file for
// the parallel rationale.

use super::primitives_registry::{
    ContextScope, ParamInfo, PrimitiveRegistry, PrimitiveSignature, ScriptPrimitive,
};
use std::sync::Arc;

const REGISTER_HANDLER_NAME: &str = "registerHandler";
const REGISTER_HANDLER_DOC: &str =
    "Register a handler for an engine event. Currently accepts \"levelLoad\" or \"tick\".";

/// Build and install the `registerHandler` primitive into `registry`. The
/// primitive is `BehaviorOnly`: the definition context sees the stub which
/// throws `ScriptError::WrongContext`.
#[allow(clippy::arc_with_non_send_sync)]
pub(crate) fn register_register_handler(
    registry: &mut PrimitiveRegistry,
    handlers: SharedHandlerTable,
) {
    let quickjs_installer = {
        let handlers = handlers.clone();
        Arc::new(move |ctx: &rquickjs::Ctx<'_>| -> rquickjs::Result<()> {
            let globals = ctx.globals();
            let handlers = handlers.clone();
            let f = rquickjs::Function::new(
                ctx.clone(),
                move |ctx: rquickjs::Ctx<'_>,
                      event: String,
                      callback: Persistent<JsFunction<'static>>|
                      -> rquickjs::Result<()> {
                    let Some(kind) = EventKind::parse(&event) else {
                        let err = ScriptError::InvalidArgument {
                            reason: format!(
                                "registerHandler: unknown event `{event}` (expected `levelLoad` or `tick`)"
                            ),
                        };
                        return Err(
                            rquickjs::Exception::from_message(ctx, &err.to_string())?.throw()
                        );
                    };
                    handlers
                        .borrow_mut()
                        .push(kind, HandlerCallable::QuickJs(callback));
                    Ok(())
                },
            )?;
            globals.set(REGISTER_HANDLER_NAME, f)?;
            Ok(())
        }) as super::primitives_registry::QuickJsInstaller
    };

    let luau_installer = {
        let handlers = handlers.clone();
        Arc::new(move |lua: &mlua::Lua| -> mlua::Result<()> {
            let globals = lua.globals();
            let handlers = handlers.clone();
            let f = lua.create_function(
                move |_lua: &mlua::Lua, (event, callback): (String, LuaFunction)| {
                    let Some(kind) = EventKind::parse(&event) else {
                        return Err(mlua::Error::RuntimeError(format!(
                            "registerHandler: unknown event `{event}` (expected `levelLoad` or `tick`)"
                        )));
                    };
                    handlers
                        .borrow_mut()
                        .push(kind, HandlerCallable::Luau(callback));
                    Ok(())
                },
            )?;
            globals.set(REGISTER_HANDLER_NAME, f)?;
            Ok(())
        }) as super::primitives_registry::LuauInstaller
    };

    // Stub installers throw WrongContext — hand-roll them here (they can't go
    // through the private `make_*_stub` helpers in primitives_registry).
    let quickjs_stub_installer = {
        Arc::new(move |ctx: &rquickjs::Ctx<'_>| -> rquickjs::Result<()> {
            let globals = ctx.globals();
            let f = rquickjs::Function::new(ctx.clone(), move |ctx: rquickjs::Ctx<'_>| {
                let err = ScriptError::WrongContext {
                    primitive: REGISTER_HANDLER_NAME,
                    current: "definition",
                };
                Err::<rquickjs::Value, _>(
                    rquickjs::Exception::from_message(ctx, &err.to_string())?.throw(),
                )
            })?;
            globals.set(REGISTER_HANDLER_NAME, f)?;
            Ok(())
        }) as super::primitives_registry::QuickJsInstaller
    };

    let luau_stub_installer = {
        Arc::new(move |lua: &mlua::Lua| -> mlua::Result<()> {
            let globals = lua.globals();
            let f = lua.create_function(move |_lua: &mlua::Lua, _args: mlua::MultiValue| {
                let err = ScriptError::WrongContext {
                    primitive: REGISTER_HANDLER_NAME,
                    current: "definition",
                };
                Err::<mlua::Value, _>(mlua::Error::RuntimeError(err.to_string()))
            })?;
            globals.set(REGISTER_HANDLER_NAME, f)?;
            Ok(())
        }) as super::primitives_registry::LuauInstaller
    };

    let primitive = ScriptPrimitive {
        name: REGISTER_HANDLER_NAME,
        doc: REGISTER_HANDLER_DOC,
        signature: PrimitiveSignature {
            params: vec![
                ParamInfo {
                    name: "event",
                    ty_name: "String",
                },
                ParamInfo {
                    name: "handler",
                    ty_name: "HandlerFn",
                },
            ],
            return_ty_name: "()",
        },
        context_scope: ContextScope::BehaviorOnly,
        quickjs_installer,
        luau_installer,
        quickjs_stub_installer,
        luau_stub_installer,
    };
    registry.push_manual(primitive);
}

/// Register shared SDK types introduced by Sub-plan 5: `ScriptCallContext` and
/// the `HandlerFn` shorthand used in `registerHandler`'s signature.
pub(crate) fn register_shared_types(registry: &mut PrimitiveRegistry) {
    registry
        .register_type("ScriptCallContext")
        .field("delta", "f32", "Seconds since the previous tick.")
        .field(
            "time",
            "f32",
            "Seconds since level load; monotonic within a level.",
        )
        .finish();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scripting::ctx::ScriptCtx;
    use crate::scripting::primitives::register_all;
    use crate::scripting::runtime::{ScriptRuntime, ScriptRuntimeConfig, Which};

    #[test]
    fn event_kind_round_trip() {
        assert_eq!(EventKind::parse("levelLoad"), Some(EventKind::LevelLoad));
        assert_eq!(EventKind::parse("tick"), Some(EventKind::Tick));
        assert_eq!(EventKind::parse("nope"), None);
        assert_eq!(EventKind::LevelLoad.as_str(), "levelLoad");
        assert_eq!(EventKind::Tick.as_str(), "tick");
    }

    #[test]
    fn handler_table_clear_empties_storage() {
        let mut t = HandlerTable::new();
        t.set_current_source(Some("a.ts".into()));
        let lua = mlua::Lua::new();
        let f = lua.create_function(|_, ()| Ok(())).unwrap();
        t.push(EventKind::LevelLoad, HandlerCallable::Luau(f));
        assert_eq!(t.len(), 1);
        t.clear();
        assert_eq!(t.len(), 0);
    }

    fn runtime() -> (ScriptRuntime, ScriptCtx) {
        let ctx = ScriptCtx::new();
        let mut registry = PrimitiveRegistry::new();
        register_all(&mut registry, ctx.clone());
        let rt = ScriptRuntime::new(&registry, &ScriptRuntimeConfig::default(), &ctx).unwrap();
        (rt, ctx)
    }

    fn temp_script(name: &str, content: &str) -> std::path::PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let mut p = std::env::temp_dir();
        p.push(format!(
            "postretro_event_dispatch_{}_{}_{name}",
            std::process::id(),
            n,
        ));
        std::fs::write(&p, content).unwrap();
        p
    }

    #[test]
    fn level_load_handler_runs_once_with_no_argument_quickjs() {
        let (rt, ctx) = runtime();
        let path = temp_script(
            "level_load.js",
            r#"
            globalThis.__marker = 0;
            registerHandler("levelLoad", function() {
                globalThis.__marker += 1;
                // Passing *anything* (including `undefined`) and expecting a
                // zero-arg handler means `arguments.length` must be 0.
                if (arguments.length !== 0) {
                    throw new Error("expected 0 args, got " + arguments.length);
                }
            });
            "#,
        );
        rt.run_script_file(Which::Behavior, &path).unwrap();
        assert_eq!(ctx.handlers.borrow().len(), 1);
        rt.fire_level_load();
        // Read the marker back from the JS side.
        rt.quickjs().behavior_ctx().with(|ctx| {
            let v: u32 = ctx.eval("globalThis.__marker").unwrap();
            assert_eq!(v, 1);
        });
        // Firing tick must NOT run the levelLoad handler again.
        rt.fire_tick(ScriptCallContext {
            delta: 0.016,
            time: 0.016,
        });
        rt.quickjs().behavior_ctx().with(|ctx| {
            let v: u32 = ctx.eval("globalThis.__marker").unwrap();
            assert_eq!(v, 1);
        });
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn tick_handler_receives_call_context_quickjs() {
        let (rt, _ctx) = runtime();
        let path = temp_script(
            "tick.js",
            r#"
            globalThis.__ticks = [];
            registerHandler("tick", function(ctx) {
                globalThis.__ticks.push(ctx.delta, ctx.time);
            });
            "#,
        );
        rt.run_script_file(Which::Behavior, &path).unwrap();
        rt.fire_tick(ScriptCallContext {
            delta: 0.016_5,
            time: 0.016_5,
        });
        rt.fire_tick(ScriptCallContext {
            delta: 0.017_25,
            time: 0.033_75,
        });
        rt.quickjs().behavior_ctx().with(|ctx| {
            let values: Vec<f32> = ctx.eval("globalThis.__ticks").unwrap();
            assert_eq!(values.len(), 4);
            assert!((values[0] - 0.016_5).abs() < 1e-6, "{values:?}");
            assert!((values[1] - 0.016_5).abs() < 1e-6);
            assert!((values[2] - 0.017_25).abs() < 1e-6);
            assert!((values[3] - 0.033_75).abs() < 1e-6);
        });
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn multiple_handlers_fire_in_registration_order_quickjs() {
        let (rt, _ctx) = runtime();
        let a = temp_script(
            "a.js",
            r#"
            globalThis.__order = [];
            registerHandler("levelLoad", function() { globalThis.__order.push("a1"); });
            registerHandler("levelLoad", function() { globalThis.__order.push("a2"); });
            "#,
        );
        let b = temp_script(
            "b.js",
            r#"
            registerHandler("levelLoad", function() { globalThis.__order.push("b1"); });
            "#,
        );
        // Files are loaded in sorted order in main.rs; mirror that invariant
        // by loading `a` before `b` here.
        rt.run_script_file(Which::Behavior, &a).unwrap();
        rt.run_script_file(Which::Behavior, &b).unwrap();
        rt.fire_level_load();
        rt.quickjs().behavior_ctx().with(|ctx| {
            let order: Vec<String> = ctx.eval("globalThis.__order").unwrap();
            assert_eq!(order, vec!["a1", "a2", "b1"]);
        });
        let _ = std::fs::remove_file(&a);
        let _ = std::fs::remove_file(&b);
    }

    #[test]
    fn throwing_handler_is_logged_and_siblings_still_run_quickjs() {
        let (rt, _ctx) = runtime();
        let path = temp_script(
            "throw.js",
            r#"
            globalThis.__survived = false;
            registerHandler("levelLoad", function() {
                throw new Error("intentional");
            });
            registerHandler("levelLoad", function() {
                globalThis.__survived = true;
            });
            "#,
        );
        rt.run_script_file(Which::Behavior, &path).unwrap();
        rt.fire_level_load(); // must not panic
        rt.quickjs().behavior_ctx().with(|ctx| {
            let ok: bool = ctx.eval("globalThis.__survived").unwrap();
            assert!(ok, "second handler must run after first one throws");
        });
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn date_now_is_not_available_in_behavior_context() {
        let (rt, _ctx) = runtime();
        rt.quickjs().behavior_ctx().with(|ctx| {
            let msg: String = ctx
                .eval(
                    r#"
                    try { Date.now(); "no-throw" }
                    catch (e) {
                        // ReferenceError on bare access; capture the .name.
                        (e && e.name) ? e.name + ": " + (e.message || "") : String(e)
                    }
                    "#,
                )
                .unwrap();
            assert!(
                msg.contains("ReferenceError"),
                "expected ReferenceError for `Date`, got: {msg}",
            );
        });
    }

    #[test]
    fn register_handler_in_definition_context_throws_wrong_context_quickjs() {
        let (rt, _ctx) = runtime();
        rt.quickjs().definition_ctx().with(|ctx| {
            let msg: String = ctx
                .eval(
                    r#"
                    try {
                        registerHandler("levelLoad", function() {});
                        "no-throw"
                    } catch (e) { String(e.message || e) }
                    "#,
                )
                .unwrap();
            assert!(
                msg.contains("registerHandler") && msg.contains("not available"),
                "expected WrongContext message, got: {msg}",
            );
        });
    }

    #[test]
    fn os_time_and_os_clock_denied_in_luau() {
        let (rt, _ctx) = runtime();
        let (time_nil, clock_nil): (bool, bool) = rt
            .luau()
            .run_source(
                crate::scripting::luau::Which::Behavior,
                r#"return os.time == nil, os.clock == nil"#,
                "denied.luau",
            )
            .unwrap();
        assert!(time_nil, "os.time must be denied");
        assert!(clock_nil, "os.clock must be denied");
    }

    #[test]
    fn luau_level_load_handler_runs() {
        let (rt, _ctx) = runtime();
        // Luau sandbox freezes `_G`, so stash a marker in a Rust-side primitive
        // the handler can call. We seed a custom primitive via the runtime's
        // `lua` state directly — only possible from Rust, not from script.
        let marker = std::rc::Rc::new(std::cell::RefCell::new(0_u32));
        let lua = rt.luau().behavior_lua();
        {
            let marker = marker.clone();
            let bump = lua
                .create_function(move |_, ()| {
                    *marker.borrow_mut() += 1;
                    Ok(())
                })
                .unwrap();
            lua.globals().set("__bump", bump).unwrap();
        }
        let path = temp_script(
            "lua_load.luau",
            r#"registerHandler("levelLoad", function() __bump() end)"#,
        );
        rt.run_script_file(Which::Behavior, &path).unwrap();
        rt.fire_level_load();
        assert_eq!(*marker.borrow(), 1);
        // Firing tick must NOT run the levelLoad handler again.
        rt.fire_tick(ScriptCallContext {
            delta: 0.016,
            time: 0.016,
        });
        assert_eq!(*marker.borrow(), 1);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn luau_tick_handler_receives_ctx() {
        let (rt, _ctx) = runtime();
        let captured: std::rc::Rc<std::cell::RefCell<(f32, f32)>> =
            std::rc::Rc::new(std::cell::RefCell::new((0.0, 0.0)));
        let lua = rt.luau().behavior_lua();
        {
            let captured = captured.clone();
            let sink = lua
                .create_function(move |_, (d, t): (f32, f32)| {
                    *captured.borrow_mut() = (d, t);
                    Ok(())
                })
                .unwrap();
            lua.globals().set("__record", sink).unwrap();
        }
        let path = temp_script(
            "lua_tick.luau",
            r#"registerHandler("tick", function(ctx) __record(ctx.delta, ctx.time) end)"#,
        );
        rt.run_script_file(Which::Behavior, &path).unwrap();
        rt.fire_tick(ScriptCallContext {
            delta: 0.020,
            time: 0.100,
        });
        let (d, t) = *captured.borrow();
        assert!((d - 0.020).abs() < 1e-6, "delta: {d}");
        assert!((t - 0.100).abs() < 1e-6, "time: {t}");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn clear_level_handlers_empties_table() {
        let (rt, ctx) = runtime();
        let path = temp_script(
            "clearme.js",
            r#"registerHandler("levelLoad", function() {});"#,
        );
        rt.run_script_file(Which::Behavior, &path).unwrap();
        assert_eq!(ctx.handlers.borrow().len(), 1);
        rt.clear_level_handlers();
        assert_eq!(ctx.handlers.borrow().len(), 0);
        let _ = std::fs::remove_file(&path);
    }
}
