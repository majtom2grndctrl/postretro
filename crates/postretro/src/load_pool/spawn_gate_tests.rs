// Grep gate: load stages start no threads of their own; the load pool is the
// one load-stage module that may.

/// Every load-stage source file, relative to `src/`. Later stages add theirs.
const LOAD_STAGE_MODULES: &[&str] = &[
    LOAD_POOL_MODULE,
    "level_load/commit.rs",
    "level_load/coordinator.rs",
    "level_load/failure.rs",
    "level_load/failure_tests.rs",
    "level_load/gpu_gate_tests.rs",
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
    "session/level_streaming/hooks.rs",
    "session/level_streaming/level_data.rs",
    "startup/level_loads.rs",
    "startup/lifecycle.rs",
    "startup/lifecycle_beside.rs",
    "startup/lifecycle_beside_tests.rs",
    "startup/lifecycle_boot_state.rs",
    "startup/lifecycle_commit.rs",
    "startup/lifecycle_commit_tests.rs",
    "startup/lifecycle_finish.rs",
    "startup/lifecycle_install.rs",
    "startup/lifecycle_install_presentation.rs",
    "startup/lifecycle_install_world.rs",
    "startup/lifecycle_load.rs",
    "startup/lifecycle_net.rs",
    "startup/lifecycle_spawn_residency.rs",
    "startup/lifecycle_sprite_collections.rs",
    "startup/lifecycle_world_cpu.rs",
    "startup/loading_screen.rs",
    "startup/loading_screen_images.rs",
    "startup/loading_screen_tests.rs",
    "startup/lifecycle_model_sweep.rs",
    "startup/settling.rs",
];

/// Load-stage modules whose own tests start stand-in threads: only their
/// production half (before the inline test module) is scanned. The app's one
/// retirement list polls every load's stages.
const LOAD_STAGE_PRODUCTION_HALVES: &[&str] = &["app/stage_retirements.rs"];

/// The marker that starts a module's inline tests.
const INLINE_TESTS: &str = "\n#[cfg(test)]\nmod tests";

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

// The production halves of load-stage modules whose own tests spawn
// stand-ins start no thread either.
#[test]
fn load_stage_production_halves_spawn_no_threads() {
    let strays: Vec<String> = LOAD_STAGE_PRODUCTION_HALVES
        .iter()
        .flat_map(|module| {
            let source = read_module(module);
            let production = source
                .split(INLINE_TESTS)
                .next()
                .expect("split yields the production prefix")
                .to_string();
            assert!(
                production.len() < source.len(),
                "{module} has the inline test module the split expects"
            );
            stray_thread_starts(module, &production)
                .into_iter()
                .map(move |line| format!("{module}:{line}"))
        })
        .collect();
    assert!(
        strays.is_empty(),
        "load stages start threads outside the load pool: {strays:?}"
    );
}

// Every file under `level_load/` is a load-stage module, so a new stage file
// cannot escape the gate by being left off the list.
#[test]
fn every_level_load_file_is_a_load_stage_module() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/level_load");
    let unlisted: Vec<String> = std::fs::read_dir(&dir)
        .expect("level_load/ is readable")
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            name.ends_with(".rs").then(|| format!("level_load/{name}"))
        })
        .filter(|module| !LOAD_STAGE_MODULES.contains(&module.as_str()))
        .collect();
    assert!(
        unlisted.is_empty(),
        "level_load/ files missing from LOAD_STAGE_MODULES: {unlisted:?}"
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
