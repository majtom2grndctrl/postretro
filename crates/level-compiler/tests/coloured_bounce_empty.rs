//! Empty material/geometry tails through the current compiler, and as
//! one-shot pre-change oracles against the saved pre-colour compiler.
//! See: context/lib/build_pipeline.md §Compiler pipeline

mod coloured_bounce_support;

use std::path::Path;

use coloured_bounce_support::fixtures::{self, Appearance, box_brush};
use coloured_bounce_support::{
    Bake, FOUR_SHADOW_SAMPLES, Fixture, MATERIALS_START, SH_START, current_compiler, grey_compiler,
    sha256_hex,
};
use postretro_level_format::SectionId;
use postretro_level_format::geometry::GeometrySection;
use postretro_level_format::kinematic_geometry::KinematicGeometrySection;
use postretro_level_format::prm::cache_filename_for_key;
use postretro_level_format::sh_volume::OctahedralShVolumeSection;
use postretro_level_format::texture_cache_keys::TextureCacheKeysSection;
use postretro_level_format::texture_names::TextureNamesSection;

/// Sections both compilers must agree on when every bounce surface is grey.
const PRE_COLOUR_IDS: [u32; 6] = [16, 17, 27, 32, 34, 43];

fn empty_pin(map_text: &str, textured: bool) -> Fixture {
    // Uniform storage keeps this empty/grey proof independent of the
    // separately tested, owner-approved adaptive direct-storage boundary.
    Fixture::new(
        &fixtures::with_worldspawn_key(map_text, "_sh_coarsen", "0"),
        textured,
    )
}

fn bake(fixture: &Fixture, compiler: &Path, label: &str) -> Bake {
    let mut flags = vec!["--no-cache", "--jobs", "1"];
    flags.extend(FOUR_SHADOW_SAMPLES);
    // Independent baked roots per label: both compilers see absent sidecars.
    fixture.bake(compiler, label, "info", &flags)
}

fn sh(bake: &Bake) -> OctahedralShVolumeSection {
    OctahedralShVolumeSection::from_bytes(bake.section(SectionId::OctahedralShVolume)).unwrap()
}

fn geometry(bake: &Bake) -> GeometrySection {
    GeometrySection::from_bytes(bake.section(SectionId::Geometry)).unwrap()
}

fn assert_materials_before_sh(bake: &Bake) {
    let materials = bake.log.find(MATERIALS_START).expect("material stage ran");
    let sh = bake.log.find(SH_START).expect("base SH stage ran");
    assert!(materials < sh, "materials must finish before SH begins");
}

fn assert_key_sidecar_correspondence(bake: &Bake) {
    let names = bake.names();
    let keys = TextureCacheKeysSection::from_bytes(bake.section(SectionId::TextureCacheKeys))
        .unwrap()
        .keys;
    assert_eq!(names.len(), keys.len());
    for (name, key) in names.iter().zip(keys) {
        if key != [0; 32] {
            assert!(
                bake.sidecars
                    .contains_key(&format!("{}.prm", cache_filename_for_key(&key))),
                "{name} must reach its baked sidecar"
            );
        }
    }
}

/// What the current compiler must show on its own: one byte report, the
/// material stage ahead of SH, and every named key reaching its sidecar.
fn assert_current_shape(current: &Bake) {
    assert_eq!(
        current
            .byte_report()
            .iter()
            .filter(|line| line.starts_with("texture bytes:"))
            .count(),
        1
    );
    assert_materials_before_sh(current);
    assert_key_sidecar_correspondence(current);
}

fn assert_precolour_equal(grey: &Bake, current: &Bake) {
    // Missing optional sections compare as absence, never as a fabricated payload.
    for id in PRE_COLOUR_IDS {
        assert_eq!(
            grey.sections.get(&id),
            current.sections.get(&id),
            "pre-colour id {id}"
        );
    }
    assert_eq!(
        grey.sidecars, current.sidecars,
        "material payloads stay byte-identical"
    );
    assert!(
        !grey.byte_report().is_empty(),
        "baseline must emit its byte report"
    );
    assert_eq!(
        grey.byte_report(),
        current.byte_report(),
        "single byte report and first-name accounting"
    );
}

