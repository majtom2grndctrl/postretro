use super::*;
use postretro_level_format::probe_depth_tiles::{
    MAX_PROBE_DEPTH_TILES, PROBE_DEPTH_TILE_TEXELS, PROBE_DEPTH_TILES_VERSION,
};
use postretro_level_format::{SectionBlob, SectionId};

use crate::PrlLoadError;
use crate::prl_loader::load_prl_with_streaming_mode_for_test;
use crate::sh_stream::ShStreamingMode;
use crate::sh_stream_tests::{
    STREAM_FIXTURE_GRID, write_stream_fixture_with_base, write_whole_load_fixture_with_base,
};

const LATTICE: u32 = STREAM_FIXTURE_GRID[0] * STREAM_FIXTURE_GRID[1] * STREAM_FIXTURE_GRID[2];

fn tiles(indices: &[u32]) -> ProbeDepthTilesSection {
    ProbeDepthTilesSection {
        probe_indices: indices.to_vec(),
        texels: vec![[0x3c00, 0x3c00]; indices.len() * PROBE_DEPTH_TILE_TEXELS],
    }
}

fn tiles_blob(data: Vec<u8>) -> SectionBlob {
    SectionBlob {
        section_id: SectionId::ProbeDepthTiles as u32,
        version: 1,
        data,
    }
}

fn valid_probes(count: usize) -> Vec<OctahedralShProbe> {
    vec![
        OctahedralShProbe {
            validity: 1,
            mean_distance: 0x3c00,
            mean_sq_distance: 0x4000,
            ..Default::default()
        };
        count
    ]
}

/// The refusal's section and message from each id-34 path: the streamed
/// metadata base, the `off` escape over the same file, and a level with no
/// ids 49/50 that only loads whole.
fn refusals(
    edit_base: impl Fn(&mut postretro_level_format::sh_volume::OctahedralShVolumeSection),
    id59: Option<Vec<u8>>,
) -> Vec<(&'static str, String)> {
    let extra = || id59.clone().into_iter().map(tiles_blob).collect::<Vec<_>>();
    let streamed = write_stream_fixture_with_base(&edit_base, extra());
    let whole = write_whole_load_fixture_with_base(&edit_base, extra());
    [
        (&streamed, ShStreamingMode::Async),
        (&streamed, ShStreamingMode::Off),
        (&whole, ShStreamingMode::Async),
    ]
    .into_iter()
    .map(|(fixture, mode)| {
        match load_prl_with_streaming_mode_for_test(fixture.path.to_str().unwrap(), mode) {
            Err(PrlLoadError::SectionValidation { section, message }) => (section, message),
            other => panic!("{mode:?}: expected a named refusal, got {other:?}"),
        }
    })
    .collect()
}

#[test]
fn valid_tiles_reach_the_level_in_both_sh_storage_modes() {
    let section = tiles(&[0, 17, LATTICE - 1]);
    let fixture = write_stream_fixture_with_base(|_| {}, vec![tiles_blob(section.to_bytes())]);
    for mode in [ShStreamingMode::Async, ShStreamingMode::Off] {
        let world = load_prl_with_streaming_mode_for_test(fixture.path.to_str().unwrap(), mode)
            .unwrap_or_else(|error| panic!("{mode:?}: {error}"));
        assert_eq!(world.probe_depth_tiles(), Some(&section), "{mode:?}");
    }
    let whole = write_whole_load_fixture_with_base(|_| {}, vec![tiles_blob(section.to_bytes())]);
    let world =
        load_prl_with_streaming_mode_for_test(whole.path.to_str().unwrap(), ShStreamingMode::Async)
            .unwrap();
    assert!(
        world.sh_stream_manifest().is_none(),
        "no ids 49/50: SH loads whole"
    );
    assert_eq!(world.probe_depth_tiles(), Some(&section));
    let without = write_whole_load_fixture_with_base(|_| {}, Vec::new());
    let world = load_prl_with_streaming_mode_for_test(
        without.path.to_str().unwrap(),
        ShStreamingMode::Async,
    )
    .unwrap();
    assert_eq!(world.probe_depth_tiles(), None);
}

#[test]
fn a_valid_probe_with_a_signed_moment_refuses_the_level_by_name_on_every_id34_path() {
    for (label, mean_flag, mean_sq_flag) in [("E[d]", 0x8000u16, 0u16), ("E[d²]", 0, 0x8000)] {
        for (section, message) in refusals(
            |base| {
                base.probes[5].mean_distance |= mean_flag;
                base.probes[5].mean_sq_distance |= mean_sq_flag;
            },
            None,
        ) {
            assert_eq!(section, "OctahedralShVolume", "{label}");
            assert!(message.contains("valid probe 5"), "{label}: {message}");
        }
    }
}

