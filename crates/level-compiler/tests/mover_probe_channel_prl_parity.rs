//! A map with no movers, and a map whose movers change nothing in the probe data,
//! emit no MoverProbeChannel (id 57) and pack byte-identical to the parent commit's compiler.
//! A map whose every mover bakes `light_poses 2` packs byte-identical too but for id 57's epoch
//! byte, and with mid poses every terminus state equals the parent's.
//!
//! The parent writes id 57 at epoch 5. Epoch 6 is epoch 5's byte layout under the
//! mean-and-variance depth rule, so a two-pose section, which never reads that rule and skips
//! nothing, is epoch 5's bytes with its first byte 6. That byte is the one difference these tests
//! allow: they rewrite the parent's to this branch's epoch and compare everything else.
//!
//! Opt-in and `#[ignore]`: it runs a second compiler. A missing baseline fails the test, so a
//! green run always means both maps were compared.
//!
//! Baseline: the parent commit's `prl-build`, named by
//! `POSTRETRO_MOVER_CHANNEL_PARITY_BASELINE_BIN`. Both compilers build the same map files from
//! this checkout with the same flags, so no path differs between them. Build the binary at the
//! parent in this checkout, copy it out, then return (`development_guide.md` §Worktree builds):
//!
//! ```text
//! git switch --detach "$PARENT"
//! cargo build --release -p postretro-level-compiler --bin prl-build
//! cp target/release/prl-build "$BASE/prl-build-parent"
//! git switch -
//! POSTRETRO_MOVER_CHANNEL_PARITY_BASELINE_BIN="$BASE/prl-build-parent" \
//!   cargo test -p postretro-level-compiler --test mover_probe_channel_prl_parity -- --ignored
//! ```
//!
//! The no-change map is `mover-closet-no-door` with a mover parked far outside its sealed hull:
//! no light reaches it and no probe ray can, so its direct, bounce and depth blocks are empty.
//!
//! See: context/lib/build_pipeline.md §Compiler pipeline step 12 (Mover probe channel)

mod prl_build_support;

use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::process::Command;

use postretro_level_format::mover_probe_channel::{
    MOVER_PROBE_CHANNEL_EPOCH, MoverProbeChannelSection, PoseMark,
};
use postretro_level_format::{SectionId, read_container, read_section_data};
use prl_build_support::REACH_VERIFY_ENV;

const BASELINE_ENV: &str = "POSTRETRO_MOVER_CHANNEL_PARITY_BASELINE_BIN";
/// Worker count for every parity bake, baseline and branch alike.
const PARITY_JOBS: &str = "6";

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|crates| crates.parent())
        .expect("workspace root")
        .to_path_buf()
}

/// The parent commit's compiler, or a failure naming the variable.
fn baseline_compiler() -> PathBuf {
    let path = std::env::var_os(BASELINE_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            panic!(
                "set {BASELINE_ENV} to the parent commit's prl-build; this file's module doc \
                 gives the commands"
            )
        });
    assert!(
        path.is_file(),
        "baseline compiler missing: {}",
        path.display()
    );
    path
}

/// A mover parked 200 m outside `mover-closet-no-door`'s hull, with its
/// path's two waypoints.
const FAR_MOVER: &str = r#"
{
"classname" "kinematic_mover"
"name" "far_slab"
"path" "far_slab_a"
"speed" "1"
{
( 8000 8000 0 ) ( 8000 8001 0 ) ( 8000 8000 1 ) metal/metal_rough_046a 0 0 0 1 1
( 8064 8000 0 ) ( 8064 8000 1 ) ( 8064 8001 0 ) metal/metal_rough_046a 0 0 0 1 1
( 8000 8000 0 ) ( 8000 8000 1 ) ( 8001 8000 0 ) metal/metal_rough_046a 0 0 0 1 1
( 8000 8064 0 ) ( 8001 8064 0 ) ( 8000 8064 1 ) metal/metal_rough_046a 0 0 0 1 1
( 8000 8000 0 ) ( 8001 8000 0 ) ( 8000 8001 0 ) metal/metal_rough_046a 0 0 0 1 1
( 8000 8000 64 ) ( 8000 8001 64 ) ( 8001 8000 64 ) metal/metal_rough_046a 0 0 0 1 1
}
}
{
"classname" "kinematic_waypoint"
"name" "far_slab_a"
"next" "far_slab_b"
"origin" "8032 8032 32"
}
{
"classname" "kinematic_waypoint"
"name" "far_slab_b"
"origin" "8032 8032 160"
}
"#;