/// One-shot pre-change oracle: bake `map_text` with the saved pre-colour
/// compiler and the current one into independent roots and require
/// pre-colour-equal output. Skips (returns) when the saved compiler is unset.
fn compare_with_pre_colour(test: &str, map_text: &str) {
    let Some(grey_binary) = grey_compiler(test) else {
        return;
    };
    let fixture = empty_pin(map_text, true);
    let grey = bake(&fixture, &grey_binary, "grey");
    let current = bake(&fixture, &current_compiler(), "current");
    assert_precolour_equal(&grey, &current);
    assert_current_shape(&current);
}

#[test]
fn pinned_compiler_digest_is_fips_sha256() {
    // FIPS 180-4 examples: empty, one block, and a two-block message.
    for (message, digest) in [
        (
            &b""[..],
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        ),
        (
            b"abc",
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
        ),
        (
            b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq",
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1",
        ),
    ] {
        assert_eq!(sha256_hex(message), digest);
    }
}

fn no_fluid_tail_map() -> String {
    // Normal/specular-only sources produce real bundles while every bounce
    // surface remains the original exact grey fallback in both compilers.
    fixtures::without_fluid(fixtures::room_map(Appearance::Unresolved, true, true))
}

#[test]
#[ignore = "cold real-CLI empty-tail bake"]
fn no_fluid_only_tail_keeps_grey_lighting_texture_indices_and_one_report() {
    let current = bake(
        &empty_pin(&no_fluid_tail_map(), true),
        &current_compiler(),
        "current",
    );
    assert_current_shape(&current);
    let names = current.names();
    assert_eq!(names.len(), 3);
    assert!(names.contains(&"bounce/missing".to_owned()));
    assert!(names.contains(&"bounce/normal_only".to_owned()));
    assert!(names.contains(&"bounce/specular_only".to_owned()));
    assert_eq!(current.sidecars.len(), 2);
    assert!(
        current.sections.contains_key(&27),
        "animated light must exercise nonempty delta bytes"
    );
    assert!(sh(&current).total_probes() > 0);
    assert!(!current.skipped("SH Bake"));
}

#[test]
#[ignore = "one-shot pre-change oracle: needs POSTRETRO_GREY_PRL_BUILD"]
fn no_fluid_only_tail_matches_pre_colour_compiler() {
    compare_with_pre_colour(
        "no_fluid_only_tail_matches_pre_colour_compiler",
        &no_fluid_tail_map(),
    );
}

fn unlit_room_map() -> String {
    fixtures::room_map(Appearance::RedWall, false, false)
}

#[test]
#[ignore = "cold real-CLI no-light bake"]
fn textured_room_without_lights_keeps_base_metadata_and_skips_direct_delta_work() {
    let current = bake(
        &empty_pin(&unlit_room_map(), true),
        &current_compiler(),
        "current",
    );
    assert_current_shape(&current);
    assert_eq!(
        current.sidecars.len(),
        3,
        "world materials and real fluid-only tail both bake"
    );
    let sh = sh(&current);
    assert!(
        sh.total_probes() > 0 && !sh.probes.is_empty(),
        "no lights still needs probe metadata/depth moments"
    );
    assert!(!current.skipped("SH Bake"));
    for stage in ["Delta SH Bake", "Direct SH Bake", "Animated Direct SH Bake"] {
        assert!(
            current.skipped(stage),
            "{stage} must skip lighting-specific work:\n{}",
            current.log
        );
    }
    for id in [27, 35, 41, 45, 47, 48] {
        assert!(
            !current.sections.contains_key(&id),
            "no-light section {id} must be absent"
        );
    }
}

