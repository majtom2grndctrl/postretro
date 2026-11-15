//! `prl-build --whole-load-sh` through the real CLI: a default bake writes
//! ids 27, 34, 35, 41 and 45 without their bodies beside id 50, the flag writes
//! them whole, and nothing else in the PRL moves. Also proves the test-support
//! transform the baseline-compiler parity suites use to upgrade a pre-flag PRL.
//! See: context/lib/build_pipeline.md §PRL section IDs

mod prl_build_support;

use std::collections::BTreeMap;
use std::io::Cursor;
use std::path::{Path, PathBuf};

use postretro_level_format::FormatError;
use postretro_level_format::animated_direct_sh_delta_volumes::AnimatedDirectShDeltaVolumesSection;
use postretro_level_format::delta_sh_volumes::DeltaShVolumesSection;
use postretro_level_format::direct_sh_delta_volumes::DirectShDeltaVolumesSection;
use postretro_level_format::direct_sh_volume::DirectShVolumeSection;
use postretro_level_format::sh_body::pre_flag::{downgrade_to_pre_flag_prl, upgrade_pre_flag_prl};
use postretro_level_format::sh_body::{SH_BODY_SECTIONS, ShBodyPresence, read_sh_body_presence};
use postretro_level_format::sh_volume::OctahedralShVolumeSection;
use postretro_level_format::{SectionId, read_container, read_section_data};
use postretro_level_loader::{ShStreamingMode, load_prl_with_sh_streaming};

use prl_build_support::prl_build;

/// A sealed hall with static, animated and entity-shadow lights: it emits all
/// five body sections.
const FIXTURE: &str = include_str!("fixtures/reflection_probe_lights.map");

/// Every section's table version and payload, in table order.
type Sections = Vec<(u32, u16, Vec<u8>)>;

fn bake(dir: &Path, name: &str, extra: &[&str]) -> Sections {
    sections(&bake_prl(dir, name, extra))
}