/// The parity maps, written beside each other under `dir/maps`.
fn parity_maps(dir: &Path) -> Vec<PathBuf> {
    let maps = dir.join("maps");
    std::fs::create_dir_all(&maps).expect("create maps directory");
    let no_door =
        std::fs::read_to_string(workspace_root().join("content/dev/maps/mover-closet-no-door.map"))
            .expect("read mover-closet-no-door");
    let unchanged = maps.join("mover-closet-far-mover.map");
    std::fs::write(&unchanged, format!("{no_door}\n{FAR_MOVER}")).expect("write far-mover map");
    let no_movers = maps.join("mover-closet-no-door.map");
    std::fs::write(&no_movers, no_door).expect("write no-mover map");
    vec![no_movers, unchanged]
}

/// Compile `map` with `compiler` into `out` as the ship bake, `--release`: cold, and at high
/// bake fidelity on this branch. The parent bakes every mover's own `light_bounces` cold, as
/// `--release` does here, and takes `--release` as its own cold bake, so both name it.
fn compile(compiler: &Path, map: &Path, out: &Path) -> PathBuf {
    std::fs::create_dir_all(out).expect("create output directory");
    let prl = out
        .join(map.file_name().expect("map name"))
        .with_extension("prl");
    let run = Command::new(compiler)
        .arg(map)
        .arg("-o")
        .arg(&prl)
        .args(["--release", "--no-tui", "--jobs", PARITY_JOBS])
        .arg("--baked-root")
        .arg(out.join("baked"))
        .env(REACH_VERIFY_ENV, "0")
        .current_dir(workspace_root())
        .output()
        .expect("spawn prl-build");
    assert!(
        run.status.success(),
        "{} failed on {}\n{}{}",
        compiler.display(),
        map.display(),
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
    prl
}

/// The parent compiler's id-57 epoch: the same bytes, read under the through-base depth rule.
const PARENT_EPOCH: u8 = 5;

/// `prl` (the parent compiler's packed PRL) with id 57's epoch byte, the one difference these
/// tests allow, rewritten to this branch's epoch. Fails unless the parent wrote [`PARENT_EPOCH`]
/// there.
fn at_this_epoch(mut prl: Vec<u8>) -> Vec<u8> {
    let meta = read_container(&mut Cursor::new(prl.as_slice())).expect("read the container");
    if let Some(section) = meta.find_section(SectionId::MoverProbeChannel as u32) {
        let at = section.offset as usize;
        assert_eq!(prl[at], PARENT_EPOCH, "the parent writes id 57 at epoch 5");
        prl[at] = MOVER_PROBE_CHANNEL_EPOCH;
    }
    prl
}

fn has_channel(prl: &Path) -> bool {
    let bytes = std::fs::read(prl).expect("read the packed PRL");
    let mut cursor = Cursor::new(bytes.as_slice());
    let meta = read_container(&mut cursor).expect("read the container");
    read_section_data(&mut cursor, &meta, SectionId::MoverProbeChannel as u32)
        .expect("read the section table")
        .is_some()
}

#[test]
#[ignore = "opt-in comparison: set POSTRETRO_MOVER_CHANNEL_PARITY_BASELINE_BIN to the parent commit's prl-build"]
fn maps_whose_movers_change_nothing_pack_byte_identical_to_the_parent_compiler() {
    let baseline = baseline_compiler();
    let branch = PathBuf::from(env!("CARGO_BIN_EXE_prl-build"));
    let dir = tempfile::tempdir().expect("create output directory");
    let mut compared = 0;
    for map in parity_maps(dir.path()) {
        let before = compile(&baseline, &map, &dir.path().join("parent"));
        let after = compile(&branch, &map, &dir.path().join("branch"));
        assert!(!has_channel(&after), "{}: no id 57", map.display());
        let (before_bytes, after_bytes) = (
            std::fs::read(&before).expect("read baseline PRL"),
            std::fs::read(&after).expect("read compiled PRL"),
        );
        assert!(
            before_bytes == after_bytes,
            "{}: packed PRL differs from the parent compiler's: {} B against {} B, first \
             difference at byte {:?}",
            map.display(),
            before_bytes.len(),
            after_bytes.len(),
            before_bytes
                .iter()
                .zip(&after_bytes)
                .position(|(a, b)| a != b)
        );
        compared += 1;
    }
    assert_eq!(compared, 2, "both parity maps compared");
}

/// `map` with `"light_poses" "<count>"` on every kinematic mover.
fn with_light_poses(map: &str, count: u32) -> String {
    map.replace(
        "\"classname\" \"kinematic_mover\"",
        &format!("\"classname\" \"kinematic_mover\"\n\"light_poses\" \"{count}\""),
    )
}

/// The dev maps with movers the probe channel bakes.
const MOVER_MAPS: [&str; 3] = ["mover-closet", "mover-setpiece", "enemy-movers"];

// Regression guard (`light_poses 2`): a mover baking its two termini alone
// stores the parent's terminus states, so a map whose every mover sets the
// key packs byte-identical to the parent compiler's, id 57's epoch byte
// alone apart.
#[test]
#[ignore = "opt-in comparison: set POSTRETRO_MOVER_CHANNEL_PARITY_BASELINE_BIN to the parent commit's prl-build"]
fn every_mover_at_light_poses_two_packs_byte_identical_to_the_parent_compiler() {
    let baseline = baseline_compiler();
    let branch = PathBuf::from(env!("CARGO_BIN_EXE_prl-build"));
    let dir = tempfile::tempdir().expect("create output directory");
    let maps = dir.path().join("maps");
    std::fs::create_dir_all(&maps).expect("create maps directory");
    for name in MOVER_MAPS {
        let text =
            std::fs::read_to_string(workspace_root().join(format!("content/dev/maps/{name}.map")))
                .expect("read the dev map");
        let map = maps.join(format!("{name}-two-poses.map"));
        std::fs::write(&map, with_light_poses(&text, 2)).expect("write the map");
        let before = at_this_epoch(
            std::fs::read(compile(&baseline, &map, &dir.path().join("parent")))
                .expect("read baseline PRL"),
        );
        let after = std::fs::read(compile(&branch, &map, &dir.path().join("branch")))
            .expect("read compiled PRL");
        assert!(
            channel_of(&after).is_some_and(|channel| channel
                .mover_records
                .iter()
                .all(|record| record.knots == [0.0, 1.0])),
            "{name}: every mover stores two poses"
        );
        assert!(
            before == after,
            "{name}: packed PRL differs from the parent compiler's past id 57's epoch byte: {} B \
             against {} B, first difference at byte {:?}",
            before.len(),
            after.len(),
            before.iter().zip(&after).position(|(a, b)| a != b)
        );
    }
}

fn channel_of(prl: &[u8]) -> Option<MoverProbeChannelSection> {
    let mut cursor = Cursor::new(prl);
    let meta = read_container(&mut cursor).expect("read the container");
    read_section_data(&mut cursor, &meta, SectionId::MoverProbeChannel as u32)
        .expect("read the section table")
        .map(|bytes| MoverProbeChannelSection::from_bytes(&bytes).expect("id 57 decodes"))
}

// Rest is unchanged at the termini: with mid poses placed, every terminus
// state id 57 stores equals the parent compiler's two-pose bake, block by
// block. Direct: each (cell, light, mover)'s q at the first and last pose at
// every probe the parent masked, and 255 at both termini at any probe only
// a mid pose masks. Bounce: each (cell, mover) entry's stored masks and
// tiles at both termini, and no terminus storing where the parent stored
// nothing. Depth: each (mover, probe) record's termini marks and tiles, and
// a record only mid poses carry reads its base at both termini.
#[test]
#[ignore = "opt-in comparison: set POSTRETRO_MOVER_CHANNEL_PARITY_BASELINE_BIN to the parent commit's prl-build"]
fn mid_poses_keep_every_terminus_state_of_the_parent_compiler() {
    use std::collections::BTreeMap;

    let baseline = baseline_compiler();
    let branch = PathBuf::from(env!("CARGO_BIN_EXE_prl-build"));
    let dir = tempfile::tempdir().expect("create output directory");
    for name in MOVER_MAPS {
        let map = workspace_root().join(format!("content/dev/maps/{name}.map"));
        let read = |compiler: &Path, side: &str| {
            let prl = compile(compiler, &map, &dir.path().join(side));
            let bytes = std::fs::read(prl).expect("read the PRL");
            let bytes = if side == "parent" {
                at_this_epoch(bytes)
            } else {
                bytes
            };
            channel_of(&bytes).expect("id 57")
        };
        let (before, after) = (read(&baseline, "parent"), read(&branch, "branch"));
        let mid_poses: usize = after
            .mover_records
            .iter()
            .map(|record| record.knots.len() - 2)
            .sum();
        assert!(mid_poses > 0, "{name}: some mover places mid poses");

        // Direct: (cell, namespace, light, mover) → (mask, q per probe).
        type DirectKey = (u32, u8, u32, u32);
        let direct = |channel: &MoverProbeChannelSection| {
            let mut out: BTreeMap<DirectKey, BTreeMap<u32, [u8; 2]>> = BTreeMap::new();
            for view in channel.direct_record_views() {
                for mover in view.movers() {
                    let last = mover.pose_count - 1;
                    let mut slot = 0;
                    let mut probes = BTreeMap::new();
                    for local in 0..64u32 {
                        if view.record.probe_mask & (1 << local) == 0 {
                            continue;
                        }
                        probes.insert(local, [mover.q(slot, 0), mover.q(slot, last)]);
                        slot += 1;
                    }
                    let key = (
                        view.cell,
                        view.record.namespace as u8,
                        view.record.light_index,
                        mover.mover_id,
                    );
                    out.insert(key, probes);
                }
            }
            out
        };
        let (direct_before, direct_after) = (direct(&before), direct(&after));
        let mut compared = 0;
        for (key, probes) in &direct_after {
            let parent = direct_before.get(key);
            for (local, q) in probes {
                let expected = parent
                    .and_then(|p| p.get(local))
                    .copied()
                    .unwrap_or([255; 2]);
                assert_eq!(
                    *q, expected,
                    "{name}: direct {key:?} probe {local} at the termini"
                );
                compared += 1;
            }
        }
        for (key, probes) in &direct_before {
            for (local, q) in probes {
                if *q != [255; 2] {
                    assert!(
                        direct_after.get(key).is_some_and(|p| p.contains_key(local)),
                        "{name}: direct {key:?} probe {local} lost its terminus gate"
                    );
                }
            }
        }

        // Bounce: (cell, mover) → per terminus, its stored mask and tiles.
        let bounce = |channel: &MoverProbeChannelSection| {
            let mut out = BTreeMap::new();
            for view in channel.bounce_entry_views() {
                let last = view.entry.pose_count() - 1;
                let termini = [0, last].map(|pose| {
                    let stored = view.entry.poses[pose].stored;
                    let tiles: Vec<Vec<u16>> = (0..64u32)
                        .filter(|local| stored & (1 << local) != 0)
                        .map(|local| view.pose_tile(pose, local).expect("a stored tile").to_vec())
                        .collect();
                    (stored, tiles)
                });
                out.insert((view.cell, view.entry.mover_id), termini);
            }
            out
        };
        let (bounce_before, bounce_after) = (bounce(&before), bounce(&after));
        for (key, termini) in &bounce_before {
            assert_eq!(
                bounce_after.get(key),
                Some(termini),
                "{name}: bounce entry {key:?} at the termini"
            );
        }
        for (key, termini) in &bounce_after {
            if !bounce_before.contains_key(key) {
                assert!(
                    termini.iter().all(|(stored, _)| *stored == 0),
                    "{name}: bounce entry {key:?} only mid poses add stores at a terminus"
                );
            }
        }

        // Depth: (mover, probe) → termini marks and tiles.
        let depth = |channel: &MoverProbeChannelSection| {
            channel
                .depth_record_views()
                .map(|view| {
                    let last = view.record.pose_count() - 1;
                    let termini = [0, last].map(|pose| {
                        (
                            view.record.marks[pose],
                            view.pose_tile(pose).map(<[_]>::to_vec),
                        )
                    });
                    (view.record.order_key(), termini)
                })
                .collect::<BTreeMap<_, _>>()
        };
        let (depth_before, depth_after) = (depth(&before), depth(&after));
        for (key, termini) in &depth_before {
            assert_eq!(depth_after.get(key), Some(termini), "{name}: depth {key:?}");
        }
        for (key, termini) in &depth_after {
            if !depth_before.contains_key(key) {
                assert!(
                    termini.iter().all(|(mark, _)| *mark == PoseMark::Unmarked),
                    "{name}: depth {key:?} carried only by mid poses reads its base at rest"
                );
            }
        }
        eprintln!(
            "{name}: {mid_poses} mid pose(s); {compared} (direct entry, probe) pair(s), {} bounce \
             entr(y/ies) and {} depth record(s) of the parent's kept at the termini; {} entr(y/ies) \
             and {} record(s) new",
            bounce_before.len(),
            depth_before.len(),
            bounce_after.len() - bounce_before.len(),
            depth_after.len() - depth_before.len(),
        );
    }
}
