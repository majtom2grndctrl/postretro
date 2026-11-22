//! The closet and its two-door edit pack the parent commit's bytes: every
//! emitted section, at `low` and `high`, cold, warm over an empty cache and
//! on a full hit, on one worker and four. The goldens are the whole PRL's
//! blake3, captured from the compiler before the mover channel's bounded bake
//! (each mover finishing before the next, the payload cap, the layer cap)
//! changed any production code. A low build over a cache whose `high` entry
//! for one pose is corrupt and whose `low` entry is whole is pinned the same
//! way.
//!
//! Recapture only for a change meant to move these bytes, never to make this
//! test pass: `POSTRETRO_CAPTURE_MOVER_GOLDENS=1 cargo test -p
//! postretro-level-compiler --test mover_bounded_bake_goldens -- --nocapture`
//! prints the table instead of asserting. A mismatch prints every section's
//! digest on the branch side; the on-demand parity suite
//! (`mover_bounded_bake_parity`) names the section against a parent binary.
//!
//! See: context/lib/build_pipeline.md §Compiler pipeline step 12 (Mover probe channel)

mod prl_build_support;

use std::collections::BTreeSet;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::process::Command;

use postretro_level_format::{read_container, read_section_data};
use prl_build_support::{BuildDir, REACH_VERIFY_ENV, link_dev_textures, prl_build};

const CAPTURE_ENV: &str = "POSTRETRO_CAPTURE_MOVER_GOLDENS";

/// Captured at 10df2fcb (1a66523a plus this brief's plan), before production
/// edits. One digest per (fixture, fidelity): cache mode and worker count
/// never change the bytes. Recaptured on `area-lights` for AlphaLights (id 18)
/// v5. Its version word was the only byte that moved: the closet's low build,
/// with that word patched back to 4, reproduced the original digest.
fn golden(fixture: &str, fidelity: &str) -> &'static str {
    match (fixture, fidelity) {
        ("closet", "low") => "a9d2a5476472929f2a29f52dfeae427fb23ba1cf226ef38987d6f4f6ffa01494",
        ("closet", "high") => "906a1c9cc803f209f7b7f3ca6553ced19d007c657277aed94e0860eb13f0d74a",
        ("closet-two-doors", "low") => {
            "1e2820007cae82196d2444ced937ae170767a2dac8e14e49a42fce837dbf9224"
        }
        ("closet-two-doors", "high") => {
            "fd1c46fafde0e3a6bd55e4a073ac50ba62ff9345b52e91daa39884ddec486d9d"
        }
        ("closet-two-doors-corrupt-high", "low") => {
            "46151af9e5f6a64cae33a88ce40672274b9b69b7068fb4c4e16afccb4163660c"
        }
        _ => panic!("no golden for {fixture} at {fidelity}"),
    }
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root")
        .to_path_buf()
}

fn closet() -> String {
    std::fs::read_to_string(workspace_root().join("content/dev/maps/mover-closet.map"))
        .expect("read mover-closet.map")
}

/// A second door, `closet_door_b`, the first's copy on its path: two mover
/// jobs.
fn two_doors(text: String) -> String {
    let start = text
        .find("{\n\"classname\" \"kinematic_mover\"")
        .expect("the door");
    let end = start
        + text[start..]
            .find("\n{\n\"classname\" \"kinematic_waypoint\"")
            .expect("its waypoints")
        + 1;
    let door = text[start..end].replace("\"name\" \"closet_door\"", "\"name\" \"closet_door_b\"");
    format!("{}{door}{}", &text[..end], &text[end..])
}

#[derive(Clone, Copy)]
enum Cache<'a> {
    Cold,
    Warm(&'a Path),
}

struct Built {
    _dir: BuildDir,
    prl: PathBuf,
    logs: String,
}

fn build(map: &str, fidelity: &str, cache: Cache<'_>, jobs: u32) -> Built {
    let dir = BuildDir::temp();
    let maps = dir.path().join("maps");
    std::fs::create_dir_all(&maps).expect("create maps directory");
    link_dev_textures(dir.path());
    let input = maps.join("closet.map");
    std::fs::write(&input, map).expect("write map");
    let prl = dir.path().join("closet.prl");
    let mut command: Command = prl_build();
    command
        .env("RUST_LOG", "info")
        .arg(&input)
        .arg("-o")
        .arg(&prl)
        .args(["--no-tui", "--jobs", &jobs.to_string()])
        .args(["--bake-fidelity", fidelity])
        .arg("--baked-root")
        .arg(dir.path().join("baked"));
    match cache {
        Cache::Cold => {
            command.arg("--no-cache");
        }
        Cache::Warm(cache) => {
            command
                .env(REACH_VERIFY_ENV, "0")
                .arg("--cache-dir")
                .arg(cache);
        }
    }
    let output = command.output().expect("spawn prl-build");
    let logs = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.status.success(), "prl-build failed\n{logs}");
    Built {
        _dir: dir,
        prl,
        logs,
    }
}

