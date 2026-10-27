//! Empty material/geometry tails through current and preserved pre-colour compilers.
//! See: context/lib/build_pipeline.md §Compiler pipeline

#[path = "../src/coloured_bounce_test_fixtures.rs"]
#[allow(dead_code)] // Shared appearance variants also serve the colour/oracle suites.
mod fixtures;

use std::collections::BTreeMap;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::process::Command;

use fixtures::Appearance;
use postretro_level_format::geometry::GeometrySection;
use postretro_level_format::kinematic_geometry::KinematicGeometrySection;
use postretro_level_format::prm::cache_filename_for_key;
use postretro_level_format::sh_volume::OctahedralShVolumeSection;
use postretro_level_format::texture_cache_keys::TextureCacheKeysSection;
use postretro_level_format::texture_names::TextureNamesSection;
use postretro_level_format::{SectionId, read_container, read_section_data};

struct Fixture {
    dir: tempfile::TempDir,
    map: PathBuf,
}

struct Bake {
    sections: BTreeMap<u32, Vec<u8>>,
    sidecars: BTreeMap<String, Vec<u8>>,
    report: Vec<String>,
    log: String,
}

const SAVED_GREY_COMPILER_SHA256: &str =
    "098a6fc94c21ef020e065806b3a2e9600fccea3208c3c5240dccc6c72a332496";

fn grey_compiler() -> PathBuf {
    let path = std::env::var_os("POSTRETRO_GREY_PRL_BUILD")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../work/ab/grey/prl-build")
        });
    assert!(
        path.is_file(),
        "preserved pre-colour compiler required: {} (or POSTRETRO_GREY_PRL_BUILD)",
        path.display()
    );
    let output = Command::new("/usr/bin/shasum")
        .args(["-a", "256"])
        .arg(&path)
        .output()
        .expect("hash preserved pre-colour compiler");
    assert!(
        output.status.success(),
        "preserved compiler hash must be readable: {}",
        path.display()
    );
    let output = String::from_utf8(output.stdout).expect("compiler hash is UTF-8");
    assert_eq!(
        output.split_whitespace().next(),
        Some(SAVED_GREY_COMPILER_SHA256),
        "preserved compiler must match the artifact pinned by the coloured-bounce oracle"
    );
    path
}

impl Fixture {
    fn new(map_text: &str, textured: bool) -> Self {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("maps")).unwrap();
        std::fs::create_dir(dir.path().join("textures")).unwrap();
        if textured {
            fixtures::write_textures(dir.path());
        }
        let map = dir.path().join("maps/empty-pin.map");
        // Uniform storage keeps this empty/grey proof independent of the
        // separately tested, owner-approved adaptive direct-storage boundary.
        let map_text = map_text.replacen(
            "\"classname\" \"worldspawn\"",
            "\"classname\" \"worldspawn\"\n\"_sh_coarsen\" \"0\"",
            1,
        );
        std::fs::write(&map, map_text).unwrap();
        Self { dir, map }
    }

    fn bake(&self, compiler: &Path, label: &str) -> Bake {
        let baked = self.dir.path().join(format!("{label}-baked"));
        let prl = self.dir.path().join(format!("{label}.prl"));
        let output = Command::new(compiler)
            .env("RUST_LOG", "info")
            .arg(&self.map)
            .arg("-o")
            .arg(&prl)
            .arg("--baked-root")
            .arg(&baked)
            .args([
                "--no-cache",
                "--no-tui",
                "--jobs",
                "1",
                "--lightmap-density",
                "0.25",
                "--sh-probe-spacing",
                "1",
                "--soft-shadow-samples",
                "4",
                "--uncompressed-irradiance",
            ])
            .output()
            .expect("run tiny empty-pin bake");
        let log = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.status.success(), "{label} compile failed:\n{log}");
        let mut reader = Cursor::new(std::fs::read(prl).unwrap());
        let metadata = read_container(&mut reader).unwrap();
        let sections = [16, 17, 27, 32, 34, 35, 41, 43, 45, 47, 48]
            .into_iter()
            .filter_map(|id| {
                read_section_data(&mut reader, &metadata, id)
                    .unwrap()
                    .map(|bytes| (id, bytes))
            })
            .collect();
        let material_root = baked.join("materials");
        let sidecars = if material_root.exists() {
            std::fs::read_dir(material_root)
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .filter(|path| path.extension().is_some_and(|extension| extension == "prm"))
                .map(|path| {
                    (
                        path.file_name().unwrap().to_string_lossy().into_owned(),
                        std::fs::read(path).unwrap(),
                    )
                })
                .collect()
        } else {
            BTreeMap::new()
        };
        // The report includes all per-slot/mip/first-name accounting lines;
        // timings and paths elsewhere in the compiler log do not participate.
        let report = log
            .lines()
            .filter_map(|line| line.split_once("[prl-build] ").map(|(_, text)| text))
            .filter(|text| {
                ["texture bytes:", "  by slot:", "  by mip:", "  largest:"]
                    .iter()
                    .any(|prefix| text.starts_with(prefix))
            })
            .map(str::to_owned)
            .collect();
        Bake {
            sections,
            sidecars,
            report,
            log,
        }
    }

    fn pair(&self) -> (Bake, Bake) {
        // Independent baked roots ensure both compilers see absent sidecars.
        let grey = self.bake(&grey_compiler(), "grey");
        let current = self.bake(Path::new(env!("CARGO_BIN_EXE_prl-build")), "current");
        (grey, current)
    }
}

