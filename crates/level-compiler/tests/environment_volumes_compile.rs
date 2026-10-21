//! Environment volumes are non-solid: a volume changes only its own section, the texture tables
//! and, for a fluid with an up-facing surface, the reflection probes it derives.
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
use postretro_level_format::push_forces::PushForcesSection;
use postretro_level_format::reflection_probes::{
    REFLECTION_PROBE_TILE_DIMENSION, ReflectionProbesSection,
};
use postretro_level_format::texture_cache_keys::TextureCacheKeysSection;
use postretro_level_format::texture_names::TextureNamesSection;
use postretro_level_format::{SectionId, read_container, read_section_data};

const TEXTURE: &str = "50-free-textures/concrete_stone_021";
/// A texture the two-room world never uses.
const FLUID_TEXTURE: &str = "test/concrete_grimey";
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

/// The base map's section ids, with ReflectionProbes (id 54) when the added
/// volume is a fluid whose up-facing surface derives a probe.
fn expected_sections(base: &BTreeMap<u32, Vec<u8>>, with_probes: bool) -> Vec<u32> {
    assert!(
        !base.contains_key(&(SectionId::ReflectionProbes as u32)),
        "a map without fluid has no reflection probes"
    );
    let mut ids: Vec<u32> = base.keys().copied().collect();
    if with_probes {
        ids.push(SectionId::ReflectionProbes as u32);
        ids.sort_unstable();
    }
    ids
}

/// The doorway fluid's top is one plane in two rooms: the wall cuts it into
/// a piece in each room and one in the doorway. Each room's piece fits its
/// own room, so the boxes do not match and each gets its own probe, captured
/// 2 mm above the water and inside its box. Every up-facing piece binds a
/// probe; the sides bind nothing.
fn assert_doorway_pool_probes(sections: &BTreeMap<u32, Vec<u8>>) -> usize {
    let probes =
        ReflectionProbesSection::from_bytes(section(sections, SectionId::ReflectionProbes))
            .expect("ReflectionProbes decodes");
    let water = 80.0 * UNIT;
    for probe in &probes.probes {
        assert_eq!(probe.source_entity, FIRST_VOLUME_ENTITY);
        assert!(
            (probe.capture_point[1] - (water + 0.002)).abs() < 1e-4,
            "{probe:?}"
        );
        for axis in 0..3 {
            assert!(probe.box_min[axis] <= probe.capture_point[axis]);
            assert!(probe.capture_point[axis] <= probe.box_max[axis]);
        }
    }

    let environment = environment(sections);
    let top_faces: Vec<u32> = environment.volumes[0]
        .surface_faces
        .iter()
        .enumerate()
        .filter(|(_, face)| {
            face.vertices
                .iter()
                .all(|v| (v.position[1] - water).abs() < 1e-4)
        })
        .map(|(index, _)| index as u32)
        .collect();
    assert!(!top_faces.is_empty());
    assert_eq!(
        probes
            .bindings
            .iter()
            .map(|b| (b.volume, b.face))
            .collect::<Vec<_>>(),
        top_faces.iter().map(|&face| (0, face)).collect::<Vec<_>>(),
        "every piece of the top binds a probe; sides bind nothing"
    );

    // The pieces at either end of the pool: map x −64 and +64 are engine
    // z +64 u and −64 u. Each binds a probe fitted to its own room: the
    // ceiling (128 u), the side walls (±128 u along map y, engine x) and the
    // room's end wall and the dividing wall (map x ±256 and ±16).
    let probe_at = |engine_z: f32| {
        let face = top_faces
            .iter()
            .copied()
            .find(|&face| {
                environment.volumes[0].surface_faces[face as usize]
                    .vertices
                    .iter()
                    .any(|v| (v.position[2] - engine_z).abs() < 1e-4)
            })
            .expect("a top piece at the pool's end");
        let binding = probes.bindings.iter().find(|b| b.face == face).unwrap();
        probes.probes[binding.probe as usize]
    };
    let west = probe_at(64.0 * UNIT);
    let east = probe_at(-64.0 * UNIT);
    assert_ne!(west, east, "one probe per room");
    for (probe, near, far) in [(west, 16.0, 256.0), (east, -256.0, -16.0)] {
        let close = |a: f32, b: f32| (a - b * UNIT).abs() < 0.25;
        assert!(close(probe.box_max[1], 128.0), "{probe:?}");
        assert!(close(probe.box_min[0], -128.0) && close(probe.box_max[0], 128.0));
        assert!(
            close(probe.box_min[2], near) && close(probe.box_max[2], far),
            "{probe:?}"
        );
    }
    assert_eq!(probes.tiles.len(), probes.probes.len() * probe_tile_bytes());
    probes.probes.len()
}

