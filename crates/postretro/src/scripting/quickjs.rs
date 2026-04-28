// QuickJS subsystem: one `rquickjs::Runtime` plus definition and behavior contexts.
// See: context/lib/scripting.md
//
// Lifecycle:
//   * One `rquickjs::Runtime` per subsystem (owns GC, memory limit).
//   * Two `Context`s: `definition_ctx` runs definition scripts once per level
//     load; `behavior_ctx` runs behavior scripts for the level's lifetime.
//   * Definition context has DefinitionOnly/Both primitives as real functions;
//     BehaviorOnly primitives install as stubs that throw `ScriptError::WrongContext`.
//     The behavior context flips the scopes.
//   * `__collect_definitions` is a magic sink injected into the definition
//     context only. It is NOT a registered primitive.

use std::cell::RefCell;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::rc::Rc;

use rquickjs::{CatchResultExt, CaughtError, Context, Ctx, FromJs, Function, IntoJs, Runtime};
use serde::{Deserialize, Serialize};

use super::error::ScriptError;
use super::primitives_registry::{ContextScope, PrimitiveRegistry, ScriptPrimitive};

/// Default memory cap per QuickJS `Runtime`. 100 MB is a comfortable ceiling
/// for the single-level working set; tune after profiling real content.
const DEFAULT_MEMORY_LIMIT: usize = 100 * 1024 * 1024;

/// Engine-internal name for the accumulator sink installed into the definition
/// context. Leading underscore is the convention the type-definition generator
/// uses to skip engine-internal functions.
const COLLECT_FN_NAME: &str = "__collect_definitions";

/// SDK library prelude bundled at compile time. Evaluated in every QuickJS
/// context (definition + behavior + pooled) before user scripts run so the
/// vocabulary symbols (`world`, `flicker`, `pulse`, …) resolve as globals.
/// Regenerate with: `cargo run -p postretro-script-compiler -- --prelude
/// --sdk-root sdk/lib --out sdk/lib/prelude.js`.
const SDK_PRELUDE_JS: &str = include_str!("../../../../sdk/lib/prelude.js");

/// Evaluate the SDK prelude inside `ctx`. The prelude is plain script-mode JS
/// — its `globalThis.x = expr` tail leaves the symbols available to scripts
/// loaded into the same context later.
pub(crate) fn evaluate_prelude(ctx: &Ctx<'_>) -> Result<(), ScriptError> {
    ctx.eval::<(), _>(SDK_PRELUDE_JS)
        .map_err(|e| ScriptError::ScriptThrew {
            msg: format!("failed to evaluate SDK prelude: {e}"),
            source_name: "sdk/lib/prelude.js".to_string(),
        })?;
    Ok(())
}

/// Configuration for a [`QuickJsSubsystem`]. `memory_limit_bytes` defaults to
/// 100 MB; override for measured workloads. `pool_size` tunes the ephemeral-
/// context pool; it does NOT affect the shared behavior context, which is never
/// pooled.
#[derive(Clone, Copy, Debug)]
pub(crate) struct QuickJsConfig {
    pub(crate) memory_limit_bytes: usize,
    pub(crate) pool_size: usize,
}

impl Default for QuickJsConfig {
    fn default() -> Self {
        Self {
            memory_limit_bytes: DEFAULT_MEMORY_LIMIT,
            pool_size: super::pool::DEFAULT_POOL_SIZE,
        }
    }
}

/// Placeholder archetype record. Sufficient to prove the Rust/JS round-trip
/// for definition-time accumulation; a future archetype plan replaces this
/// with the real descriptor shape.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ArchetypeDescriptor {
    pub(crate) name: String,
}

/// Shared accumulator feeding `__collect_definitions`. Definition helpers push
/// into this `Vec`; the sink drains and returns it.
///
/// `Rc<RefCell<_>>` over `Arc<Mutex<_>>`: scripting is single-threaded (see
/// `scripting::ctx`) and `RefCell` does not poison.
pub(crate) type ArchetypeAccumulator = Rc<RefCell<Vec<ArchetypeDescriptor>>>;

