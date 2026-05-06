// QuickJS subsystem: one `rquickjs::Runtime` plus the definition context.
// See: context/lib/scripting.md
//
// Lifecycle:
//   * One `rquickjs::Runtime` per subsystem (owns GC, memory limit).
//   * Two contexts per level:
//       - `definition_ctx`: long-lived context for cross-script definition-scope
//         code. Carries DefinitionOnly/Both primitives as real functions.
//       - Ephemeral data context: a short-lived context created fresh per level
//         in `run_data_script_quickjs`. This is the correct entry point for
//         level-load data-script execution — not `definition_ctx`.
//   * `__collect_definitions` is a magic sink injected into the definition
//     context only. It is NOT a registered primitive.

use std::cell::RefCell;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::rc::Rc;

use rquickjs::{CatchResultExt, CaughtError, Context, Ctx, FromJs, Function, IntoJs, Runtime};
use serde::{Deserialize, Serialize};

use super::error::ScriptError;
use super::primitives_registry::{PrimitiveRegistry, ScriptPrimitive};

/// Default memory cap per QuickJS `Runtime`. 100 MB is a comfortable ceiling
/// for the single-level working set; tune after profiling real content.
const DEFAULT_MEMORY_LIMIT: usize = 100 * 1024 * 1024;

/// Engine-internal name for the accumulator sink installed into the definition
/// context. Leading underscore is the convention the type-definition generator
/// uses to skip engine-internal functions.
const COLLECT_FN_NAME: &str = "__collect_definitions";

/// SDK library prelude bundled at compile time. Evaluated in every QuickJS
/// context (definition + ephemeral data) before user scripts run so the
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
/// 100 MB; override for measured workloads.
#[derive(Clone, Copy, Debug)]
pub(crate) struct QuickJsConfig {
    pub(crate) memory_limit_bytes: usize,
}

impl Default for QuickJsConfig {
    fn default() -> Self {
        Self {
            memory_limit_bytes: DEFAULT_MEMORY_LIMIT,
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

/// rquickjs subsystem: one `Runtime`, one definition context, and the
/// primitive registry handle used by short-lived data contexts.
pub(crate) struct QuickJsSubsystem {
    runtime: Runtime,
    definition_ctx: Context,
    /// Kept so short-lived data contexts can reinstall primitives without
    /// requiring the caller to pass the registry back in. Each `ScriptPrimitive`
    /// is `Clone` with `Arc`-backed closures — cheap shallow copy.
    primitives: Vec<ScriptPrimitive>,
    /// Shared with the `__collect_definitions` function installed into
    /// `definition_ctx`.
    archetypes: ArchetypeAccumulator,
}

impl QuickJsSubsystem {
    /// Construct a subsystem: build the runtime, set the memory limit, and
    /// create the definition context with primitives installed.
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

        Ok(Self {
            runtime,
            definition_ctx,
            primitives: primitives_snapshot,
            archetypes,
        })
    }

    /// Borrow the definition context so callers can enter it via `ctx.with`.
    pub(crate) fn definition_ctx(&self) -> &Context {
        &self.definition_ctx
    }

    /// Borrow the underlying `rquickjs::Runtime`.
    pub(crate) fn runtime(&self) -> &Runtime {
        &self.runtime
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
        install_primitives(&ctx, primitives)?;
        install_collect_definitions(&ctx, archetypes)?;
        evaluate_prelude(&ctx)?;
        Ok(())
    })?;
    Ok(ctx)
}

/// Install each primitive into `ctx`.
fn install_primitives(ctx: &Ctx<'_>, primitives: &[ScriptPrimitive]) -> Result<(), ScriptError> {
    for p in primitives {
        (p.quickjs_installer)(ctx).map_err(|e| ScriptError::InvalidArgument {
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
    fn new_constructs_runtime_and_definition_context() {
        let (subsys, _ctx) = setup();
        subsys.definition_ctx().with(|ctx| {
            let v: u32 = ctx.eval("1 + 2").unwrap();
            assert_eq!(v, 3);
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
    fn run_script_returns_script_threw_and_context_is_not_poisoned() {
        let (subsys, _ctx) = setup();
        subsys.definition_ctx().with(|ctx| {
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
        subsys.definition_ctx().with(|ctx| {
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
    fn sdk_prelude_installs_globals() {
        // The prelude rewrites `export const world = ...` and friends as
        // `globalThis.x = ...` assignments. Verify each surfaces in the
        // definition context.
        let (subsys, _ctx) = setup();
        subsys.definition_ctx().with(|ctx| {
            let typeof_world: String = ctx.eval("typeof world").unwrap();
            assert_eq!(typeof_world, "object", "world missing");
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
                    .unwrap_or_else(|e| panic!("{fn_name}: {e}"));
                assert_eq!(kind, "function", "{fn_name}");
            }
        });
    }
}
