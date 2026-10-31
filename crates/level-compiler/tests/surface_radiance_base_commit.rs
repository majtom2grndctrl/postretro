//! The surface radiance cache against the compiler at its plan's base commit
//! (d2bda6032): the animated deltas (id 27) bake the base commit's bytes, and
//! a warm build over the base commit's cache hits every entry whose key
//! helpers left `sh_group.rs` unchanged. Both need that compiler: set
//! `POSTRETRO_BASE_PRL_BUILD` to its `prl-build` (each test skips, saying
//! so, when it is unset). `POSTRETRO_NEW_PRL_BUILD` may name the current
//! compiler, a `--release` build say; it defaults to this target's.
//! See: context/lib/build_pipeline.md §Build Cache

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::process::Command;

use postretro_level_format::{SectionId, read_container, read_section_data};

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("crates/level-compiler sits two below the workspace")
        .to_path_buf()
}

/// The base commit's compiler and the current one, or `None` (with a note)
/// when the base compiler is not configured.
fn compilers(test: &str) -> Option<(PathBuf, PathBuf)> {
    let Some(base) = std::env::var_os("POSTRETRO_BASE_PRL_BUILD") else {
        eprintln!("{test}: skipped, POSTRETRO_BASE_PRL_BUILD is unset");
        return None;
    };
    let base = PathBuf::from(base);
    assert!(base.is_file(), "no base compiler at {}", base.display());
    let current = std::env::var_os("POSTRETRO_NEW_PRL_BUILD").map_or_else(
        || PathBuf::from(env!("CARGO_BIN_EXE_prl-build")),
        PathBuf::from,
    );
    Some((base, current))
}

/// `PATH` with this target's directory first, so a compiler without a
/// `scripts-build` beside it finds this checkout's (the script compiler is
/// unchanged on the branch).
fn path_with_scripts_build() -> OsString {
    let dir = Path::new(env!("CARGO_BIN_EXE_prl-build"))
        .parent()
        .expect("the compiler sits in a target directory")
        .to_path_buf();
    let mut paths = vec![dir];
    if let Some(path) = std::env::var_os("PATH") {
        paths.extend(std::env::split_paths(&path));
    }
    std::env::join_paths(paths).expect("PATH joins")
}

/// Compile `map` with `compiler` to `output` and return its log.
fn compile(compiler: &Path, map: &Path, output: &Path, scratch: &Path, args: &[&str]) -> String {
    let out = Command::new(compiler)
        .current_dir(workspace_root())
        .env("PATH", path_with_scripts_build())
        .env("RUST_LOG", "info,prl_build::delta_sh_cache=debug")
        .arg(map)
        .arg("-o")
        .arg(output)
        .arg("--baked-root")
        .arg(scratch.join("baked"))
        .arg("--no-tui")
        .args(args)
        .output()
        .expect("spawn prl-build");
    let log =
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "{} failed on {}:\n{log}",
        compiler.display(),
        map.display()
    );
    log
}

fn section(prl: &Path, id: SectionId) -> Option<Vec<u8>> {
    let bytes = std::fs::read(prl).expect("read compiled PRL");
    let mut cursor = Cursor::new(bytes);
    let metadata = read_container(&mut cursor).expect("read PRL container");
    read_section_data(&mut cursor, &metadata, id as u32).expect("read section")
}

fn map(name: &str) -> PathBuf {
    workspace_root().join(format!("content/dev/maps/{name}.map"))
}

/// The animated-light fixture both tests compile.
const ANIMATED_FIXTURE: &str = "test_animated_weight_maps_mixed";

