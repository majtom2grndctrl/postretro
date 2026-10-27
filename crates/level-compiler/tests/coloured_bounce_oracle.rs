//! Pre-colour section and material-byte oracle, recorded on the indirect-scale commit.
//! See: context/lib/build_pipeline.md §Compiler pipeline

mod coloured_bounce_support;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use coloured_bounce_support::fixtures::{self, Appearance};
use coloured_bounce_support::{Fixture, current_compiler, pinned_old_compiler};
use postretro_level_format::animated_direct_sh_delta_volumes::AnimatedDirectShDeltaVolumesSection;
use postretro_level_format::delta_sh_volumes::DeltaShVolumesSection;
use postretro_level_format::direct_sh_delta_volumes::DirectShDeltaVolumesSection;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
struct Oracle {
    baseline_revision: String,
    fixtures: BTreeMap<String, FixtureDigests>,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
struct FixtureDigests {
    sections: BTreeMap<u32, String>,
    sidecars: BTreeMap<String, String>,
}

fn baseline_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/coloured_bounce_oracle.json")
}

const BASELINE_REVISION: &str = "ecbc27435023931f919484036619f339438f1e30";
const SAVED_COMPILER_BLAKE3: &str = coloured_bounce_support::GREY_COMPILER_BLAKE3;
/// The recorder's own opt-in: its value is the saved compiler's path.
const RECORD_UNIFORM_ENV: &str = "POSTRETRO_RECORD_COLOURED_BOUNCE_ID41_UNIFORM";

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Id41Shape {
    section_bytes: usize,
    entries: usize,
    valid_probes: u32,
    cell_levels: [usize; 3],
    payload_halves: usize,
}

#[derive(Debug, Serialize, Deserialize)]
struct UniformId41Oracle {
    baseline_revision: String,
    compiler_blake3: String,
    fixture_map_blake3: String,
    section_blake3: String,
    shape: Id41Shape,
}

fn uniform_baseline_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/coloured_bounce_id41_uniform.json")
}

fn uniform_room_map() -> String {
    fixtures::with_worldspawn_key(
        &fixtures::room_map(Appearance::RedWall, true, true),
        "_sh_coarsen",
        "0",
    )
}

fn fixture_map_digest() -> String {
    blake3::hash(uniform_room_map().as_bytes())
        .to_hex()
        .to_string()
}

fn bake(appearance: Appearance, ids: &[u32]) -> FixtureDigests {
    bake_using(&current_compiler(), appearance, ids, false).0
}

/// The flags the checked-in digests were recorded with; changing them
/// invalidates the oracle, so they stay exactly these.
const ORACLE_FLAGS: [&str; 3] = ["--no-cache", "--jobs", "1"];

fn bake_using(
    binary: &Path,
    appearance: Appearance,
    ids: &[u32],
    uniform: bool,
) -> (FixtureDigests, Option<Id41Shape>) {
    let map_text = if uniform {
        uniform_room_map()
    } else {
        fixtures::room_map(appearance, true, true)
    };
    let fixture = Fixture::new(&map_text, true);
    let bake = fixture.bake(binary, "room", "warn", &ORACLE_FLAGS);
    // The approved storage dependency applies to id 41 only.
    for id in [27, 45] {
        if let Some(bytes) = bake.sections.get(&id) {
            let levels = if id == 27 {
                DeltaShVolumesSection::from_bytes(bytes)
                    .unwrap()
                    .cell_levels
            } else {
                AnimatedDirectShDeltaVolumesSection::from_bytes(bytes)
                    .unwrap()
                    .cell_levels
            };
            assert!(
                levels.iter().all(|&level| level == 0),
                "id {id} must retain uniform L0 storage"
            );
        }
    }
    let shape = bake.sections.get(&41).map(|bytes| {
        let section = DirectShDeltaVolumesSection::from_bytes(bytes).unwrap();
        let mut cell_levels = [0; 3];
        for &level in &section.cell_levels {
            cell_levels[usize::from(level)] += 1;
        }
        let shape = Id41Shape {
            section_bytes: bytes.len(),
            entries: section.affinity_lights.len(),
            valid_probes: section
                .valid_probe_masks
                .iter()
                .map(|mask| mask.count_ones())
                .sum(),
            cell_levels,
            payload_halves: section.delta_subblocks.len(),
        };
        eprintln!(
            "[id41 storage] uniform={uniform}, compiler={}, {shape:?}",
            binary.display()
        );
        if uniform {
            assert!(
                shape.entries > 0 && shape.payload_halves > 0,
                "uniform oracle must carry actual direct samples"
            );
            assert!(
                shape.cell_levels[0] > 0 && shape.cell_levels[1..] == [0, 0],
                "uniform fixture must disable adaptive id41 storage"
            );
        }
        shape
    });
    let digest = |bytes: &[u8]| blake3::hash(bytes).to_hex().to_string();
    let sections = bake
        .required(ids)
        .into_iter()
        .map(|(id, bytes)| {
            assert!(!bytes.is_empty(), "section {id} must carry a payload");
            (id, digest(&bytes))
        })
        .collect();
    let sidecars = bake
        .sidecars
        .iter()
        .map(|(name, bytes)| (name.clone(), digest(bytes)))
        .collect();
    (FixtureDigests { sections, sidecars }, shape)
}

