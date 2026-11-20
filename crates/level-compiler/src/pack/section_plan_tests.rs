// Pack-time refusals of ids 34, 59 and 60, and the SH body-presence choice.
// See: context/lib/build_pipeline.md §PRL section IDs

use postretro_level_format::probe_depth_tiles::{PROBE_DEPTH_TILE_TEXELS, ProbeDepthTilesSection};
use postretro_level_format::sh_volume::{OctahedralShProbe, OctahedralShVolumeSection};

use postretro_level_format::emissive::{EmissiveSection, EmissiveVolumeRecord};
use postretro_level_format::geometry::{FaceMeta, GeometrySection};
use postretro_level_format::sh_body::ShBodyPresence;
use postretro_level_format::texture_names::TextureNamesSection;

use super::{legacy_sh_body, refuse_unstampable_emissive_members, refuse_what_the_loader_refuses};
use crate::geometry::{FaceIndexRange, GeometryResult};

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

// Ids 27/34/35/41/45 drop their bodies only when id 50 carries the payload
// and `--whole-load-sh` is off.
#[test]
fn sh_bodies_are_omitted_only_beside_id_50_without_the_flag() {
    assert_eq!(legacy_sh_body(true, false), ShBodyPresence::Omitted);
    assert_eq!(legacy_sh_body(true, true), ShBodyPresence::Present);
    assert_eq!(legacy_sh_body(false, false), ShBodyPresence::Present);
    assert_eq!(legacy_sh_body(false, true), ShBodyPresence::Present);
}

/// Two faces, the second with no triangle. Vertices are not read.
fn geometry_with_a_triangle_less_face() -> GeometryResult {
    GeometryResult {
        geometry: GeometrySection {
            vertices: Vec::new(),
            indices: vec![0, 1, 2],
            faces: vec![
                FaceMeta {
                    leaf_index: 0,
                    texture_index: 0,
                };
                2
            ],
        },
        texture_names: TextureNamesSection {
            names: vec!["glow".to_owned()],
        },
        face_index_ranges: vec![
            FaceIndexRange {
                index_offset: 0,
                index_count: 3,
            },
            FaceIndexRange {
                index_offset: 3,
                index_count: 0,
            },
        ],
    }
}

fn emissive_listing(faces: Vec<u32>) -> EmissiveSection {
    EmissiveSection {
        textures: EmissiveSection::dark(1).textures,
        volumes: vec![EmissiveVolumeRecord {
            min: 0.0,
            max: 1.0,
            color: [1.0; 3],
            origin: [0.0; 3],
            faces,
            tags: Vec::new(),
        }],
    }
}

// Once any volume lists a face, the loader recovers every face's range from
// its BVH leaf, and a face with no triangle has none: the build fails naming
// it rather than write a level the loader refuses. With no member listed the
// loader never looks, so the level packs.
#[test]
fn emissive_members_refuse_a_level_with_a_triangle_less_face_by_name() {
    let geometry = geometry_with_a_triangle_less_face();
    let message = refuse_unstampable_emissive_members(&emissive_listing(vec![0]), &geometry)
        .expect_err("a member list beside a triangle-less face is refused")
        .to_string();
    assert!(
        message.contains("Emissive volume membership needs a triangle on every Geometry face")
            && message.contains("face 1 has none"),
        "{message}"
    );
    refuse_unstampable_emissive_members(&emissive_listing(Vec::new()), &geometry)
        .expect("an empty volume stamps nothing");
    refuse_unstampable_emissive_members(&EmissiveSection::dark(1), &geometry)
        .expect("no volume stamps nothing");
}