fn digest(built: &Built) -> String {
    blake3::hash(&std::fs::read(&built.prl).expect("read PRL"))
        .to_hex()
        .to_string()
}

/// Every section's digest, by id, for a mismatch message.
fn section_digests(built: &Built) -> String {
    let bytes = std::fs::read(&built.prl).expect("read PRL");
    let mut cursor = Cursor::new(bytes.as_slice());
    let meta = read_container(&mut cursor).expect("read container");
    meta.sections
        .iter()
        .map(|entry| {
            let data = read_section_data(&mut cursor, &meta, entry.section_id)
                .expect("read section")
                .expect("listed section");
            format!(
                "  id {:>3}: {}",
                entry.section_id,
                blake3::hash(&data).to_hex()
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Assert `built` matches its golden, or print it in capture mode.
fn check(
    fixture: &str,
    fidelity: &str,
    what: &str,
    built: &Built,
    captured: &mut BTreeSet<String>,
) {
    let digest = digest(built);
    if std::env::var_os(CAPTURE_ENV).is_some() {
        let row = format!("(\"{fixture}\", \"{fidelity}\") => \"{digest}\",");
        if captured.insert(row.clone()) {
            eprintln!("{row}");
        }
        return;
    }
    assert_eq!(
        digest,
        golden(fixture, fidelity),
        "{fixture} at {fidelity}, {what}: the PRL differs from the parent's\n{}",
        section_digests(built)
    );
}

// Every cache mode and worker count packs one PRL per fixture and fidelity,
// the parent's.
#[test]
fn mover_sections_match_parent_goldens() {
    let mut captured = BTreeSet::new();
    for (fixture, map) in [
        ("closet", closet()),
        ("closet-two-doors", two_doors(closet())),
    ] {
        for fidelity in ["low", "high"] {
            for jobs in [1, 4] {
                let cold = build(&map, fidelity, Cache::Cold, jobs);
                check(
                    fixture,
                    fidelity,
                    &format!("cold x{jobs}"),
                    &cold,
                    &mut captured,
                );
                let cache = tempfile::tempdir().expect("cache");
                let empty = build(&map, fidelity, Cache::Warm(cache.path()), jobs);
                check(
                    fixture,
                    fidelity,
                    &format!("warm empty x{jobs}"),
                    &empty,
                    &mut captured,
                );
                let hit = build(&map, fidelity, Cache::Warm(cache.path()), jobs);
                assert!(
                    hit.logs.contains("mover_probe_bounce hit ("),
                    "{fixture} {fidelity} x{jobs}: a full hit\n{}",
                    hit.logs
                );
                check(
                    fixture,
                    fidelity,
                    &format!("full hit x{jobs}"),
                    &hit,
                    &mut captured,
                );
            }
        }
    }
}

/// The stage cache's entry files under `dir`.
fn entries(dir: &Path) -> BTreeSet<String> {
    std::fs::read_dir(dir)
        .expect("read cache dir")
        .map(|entry| {
            entry
                .expect("cache entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .filter(|name| name.len() == 64 && name.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .collect()
}

fn copy_entries(from: &Path, to: &Path) {
    for name in entries(from) {
        std::fs::copy(from.join(&name), to.join(&name)).expect("copy cache entry");
    }
}

// At `low`, warm over a cache holding both fidelities' entries, where one
// `high` pose entry is corrupt and its `low` entry whole: the lookup serves
// that pose from `low` and the others from `high`. The parent's bytes on the
// same cache (M5).
#[test]
fn a_corrupt_high_entry_serves_low_as_the_parent_did() {
    let map = two_doors(closet());
    let low = tempfile::tempdir().expect("low cache");
    build(&map, "low", Cache::Warm(low.path()), 4);
    let high = tempfile::tempdir().expect("high cache");
    build(&map, "high", Cache::Warm(high.path()), 4);
    // Only the bounce memo keys on fidelity: the entries a `high` build
    // writes and a `low` one does not are its `high` poses.
    let high_poses: Vec<String> = entries(high.path())
        .difference(&entries(low.path()))
        .cloned()
        .collect();
    assert!(!high_poses.is_empty(), "the high build writes high poses");
    let merged = tempfile::tempdir().expect("merged cache");
    copy_entries(low.path(), merged.path());
    copy_entries(high.path(), merged.path());
    let corrupt = merged.path().join(&high_poses[0]);
    let mut bytes = std::fs::read(&corrupt).expect("read entry");
    let last = bytes.len() - 1;
    bytes[last] ^= 0xff;
    std::fs::write(&corrupt, bytes).expect("corrupt entry");

    let built = build(&map, "low", Cache::Warm(merged.path()), 4);
    assert!(
        built.logs.contains(&high_poses[0]) && built.logs.contains("hash mismatch"),
        "the corrupt entry is read and refused\n{}",
        built.logs
    );
    check(
        "closet-two-doors-corrupt-high",
        "low",
        "warm, one high entry corrupt",
        &built,
        &mut BTreeSet::new(),
    );
}
