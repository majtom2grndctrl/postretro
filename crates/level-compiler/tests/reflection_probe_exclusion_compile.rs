//! Reflection-probe capture exclusion end to end: a pillar in a pool, compiled by `prl-build`
//! from a `.map` with no override, reflects the wall behind it in id 54, where the same pillar
//! set to `_reflection_capture always` reflects itself.
//! See: context/lib/resource_management.md §5.2 (Exclusion)

use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use postretro_level_compiler::bc6h::decode_bc6h_block;
use postretro_level_format::lightmap::LIGHTMAP_POOL_LAYER_EDGE;
use postretro_level_format::octahedral::irradiance_tile_texel_coord;
use postretro_level_format::reflection_probes::ReflectionProbesSection;
use postretro_level_format::{SectionId, read_container, read_section_data};

/// Quake map units to engine meters.
const UNIT: f32 = 0.0254;
/// Fixture textures, one flat colour each: the pillar red, the wall behind
/// it green, so a texel says which it reflects.
const TEXTURES: [(&str, [u8; 3]); 5] = [
    ("test/wall", [0, 255, 0]),
    ("test/pillar", [255, 0, 0]),
    ("test/ceiling", [128, 128, 128]),
    ("test/floor", [96, 96, 96]),
    ("test/water", [0, 0, 255]),
];
/// The pillar, map units: 16 u square, floor to ceiling, in the pool.
const PILLAR_MIN: [f32; 3] = [40.0, -8.0, 0.0];
/// The wall behind the pillar from the capture point: map x = 128.
const BACK_WALL_X: f32 = 128.0;
/// The pool room's interior height, map units.
const ROOM_HEIGHT: f32 = 128.0;
/// The compiler's default lightmap density, metres per texel: a copy of
/// `lightmap_bake::DEFAULT_TEXEL_DENSITY_METERS`, which the library does not
/// export.
const DEFAULT_LIGHTMAP_DENSITY_M: f32 = 0.025;

/// An interior height, map units, whose walls' charts pass one lightmap pool
/// layer at the default density: one layer's span, on the 16 u grid, and
/// 128 u more, so padding never decides it.
fn oversize_room_height() -> f32 {
    let layer = LIGHTMAP_POOL_LAYER_EDGE as f32 * DEFAULT_LIGHTMAP_DENSITY_M / UNIT;
    (layer / 16.0).ceil() * 16.0 + 128.0
}

struct TempBuildDir(PathBuf);

/// Distinguishes directories made in one process: tests run in parallel, and
/// the clock alone can give two of them the same reading.
static NEXT_BUILD_DIR: AtomicU64 = AtomicU64::new(0);

impl TempBuildDir {
    fn new(label: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock must be after the Unix epoch")
            .as_nanos();
        let serial = NEXT_BUILD_DIR.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "postretro-probe-exclusion-{label}-{}-{serial}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir_all(path.join("maps")).expect("create isolated compiler directory");
        for (name, rgb) in TEXTURES {
            write_png(&path.join("textures").join(format!("{name}.png")), rgb);
        }
        Self(path)
    }
}

impl Drop for TempBuildDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A 16 × 16 opaque PNG of one colour.
fn write_png(path: &Path, rgb: [u8; 3]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let file = std::fs::File::create(path).unwrap();
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), 16, 16);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().unwrap();
    let texel = [rgb[0], rgb[1], rgb[2], 255];
    writer.write_image_data(&texel.repeat(16 * 16)).unwrap();
}

fn box_brush(min: [f32; 3], max: [f32; 3], texture: &str) -> String {
    let ([x0, y0, z0], [x1, y1, z1]) = (min, max);
    format!(
        "{{\n\
         ( {x0} 0 0 ) ( {x0} 1 0 ) ( {x0} 0 1 ) {texture} 0 0 0 1 1\n\
         ( {x1} 0 0 ) ( {x1} 0 1 ) ( {x1} 1 0 ) {texture} 0 0 0 1 1\n\
         ( 0 {y0} 0 ) ( 0 {y0} 1 ) ( 1 {y0} 0 ) {texture} 0 0 0 1 1\n\
         ( 0 {y1} 0 ) ( 1 {y1} 0 ) ( 0 {y1} 1 ) {texture} 0 0 0 1 1\n\
         ( 0 0 {z0} ) ( 1 0 {z0} ) ( 0 1 {z0} ) {texture} 0 0 0 1 1\n\
         ( 0 0 {z1} ) ( 0 1 {z1} ) ( 1 0 {z1} ) {texture} 0 0 0 1 1\n\
         }}"
    )
}