/// One 128² BC6H tile.
fn probe_tile_bytes() -> usize {
    (REFLECTION_PROBE_TILE_DIMENSION as usize / 4).pow(2) * 16
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
        let prl = compile(
            &temp,
            classname,
            &two_room_map(&[doorway_volume(classname, field)]),
        );
        let with_volume = sections(&prl);

        assert_eq!(
            with_volume.keys().copied().collect::<Vec<_>>(),
            expected_sections(&base, classname == "fluid_volume"),
            "{classname}: the same sections are emitted, plus a fluid's reflection probes"
        );
        if classname == "fluid_volume" {
            let count = assert_doorway_pool_probes(&with_volume);
            // The runtime loader's cross-section checks accept it.
            let world = postretro_level_loader::load_prl(prl.to_str().expect("UTF-8 path"))
                .expect("the compiled level loads");
            let probes = world
                .reflection_probes
                .expect("the loader keeps the probes");
            assert_eq!(probes.probes.len(), count);
        }
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
        if classname == "fluid_volume" {
            // The fluid shares the world's texture, so TextureNames and
            // TextureCacheKeys stayed byte-identical above, and its faces
            // index the world's existing entry.
            let names =
                TextureNamesSection::from_bytes(section(&with_volume, SectionId::TextureNames))
                    .unwrap();
            assert!(
                !record.surface_faces.is_empty(),
                "the fluid draws a surface"
            );
            for face in &record.surface_faces {
                assert_eq!(names.names[face.texture_index as usize], TEXTURE);
            }
        } else {
            assert!(
                record.surface_faces.is_empty(),
                "{classname} draws no surface"
            );
        }
    }
}