/// rquickjs subsystem: one `Runtime`, one definition context, one behavior
/// context, and the primitive registry handle used to reinstall primitives on
/// context reload.
pub(crate) struct QuickJsSubsystem {
    runtime: Runtime,
    definition_ctx: Context,
    behavior_ctx: Context,
    /// Kept so `reload_behavior_context` can reinstall primitives without
    /// requiring the caller to pass the registry back in. Each `ScriptPrimitive`
    /// is `Clone` with `Arc`-backed closures — cheap shallow copy.
    primitives: Vec<ScriptPrimitive>,
    /// Shared with the `__collect_definitions` function installed into
    /// `definition_ctx`. Kept here so reloads can swap in a fresh accumulator
    /// without losing the Rust-side handle.
    archetypes: ArchetypeAccumulator,
}

impl QuickJsSubsystem {
    /// Construct a subsystem: build the runtime, set the memory limit, and
    /// create both contexts with their primitives installed.
    pub(crate) fn new(
        registry: &PrimitiveRegistry,
        cfg: &QuickJsConfig,
    ) -> Result<Self, ScriptError> {
        let runtime = Runtime::new().map_err(|e| ScriptError::InvalidArgument {
            reason: e.to_string(),
        })?;
        runtime.set_memory_limit(cfg.memory_limit_bytes);

        let archetypes: ArchetypeAccumulator = Rc::new(RefCell::new(Vec::new()));

        let primitives_snapshot: Vec<ScriptPrimitive> = registry.iter().cloned().collect();
        let definition_ctx =
            build_definition_context_from_snapshot(&runtime, &primitives_snapshot, &archetypes)?;
        let behavior_ctx = build_behavior_context_from_snapshot(&runtime, &primitives_snapshot)?;

        Ok(Self {
            runtime,
            definition_ctx,
            behavior_ctx,
            primitives: primitives_snapshot,
            archetypes,
        })
    }

    /// Borrow the definition context so callers can enter it via `ctx.with`.
    pub(crate) fn definition_ctx(&self) -> &Context {
        &self.definition_ctx
    }

    /// Borrow the behavior context so callers can enter it via `ctx.with`.
    pub(crate) fn behavior_ctx(&self) -> &Context {
        &self.behavior_ctx
    }

    /// Borrow the underlying `rquickjs::Runtime`. Used by the context pool
    /// so pooled contexts share the runtime (GC heap, memory limit) with the
    /// shared behavior/definition contexts.
    pub(crate) fn runtime(&self) -> &Runtime {
        &self.runtime
    }

    /// Borrow the primitive snapshot. Used by the context pool to pre-warm
    /// its contexts with the same primitive set the subsystem was built
    /// against.
    pub(crate) fn primitives(&self) -> &[ScriptPrimitive] {
        &self.primitives
    }

    /// Shared handle to the archetype accumulator. Exposed for tests and for
    /// the caller that drains it after evaluating definition scripts.
    pub(crate) fn archetypes(&self) -> &ArchetypeAccumulator {
        &self.archetypes
    }

    /// Drop the current behavior context and build a fresh one. Used by the
    /// dev-mode hot-reload path so re-running behavior scripts that contain
    /// top-level `const`/`let` declarations does not throw `SyntaxError:
    /// redeclaration` against state left over from the previous load.
    /// Primitives, `Date` deletion, and the SDK prelude are reinstalled from
    /// the snapshot.
    pub(crate) fn reload_behavior_context(&mut self) -> Result<(), ScriptError> {
        self.behavior_ctx = build_behavior_context_from_snapshot(&self.runtime, &self.primitives)?;
        Ok(())
    }
}

/// Evaluate `source` inside `ctx`, converting JS exceptions into
/// `ScriptError::ScriptThrew` and logging at `error` level. Must be called
/// inside a `ctx.with(...)` closure. A thrown exception does not poison the
/// context — subsequent calls continue to work.
pub(crate) fn run_script<'js, T>(ctx: &Ctx<'js>, source: &str, name: &str) -> Result<T, ScriptError>
where
    T: FromJs<'js>,
{
    match ctx.eval::<T, _>(source).catch(ctx) {
        Ok(v) => Ok(v),
        Err(caught) => Err(caught_error_to_script_error(caught, name)),
    }
}