#[test]
#[ignore = "cold fixture byte-oracle regression"]
fn coloured_bounce_preserves_unaffected_section_and_sidecar_bytes() {
    let mut actual = BTreeMap::from([
        (
            "red-wall".to_owned(),
            bake(Appearance::RedWall, &[35, 41, 45, 47, 48]),
        ),
        (
            "unresolved-slots".to_owned(),
            bake(Appearance::Unresolved, &[34, 27, 54]),
        ),
        (
            "unresolved-sources".to_owned(),
            bake(Appearance::MissingSources, &[34, 27, 54]),
        ),
    ]);
    assert!(
        !actual["red-wall"].sidecars.is_empty(),
        "fixture PNGs must produce real sidecars"
    );
    assert!(
        !actual["unresolved-slots"].sidecars.is_empty(),
        "slot-only sources must bake"
    );
    assert!(
        actual["unresolved-sources"].sidecars.is_empty(),
        "absent sources have no sidecar"
    );
    let bytes =
        std::fs::read(baseline_path()).expect("the immutable pre-colour oracle must be present");
    let mut expected: Oracle = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(expected.baseline_revision, BASELINE_REVISION);
    // Preserve the original adaptive digest in its checked-in record. Its direct
    // samples are now guarded at the owner's approved uniform-storage boundary.
    let old_adaptive = expected
        .fixtures
        .get_mut("red-wall")
        .unwrap()
        .sections
        .remove(&41)
        .expect("original adaptive id41 baseline remains recorded");
    let new_adaptive = actual
        .get_mut("red-wall")
        .unwrap()
        .sections
        .remove(&41)
        .unwrap();
    eprintln!("[id41 adaptive digest] pre-colour={old_adaptive}, current={new_adaptive}");
    assert_eq!(
        actual, expected.fixtures,
        "byte oracle from {}",
        expected.baseline_revision
    );
}

/// One-shot pre-change oracle recorder: only the saved, BLAKE3-pinned
/// pre-colour executable may generate the uniform id 41 record, and only into
/// a missing file. The original adaptive oracle is never written here. Skips
/// with a message unless its opt-in names that executable.
#[test]
#[ignore = "opt-in uniform-id41 baseline capture from the saved old compiler"]
fn record_uniform_id41_from_saved_pre_colour_compiler() {
    let Some(saved) = pinned_old_compiler(
        RECORD_UNIFORM_ENV,
        "record_uniform_id41_from_saved_pre_colour_compiler",
    ) else {
        return;
    };
    assert!(
        !uniform_baseline_path().exists(),
        "uniform baseline already exists; recorder cannot overwrite it"
    );
    let (digests, shape) = bake_using(&saved, Appearance::RedWall, &[41], true);
    let oracle = UniformId41Oracle {
        baseline_revision: BASELINE_REVISION.into(),
        compiler_blake3: SAVED_COMPILER_BLAKE3.into(),
        fixture_map_blake3: fixture_map_digest(),
        section_blake3: digests.sections[&41].clone(),
        shape: shape.expect("uniform fixture must emit id41"),
    };
    std::fs::write(
        uniform_baseline_path(),
        serde_json::to_string_pretty(&oracle).unwrap() + "\n",
    )
    .unwrap();
}

#[test]
#[ignore = "cold uniform-storage direct-sample byte-oracle regression"]
fn uniform_id41_preserves_pre_colour_direct_samples() {
    let bytes = std::fs::read(uniform_baseline_path())
        .expect("capture uniform id41 with the saved old compiler first");
    let expected: UniformId41Oracle = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(expected.baseline_revision, BASELINE_REVISION);
    assert_eq!(expected.compiler_blake3, SAVED_COMPILER_BLAKE3);
    assert_eq!(
        expected.fixture_map_blake3,
        fixture_map_digest(),
        "both compilers must use exactly the same fixture map"
    );
    let (actual, shape) = bake_using(&current_compiler(), Appearance::RedWall, &[41], true);
    assert_eq!(
        actual.sections[&41], expected.section_blake3,
        "uniform id41 direct samples, masks and CSR must remain exact"
    );
    assert_eq!(
        shape.unwrap(),
        expected.shape,
        "uniform direct storage shape must remain exact"
    );
    // Packing follows the legacy exact-zero drop and geometry-validity compaction.
    // Neither reads indirect radiance: dropped raw zeros are not separately hashed.
}
