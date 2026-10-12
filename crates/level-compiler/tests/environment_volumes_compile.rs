//! Environment volumes are non-solid: a volume spanning a doorway changes no PRL section but its own.
//! See: context/lib/build_pipeline.md §Compiler pipeline

use std::collections::BTreeMap;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

use postretro_level_format::cells::CellsSection;
use postretro_level_format::environment_volumes::EnvironmentVolumesSection;
use postretro_level_format::geometry::GeometrySection;
use postretro_level_format::portals::PortalsSection;
use postretro_level_format::{SectionId, read_container, read_section_data};

const TEXTURE: &str = "50-free-textures/concrete_stone_021";
/// Quake map units to engine meters.
const UNIT: f32 = 0.0254;
/// Authored worldspawn gravity, distinct from the default so the section
/// visibly carries the map's value.
const LEVEL_GRAVITY: f32 = -7.5;

struct TempBuildDir(PathBuf);

impl TempBuildDir {
    fn new() -> Self {
        static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let sequence = SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock must be after the Unix epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "postretro-environment-volumes-compile-{}-{nonce}-{sequence}",
            std::process::id()
        ));
        std::fs::create_dir_all(path.join("maps")).expect("create isolated compiler directory");
        Self(path)
    }
}

impl Drop for TempBuildDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn box_brush(min: [i32; 3], max: [i32; 3]) -> String {
    let ([x0, y0, z0], [x1, y1, z1]) = (min, max);
    format!(
        "{{\n\
         ( {x0} 0 0 ) ( {x0} 1 0 ) ( {x0} 0 1 ) {TEXTURE} 0 0 0 1 1\n\
         ( {x1} 0 0 ) ( {x1} 0 1 ) ( {x1} 1 0 ) {TEXTURE} 0 0 0 1 1\n\
         ( 0 {y0} 0 ) ( 0 {y0} 1 ) ( 1 {y0} 0 ) {TEXTURE} 0 0 0 1 1\n\
         ( 0 {y1} 0 ) ( 1 {y1} 0 ) ( 0 {y1} 1 ) {TEXTURE} 0 0 0 1 1\n\
         ( 0 0 {z0} ) ( 1 0 {z0} ) ( 0 1 {z0} ) {TEXTURE} 0 0 0 1 1\n\
         ( 0 0 {z1} ) ( 0 1 {z1} ) ( 1 0 {z1} ) {TEXTURE} 0 0 0 1 1\n\
         }}"
    )
}

fn light(origin: [i32; 3], extra: &str) -> String {
    let [x, y, z] = origin;
    format!(
        "{{\n\"classname\" \"light\"\n\"origin\" \"{x} {y} {z}\"\n\"light\" \"300\"\n\
         \"_falloff_range\" \"320\"\n\"_light_size\" \"0\"\n{extra}}}"
    )
}

/// Two sealed rooms joined by a 64 u wide, 96 u tall doorway in a 32 u wall
/// at x = 0. Entity 2 is a lightmapped light in the west room; entity 3 an
/// SDF-shadowed light in the east room, so the SDF atlas is emitted.
/// `volumes` are appended last, so no earlier entity index moves.
fn two_room_map(volumes: &[String]) -> String {
    let brushes = [
        box_brush([-272, -144, -16], [272, 144, 0]),
        box_brush([-272, -144, 128], [272, 144, 144]),
        box_brush([-272, -144, 0], [-256, 144, 128]),
        box_brush([256, -144, 0], [272, 144, 128]),
        box_brush([-256, -144, 0], [256, -128, 128]),
        box_brush([-256, 128, 0], [256, 144, 128]),
        // The dividing wall and its doorway.
        box_brush([-16, -128, 0], [16, -32, 128]),
        box_brush([-16, 32, 0], [16, 128, 128]),
        box_brush([-16, -32, 96], [16, 32, 128]),
    ];
    let mut entities = vec![
        format!(
            "{{\n\"classname\" \"worldspawn\"\n\"initialGravity\" \"{LEVEL_GRAVITY}\"\n{}\n}}",
            brushes.join("\n")
        ),
        "{\n\"classname\" \"player_spawn\"\n\"origin\" \"-128 0 32\"\n}".to_string(),
        light([-128, 0, 64], ""),
        light([128, 0, 64], "\"_shadow_type\" \"sdf\"\n"),
    ];
    entities.extend(volumes.iter().cloned());
    entities.join("\n")
}