/// Convert a `CaughtError` to `ScriptError::ScriptThrew` and log it.
/// Shared by `run_script` and future helpers that call into the context.
fn caught_error_to_script_error(caught: CaughtError<'_>, source: &str) -> ScriptError {
    let msg = caught.to_string();
    log::error!(target: "script/quickjs", "script `{source}` threw: {msg}");
    ScriptError::ScriptThrew {
        msg,
        source_name: source.to_string(),
    }
}

// ---------------------------------------------------------------------------
// Context construction helpers.

fn build_definition_context_from_snapshot(
    runtime: &Runtime,
    primitives: &[ScriptPrimitive],
    archetypes: &ArchetypeAccumulator,
) -> Result<Context, ScriptError> {
    let ctx = Context::full(runtime).map_err(|e| ScriptError::InvalidArgument {
        reason: e.to_string(),
    })?;
    let archetypes = archetypes.clone();
    ctx.with(|ctx| -> Result<(), ScriptError> {
        install_primitives(&ctx, primitives, ContextScope::DefinitionOnly)?;
        install_collect_definitions(&ctx, archetypes)?;
        evaluate_prelude(&ctx)?;
        Ok(())
    })?;
    Ok(ctx)
}

fn build_behavior_context_from_snapshot(
    runtime: &Runtime,
    primitives: &[ScriptPrimitive],
) -> Result<Context, ScriptError> {
    let ctx = Context::full(runtime).map_err(|e| ScriptError::InvalidArgument {
        reason: e.to_string(),
    })?;
    ctx.with(|ctx| -> Result<(), ScriptError> {
        install_primitives(&ctx, primitives, ContextScope::BehaviorOnly)?;
        deny_wall_clock(&ctx)?;
        evaluate_prelude(&ctx)?;
        Ok(())
    })?;
    Ok(ctx)
}

/// Remove wall-clock access from the behavior context globals. Scripts must
/// take their timing from `ScriptCallContext` only, not from `Date.now()`.
/// Deleting the global makes `Date` a `ReferenceError` on access.
fn deny_wall_clock(ctx: &Ctx<'_>) -> Result<(), ScriptError> {
    let globals = ctx.globals();
    // `Object.remove` is the rquickjs API for true delete. Assigning
    // `undefined` would leave the property present and `typeof Date` would
    // still evaluate; `remove` causes a `ReferenceError` on bare access.
    globals
        .remove("Date")
        .map_err(|e| ScriptError::InvalidArgument {
            reason: format!("failed to delete `Date` global: {e}"),
        })?;
    Ok(())
}

/// Install each primitive into `ctx`. `target` names the scope this context
/// represents:
///   * `DefinitionOnly` → install `DefinitionOnly` + `Both` as real, install
///     `BehaviorOnly` as stubs.
///   * `BehaviorOnly` → install `BehaviorOnly` + `Both` as real, install
///     `DefinitionOnly` as stubs.
///   * `Both` is not a valid target here — it only labels primitives.
fn install_primitives(
    ctx: &Ctx<'_>,
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
            &p.quickjs_installer
        } else {
            &p.quickjs_stub_installer
        };
        installer(ctx).map_err(|e| ScriptError::InvalidArgument {
            reason: e.to_string(),
        })?;
    }
    Ok(())
}

/// Install the `__collect_definitions` sink as a global function on `ctx`.
/// Signature: `() -> ArchetypeDescriptor[]`. Drains the accumulator on each
/// call so a single definition pass cannot double-report.
fn install_collect_definitions(
    ctx: &Ctx<'_>,
    archetypes: ArchetypeAccumulator,
) -> Result<(), ScriptError> {
    let globals = ctx.globals();
    // `Vec<DescriptorJs>`: rquickjs' `IntoJs` for `Vec` encodes as a JS array;
    // the closure blanket impl handles `'js` lifetime threading without explicit
    // naming (not possible for closures in stable Rust).
    let f = Function::new(
        ctx.clone(),
        move |ctx: rquickjs::Ctx<'_>| -> rquickjs::Result<Vec<DescriptorJs>> {
            let result = catch_unwind(AssertUnwindSafe(|| {
                let drained: Vec<ArchetypeDescriptor> = archetypes.borrow_mut().drain(..).collect();
                drained
                    .into_iter()
                    .map(DescriptorJs::from)
                    .collect::<Vec<_>>()
            }));
            match result {
                Ok(v) => Ok(v),
                Err(_) => {
                    let err = ScriptError::Panicked {
                        name: COLLECT_FN_NAME,
                    };
                    Err(rquickjs::Exception::from_message(ctx.clone(), &err.to_string())?.throw())
                }
            }
        },
    )
    .map_err(|e| ScriptError::InvalidArgument {
        reason: e.to_string(),
    })?;
    globals
        .set(COLLECT_FN_NAME, f)
        .map_err(|e| ScriptError::InvalidArgument {
            reason: e.to_string(),
        })?;
    Ok(())
}