/// A fluid whose surface texture the world does not use changes
/// only EnvironmentVolumes, and TextureNames and TextureCacheKeys gain that one
/// texture, appended after every world entry.
#[test]
fn fluid_surface_texture_appends_to_the_texture_tables_and_changes_no_other_section() {
    let temp = TempBuildDir::new();
    // A real PNG, so the appended cache key is a baked bundle, not the
    // missing-texture zero key.
    let texture_dir = temp.0.join("textures").join("test");
    std::fs::create_dir_all(&texture_dir).expect("create fixture texture dir");
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../content/dev/textures")
        .join(format!("{FLUID_TEXTURE}.png"));
    std::fs::copy(&source, texture_dir.join("concrete_grimey.png"))
        .unwrap_or_else(|error| panic!("copy {}: {error}", source.display()));

    let base = sections(&compile(&temp, "base", &two_room_map(&[])));
    let fluid = format!(
        "{{\n\"classname\" \"fluid_volume\"\n\"fluid\" \"water\"\n{}\n}}",
        box_brush([-64, -48, 0], [64, 48, 80]).replace(TEXTURE, FLUID_TEXTURE)
    );
    let with_fluid = sections(&compile(&temp, "fluid", &two_room_map(&[fluid])));

    assert_eq!(
        with_fluid.keys().copied().collect::<Vec<_>>(),
        expected_sections(&base, true),
        "the same sections are emitted, plus the fluid's reflection probes"
    );
    let tables = [
        SectionId::EnvironmentVolumes as u32,
        SectionId::TextureNames as u32,
        SectionId::TextureCacheKeys as u32,
    ];
    for (id, payload) in &base {
        if !tables.contains(id) {
            assert!(
                with_fluid[id] == *payload,
                "section {id} ({:?}) differs from the map without the volume",
                SectionId::from_u32(*id)
            );
        }
    }

    let base_names =
        TextureNamesSection::from_bytes(section(&base, SectionId::TextureNames)).unwrap();
    let names =
        TextureNamesSection::from_bytes(section(&with_fluid, SectionId::TextureNames)).unwrap();
    let mut expected_names = base_names.names.clone();
    expected_names.push(FLUID_TEXTURE.to_string());
    assert_eq!(
        names.names, expected_names,
        "only the fluid texture is appended"
    );

    let base_keys =
        TextureCacheKeysSection::from_bytes(section(&base, SectionId::TextureCacheKeys)).unwrap();
    let keys =
        TextureCacheKeysSection::from_bytes(section(&with_fluid, SectionId::TextureCacheKeys))
            .unwrap();
    assert_eq!(keys.keys.len(), base_keys.keys.len() + 1);
    assert_eq!(keys.keys[..base_keys.keys.len()], base_keys.keys[..]);
    assert_ne!(
        keys.keys[base_keys.keys.len()],
        [0u8; 32],
        "the appended key names the baked fluid texture"
    );

    let environment = environment(&with_fluid);
    let record = &environment.volumes[0];
    assert!(
        !record.surface_faces.is_empty(),
        "the fluid draws a surface"
    );
    for face in &record.surface_faces {
        assert_eq!(face.texture_index as usize, base_names.names.len());
    }
}

/// A push volume authoring a force other than 1 writes the PushForces section
/// beside EnvironmentVolumes, one force per record in record order; a map whose
/// pushes author no force writes none.
#[test]
fn an_authored_push_force_writes_the_push_forces_section_in_record_order() {
    let temp = TempBuildDir::new();
    let west_push = |field: &str| {
        format!(
            "{{\n\"classname\" \"push_volume\"\n\"push\" \"8 0 0\"\n{field}{}\n}}",
            box_brush([-224, -96, 0], [-160, -32, 64])
        )
    };

    let forced = sections(&compile(
        &temp,
        "forced",
        &two_room_map(&[
            doorway_volume("fluid_volume", "\"fluid\" \"water\""),
            west_push("\"force\" \"0\"\n"),
            west_push(""),
        ]),
    ));
    let records = environment(&forced).volumes;
    let forces = PushForcesSection::from_bytes(section(&forced, SectionId::PushForces))
        .expect("PushForces decodes");
    assert_eq!(forces.forces, vec![1.0, 0.0, 1.0]);
    assert_eq!(forces.forces.len(), records.len(), "one force per record");
    let pushed: Vec<bool> = records.iter().map(|record| record.push.is_some()).collect();
    assert_eq!(pushed, [false, true, true], "forces follow record order");

    let unforced = sections(&compile(&temp, "unforced", &two_room_map(&[west_push("")])));
    assert_eq!(environment(&unforced).volumes.len(), 1);
    assert!(
        !unforced.contains_key(&(SectionId::PushForces as u32)),
        "a push with no authored force writes no PushForces section"
    );
}