impl Bake {
    fn section(&self, id: SectionId) -> &[u8] {
        self.sections
            .get(&(id as u32))
            .unwrap_or_else(|| panic!("required {id:?} section missing"))
            .as_slice()
    }

    fn names(&self) -> Vec<String> {
        TextureNamesSection::from_bytes(self.section(SectionId::TextureNames))
            .unwrap()
            .names
    }

    fn geometry(&self) -> GeometrySection {
        GeometrySection::from_bytes(self.section(SectionId::Geometry)).unwrap()
    }

    fn sh(&self) -> OctahedralShVolumeSection {
        OctahedralShVolumeSection::from_bytes(self.section(SectionId::OctahedralShVolume)).unwrap()
    }

    fn assert_materials_before_sh(&self) {
        let materials = self
            .log
            .find("World material bake...")
            .expect("material stage ran");
        let sh = self
            .log
            .find("SH volume bake...")
            .expect("base SH stage ran");
        assert!(materials < sh, "materials must finish before SH begins");
    }

    fn skipped(&self, stage: &str) -> bool {
        self.log
            .lines()
            .any(|line| line.trim() == format!("{stage}: skipped"))
    }

    fn assert_key_sidecar_correspondence(&self) {
        let names = self.names();
        let keys = TextureCacheKeysSection::from_bytes(self.section(SectionId::TextureCacheKeys))
            .unwrap()
            .keys;
        assert_eq!(names.len(), keys.len());
        for (name, key) in names.iter().zip(keys) {
            if key != [0; 32] {
                assert!(
                    self.sidecars
                        .contains_key(&format!("{}.prm", cache_filename_for_key(&key))),
                    "{name} must reach its baked sidecar"
                );
            }
        }
    }
}

fn assert_precolour_equal(grey: &Bake, current: &Bake) {
    // Missing optional sections compare as absence, never as a fabricated payload.
    for id in [16, 17, 27, 32, 34, 43] {
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
        !grey.report.is_empty(),
        "baseline must emit its byte report"
    );
    assert_eq!(
        grey.report, current.report,
        "single byte report and first-name accounting"
    );
    assert_eq!(
        current
            .report
            .iter()
            .filter(|line| line.starts_with("texture bytes:"))
            .count(),
        1
    );
    current.assert_materials_before_sh();
    current.assert_key_sidecar_correspondence();
}

fn no_fluid(map: String) -> String {
    let end = map
        .rfind("{\n\"classname\" \"fluid_volume\"")
        .expect("shared fixture ends with fluid entity");
    map[..end].to_owned()
}

#[test]
#[ignore = "cold real-CLI empty-tail comparison with preserved pre-colour compiler"]
fn no_fluid_only_tail_preserves_grey_lighting_texture_indices_and_report() {
    // Normal/specular-only sources produce real bundles while every bounce
    // surface remains the original exact grey fallback in both compilers.
    let fixture = Fixture::new(
        &no_fluid(fixtures::room_map(Appearance::Unresolved, true, true)),
        true,
    );
    let (grey, current) = fixture.pair();
    assert_precolour_equal(&grey, &current);
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
    assert!(current.sh().total_probes() > 0);
    assert!(!current.skipped("SH Bake"));
}