#[test]
#[ignore = "one-shot pre-change oracle: needs POSTRETRO_GREY_PRL_BUILD"]
fn textured_room_without_lights_matches_pre_colour_compiler() {
    compare_with_pre_colour(
        "textured_room_without_lights_matches_pre_colour_compiler",
        &unlit_room_map(),
    );
}

const EMPTY_WORLD: &str = "{\n\"classname\" \"worldspawn\"\n}\n{\n\"classname\" \"player_spawn\"\n\"origin\" \"0 0 32\"\n}\n";

// A brushless map is rejected by the pre-existing CellVisibility stage. A
// finite brush establishes a BSP but interns its side names before exterior
// culling, even if no static render geometry survives. The canonical truly
// empty name/geometry bake is therefore covered in sh_bake::empty_material_tests,
// without claiming the source-map adapter can produce that full CLI state.

fn mover_only_map() -> String {
    // An isolated solid box creates a supported BSP. All its visible sides
    // belong to the exterior and produce no static render vertices. Geometry
    // interns partition-side names before culling, so share the mover's name
    // rather than adding an unrelated name to the one-entry material table.
    let world_brush = box_brush([-128; 3], [-96; 3], "bounce/red");
    let world = format!(
        "{{\n\"classname\" \"worldspawn\"\n{world_brush}\n}}\n{{\n\"classname\" \"player_spawn\"\n\"origin\" \"0 0 32\"\n}}\n"
    );
    let brush = box_brush([-16, -16, 0], [16, 16, 16], "bounce/red");
    format!(
        "{world}{{\n\"classname\" \"kinematic_mover\"\n\"name\" \"lift\"\n\"path\" \"lift_a\"\n\"speed\" \"1\"\n\"move_mode\" \"ping_pong\"\n{brush}\n}}\n\
        {{\n\"classname\" \"kinematic_waypoint\"\n\"name\" \"lift_a\"\n\"next\" \"lift_b\"\n\"origin\" \"0 0 0\"\n}}\n\
        {{\n\"classname\" \"kinematic_waypoint\"\n\"name\" \"lift_b\"\n\"origin\" \"0 0 32\"\n}}\n"
    )
}

#[test]
#[ignore = "cold real-CLI mover-only/degenerate-grid bake"]
fn mover_only_names_reach_materials_without_creating_static_world_sh_geometry() {
    let current = bake(
        &empty_pin(&mover_only_map(), true),
        &current_compiler(),
        "current",
    );
    assert_current_shape(&current);
    assert_eq!(current.names(), ["bounce/red"]);
    assert!(geometry(&current).faces.is_empty());
    assert!(geometry(&current).vertices.is_empty());
    let movers =
        KinematicGeometrySection::from_bytes(current.section(SectionId::KinematicGeometry))
            .unwrap();
    assert_eq!(movers.movers.len(), 1);
    assert!(!movers.movers[0].face_meta.is_empty());
    assert!(
        movers.movers[0]
            .face_meta
            .iter()
            .all(|face| face.texture_index == 0)
    );
    assert_eq!(current.sidecars.len(), 1);
    assert_eq!(sh(&current).grid_dimensions, [0; 3]);
    assert!(current.skipped("SH Bake"));
    assert!(current.skipped("Delta SH Bake"));
}

#[test]
#[ignore = "one-shot pre-change oracle: needs POSTRETRO_GREY_PRL_BUILD"]
fn mover_only_names_match_pre_colour_compiler() {
    compare_with_pre_colour(
        "mover_only_names_match_pre_colour_compiler",
        &mover_only_map(),
    );
}