// AC 27: `--release` PRLs of `stress-warren-mini` and an animated-light
// fixture from the base commit's compiler and the current one carry the
// same id-27 bytes; the SH base (id 34) and its cluster payloads (id 50)
// are what the branch changes.
#[test]
#[ignore = "needs POSTRETRO_BASE_PRL_BUILD; four --release compiles including stress-warren-mini: run on demand with -- --ignored"]
fn id_27_equals_the_base_commits_bytes() {
    let Some((base, current)) = compilers("id_27_equals_the_base_commits_bytes") else {
        return;
    };
    let scratch = tempfile::tempdir().expect("scratch dir");
    for name in ["stress-warren-mini", ANIMATED_FIXTURE] {
        let built: Vec<PathBuf> = [("base", &base), ("current", &current)]
            .into_iter()
            .map(|(label, compiler)| {
                let output = scratch.path().join(format!("{name}-{label}.prl"));
                compile(
                    compiler,
                    &map(name),
                    &output,
                    scratch.path(),
                    &["--release"],
                );
                output
            })
            .collect();
        let at = |id| (section(&built[0], id), section(&built[1], id));
        let (base_27, current_27) = at(SectionId::DeltaShVolumes);
        assert!(base_27.is_some(), "{name} bakes animated deltas");
        assert!(
            base_27 == current_27,
            "{name}: id 27 differs from the base commit's"
        );
        for id in [SectionId::OctahedralShVolume, SectionId::ClusterShPayloads] {
            let (base_bytes, current_bytes) = at(id);
            eprintln!(
                "{name}: {id:?} {} the base commit's",
                if base_bytes == current_bytes {
                    "equals"
                } else {
                    "differs from"
                }
            );
        }
    }
}

/// Each cache stage's hit and miss counts in a `RUST_LOG` capture.
fn cache_outcomes(log: &str) -> BTreeMap<String, (usize, usize)> {
    let mut outcomes: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    for line in log.lines() {
        let Some((_, rest)) = line.split_once("[cache] ") else {
            continue;
        };
        let mut words = rest.split_whitespace();
        let (Some(stage), Some(outcome)) = (words.next(), words.next()) else {
            continue;
        };
        let counts = outcomes.entry(stage.to_owned()).or_default();
        match outcome {
            "hit" => counts.0 += 1,
            "miss" => counts.1 += 1,
            _ => {}
        }
    }
    outcomes
}

// AC 27: a warm build over a cache the base commit's compiler wrote hits
// every pre-change indirect-delta, direct-SH, billboard-scatter and
// chunk-light-list entry with no miss (key helpers leaving `sh_group.rs`
// kept their bytes), and misses the SH base's new layer and volume entries.
// The delta entries log at debug, so the capture raises that module.
#[test]
#[ignore = "needs POSTRETRO_BASE_PRL_BUILD; two warm compiles: run on demand with -- --ignored"]
fn a_warm_build_over_the_base_commits_cache_hits_every_pre_change_entry() {
    let Some((base, current)) =
        compilers("a_warm_build_over_the_base_commits_cache_hits_every_pre_change_entry")
    else {
        return;
    };
    let scratch = tempfile::tempdir().expect("scratch dir");
    let cache = scratch.path().join("cache");
    let warm = |compiler: &Path, label: &str| {
        compile(
            compiler,
            &map(ANIMATED_FIXTURE),
            &scratch.path().join(format!("{label}.prl")),
            scratch.path(),
            &["--cache-dir", cache.to_str().unwrap()],
        )
    };
    warm(&base, "base");
    let outcomes = cache_outcomes(&warm(&current, "current"));
    eprintln!("cache outcomes over the base commit's cache: {outcomes:?}");
    for stage in [
        "indirect_delta_sh_subblock",
        "direct_sh_volume",
        "billboard_direct_scatter",
        "animated_billboard_direct_scatter",
        "chunk_light_list",
    ] {
        let (hits, misses) = outcomes.get(stage).copied().unwrap_or_default();
        assert!(
            hits > 0 && misses == 0,
            "{stage}: {hits} hits, {misses} misses"
        );
    }
    for stage in ["sh_base_volume", "sh_patch_layer"] {
        let (hits, misses) = outcomes.get(stage).copied().unwrap_or_default();
        assert!(
            hits == 0 && misses > 0,
            "{stage}: {hits} hits, {misses} misses"
        );
    }
}
