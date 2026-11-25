//! Real `prl-build --glossy-reflections` output reaches the runtime: the loader's world probe
//! stream stamps exactly the bound faces' vertices with their probe, and the compiler's opt-in
//! verdict for each material equals the runtime's on the same baked `.prm` bytes.
//! See: context/lib/resource_management.md §5.3, §5.4 · rendering_pipeline.md §7.3a

mod prl_build_support;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use glam::Vec3;
use postretro_level_format::octahedral;
use postretro_level_loader::{LevelWorld, NO_WORLD_PROBE, build_vertex_world_probes, load_prl};
use postretro_render_cpu::loaded_texture::specular_slot_reflects_in;
use postretro_render_cpu::texture_prep::{PreparedTexture, prepare_world_textures};

use prl_build_support::{BuildDir, link_dev_textures, prl_build};

/// `_s` 255 and `_p` 200 (exponent about 922): reflects at full weight.
const GLOSSY_FLOOR: &str = "test/metal_polished";
/// `_s` 255 and `_p` 97 (exponent about 11.6, inside the fade band): reflects.
const GLOSSY_WALL: &str = "test/metal_fade_band";
/// `_s` 255 and `_p` 64 (exponent about 4.4, under the sharpness floor):
/// a four-channel slot that never reflects.
const SATIN: &str = "test/concrete_satin";
/// `_s` and `_t` without `_p`: smoothness 0, so it never reflects.
const NO_SHARPNESS: &str = "test/metal_tint_inlay";
/// `_s` alone: a one-channel slot, which never reflects.
const MATTE: &str = "50-free-textures/concrete_stone_021";

/// A brush from `min` to `max` (map units), each side its own texture in the
/// order −x, +x, −y, +y, −z, +z.
fn brush(min: [i32; 3], max: [i32; 3], sides: [&str; 6]) -> String {
    let ([x0, y0, z0], [x1, y1, z1]) = (min, max);
    let [nx, px, ny, py, nz, pz] = sides;
    format!(
        "{{\n\
         ( {x0} 0 0 ) ( {x0} 1 0 ) ( {x0} 0 1 ) {nx} 0 0 0 1 1\n\
         ( {x1} 0 0 ) ( {x1} 0 1 ) ( {x1} 1 0 ) {px} 0 0 0 1 1\n\
         ( 0 {y0} 0 ) ( 0 {y0} 1 ) ( 1 {y0} 0 ) {ny} 0 0 0 1 1\n\
         ( 0 {y1} 0 ) ( 1 {y1} 0 ) ( 0 {y1} 1 ) {py} 0 0 0 1 1\n\
         ( 0 0 {z0} ) ( 1 0 {z0} ) ( 0 1 {z0} ) {nz} 0 0 0 1 1\n\
         ( 0 0 {z1} ) ( 0 1 {z1} ) ( 1 0 {z1} ) {pz} 0 0 0 1 1\n\
         }}"
    )
}

/// A sealed room, interior map x −128..128, y −128..128, z 0..128. Its
/// floor and its +y wall show glossy materials on their inner sides; the −x
/// wall shows `SATIN`, the +x wall `NO_SHARPNESS`, the rest `MATTE`. So the
/// build derives two probes, one for the floor and one for the +y wall, and
/// each non-glossy material is a different reason not to reflect.
fn glossy_room_map() -> String {
    let m = MATTE;
    let brushes = [
        brush(
            [-144, -144, -16],
            [144, 144, 0],
            [m, m, m, m, m, GLOSSY_FLOOR],
        ),
        brush([-144, -144, 128], [144, 144, 144], [m; 6]),
        brush([-144, -144, 0], [-128, 144, 128], [m, SATIN, m, m, m, m]),
        brush(
            [128, -144, 0],
            [144, 144, 128],
            [NO_SHARPNESS, m, m, m, m, m],
        ),
        brush([-128, -144, 0], [128, -128, 128], [m; 6]),
        brush(
            [-128, 128, 0],
            [128, 144, 128],
            [m, m, GLOSSY_WALL, m, m, m],
        ),
    ];
    [
        format!(
            "{{\n\"classname\" \"worldspawn\"\n{}\n}}",
            brushes.join("\n")
        ),
        "{\n\"classname\" \"player_spawn\"\n\"origin\" \"-64 0 32\"\n}".to_string(),
        "{\n\"classname\" \"light\"\n\"origin\" \"0 0 96\"\n\"light\" \"300\"\n\
         \"_falloff_range\" \"320\"\n\"_light_size\" \"0\"\n}"
            .to_string(),
    ]
    .join("\n")
}

