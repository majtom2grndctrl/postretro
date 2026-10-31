// Grep gate: load stages start no threads of their own; the load pool is the
// one load-stage module that may.

/// Every load-stage source file, relative to `src/`. Later stages add theirs.
const LOAD_STAGE_MODULES: &[&str] = &[
    LOAD_POOL_MODULE,
    "level_load/coordinator.rs",
    "level_load/failure.rs",
    "level_load/failure_tests.rs",
    "level_load/inputs.rs",
    "level_load/mod.rs",
    "level_load/model_cache.rs",
    "level_load/model_cache_tests.rs",
    "level_load/model_key.rs",
    "level_load/model_prediction.rs",
    "level_load/model_tests.rs",
    "level_load/oracle_tests.rs",
    "level_load/products.rs",
    "level_load/sound_tests.rs",
    "level_load/stages.rs",
    "level_load/stages_tests.rs",
    "level_load/tasks.rs",
    "level_load/test_support.rs",
    "level_load/tests.rs",
    "level_load/upload.rs",
    "session/level_streaming/level_data.rs",
    "startup/level_loads.rs",
    "startup/lifecycle.rs",
    "startup/lifecycle_boot_state.rs",
    "startup/lifecycle_commit.rs",
    "startup/lifecycle_finish.rs",
    "startup/lifecycle_load.rs",
    "startup/loading_screen.rs",
    "startup/loading_screen_images.rs",
    "startup/loading_screen_tests.rs",
    "startup/lifecycle_model_sweep.rs",
    "startup/settling.rs",
];

/// The one load-stage module allowed to start threads.
const LOAD_POOL_MODULE: &str = "load_pool/mod.rs";

const THREAD_STARTS: &[&str] = &["thread::spawn", "thread::Builder", "thread::scope"];

/// 1-based lines of `source` that start a thread, ignoring line comments.
fn thread_start_lines(source: &str) -> Vec<usize> {
    source
        .lines()
        .enumerate()
        .filter(|(_, line)| {
            let code = line.split("//").next().unwrap_or("");
            THREAD_STARTS.iter().any(|start| code.contains(start))
        })
        .map(|(index, _)| index + 1)
        .collect()
}

/// `module`'s stray thread starts: every one, unless it is the pool.
fn stray_thread_starts(module: &str, source: &str) -> Vec<usize> {
    if module == LOAD_POOL_MODULE {
        Vec::new()
    } else {
        thread_start_lines(source)
    }
}

fn read_module(module: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join(module);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("load-stage module {} unreadable: {err}", path.display()))
}

#[test]
fn load_stages_spawn_no_threads() {
    let strays: Vec<String> = LOAD_STAGE_MODULES
        .iter()
        .flat_map(|module| {
            stray_thread_starts(module, &read_module(module))
                .into_iter()
                .map(move |line| format!("{module}:{line}"))
        })
        .collect();
    assert!(
        strays.is_empty(),
        "load stages start threads outside the load pool: {strays:?}"
    );
}

#[test]
fn load_stages_spawn_gate_catches_a_stray_spawn() {
    let fixture = "fn decode() {\n    // thread::spawn in a comment is fine\n    let worker = std::thread::spawn(|| {});\n    std::thread::Builder::new().name(\"x\".into());\n    std::thread::scope(|_| {});\n}\n";
    assert_eq!(
        stray_thread_starts("startup/loading_screen_images.rs", fixture),
        [3, 4, 5]
    );
}

#[test]
fn load_stages_spawn_gate_passes_the_pool_module() {
    let pool = read_module(LOAD_POOL_MODULE);
    assert!(
        !thread_start_lines(&pool).is_empty(),
        "the pool starts its workers, so its allowance is not vacuous"
    );
    assert!(stray_thread_starts(LOAD_POOL_MODULE, &pool).is_empty());
}