#[test]
fn an_invalid_probe_may_carry_any_moment_bits() {
    let mut probes = valid_probes(4);
    probes[2].validity = 0;
    probes[2].mean_distance = 0xffff;
    probes[2].mean_sq_distance = 0xffff;
    refuse_signed_valid_moments(&probes).unwrap();
    probes[1].mean_sq_distance = 0x8001;
    let error = refuse_signed_valid_moments(&probes).unwrap_err();
    assert!(
        matches!(&error, PrlLoadError::SectionValidation { section: "OctahedralShVolume", message } if message.contains("valid probe 1")),
        "{error}"
    );
}

#[test]
fn a_malformed_id59_refuses_the_level_by_name_on_every_id34_path() {
    let mut wrong_version = tiles(&[3]).to_bytes();
    wrong_version[0..4].copy_from_slice(&(PROBE_DEPTH_TILES_VERSION + 1).to_le_bytes());
    for (section, message) in refusals(|_| {}, Some(wrong_version)) {
        assert_eq!(section, "ProbeDepthTiles");
        assert!(message.contains("version"), "{message}");
    }
    for (section, message) in refusals(|_| {}, Some(tiles(&[LATTICE]).to_bytes())) {
        assert_eq!(section, "ProbeDepthTiles");
        assert!(message.contains("outside"), "{message}");
    }
}

/// Every malformed shape of the id-59 wire contract, through the loader's
/// decode: wrong version, tile edge not 8, border not 1, more than 32,768
/// tiles, an index outside the lattice or naming an invalid probe, unsorted or
/// duplicate indices, truncated or trailing bytes.
#[test]
fn malformed_id59_shapes_each_refuse_as_probe_depth_tiles() {
    let grid = [4, 4, 2];
    let mut probes = valid_probes(32);
    probes[9].validity = 0;
    let header = |version: u32, edge: u32, border: u32, count: u32| {
        [version, edge, border, count]
            .iter()
            .flat_map(|word| word.to_le_bytes())
            .collect::<Vec<u8>>()
    };
    let valid = tiles(&[1, 4]).to_bytes();
    let mut truncated = valid.clone();
    truncated.pop();
    let mut trailing = valid.clone();
    trailing.push(0);
    let unsorted = bytes_with_indices(&[4, 1]);
    let duplicate = bytes_with_indices(&[4, 4]);
    let cases: [(&str, Vec<u8>, &str); 10] = [
        ("wrong version", header(2, 8, 1, 0), "version"),
        ("edge other than 8", header(1, 6, 1, 0), "edge"),
        ("border other than 1", header(1, 8, 0, 0), "border"),
        (
            "more than the cap",
            header(1, 8, 1, MAX_PROBE_DEPTH_TILES + 1),
            "cap",
        ),
        ("index outside lattice", tiles(&[32]).to_bytes(), "outside"),
        ("invalid probe", tiles(&[9]).to_bytes(), "invalid probe"),
        ("unsorted", unsorted, "ascend"),
        ("duplicate", duplicate, "ascend"),
        ("truncated", truncated, "bytes"),
        ("trailing", trailing, "bytes"),
    ];
    for (label, data, needle) in cases {
        match decode_probe_depth_tiles(&data, grid, &probes) {
            Err(PrlLoadError::SectionValidation { section, message }) => {
                assert_eq!(section, "ProbeDepthTiles", "{label}");
                assert!(message.contains(needle), "{label}: {message}");
            }
            other => panic!("{label}: expected a ProbeDepthTiles refusal, got {other:?}"),
        }
    }
    decode_probe_depth_tiles(&tiles(&[]).to_bytes(), grid, &probes).unwrap();
    assert_eq!(
        decode_probe_depth_tiles(&valid, grid, &probes).unwrap(),
        tiles(&[1, 4])
    );
}

/// `to_bytes` debug-asserts ascending indices; write a bad order by hand.
fn bytes_with_indices(indices: &[u32]) -> Vec<u8> {
    let placeholder: Vec<u32> = (0..indices.len() as u32).collect();
    let mut bytes = tiles(&placeholder).to_bytes();
    for (i, index) in indices.iter().enumerate() {
        bytes[16 + i * 4..20 + i * 4].copy_from_slice(&index.to_le_bytes());
    }
    bytes
}