/// Index of the first appended volume entity.
const FIRST_VOLUME_ENTITY: u32 = 4;

/// A box spanning the doorway: through the wall and into both rooms.
fn doorway_volume(classname: &str, field: &str) -> String {
    format!(
        "{{\n\"classname\" \"{classname}\"\n{field}\n{}\n}}",
        box_brush([-64, -48, 0], [64, 48, 80])
    )
}

fn compile(temp: &TempBuildDir, name: &str, map_text: &str) -> PathBuf {
    let input = temp.0.join("maps").join(format!("{name}.map"));
    let output_path = temp.0.join(format!("{name}.prl"));
    std::fs::write(&input, format!("{map_text}\n")).expect("write fixture map");
    let output = Command::new(env!("CARGO_BIN_EXE_prl-build"))
        .env("RUST_LOG", "warn")
        .arg(&input)
        .arg("-o")
        .arg(&output_path)
        .arg("--no-cache")
        .arg("--no-tui")
        .arg("--sh-probe-spacing")
        .arg("4")
        .arg("--lightmap-density")
        .arg("0.25")
        .output()
        .expect("spawn prl-build");
    assert!(
        output.status.success(),
        "prl-build failed for {name}\n{}",
        combined_logs(&output)
    );
    output_path
}

fn combined_logs(output: &Output) -> String {
    format!(
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

/// Every section payload, keyed by section id.
fn sections(prl: &Path) -> BTreeMap<u32, Vec<u8>> {
    let bytes = std::fs::read(prl).expect("read compiled PRL");
    let mut cursor = Cursor::new(bytes);
    let metadata = read_container(&mut cursor).expect("read PRL container");
    let ids: Vec<u32> = metadata
        .sections
        .iter()
        .map(|entry| entry.section_id)
        .collect();
    ids.into_iter()
        .map(|id| {
            let payload = read_section_data(&mut cursor, &metadata, id)
                .unwrap_or_else(|error| panic!("read section {id}: {error}"))
                .unwrap_or_else(|| panic!("section {id} is listed"));
            (id, payload)
        })
        .collect()
}

fn environment(sections: &BTreeMap<u32, Vec<u8>>) -> EnvironmentVolumesSection {
    let bytes = sections
        .get(&(SectionId::EnvironmentVolumes as u32))
        .expect("EnvironmentVolumes is always written");
    EnvironmentVolumesSection::from_bytes(bytes).expect("EnvironmentVolumes decodes")
}

fn section(sections: &BTreeMap<u32, Vec<u8>>, id: SectionId) -> &[u8] {
    sections
        .get(&(id as u32))
        .unwrap_or_else(|| panic!("{id:?} must be present"))
}

#[test]
fn doorway_volumes_leave_every_other_prl_section_byte_identical() {
    let temp = TempBuildDir::new();
    let base = sections(&compile(&temp, "base", &two_room_map(&[])));

    // The fixture exercises the bakes the row names, and the doorway is a
    // real portal for the volumes to span.
    for id in [
        SectionId::Cells,
        SectionId::Portals,
        SectionId::Geometry,
        SectionId::NavMesh,
        SectionId::OctahedralShVolume,
        SectionId::Lightmap,
        SectionId::SdfAtlas,
    ] {
        section(&base, id);
    }
    let base_cells = CellsSection::from_bytes(section(&base, SectionId::Cells)).unwrap();
    let base_portals = PortalsSection::from_bytes(section(&base, SectionId::Portals)).unwrap();
    let base_geometry = GeometrySection::from_bytes(section(&base, SectionId::Geometry)).unwrap();
    assert!(!base_portals.portals.is_empty(), "the doorway is a portal");

    let base_environment = environment(&base);
    assert_eq!(base_environment.level_gravity, LEVEL_GRAVITY);
    assert!(base_environment.volumes.is_empty());

    for (classname, field) in [
        ("fluid_volume", "\"fluid\" \"water\""),
        ("gravity_volume", "\"gravity\" \"-3\""),
        ("push_volume", "\"push\" \"0 0 15\""),
    ] {
        let with_volume = sections(&compile(
            &temp,
            classname,
            &two_room_map(&[doorway_volume(classname, field)]),
        ));

        assert_eq!(
            with_volume.keys().collect::<Vec<_>>(),
            base.keys().collect::<Vec<_>>(),
            "{classname}: the same sections are emitted"
        );
        for (id, payload) in &base {
            if *id == SectionId::EnvironmentVolumes as u32 {
                continue;
            }
            assert!(
                with_volume[id] == *payload,
                "{classname}: section {id} ({:?}) differs from the map without the volume",
                SectionId::from_u32(*id)
            );
        }

        // Cell, portal, static-collision-triangle and navmesh identity, read
        // through the decoders rather than only as bytes.
        let cells = CellsSection::from_bytes(section(&with_volume, SectionId::Cells)).unwrap();
        let portals =
            PortalsSection::from_bytes(section(&with_volume, SectionId::Portals)).unwrap();
        let geometry =
            GeometrySection::from_bytes(section(&with_volume, SectionId::Geometry)).unwrap();
        assert_eq!(
            cells.cells.len(),
            base_cells.cells.len(),
            "{classname}: cells"
        );
        assert_eq!(
            portals.portals.len(),
            base_portals.portals.len(),
            "{classname}: portals"
        );
        assert_eq!(
            geometry.indices.len(),
            base_geometry.indices.len(),
            "{classname}: static triangles"
        );
        assert_eq!(
            section(&with_volume, SectionId::NavMesh),
            section(&base, SectionId::NavMesh),
            "{classname}: navmesh"
        );

        let environment = environment(&with_volume);
        assert_eq!(environment.level_gravity, LEVEL_GRAVITY);
        assert_eq!(environment.volumes.len(), 1, "{classname}");
        let record = &environment.volumes[0];
        assert_eq!(record.source_entity, FIRST_VOLUME_ENTITY);
        assert_eq!(record.priority, 0);
        match classname {
            "fluid_volume" => {
                assert_eq!(record.fluid, "water");
                assert_eq!((record.gravity, record.push), (None, None));
            }
            "gravity_volume" => {
                assert_eq!(record.gravity, Some(-3.0));
                assert_eq!((record.fluid.as_str(), record.push), ("", None));
            }
            _ => {
                assert_eq!(record.push, Some([0.0, 15.0, 0.0]));
                assert_eq!((record.fluid.as_str(), record.gravity), ("", None));
            }
        }
        // Quake box (-64,-48,0)..(64,48,80) → engine x = -y, y = z, z = -x.
        assert_eq!(record.brushes.len(), 1);
        let brush = &record.brushes[0];
        let expected_min = [-48.0 * UNIT, 0.0, -64.0 * UNIT];
        let expected_max = [48.0 * UNIT, 80.0 * UNIT, 64.0 * UNIT];
        for axis in 0..3 {
            assert!((brush.aabb_min[axis] - expected_min[axis]).abs() < 1e-5);
            assert!((brush.aabb_max[axis] - expected_max[axis]).abs() < 1e-5);
        }
        assert_eq!(brush.planes.len(), 6);
        assert!(record.surface_faces.is_empty());
    }
}