/// The pool room: interior map x −128..128, y −128..128, z 0..`z1`, 16 u
/// walls and floor and a 32 u ceiling slab; water 16 u deep over x −96..56,
/// y −96..96, its far edge flush with the pillar's back; a light off the
/// pillar's line, so the wall behind it is lit and unshadowed. The pillar
/// stands in a `func_group` whose properties are `pillar_properties`.
fn pool_room_map(z1: f32, pillar_properties: &str) -> String {
    let (x0, x1, y0, y1, w) = (-128.0, 128.0, -128.0, 128.0, 16.0);
    let world = [
        box_brush([x0 - w, y0 - w, -w], [x1 + w, y1 + w, 0.0], "test/floor"),
        box_brush(
            [x0 - w, y0 - w, z1],
            [x1 + w, y1 + w, z1 + 32.0],
            "test/ceiling",
        ),
        box_brush([x0 - w, y0 - w, 0.0], [x0, y1 + w, z1], "test/wall"),
        box_brush([x1, y0 - w, 0.0], [x1 + w, y1 + w, z1], "test/wall"),
        box_brush([x0, y0 - w, 0.0], [x1, y0, z1], "test/wall"),
        box_brush([x0, y1, 0.0], [x1, y1 + w, z1], "test/wall"),
    ]
    .join("\n");
    let pillar = box_brush(PILLAR_MIN, [56.0, 8.0, z1], "test/pillar");
    let water = box_brush([-96.0, -96.0, 0.0], [56.0, 96.0, 16.0], "test/water");
    format!(
        "{{\n\"classname\" \"worldspawn\"\n{world}\n}}\n\
         {{\n\"classname\" \"func_group\"\n{pillar_properties}{pillar}\n}}\n\
         {{\n\"classname\" \"player_spawn\"\n\"origin\" \"-112 -112 32\"\n}}\n\
         {{\n\"classname\" \"light\"\n\"origin\" \"0 96 112\"\n\"light\" \"300\"\n\
         \"_falloff_range\" \"256\"\n\"_light_size\" \"0\"\n}}\n\
         {{\n\"classname\" \"fluid_volume\"\n\"fluid\" \"water\"\n{water}\n}}\n"
    )
}

/// `prl-build` on `map_text`, cold, at `jobs` workers; id 54 and the log.
fn compile(map_text: &str, jobs: u32) -> (ReflectionProbesSection, String) {
    let temp = TempBuildDir::new(&format!("j{jobs}"));
    let input = temp.0.join("maps").join("probe_exclusion.map");
    let output_path = temp.0.join("probe_exclusion.prl");
    std::fs::write(&input, map_text).expect("write fixture map");
    let output = Command::new(env!("CARGO_BIN_EXE_prl-build"))
        .env("RUST_LOG", "info")
        .arg(&input)
        .arg("-o")
        .arg(&output_path)
        .arg("--baked-root")
        .arg(temp.0.join("baked"))
        .arg("--no-cache")
        .arg("--no-tui")
        .arg("--jobs")
        .arg(jobs.to_string())
        .output()
        .expect("spawn prl-build");
    let log = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.status.success(), "prl-build failed\n{log}");
    let bytes = std::fs::read(&output_path).expect("read compiled PRL");
    let mut cursor = Cursor::new(bytes);
    let metadata = read_container(&mut cursor).expect("read PRL container");
    let payload = read_section_data(&mut cursor, &metadata, SectionId::ReflectionProbes as u32)
        .expect("read id 54")
        .expect("id 54 is written");
    (
        ReflectionProbesSection::from_bytes(&payload).expect("id 54 decodes"),
        log,
    )
}

fn engine(map: [f32; 3]) -> [f32; 3] {
    [-map[1] * UNIT, map[2] * UNIT, -map[0] * UNIT]
}

/// What probe 0's tiles hold toward one direction.
#[derive(Debug, Clone, Copy)]
struct Texel {
    rgb: [f32; 3],
    /// Mean inverse L1 distance.
    inverse_l1: f32,
}

fn texel(section: &ReflectionProbesSection, direction: [f32; 3]) -> Texel {
    let dimension = section.tile_dimension;
    let [x, y] = irradiance_tile_texel_coord(direction, dimension, section.tile_border);
    let (x, y) = (x as usize, y as usize);
    let at = y * dimension as usize + x;
    let block = (y / 4) * (dimension as usize / 4) + x / 4;
    let bytes: &[u8; 16] = section.tile(0)[block * 16..block * 16 + 16]
        .try_into()
        .expect("a BC6H block is 16 bytes");
    let rgb = decode_bc6h_block(bytes).expect("colour block decodes")[(y % 4) * 4 + x % 4];
    let distance = section.distance_tile(0);
    let bits = u16::from_le_bytes([distance[at * 2], distance[at * 2 + 1]]);
    Texel {
        rgb,
        inverse_l1: f16_to_f32(bits),
    }
}

fn f16_to_f32(bits: u16) -> f32 {
    let sign = if bits & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exponent = i32::from((bits >> 10) & 0x1f);
    let mantissa = f32::from(bits & 0x3ff);
    sign * match exponent {
        0 => mantissa * 2f32.powi(-24),
        31 => f32::INFINITY,
        e => (1.0 + mantissa / 1024.0) * 2f32.powi(e - 15),
    }
}