/// The fixture compiled into `dir/<name>.prl` with `extra` flags.
fn bake_prl(dir: &Path, name: &str, extra: &[&str]) -> PathBuf {
    let maps = dir.join("maps");
    std::fs::create_dir_all(&maps).expect("create maps dir");
    let input = maps.join(format!("{name}.map"));
    std::fs::write(&input, FIXTURE).expect("write fixture map");
    let output_path = dir.join(format!("{name}.prl"));
    let output = prl_build()
        .env("RUST_LOG", "warn")
        .arg(&input)
        .arg("-o")
        .arg(&output_path)
        .args(["--no-cache", "--no-tui", "--lightmap-density", "0.25"])
        .arg("--baked-root")
        .arg(dir.join("baked"))
        .args(extra)
        .output()
        .expect("spawn prl-build");
    assert!(
        output.status.success(),
        "prl-build failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    output_path
}

fn sections(prl: &PathBuf) -> Sections {
    let bytes = std::fs::read(prl).expect("read compiled PRL");
    let mut cursor = Cursor::new(bytes);
    let meta = read_container(&mut cursor).expect("read PRL container");
    meta.sections
        .iter()
        .map(|entry| {
            let data = read_section_data(&mut cursor, &meta, entry.section_id)
                .expect("read section")
                .expect("listed section is present");
            (entry.section_id, entry.version, data)
        })
        .collect()
}

fn by_id(sections: &Sections) -> BTreeMap<u32, &[u8]> {
    sections
        .iter()
        .map(|(id, _, data)| (*id, data.as_slice()))
        .collect()
}

/// `from_bytes` for one of the five sections, its error only.
fn decode_error(id: SectionId, bytes: &[u8]) -> Option<FormatError> {
    match id {
        SectionId::DeltaShVolumes => DeltaShVolumesSection::from_bytes(bytes).err(),
        SectionId::OctahedralShVolume => OctahedralShVolumeSection::from_bytes(bytes).err(),
        SectionId::DirectShVolume => DirectShVolumeSection::from_bytes(bytes).err(),
        SectionId::DirectShDeltaVolumes => DirectShDeltaVolumesSection::from_bytes(bytes).err(),
        SectionId::AnimatedDirectShDeltaVolumes => {
            AnimatedDirectShDeltaVolumesSection::from_bytes(bytes).err()
        }
        other => panic!("{other:?} carries no SH body"),
    }
}

/// The flagged section decoded in memory and re-encoded with its body omitted.
fn omitted_from_whole(id: SectionId, whole: &[u8]) -> Vec<u8> {
    let omitted = ShBodyPresence::Omitted;
    match id {
        SectionId::DeltaShVolumes => DeltaShVolumesSection::from_bytes(whole)
            .expect("decode id 27")
            .to_bytes_with_body(omitted),
        SectionId::OctahedralShVolume => OctahedralShVolumeSection::from_bytes(whole)
            .expect("decode id 34")
            .try_to_bytes_with_body(omitted)
            .expect("encode id 34"),
        SectionId::DirectShVolume => DirectShVolumeSection::from_bytes(whole)
            .expect("decode id 35")
            .try_to_bytes_with_body(omitted)
            .expect("encode id 35"),
        SectionId::DirectShDeltaVolumes => DirectShDeltaVolumesSection::from_bytes(whole)
            .expect("decode id 41")
            .to_bytes_with_body(omitted),
        SectionId::AnimatedDirectShDeltaVolumes => {
            AnimatedDirectShDeltaVolumesSection::from_bytes(whole)
                .expect("decode id 45")
                .try_to_bytes_with_body(omitted)
                .expect("encode id 45")
        }
        other => panic!("{other:?} carries no SH body"),
    }
}

fn bake_both() -> (tempfile::TempDir, Sections, Sections) {
    let dir = tempfile::tempdir().expect("create temp dir");
    let default = bake(dir.path(), "default", &[]);
    let whole = bake(dir.path(), "whole", &["--whole-load-sh"]);
    (dir, default, whole)
}

#[test]
fn default_bake_omits_the_five_sh_bodies_and_whole_load_sh_writes_them() {
    let (_dir, default, whole) = bake_both();
    let (default, whole) = (by_id(&default), by_id(&whole));
    assert!(
        default.contains_key(&(SectionId::ClusterShPayloads as u32)),
        "the default bake streams SH from id 50"
    );
    for id in SH_BODY_SECTIONS {
        let key = id as u32;
        let omitted = default
            .get(&key)
            .unwrap_or_else(|| panic!("the fixture must emit {id:?}"));
        let present = whole[&key];
        assert_eq!(
            read_sh_body_presence(id, omitted).expect("default flag reads"),
            ShBodyPresence::Omitted,
            "{id:?} in the default bake"
        );
        assert_eq!(
            read_sh_body_presence(id, present).expect("whole flag reads"),
            ShBodyPresence::Present,
            "{id:?} under --whole-load-sh"
        );
        assert!(
            omitted.len() < present.len(),
            "{id:?}: the fixture must give it a nonempty body"
        );
        assert!(
            matches!(
                decode_error(id, omitted),
                Some(FormatError::ShBodyOmitted { section_id }) if section_id == key
            ),
            "{id:?}: a whole decode of the default bake names the omitted body"
        );
        assert!(
            decode_error(id, present).is_none(),
            "{id:?} decodes whole under --whole-load-sh"
        );
        assert!(
            *omitted == omitted_from_whole(id, present).as_slice(),
            "{id:?}: the default section is the flagged one with the flag cleared and the body cut"
        );
    }
}

#[test]
fn whole_load_sh_changes_no_other_section() {
    let (_dir, default, whole) = bake_both();
    let order = |sections: &Sections| -> Vec<(u32, u16)> {
        sections
            .iter()
            .map(|(id, version, _)| (*id, *version))
            .collect()
    };
    assert_eq!(
        order(&default),
        order(&whole),
        "same sections, table versions and order"
    );
    let body_sections: Vec<u32> = SH_BODY_SECTIONS.iter().map(|&id| id as u32).collect();
    for ((id, _, default_bytes), (_, _, whole_bytes)) in default.iter().zip(&whole) {
        if !body_sections.contains(id) {
            assert!(
                default_bytes == whole_bytes,
                "section {id} differs between the default and --whole-load-sh bakes"
            );
        }
    }
}

// The baseline-compiler parity suites upgrade a pre-flag PRL with
// `upgrade_pre_flag_prl`. Its inverse turns this compiler's whole-load bake
// into the pre-flag form; upgrading that must give back the bake byte for
// byte, and the upgraded file must load whole and streamed.
#[test]
fn pre_flag_upgrade_round_trips_a_whole_load_sh_bake_and_loads_in_both_modes() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let baked = bake_prl(dir.path(), "whole", &["--whole-load-sh"]);
    let whole = std::fs::read(&baked).expect("read compiled PRL");
    let table = sections(&baked);
    for id in SH_BODY_SECTIONS
        .into_iter()
        .chain([SectionId::ClusterShPayloads])
    {
        assert!(
            table.iter().any(|(key, ..)| *key == id as u32),
            "the fixture must emit {id:?}"
        );
    }

    let pre_flag = downgrade_to_pre_flag_prl(&whole).expect("synthesize the pre-flag PRL");
    let pre_flag_path = dir.path().join("pre-flag.prl");
    std::fs::write(&pre_flag_path, &pre_flag).expect("write pre-flag PRL");
    let pre_flag_table = sections(&pre_flag_path);
    for ((id, version, bytes), (old_id, old_version, old_bytes)) in
        table.iter().zip(&pre_flag_table)
    {
        assert_eq!(
            (id, version),
            (old_id, old_version),
            "table order and versions"
        );
        let flagged = SH_BODY_SECTIONS
            .iter()
            .any(|&section| section as u32 == *id);
        let id50 = *id == SectionId::ClusterShPayloads as u32;
        assert_eq!(
            bytes == old_bytes,
            !flagged && !id50,
            "section {id}: only the five SH sections and id 50 change"
        );
    }

    let upgraded = upgrade_pre_flag_prl(&pre_flag).expect("upgrade the pre-flag PRL");
    assert!(
        upgraded == whole,
        "the upgraded PRL differs from the whole-load bake: {} B against {} B, first difference \
         at byte {:?}",
        upgraded.len(),
        whole.len(),
        upgraded.iter().zip(&whole).position(|(a, b)| a != b)
    );

    let upgraded_path = dir.path().join("upgraded.prl");
    std::fs::write(&upgraded_path, &upgraded).expect("write upgraded PRL");
    let path = upgraded_path.to_str().expect("UTF-8 temp path");
    for mode in [ShStreamingMode::Off, ShStreamingMode::SyncProof] {
        let world = load_prl_with_sh_streaming(path, mode)
            .unwrap_or_else(|error| panic!("the upgraded PRL loads under {mode:?}: {error}"));
        assert_eq!(
            world.sh_storage.is_streaming(),
            mode == ShStreamingMode::SyncProof,
            "{mode:?} picks its SH storage"
        );
    }
    let stale = pre_flag_path.to_str().expect("UTF-8 temp path");
    assert!(
        load_prl_with_sh_streaming(stale, ShStreamingMode::Off).is_err(),
        "the engine refuses the pre-flag PRL it was synthesized as"
    );
}
