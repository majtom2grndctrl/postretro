// Pack-time refusals of id 34 and id 59: the compiler never writes what the loader refuses.
// See: context/lib/build_pipeline.md §PRL section IDs

use postretro_level_format::probe_depth_tiles::{PROBE_DEPTH_TILE_TEXELS, ProbeDepthTilesSection};
use postretro_level_format::sh_volume::{OctahedralShProbe, OctahedralShVolumeSection};

use super::refuse_what_the_loader_refuses;

/// A one-row id 34 holding `probes`.
fn volume(probes: Vec<OctahedralShProbe>) -> OctahedralShVolumeSection {
    let mut section = OctahedralShVolumeSection::placeholder();
    section.grid_dimensions = [probes.len() as u32, 1, 1];
    section.probes = probes;
    section
}

fn probe(validity: u8, mean_distance: u16, mean_sq_distance: u16) -> OctahedralShProbe {
    OctahedralShProbe {
        validity,
        mean_distance,
        mean_sq_distance,
        ..Default::default()
    }
}

/// Valid probes with `E[d]` = 1 m and `E[d²]` = 2 m².
fn clean_probes() -> Vec<OctahedralShProbe> {
    vec![probe(1, 0x3c00, 0x4000); 3]
}

#[test]
fn a_valid_probe_with_a_signed_moment_fails_the_build_by_name() {
    let no_tiles = ProbeDepthTilesSection::default();
    refuse_what_the_loader_refuses(&volume(clean_probes()), &no_tiles)
        .expect("unsigned moments pack");
    for signed in [probe(1, 0x8000, 0x4000), probe(1, 0x3c00, 0xbc00)] {
        let mut probes = clean_probes();
        probes[2] = signed;
        let message = refuse_what_the_loader_refuses(&volume(probes), &no_tiles)
            .expect_err("a signed moment on a valid probe is refused")
            .to_string();
        assert!(
            message.contains("OctahedralShVolume valid probe 2"),
            "{message}"
        );
    }
    // An invalid probe never reaches the moment texel, so the loader does not
    // check its bits either.
    let mut probes = clean_probes();
    probes[1] = probe(0, 0xffff, 0xffff);
    refuse_what_the_loader_refuses(&volume(probes), &no_tiles)
        .expect("an invalid probe's moments are not checked");
}

#[test]
fn a_tile_naming_an_invalid_probe_fails_the_build() {
    let mut probes = clean_probes();
    probes[1].validity = 0;
    let tiles = ProbeDepthTilesSection {
        probe_indices: vec![1],
        texels: vec![[0, 0]; PROBE_DEPTH_TILE_TEXELS],
    };
    let message = refuse_what_the_loader_refuses(&volume(probes), &tiles)
        .expect_err("a tile on an invalid probe is refused")
        .to_string();
    assert!(
        message.contains("ProbeDepthTiles does not fit id 34")
            && message.contains("probe index 1 names an invalid probe"),
        "{message}"
    );
}