/// Compile [`glossy_room_map`] with the glossy gate into `root`, its `.prm`
/// sidecars under `root/baked/materials`.
fn compile(root: &Path) -> PathBuf {
    let input = root.join("maps").join("glossy_room.map");
    let output_path = root.join("glossy_room.prl");
    std::fs::write(&input, format!("{}\n", glossy_room_map())).expect("write fixture map");
    let output = prl_build()
        .env("RUST_LOG", "warn")
        .arg(&input)
        .arg("-o")
        .arg(&output_path)
        .arg("--no-cache")
        .arg("--no-tui")
        .arg("--glossy-reflections")
        .arg("--baked-root")
        .arg(root.join("baked"))
        .arg("--sh-probe-spacing")
        .arg("4")
        .arg("--lightmap-density")
        .arg("0.25")
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

/// The compiled room, loaded as the engine loads it.
fn load_room() -> (BuildDir, LevelWorld) {
    let dir = BuildDir::temp();
    std::fs::create_dir_all(dir.path().join("maps")).expect("create maps directory");
    link_dev_textures(dir.path());
    let prl = compile(dir.path());
    let world = load_prl(&prl.to_string_lossy()).expect("the loader accepts the compiled level");
    (dir, world)
}

/// The renderer's glossy opt-in for one prepared world material: render-cpu
/// `specular_slot_reflects_in` over its specular slot, as the renderer calls
/// it at upload. A placeholder never reflects.
fn runtime_reflects(entry: &PreparedTexture) -> bool {
    let PreparedTexture::Bundle { slots, plan, .. } = entry else {
        return false;
    };
    specular_slot_reflects_in(&slots[1], plan.consume[1])
}

/// One probe's bound plane, measured from the vertices the stream stamps
/// with it: their shared unit normal and their offset along it.
struct BoundPlane {
    normal: Vec3,
    offset: f32,
}

/// Octahedral decode and position quantization stay within these.
const NORMAL_TOLERANCE: f32 = 1.0e-3;
const PLANE_TOLERANCE_M: f32 = 1.0e-3;

impl BoundPlane {
    fn holds(&self, position: Vec3, normal: Vec3) -> bool {
        normal.dot(self.normal) >= 1.0 - NORMAL_TOLERANCE
            && (position.dot(self.normal) - self.offset).abs() <= PLANE_TOLERANCE_M
    }
}

// The renderer builds the overlay's per-vertex probe index from the loaded
// level's vertices, indices, BVH leaves, id-17 face count and id 54's world
// bindings. On real compiler output every vertex of a bound face carries its
// probe, measured by geometry rather than by the compiler's own face ranges:
// a probe's vertices lie on one plane, fronting its capture point, and every
// vertex on that plane facing that way is its; no other vertex carries one.
#[test]
fn compiled_world_bindings_stamp_exactly_their_faces_vertices() {
    let (_dir, world) = load_room();
    let probes = world
        .reflection_probes
        .as_ref()
        .expect("the glossy room keeps its probes");
    assert!(probes.fluid_bindings.is_empty(), "the room has no water");
    assert_eq!(
        probes.probes.len(),
        2,
        "one probe for the floor, one for the wall"
    );
    assert!(!probes.world_bindings.is_empty());

    let stream = build_vertex_world_probes(
        world.vertices.len(),
        &world.indices,
        &world.bvh.leaves,
        world.face_meta.len(),
        &probes.world_bindings,
        probes.probes.len(),
    )
    .expect("the world bindings reach the world vertices");
    assert_eq!(stream.len(), world.vertices.len());

    let position = |v: usize| Vec3::from(world.vertices[v].position);
    let normal = |v: usize| Vec3::from(octahedral::decode(world.vertices[v].normal_oct));
    let mut stamped: BTreeMap<u32, Vec<usize>> = BTreeMap::new();
    for (vertex, &probe) in stream.iter().enumerate() {
        if probe != NO_WORLD_PROBE {
            stamped.entry(probe).or_default().push(vertex);
        }
    }
    assert_eq!(
        stamped.keys().copied().collect::<Vec<_>>(),
        vec![0, 1],
        "both probes stamp vertices"
    );

    let mut planes = Vec::new();
    for (&probe, vertices) in &stamped {
        let record = &probes.probes[probe as usize];
        let mean: Vec3 = vertices.iter().map(|&v| normal(v)).sum();
        let plane_normal = mean.normalize();
        let plane = BoundPlane {
            normal: plane_normal,
            offset: position(vertices[0]).dot(plane_normal),
        };
        let capture = Vec3::from(record.capture_point);
        let (box_min, box_max) = (Vec3::from(record.box_min), Vec3::from(record.box_max));
        for &v in vertices {
            let p = position(v);
            assert!(
                plane.holds(p, normal(v)),
                "probe {probe}: vertex {v} at {p} (normal {}) is off its plane",
                normal(v)
            );
            let lift = (capture - p).dot(plane.normal);
            assert!(
                (0.0..0.01).contains(&lift),
                "probe {probe}: its capture point fronts vertex {v} by {lift} m"
            );
            // An up probe's box starts at its capture height, as water's does, so a
            // floor vertex is checked where the search levels it: lifted onto the
            // capture plane.
            let levelled = p + plane.normal * lift;
            assert!(
                levelled.cmpge(box_min - PLANE_TOLERANCE_M).all()
                    && levelled.cmple(box_max + PLANE_TOLERANCE_M).all(),
                "probe {probe}: vertex {v} at {p}, levelled to {levelled}, lies outside its box {box_min}..{box_max}"
            );
        }
        planes.push((probe, plane));
    }
    let up = planes
        .iter()
        .filter(|(_, plane)| plane.normal.y > 1.0 - NORMAL_TOLERANCE)
        .count();
    assert_eq!(up, 1, "one probe binds the floor, the other the wall");

    assert_eq!(
        stream.len(),
        world.vertices.len(),
        "one stream entry per vertex"
    );
    for (vertex, &carried) in stream.iter().enumerate() {
        let (p, n) = (position(vertex), normal(vertex));
        let owner = planes
            .iter()
            .find(|(_, plane)| plane.holds(p, n))
            .map_or(NO_WORLD_PROBE, |&(probe, _)| probe);
        assert_eq!(
            carried, owner,
            "vertex {vertex} at {p} (normal {n}) carries probe {carried} where its plane gives {owner}"
        );
    }
}

// The compiler decides which materials reflect from each baked specular
// slot; the runtime decides it again from the `.prm` bytes it uploads. On
// real compiler output the two agree for every material a face uses: a
// material's faces bind a probe exactly where the runtime's opt-in marks it
// glossy. The room's materials cover both verdicts and each reason to refuse.
#[test]
fn compiled_glossy_verdicts_equal_the_runtime_opt_in_on_the_same_sidecars() {
    let (dir, world) = load_room();
    let probes = world
        .reflection_probes
        .as_ref()
        .expect("the glossy room keeps its probes");
    let bound: BTreeSet<u32> = probes
        .world_bindings
        .iter()
        .filter_map(|binding| world.face_meta[binding.face as usize].texture_index)
        .collect();
    let used: BTreeSet<u32> = world
        .face_meta
        .iter()
        .filter_map(|face| face.texture_index)
        .collect();

    let prepared = prepare_world_textures(
        &world.texture_names,
        &world.texture_cache_keys,
        &dir.path().join("baked").join("materials"),
    );
    let mut runtime: BTreeMap<&str, bool> = BTreeMap::new();
    for &texture in &used {
        let name = world.texture_names[texture as usize].as_str();
        let reflects = runtime_reflects(&prepared.entries[texture as usize]);
        assert_eq!(
            bound.contains(&texture),
            reflects,
            "{name}: the compiler binds its faces {} but the runtime opt-in says {reflects}",
            bound.contains(&texture)
        );
        runtime.insert(name, reflects);
    }
    for (name, reflects) in [
        (GLOSSY_FLOOR, true),
        (GLOSSY_WALL, true),
        (SATIN, false),
        (NO_SHARPNESS, false),
        (MATTE, false),
    ] {
        assert_eq!(
            runtime.get(name),
            Some(&reflects),
            "{name} is used and reflects: {reflects} ({runtime:?})"
        );
    }
}