#[test]
#[ignore = "cold real-CLI no-light comparison with preserved pre-colour compiler"]
fn textured_room_without_lights_keeps_base_metadata_and_skips_direct_delta_work() {
    let fixture = Fixture::new(&fixtures::room_map(Appearance::RedWall, false, false), true);
    let (grey, current) = fixture.pair();
    assert_precolour_equal(&grey, &current);
    assert_eq!(
        current.sidecars.len(),
        3,
        "world materials and real fluid-only tail both bake"
    );
    let sh = current.sh();
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

const EMPTY_WORLD: &str = "{\n\"classname\" \"worldspawn\"\n}\n{\n\"classname\" \"player_spawn\"\n\"origin\" \"0 0 32\"\n}\n";

// A brushless map is rejected by the pre-existing CellVisibility stage. A
// finite brush establishes a BSP but interns its side names before exterior
// culling, even if no static render geometry survives. The canonical truly
// empty name/geometry bake is therefore covered in sh_bake::empty_material_tests,
// without claiming the source-map adapter can produce that full CLI state.

fn box_brush(min: [i32; 3], max: [i32; 3], texture: &str) -> String {
    let ([x0, y0, z0], [x1, y1, z1]) = (min, max);
    format!(
        "{{\n\
        ( {x0} 0 0 ) ( {x0} 1 0 ) ( {x0} 0 1 ) {texture} 0 0 0 1 1\n\
        ( {x1} 0 0 ) ( {x1} 0 1 ) ( {x1} 1 0 ) {texture} 0 0 0 1 1\n\
        ( 0 {y0} 0 ) ( 0 {y0} 1 ) ( 1 {y0} 0 ) {texture} 0 0 0 1 1\n\
        ( 0 {y1} 0 ) ( 1 {y1} 0 ) ( 0 {y1} 1 ) {texture} 0 0 0 1 1\n\
        ( 0 0 {z0} ) ( 1 0 {z0} ) ( 0 1 {z0} ) {texture} 0 0 0 1 1\n\
        ( 0 0 {z1} ) ( 0 1 {z1} ) ( 1 0 {z1} ) {texture} 0 0 0 1 1\n}}\n"
    )
}

fn mover_only_map() -> String {
    // An isolated solid box creates a supported BSP. All its visible sides
    // belong to the exterior and produce no static render vertices. Geometry
    // interns partition-side names before culling, so share the mover's name
    // rather than adding an unrelated name to the one-entry material table.
    let world_brush = box_brush([-128; 3], [-96; 3], "bounce/red");
    let world = format!(
        "{{\n\"classname\" \"worldspawn\"\n{world_brush}}}\n{{\n\"classname\" \"player_spawn\"\n\"origin\" \"0 0 32\"\n}}\n"
    );
    let brush = box_brush([-16, -16, 0], [16, 16, 16], "bounce/red");
    format!(
        "{world}{{\n\"classname\" \"kinematic_mover\"\n\"name\" \"lift\"\n\"path\" \"lift_a\"\n\"speed\" \"1\"\n\"move_mode\" \"ping_pong\"\n{brush}}}\n\
        {{\n\"classname\" \"kinematic_waypoint\"\n\"name\" \"lift_a\"\n\"next\" \"lift_b\"\n\"origin\" \"0 0 0\"\n}}\n\
        {{\n\"classname\" \"kinematic_waypoint\"\n\"name\" \"lift_b\"\n\"origin\" \"0 0 32\"\n}}\n"
    )
}

#[test]
#[ignore = "cold real-CLI mover-only/degenerate-grid comparison with preserved pre-colour compiler"]
fn mover_only_names_reach_materials_without_creating_static_world_sh_geometry() {
    let fixture = Fixture::new(&mover_only_map(), true);
    let (grey, current) = fixture.pair();
    assert_precolour_equal(&grey, &current);
    assert_eq!(current.names(), ["bounce/red"]);
    assert!(current.geometry().faces.is_empty());
    assert!(current.geometry().vertices.is_empty());
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
    assert_eq!(current.sh().grid_dimensions, [0; 3]);
    assert!(current.skipped("SH Bake"));
    assert!(current.skipped("Delta SH Bake"));
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
    let root = fixture.dir.path();
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
            MaterialStats::from_slots(
                slots[0].as_ref().ok(),
                slots[3].as_ref().ok(),
                0.0,
                1.0,
                &lut,
            )
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
