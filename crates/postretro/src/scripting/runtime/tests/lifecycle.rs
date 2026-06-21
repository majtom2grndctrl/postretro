// Runtime construction, file-dispatch, and primitive-install perf budgets.

use super::super::types::Which;
use super::*;

#[test]
fn new_constructs_both_subsystems() {
    let (_rt, _ctx) = runtime();
}

#[test]
fn run_script_file_rejects_unknown_extension() {
    let (rt, _ctx) = runtime();
    let path = temp_script("dispatch.py", "print('nope')\n");
    let err = rt.run_script_file(Which::Definition, &path).unwrap_err();
    match err {
        ScriptError::InvalidArgument { reason } => {
            assert!(reason.contains(".py"), "reason: {reason}");
        }
        other => panic!("expected InvalidArgument, got {other:?}"),
    }
    fs::remove_file(&path).ok();
}

// Perf budgets (20 ms / 5 ms) are release-build targets — debug builds
// will exceed them. Assertions gate on `!cfg!(debug_assertions)` so the
// tests still run and print timing in debug without failing CI.

#[test]
fn shared_definition_context_primitive_install_under_20ms_release() {
    use std::time::Instant;
    let ctx = ScriptCtx::new();
    let mut registry = PrimitiveRegistry::new();
    register_all(&mut registry, ctx.clone());

    let cfg = ScriptRuntimeConfig {
        quickjs: crate::scripting::quickjs::QuickJsConfig {
            memory_limit_bytes: 100 * 1024 * 1024,
        },
        luau: crate::scripting::luau::LuauConfig::default(),
    };

    let start = Instant::now();
    let _rt = ScriptRuntime::new(&registry, &cfg, &ctx).unwrap();
    let elapsed = start.elapsed();

    if !cfg!(debug_assertions) {
        assert!(
            elapsed.as_millis() < 20,
            "shared-context install took {elapsed:?}, budget 20ms",
        );
    } else {
        eprintln!("shared-context install (debug build, not asserting): {elapsed:?}",);
    }
}

#[test]
fn thousand_primitive_calls_under_5ms_release() {
    use std::time::Instant;
    let (rt, _ctx) = runtime();

    let start = Instant::now();
    rt.quickjs().definition_ctx().with(|ctx| {
        ctx.eval::<(), _>(
            r#"
                for (let i = 0; i < 1000; i++) {
                    entityExists(i);
                }
                "#,
        )
        .unwrap();
    });
    let elapsed = start.elapsed();

    if !cfg!(debug_assertions) {
        assert!(
            elapsed.as_millis() < 5,
            "1000 primitive calls took {elapsed:?}, budget 5ms",
        );
    } else {
        eprintln!("1000 primitive calls (debug build, not asserting): {elapsed:?}",);
    }
}