// Acceptance row 10: an authored probe brush is compiler-only. It changes the
// reflection probes and nothing else: geometry, BVH and SH stay
// byte-identical, and so does every other section.
#[test]
fn adding_a_reflection_volume_changes_only_the_reflection_probes() {
    let temp = TempBuildDir::new();
    let pool = doorway_volume("fluid_volume", "\"fluid\" \"water\"");
    let without = sections(&compile(
        &temp,
        "pool",
        &two_room_map(std::slice::from_ref(&pool)),
    ));
    // The west room, a unit in from its walls: it claims the pool's west piece.
    let probe_box = format!(
        "{{\n\"classname\" \"reflection_volume\"\n{}\n}}",
        box_brush([-248, -120, 0], [-24, 120, 120])
    );
    let with = sections(&compile(
        &temp,
        "pool-probe",
        &two_room_map(&[pool, probe_box]),
    ));

    assert_eq!(
        with.keys().copied().collect::<Vec<_>>(),
        without.keys().copied().collect::<Vec<_>>()
    );
    for id in [
        SectionId::Geometry,
        SectionId::Bvh,
        SectionId::OctahedralShVolume,
    ] {
        assert!(
            section(&with, id) == section(&without, id),
            "{id:?} differs with a reflection volume"
        );
    }
    for (id, payload) in &without {
        if *id != SectionId::ReflectionProbes as u32 {
            assert!(
                with[id] == *payload,
                "section {id} ({:?}) differs with a reflection volume",
                SectionId::from_u32(*id)
            );
        }
    }
    let probes =
        ReflectionProbesSection::from_bytes(section(&with, SectionId::ReflectionProbes)).unwrap();
    assert_eq!(
        probes.probes[0].source_entity,
        FIRST_VOLUME_ENTITY + 1,
        "authored probes lead, named by the reflection volume"
    );
    assert_ne!(
        section(&with, SectionId::ReflectionProbes),
        section(&without, SectionId::ReflectionProbes)
    );
}

/// One block hanging over the west piece of the doorway pool, above its
/// capture point: it fills the probe's view straight up.
const OVER_THE_POOL: ([i32; 3], [i32; 3]) = ([-56, -40, 96], [-24, 40, 112]);

/// A kinematic mover whose brush is [`OVER_THE_POOL`], with its two waypoints.
fn mover_over_the_pool() -> [String; 3] {
    let (min, max) = OVER_THE_POOL;
    let entity = |lines: &[&str]| format!("{{\n{}\n}}", lines.join("\n"));
    [
        entity(&[
            "\"classname\" \"kinematic_mover\"",
            "\"name\" \"lift\"",
            "\"path\" \"lift_a\"",
            "\"speed\" \"1\"",
            "\"move_mode\" \"ping_pong\"",
            "\"start_on_spawn\" \"1\"",
            &box_brush(min, max),
        ]),
        entity(&[
            "\"classname\" \"kinematic_waypoint\"",
            "\"name\" \"lift_a\"",
            "\"next\" \"lift_b\"",
            "\"origin\" \"-40 0 104\"",
        ]),
        entity(&[
            "\"classname\" \"kinematic_waypoint\"",
            "\"name\" \"lift_b\"",
            "\"next\" \"\"",
            "\"origin\" \"-40 0 120\"",
        ]),
    ]
}

/// A `switch` whose brush is [`OVER_THE_POOL`]: static world brushwork.
fn switch_over_the_pool() -> String {
    let (min, max) = OVER_THE_POOL;
    format!(
        "{{\n\"classname\" \"switch\"\n\"name\" \"pool_switch\"\n\"on_fire\" \"nothing\"\n{}\n}}",
        box_brush(min, max)
    )
}