/// The map adapter interns every static-face name; it cannot express a
/// NO_TEXTURE sentinel or an unused intern table on an otherwise textured
/// static face. Exercise that material/key seam explicitly without pretending
/// this constructs or bakes a full CLI world.
#[test]
fn explicit_empty_and_unused_name_material_seams_keep_grey_and_empty_face_keys() {
    use postretro_level_compiler::geometry::GeometryResult;
    use postretro_level_compiler::surface_materials::stats::{BounceMaterials, MaterialStats};
    use postretro_level_compiler::texture_mips::bake_world_texture_mips;
    use postretro_level_format::geometry::{FaceMeta, NO_TEXTURE};
    use postretro_level_format::prm::{PrmFile, cache_filename_for_key};

    let fixture = Fixture::new(EMPTY_WORLD, true);
    let root = fixture.root();
    let (empty_keys, empty_report) =
        bake_world_texture_mips(&[], &root.join("textures"), &root.join("seam-materials")).unwrap();
    assert!(empty_keys.is_empty());
    assert!(empty_report.summary().is_empty());
    let empty = BounceMaterials::new(Vec::new(), 1.0).unwrap();
    assert!(empty.resolved_albedo(NO_TEXTURE).is_none());
    assert!(empty.resolved_albedo(0).is_none());
    assert_eq!(
        empty.albedo(NO_TEXTURE).to_array().map(f32::to_bits),
        [0.45_f32.to_bits(); 3]
    );

    let names = vec!["bounce/red".to_owned()];
    let bake_record = || {
        let (keys, report) =
            bake_world_texture_mips(&names, &root.join("textures"), &root.join("seam-materials"))
                .unwrap();
        assert_eq!(report.summary().material_count(), 1);
        let key = keys[&names[0]];
        let bytes = std::fs::read(
            root.join("seam-materials")
                .join(format!("{}.prm", cache_filename_for_key(&key))),
        )
        .unwrap();
        let (header, slots) = PrmFile::from_bytes_partial(&bytes);
        header.unwrap();
        let lut = std::array::from_fn(|index| {
            let channel = index as f32 / 255.0;
            if channel <= 0.04045 {
                channel / 12.92
            } else {
                ((channel + 0.055) / 1.055).powf(2.4)
            }
        });
        (
            key,
            MaterialStats::from_slots(slots[0].as_ref().ok(), slots[3].as_ref().ok(), 0.0, &lut)
                .unwrap(),
        )
    };
    let geometry = GeometryResult {
        geometry: GeometrySection {
            vertices: Vec::new(),
            indices: Vec::new(),
            faces: Vec::new(),
        },
        texture_names: TextureNamesSection {
            names: names.clone(),
        },
        face_index_ranges: Vec::new(),
    };
    let fold = |stats, saturation| {
        let mut hasher = blake3::Hasher::new();
        BounceMaterials::new(vec![stats], saturation)
            .unwrap()
            .fold_key(&mut hasher, &geometry);
        hasher.finalize()
    };
    let (red_key, red) = bake_record();
    assert_eq!(red.snapped_mean(), Some([255, 0, 0]));
    let path = root.join("textures/bounce/red.png");
    let mut encoder = png::Encoder::new(std::fs::File::create(path).unwrap(), 16, 16);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()
        .unwrap()
        .write_image_data(&[0, 0, 255, 255].repeat(256))
        .unwrap();
    let (blue_key, blue) = bake_record();
    assert_ne!(
        red_key, blue_key,
        "actual texture edit changes its material sidecar key"
    );
    assert_eq!(blue.snapped_mean(), Some([0, 0, 255]));
    assert_eq!(
        fold(red, 1.0),
        fold(blue, 1.0),
        "unused mover-only means never enter the static-face key fold"
    );
    assert_ne!(
        fold(red, 1.0),
        fold(red, 0.5),
        "even an empty face fold includes saturation"
    );

    // A real NO_TEXTURE value is passed directly, never mapped to a pretend
    // texture name as a substitute for the unsupported adapter scenario.
    let mut sentinel_geometry = geometry;
    sentinel_geometry.texture_names.names.clear();
    sentinel_geometry.geometry.faces.push(FaceMeta {
        leaf_index: 0,
        texture_index: NO_TEXTURE,
    });
    let mut hasher = blake3::Hasher::new();
    empty.fold_key(&mut hasher, &sentinel_geometry);
    assert!(
        empty
            .resolved_albedo(sentinel_geometry.geometry.faces[0].texture_index)
            .is_none()
    );
}