/// JS-facing shape for an `ArchetypeDescriptor`. Separate from the
/// serde-serializable record so the wire encoding stays decoupled from the
/// Rust-side representation.
struct DescriptorJs {
    name: String,
}

impl From<ArchetypeDescriptor> for DescriptorJs {
    fn from(d: ArchetypeDescriptor) -> Self {
        Self { name: d.name }
    }
}

impl<'js> IntoJs<'js> for DescriptorJs {
    fn into_js(self, ctx: &Ctx<'js>) -> rquickjs::Result<rquickjs::Value<'js>> {
        let o = rquickjs::Object::new(ctx.clone())?;
        o.set("name", self.name)?;
        Ok(o.into_value())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scripting::ctx::ScriptCtx;
    use crate::scripting::primitives::register_all;
    use crate::scripting::primitives_registry::ContextScope;

    fn setup() -> (QuickJsSubsystem, ScriptCtx) {
        let ctx = ScriptCtx::new();
        let mut registry = PrimitiveRegistry::new();
        register_all(&mut registry, ctx.clone());
        let subsys = QuickJsSubsystem::new(&registry, &QuickJsConfig::default()).unwrap();
        (subsys, ctx)
    }

    #[test]
    fn new_constructs_runtime_and_both_contexts() {
        let (subsys, _ctx) = setup();
        // Both contexts must be independently usable.
        subsys.definition_ctx().with(|ctx| {
            let v: u32 = ctx.eval("1 + 2").unwrap();
            assert_eq!(v, 3);
        });
        subsys.behavior_ctx().with(|ctx| {
            let v: u32 = ctx.eval("4 * 5").unwrap();
            assert_eq!(v, 20);
        });
    }

    #[test]
    fn collect_definitions_round_trips_through_accumulator() {
        // Build a subsystem, inject a test-only `defineEntity` stub that
        // pushes a fixed descriptor, evaluate a script that calls it, then
        // have Rust call `__collect_definitions` and verify the drain.
        let (subsys, _ctx) = setup();
        let archetypes = subsys.archetypes().clone();

        subsys.definition_ctx().with(|ctx| {
            // Install test-only defineEntity stub. It closes over the same
            // accumulator as __collect_definitions.
            let accum = archetypes.clone();
            let define = Function::new(ctx.clone(), move |name: String| -> rquickjs::Result<()> {
                accum.borrow_mut().push(ArchetypeDescriptor { name });
                Ok(())
            })
            .unwrap();
            ctx.globals().set("defineEntity", define).unwrap();

            // Script pushes three archetypes.
            ctx.eval::<(), _>(
                r#"
                defineEntity("goblin");
                defineEntity("orc");
                defineEntity("troll");
                "#,
            )
            .unwrap();

            // Drain via the magic sink and assert shape.
            let names: Vec<String> = ctx
                .eval(
                    r#"
                    __collect_definitions().map(d => d.name)
                    "#,
                )
                .unwrap();
            assert_eq!(names, vec!["goblin", "orc", "troll"]);
        });

        // Accumulator must be drained after the call.
        assert!(archetypes.borrow().is_empty());
    }

    #[test]
    fn definition_context_rejects_behavior_only_primitive() {
        // `emit_event` is BehaviorOnly — in the definition context it must
        // exist as a stub that throws WrongContext.
        let (subsys, _ctx) = setup();
        subsys.definition_ctx().with(|ctx| {
            let msg: String = ctx
                .eval(
                    r#"
                    try {
                        emit_event({ kind: "boom", payload: {} });
                        "no-throw"
                    } catch (e) { String(e.message || e) }
                    "#,
                )
                .unwrap();
            assert!(
                msg.contains("emit_event") && msg.contains("not available"),
                "expected WrongContext message mentioning emit_event, got: {msg}",
            );
        });
    }

    #[test]
    fn behavior_context_rejects_definition_only_primitive() {
        // The day-one set has no DefinitionOnly primitive. Register a
        // throwaway one here to prove the stub install path.
        let script_ctx = ScriptCtx::new();
        let mut registry = PrimitiveRegistry::new();
        register_all(&mut registry, script_ctx.clone());
        registry
            .register("test_def_only", || -> Result<u32, ScriptError> { Ok(7) })
            .scope(ContextScope::DefinitionOnly)
            .finish();

        let subsys = QuickJsSubsystem::new(&registry, &QuickJsConfig::default()).unwrap();
        subsys.behavior_ctx().with(|ctx| {
            let msg: String = ctx
                .eval(
                    r#"
                    try { test_def_only(); "no-throw" }
                    catch (e) { String(e.message || e) }
                    "#,
                )
                .unwrap();
            assert!(
                msg.contains("test_def_only") && msg.contains("not available"),
                "expected WrongContext for test_def_only, got: {msg}",
            );
        });
        // And available in the definition context.
        subsys.definition_ctx().with(|ctx| {
            let v: u32 = ctx.eval("test_def_only()").unwrap();
            assert_eq!(v, 7);
        });
        // `script_ctx` is retained until test scope-end so the registry's Rc
        // handles stay live for the duration of the subsystem under test.
        let _ = script_ctx;
    }

    #[test]
    fn run_script_returns_script_threw_and_context_is_not_poisoned() {
        let (subsys, _ctx) = setup();
        subsys.behavior_ctx().with(|ctx| {
            let err = run_script::<()>(&ctx, "throw new Error('boom');", "test.js")
                .expect_err("script should throw");
            match err {
                ScriptError::ScriptThrew { msg, source_name } => {
                    assert_eq!(source_name, "test.js");
                    assert!(msg.contains("boom"), "msg: {msg}");
                }
                other => panic!("expected ScriptThrew, got {other:?}"),
            }
            // Context must still be usable.
            let v: u32 = run_script::<u32>(&ctx, "1 + 1", "followup.js").unwrap();
            assert_eq!(v, 2);
        });
    }

    #[test]
    fn panicking_primitive_does_not_unwind_past_ffi_boundary() {
        // Verify through the full subsystem stack that a Rust-side panic
        // reaches the script as a catchable exception. `boom` captures no
        // `ScriptCtx`, so this test skips the usual setup().
        let mut registry = PrimitiveRegistry::new();
        registry
            .register("boom", || -> Result<u32, ScriptError> {
                panic!("intentional");
            })
            .scope(ContextScope::Both)
            .finish();
        let subsys = QuickJsSubsystem::new(&registry, &QuickJsConfig::default()).unwrap();
        subsys.behavior_ctx().with(|ctx| {
            let msg: String = ctx
                .eval(
                    r#"
                    try { boom(); "no-throw" }
                    catch (e) { String(e.message || e) }
                    "#,
                )
                .unwrap();
            assert!(msg.contains("panicked"), "got: {msg}");
        });
    }

    #[test]
    fn end_to_end_transform_component_round_trip() {
        // Behavior script spawns an entity, writes a fully-populated Transform
        // via set_component, reads it back via get_component, and asserts the
        // round-trip holds within float tolerance.
        //
        // `ComponentKind` crosses as a bare string (`"Transform"`) per
        // `scripting::conv`.
        let (subsys, ctx_handle) = setup();
        subsys.behavior_ctx().with(|ctx| {
            let out: rquickjs::Object = ctx
                .eval(
                    r#"
                    const id = spawn_entity({
                        position: { x: 0, y: 0, z: 0 },
                        rotation: { pitch: 0, yaw: 0, roll: 0 },
                        scale:    { x: 1, y: 1, z: 1 },
                    });
                    const input = {
                        kind: "Transform",
                        position: { x: 1.5,  y: 2.5, z: -3.25 },
                        rotation: { pitch: 15.0, yaw: 45.0, roll: -30.0 },
                        scale:    { x: 2.0, y: 2.0, z: 2.0 },
                    };
                    set_component(id, "Transform", input);
                    const out = get_component(id, "Transform");
                    out
                    "#,
                )
                .unwrap();

            // Assert returned shape matches the input within float tolerance.
            let kind: String = out.get("kind").unwrap();
            assert_eq!(kind, "Transform");
            let pos: rquickjs::Object = out.get("position").unwrap();
            let rot: rquickjs::Object = out.get("rotation").unwrap();
            let scl: rquickjs::Object = out.get("scale").unwrap();

            let px: f32 = pos.get("x").unwrap();
            let py: f32 = pos.get("y").unwrap();
            let pz: f32 = pos.get("z").unwrap();
            assert!((px - 1.5).abs() < 1e-4);
            assert!((py - 2.5).abs() < 1e-4);
            assert!((pz - (-3.25)).abs() < 1e-4);

            let pitch: f32 = rot.get("pitch").unwrap();
            let yaw: f32 = rot.get("yaw").unwrap();
            let roll: f32 = rot.get("roll").unwrap();
            assert!((pitch - 15.0).abs() < 1e-2, "pitch: {pitch}");
            assert!((yaw - 45.0).abs() < 1e-2, "yaw: {yaw}");
            assert!((roll - (-30.0)).abs() < 1e-2, "roll: {roll}");

            let sx: f32 = scl.get("x").unwrap();
            let sy: f32 = scl.get("y").unwrap();
            let sz: f32 = scl.get("z").unwrap();
            assert!((sx - 2.0).abs() < 1e-4);
            assert!((sy - 2.0).abs() < 1e-4);
            assert!((sz - 2.0).abs() < 1e-4);
        });

        // And the registry actually stored something. The script spawned
        // exactly one entity; its id is not exposed to Rust here, so we only
        // assert that the first slot (index 0, generation 0) is now live.
        assert!(
            ctx_handle
                .registry
                .borrow()
                .exists(crate::scripting::registry::EntityId::from_raw(0))
        );
    }

    #[test]
    fn sdk_prelude_installs_globals() {
        // The prelude rewrites `export const world = ...` and friends as
        // `globalThis.x = ...` assignments. Verify each surfaces in both
        // shared contexts.
        let (subsys, _ctx) = setup();
        for ctx_label in ["definition", "behavior"] {
            let ctx_handle = if ctx_label == "definition" {
                subsys.definition_ctx()
            } else {
                subsys.behavior_ctx()
            };
            ctx_handle.with(|ctx| {
                let typeof_world: String = ctx.eval("typeof world").unwrap();
                assert_eq!(typeof_world, "object", "{ctx_label}: world missing");
                for fn_name in [
                    "flicker",
                    "pulse",
                    "colorShift",
                    "sweep",
                    "timeline",
                    "sequence",
                ] {
                    let kind: String = ctx
                        .eval(format!("typeof {fn_name}").as_str())
                        .unwrap_or_else(|e| panic!("{ctx_label}/{fn_name}: {e}"));
                    assert_eq!(kind, "function", "{ctx_label}/{fn_name}");
                }
            });
        }
    }

    #[test]
    fn reload_behavior_context_allows_const_redeclaration() {
        // A script with a top-level `const` reused across hot reloads would
        // throw `SyntaxError: redeclaration` on the second eval against the
        // same context. Rebuilding the behavior context must produce a fresh
        // global scope where the same source evaluates cleanly.
        let (mut subsys, _ctx) = setup();
        let script = "const x = 1;";
        subsys.behavior_ctx().with(|ctx| {
            ctx.eval::<(), _>(script).unwrap();
        });
        subsys.reload_behavior_context().unwrap();
        subsys.behavior_ctx().with(|ctx| {
            ctx.eval::<(), _>(script).unwrap();
        });
    }

}