// Acceptance row 10: the capture sees the static world only. A mover hanging
// over the pool, in front of its probe, and a dynamic-tier light in its room
// leave id 54 byte-identical, tiles included (and the static geometry with
// it). The same brush authored as a `switch` is static world: it enters the
// geometry and appears in the probe's tile.
#[test]
fn the_capture_skips_movers_and_dynamic_lights_and_sees_switch_brushes() {
    let temp = TempBuildDir::new();
    let pool = doorway_volume("fluid_volume", "\"fluid\" \"water\"");
    let base = sections(&compile(
        &temp,
        "pool",
        &two_room_map(std::slice::from_ref(&pool)),
    ));
    let probes = section(&base, SectionId::ReflectionProbes);

    let mut with_mover = vec![pool.clone()];
    with_mover.extend(mover_over_the_pool());
    let mover = sections(&compile(&temp, "pool-mover", &two_room_map(&with_mover)));
    assert!(
        mover.contains_key(&(SectionId::KinematicGeometry as u32)),
        "the mover compiles as a mover: {:?}",
        mover.keys().collect::<Vec<_>>()
    );
    assert!(
        section(&mover, SectionId::Geometry) == section(&base, SectionId::Geometry),
        "a mover is not static geometry"
    );
    assert!(
        section(&mover, SectionId::ReflectionProbes) == probes,
        "a mover in front of the probe leaves id 54, tiles included, unchanged"
    );

    let dynamic = sections(&compile(
        &temp,
        "pool-dynamic-light",
        &two_room_map(&[
            pool.clone(),
            light([-128, 0, 100], "").replacen("\"light\"\n", "\"light_dynamic\"\n", 1),
        ]),
    ));
    let alpha = postretro_level_format::alpha_lights::AlphaLightsSection::from_bytes(section(
        &dynamic,
        SectionId::AlphaLights,
    ))
    .expect("AlphaLights decodes");
    assert!(
        alpha.lights.iter().any(|light| light.is_dynamic),
        "the light compiles as dynamic-tier"
    );
    assert!(
        section(&dynamic, SectionId::ReflectionProbes) == probes,
        "a dynamic-tier light in the room leaves id 54, tiles included, unchanged"
    );

    let switch = sections(&compile(
        &temp,
        "pool-switch",
        &two_room_map(&[pool, switch_over_the_pool()]),
    ));
    assert!(
        section(&switch, SectionId::Geometry) != section(&base, SectionId::Geometry),
        "a switch is static geometry"
    );
    let base_probes = ReflectionProbesSection::from_bytes(probes).unwrap();
    let switch_probes =
        ReflectionProbesSection::from_bytes(section(&switch, SectionId::ReflectionProbes)).unwrap();
    assert_eq!(base_probes.probes.len(), switch_probes.probes.len());
    let changed = (0..base_probes.probes.len())
        .filter(|&layer| base_probes.tile(layer) != switch_probes.tile(layer))
        .count();
    assert!(changed > 0, "the switch brush appears in a probe's tile");
}

// P16: the stress map's 574 fluid volumes still build, with no more probes
// than the cap. Slow (a full stress compile); run on demand.
#[test]
#[ignore = "compiles the stress-env-volumes map (minutes); run with --ignored"]
fn stress_env_volumes_builds_with_no_more_probes_than_the_cap() {
    let temp = TempBuildDir::new();
    let input =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/dev/maps/stress-env-volumes.map");
    let output_path = temp.0.join("stress-env-volumes.prl");
    let started = std::time::Instant::now();
    let output = Command::new(env!("CARGO_BIN_EXE_prl-build"))
        .env("RUST_LOG", "warn")
        .arg(&input)
        .arg("-o")
        .arg(&output_path)
        .arg("--no-cache")
        .arg("--no-tui")
        .output()
        .expect("spawn prl-build");
    assert!(
        output.status.success(),
        "prl-build failed for the stress map\n{}",
        combined_logs(&output)
    );
    let built = sections(&output_path);
    let probes =
        ReflectionProbesSection::from_bytes(section(&built, SectionId::ReflectionProbes)).unwrap();
    eprintln!(
        "stress-env-volumes: {} probe(s), {} binding(s), built in {:.1} s",
        probes.probes.len(),
        probes.bindings.len(),
        started.elapsed().as_secs_f64()
    );
    assert!(!probes.probes.is_empty());
    assert!(
        probes.probes.len() <= postretro_level_format::reflection_probes::MAX_REFLECTION_PROBES
    );
}