/// The capture's inverse L1 distance to the map-x plane `x` along the ray
/// from `point` (engine) through the map point `toward`.
fn inverse_l1_to_plane(point: [f32; 3], toward: [f32; 3], x: f32) -> f32 {
    let target = engine(toward);
    let d: Vec<f32> = (0..3).map(|a| target[a] - point[a]).collect();
    // Map x is engine −z.
    let plane_z = -x * UNIT;
    let t = (plane_z - point[2]) / d[2];
    1.0 / (t * d.iter().map(|v| v.abs()).sum::<f32>())
}

/// Points on the back wall the capture point sees only through the pillar.
fn wall_behind_the_pillar() -> Vec<[f32; 3]> {
    let mut points = Vec::new();
    for y in [-4.5, 0.5, 4.5] {
        for z in [48.0, 64.0, 80.0] {
            points.push([BACK_WALL_X, y, z]);
        }
    }
    points
}

/// The filtered build reads the green wall behind the pillar at the wall's
/// distance; the `always` build reads the red pillar at its front face.
fn assert_reads_through_the_pillar(
    filtered: &ReflectionProbesSection,
    kept: &ReflectionProbesSection,
) {
    assert_eq!(filtered.probes.len(), kept.probes.len());
    let point = filtered.probes[0].capture_point;
    assert_eq!(
        point, kept.probes[0].capture_point,
        "placement is unchanged"
    );
    for q in wall_behind_the_pillar() {
        let target = engine(q);
        let direction: [f32; 3] = std::array::from_fn(|a| target[a] - point[a]);
        let length = direction.iter().map(|v| v * v).sum::<f32>().sqrt();
        let direction = direction.map(|v| v / length);
        let (through, onto) = (texel(filtered, direction), texel(kept, direction));
        let wall = inverse_l1_to_plane(point, q, BACK_WALL_X);
        let pillar = inverse_l1_to_plane(point, q, PILLAR_MIN[0]);
        assert!(
            (through.inverse_l1 / wall - 1.0).abs() < 0.05,
            "{q:?}: {through:?} vs the wall's {wall}"
        );
        assert!(
            (onto.inverse_l1 / pillar - 1.0).abs() < 0.05,
            "{q:?}: {onto:?} vs the pillar's {pillar}"
        );
        assert!(
            through.rgb[1] > 0.01 && through.rgb[1] > 4.0 * through.rgb[0],
            "{q:?}: the filtered build reads the green wall: {through:?}"
        );
        assert!(
            onto.rgb[0] > 0.01 && onto.rgb[0] > 4.0 * onto.rgb[1],
            "{q:?}: the kept pillar reads red: {onto:?}"
        );
    }
}

const ALWAYS: &str = "\"_reflection_capture\" \"always\"\n";

// From a `.map` with no override, the rule excludes the pillar
// and id 54's colour and distance texels through it read the wall behind;
// the same pillar set to `always`, the same hull, reads the pillar.
#[test]
fn prl_build_captures_the_wall_behind_a_pillar_the_rule_excludes() {
    let (filtered, log) = compile(&pool_room_map(ROOM_HEIGHT, ""), 4);
    assert!(
        log.contains("probe 0: 1 brush(es) excluded from its capture"),
        "{log}"
    );
    let (kept, log) = compile(&pool_room_map(ROOM_HEIGHT, ALWAYS), 4);
    assert!(
        log.contains("probe 0: 0 brush(es) excluded from its capture"),
        "{log}"
    );
    assert_reads_through_the_pillar(&filtered, &kept);
}

// The same with the oversize lightmap cut forced. The room
// stands over 51 m tall, so its walls' charts pass one pool layer at the
// default density and are cut into sub-faces, renumbering the faces the
// pillar's mask names. It is tall rather than long because the cell-size cap
// divides any horizontal run that long at cell boundaries, so no horizontal
// face would stay oversize; the cap never splits along up. The pool, the
// capture point and the pillar's footprint stay as in the room above.
#[test]
fn prl_build_captures_through_an_excluded_pillar_with_the_oversize_cut_forced() {
    let height = oversize_room_height();
    let (filtered, log) = compile(&pool_room_map(height, ""), 4);
    // The floor and the ceiling output first and stay whole; the first wall,
    // face 2, is cut, so every face after it renumbers, the pillar's
    // included, and the pillar's own sides are cut too.
    assert!(
        log.contains("oversize lightmap face(s)"),
        "the cut runs\n{log}"
    );
    assert!(log.contains("cutting face 2 "), "{log}");
    assert!(
        log.contains("probe 0: 1 brush(es) excluded from its capture"),
        "{log}"
    );
    let (kept, _) = compile(&pool_room_map(height, ALWAYS), 4);
    assert_reads_through_the_pillar(&filtered, &kept);
}

// `--no-cache` at one worker and at four emits byte-identical id
// 54 for a level whose probe excludes a brush.
#[test]
fn prl_build_excluding_capture_is_byte_identical_at_one_and_four_jobs() {
    let map = pool_room_map(ROOM_HEIGHT, "");
    let (one, log) = compile(&map, 1);
    assert!(log.contains("probe 0: 1 brush(es) excluded from its capture"));
    let (four, _) = compile(&map, 4);
    assert_eq!(one.to_bytes(), four.to_bytes());
}
