// Analytic scenes and distance tiles for the reflection-search tests: an
// empty box room, the test map's pool room (crate) and room B (pillar), each
// captured as the compiler captures, without the compiler.
// See: context/lib/testing_guide.md §4 · content/dev/maps/reflection-probes-test.map

use std::sync::OnceLock;

use glam::{Vec2, Vec3};
use postretro_level_format::octahedral::irradiance_tile_source_texel;

use super::reflection_distance_sample;
use crate::sh_compose::f16_bits_to_f32;
use crate::sh_volume::f32_to_f16_bits;

/// Tile dimension and border, the engine's.
pub(super) const TILE: [u32; 2] = [128, 1];
/// Metres per map unit.
pub(super) const UNIT: f32 = 0.0254;
/// The capture point's lift above its water plane, in map units (2 mm).
const LIFT: f32 = 0.002 / UNIT;
/// The compiler's rotated-grid supersample offsets
/// (`reflection_probes::capture::SUPERSAMPLE_OFFSETS`), in texels.
const SUPERSAMPLES: [[f32; 2]; 4] = [
    [-0.125, -0.375],
    [0.375, -0.125],
    [0.125, 0.375],
    [-0.375, 0.125],
];

/// Map coordinates (Z up, units) to engine space (Y up, metres), as the
/// compiler converts them: `(−y, z, −x) × 0.0254`.
pub(super) fn engine(x: f32, y: f32, z: f32) -> Vec3 {
    Vec3::new(-y, z, -x) * UNIT
}

/// A map direction in engine space (no scale).
pub(super) fn engine_dir(x: f32, y: f32, z: f32) -> Vec3 {
    Vec3::new(-y, z, -x)
}

/// One solid axis-aligned brush.
#[derive(Debug, Clone, Copy)]
pub(super) struct SceneBox {
    pub min: Vec3,
    pub max: Vec3,
    pub name: &'static str,
}

/// A brush from map-space corners.
fn brush(a: [f32; 3], b: [f32; 3], name: &'static str) -> SceneBox {
    let (p, q) = (engine(a[0], a[1], a[2]), engine(b[0], b[1], b[2]));
    SceneBox {
        min: p.min(q),
        max: p.max(q),
        name,
    }
}

/// Entry distance of a ray into a box, when it enters ahead of the origin:
/// a ray starting inside a box never hits it.
fn ray_box(origin: Vec3, dir: Vec3, b: &SceneBox) -> Option<f32> {
    let (mut near, mut far) = (f32::NEG_INFINITY, f32::INFINITY);
    for axis in 0..3 {
        let (o, d) = (origin[axis], dir[axis]);
        if d.abs() < 1.0e-12 {
            if o < b.min[axis] || o > b.max[axis] {
                return None;
            }
            continue;
        }
        let (ta, tb) = ((b.min[axis] - o) / d, (b.max[axis] - o) / d);
        near = near.max(ta.min(tb));
        far = far.min(ta.max(tb));
        if near > far {
            return None;
        }
    }
    (far >= 1.0e-6 && near > 1.0e-6).then_some(near)
}

/// A probe over analytic geometry: its brushes, capture point, projection
/// box and the distance tile captured from that point.
pub(super) struct Scene {
    pub brushes: Vec<SceneBox>,
    pub capture: Vec3,
    pub box_min: Vec3,
    pub box_max: Vec3,
    /// Row-major, border included, f16-rounded, as the section stores it.
    pub tile: Vec<f32>,
}

impl Scene {
    fn new(brushes: Vec<SceneBox>, capture: Vec3, box_min: Vec3, box_max: Vec3) -> Self {
        let mut scene = Self {
            brushes,
            capture,
            box_min,
            box_max,
            tile: Vec::new(),
        };
        scene.tile = capture_tile(|d| scene.inverse_l1(d));
        scene
    }

    /// The nearest brush along unit `dir` from `origin`: its distance and name.
    pub fn trace(&self, origin: Vec3, dir: Vec3) -> Option<(f32, &'static str)> {
        self.brushes
            .iter()
            .filter_map(|b| ray_box(origin, dir, b).map(|t| (t, b.name)))
            .min_by(|a, b| a.0.total_cmp(&b.0))
    }

    /// The capture's stored quantity toward unit `dir`: `1/|hit − C|₁`, or 0.
    pub fn inverse_l1(&self, dir: Vec3) -> f32 {
        self.trace(self.capture, dir)
            .map_or(0.0, |(t, _)| 1.0 / (t * dir.abs().element_sum()))
    }

    /// The tile as the fluid pass samples it.
    pub fn distance(&self, uv: Vec2) -> f32 {
        reflection_distance_sample(&self.tile, TILE[0], uv)
    }

    /// What the probe's colour tile shows toward `lookup`: the brush the
    /// capture point sees that way.
    pub fn seen_from_capture(&self, lookup: Vec3) -> Option<&'static str> {
        (lookup.abs().element_sum() > 1.0e-9)
            .then(|| self.trace(self.capture, lookup.normalize()))
            .flatten()
            .map(|(_, name)| name)
    }
}

/// Unit direction at octahedral `(u, v)` in `[0, 1]²`: the inverse of
/// `octahedral::encode_unquantized_uv`.
pub(super) fn octahedral_direction(u: f32, v: f32) -> Vec3 {
    let (ox, oy) = (u * 2.0 - 1.0, v * 2.0 - 1.0);
    let z = 1.0 - ox.abs() - oy.abs();
    let sign = |s: f32| if s >= 0.0 { 1.0 } else { -1.0 };
    let (x, y) = if z < 0.0 {
        ((1.0 - oy.abs()) * sign(ox), (1.0 - ox.abs()) * sign(oy))
    } else {
        (ox, oy)
    };
    Vec3::new(x, y, z).normalize()
}

/// A distance tile captured as the compiler captures one: per interior
/// texel the mean of `inverse_l1` over the four supersamples, rounded
/// through f16, bordered by the shared wrap rule.
pub(super) fn capture_tile(inverse_l1: impl Fn(Vec3) -> f32) -> Vec<f32> {
    let [dimension, border] = TILE;
    let interior = dimension - 2 * border;
    let n = interior as f32;
    let mut texels = vec![0.0; (interior * interior) as usize];
    for y in 0..interior {
        for x in 0..interior {
            let sum: f32 = SUPERSAMPLES
                .iter()
                .map(|[dx, dy]| {
                    inverse_l1(octahedral_direction(
                        (x as f32 + 0.5 + dx) / n,
                        (y as f32 + 0.5 + dy) / n,
                    ))
                })
                .sum();
            let mean = sum / SUPERSAMPLES.len() as f32;
            texels[(y * interior + x) as usize] = f16_bits_to_f32(f32_to_f16_bits(mean));
        }
    }
    let mut tile = Vec::with_capacity((dimension * dimension) as usize);
    for y in 0..dimension {
        for x in 0..dimension {
            let [ix, iy] = irradiance_tile_source_texel(x, y, dimension, border);
            tile.push(texels[(iy * interior + ix) as usize]);
        }
    }
    tile
}

/// Six slabs enclosing `[min, max]` (engine metres), 0.4 m thick.
fn shell(min: Vec3, max: Vec3) -> Vec<SceneBox> {
    let t = 0.4;
    let slab = |lo: Vec3, hi: Vec3| SceneBox {
        min: lo,
        max: hi,
        name: "wall",
    };
    let (lo, hi) = (min - Vec3::splat(t), max + Vec3::splat(t));
    vec![
        slab(lo, Vec3::new(min.x, hi.y, hi.z)),
        slab(Vec3::new(max.x, lo.y, lo.z), hi),
        slab(lo, Vec3::new(hi.x, min.y, hi.z)),
        slab(Vec3::new(lo.x, max.y, lo.z), hi),
        slab(lo, Vec3::new(hi.x, hi.y, min.z)),
        slab(Vec3::new(lo.x, lo.y, max.z), hi),
    ]
}

/// A room 10 × 8 m and 4 m tall over water wall to wall at y = 0, the pit
/// 1 m deep: every wall lies on the probe's box, nothing stands inside it.
/// The capture point sits 2 mm above the water's centre.
pub(super) fn empty_room() -> &'static Scene {
    static SCENE: OnceLock<Scene> = OnceLock::new();
    SCENE.get_or_init(|| {
        let (min, max) = (Vec3::new(-5.0, -1.0, -4.0), Vec3::new(5.0, 4.0, 4.0));
        Scene::new(shell(min, max), Vec3::new(0.0, 0.002, 0.0), min, max)
    })
}

/// The test map's pool room P (map x/y 0..512, floor z 0, ceiling 256): the
/// pool pit x/y 160..352 down to −96 with its 6-unit coping, the crate east
/// of the rim, the bench and the stump; the east door modelled solid. The
/// derived box is the room; the capture point is over the pool's centre.
pub(super) fn pool_room() -> &'static Scene {
    static SCENE: OnceLock<Scene> = OnceLock::new();
    SCENE.get_or_init(|| {
        let b = brush;
        let brushes = vec![
            b([-16.0, -16.0, -112.0], [0.0, 528.0, 272.0], "wall"),
            b([512.0, -16.0, -112.0], [528.0, 528.0, 272.0], "wall"),
            b([-16.0, -16.0, -112.0], [528.0, 0.0, 272.0], "wall"),
            b([-16.0, 512.0, -112.0], [528.0, 528.0, 272.0], "wall"),
            b([-16.0, -16.0, 256.0], [528.0, 528.0, 272.0], "ceiling"),
            b([0.0, 0.0, -112.0], [160.0, 512.0, 0.0], "floor"),
            b([352.0, 0.0, -112.0], [512.0, 512.0, 0.0], "floor"),
            b([160.0, 0.0, -112.0], [352.0, 160.0, 0.0], "floor"),
            b([160.0, 352.0, -112.0], [352.0, 512.0, 0.0], "floor"),
            b([160.0, 160.0, -112.0], [352.0, 352.0, -96.0], "pitfloor"),
            b([144.0, 144.0, 0.0], [368.0, 160.0, 6.0], "coping"),
            b([144.0, 352.0, 0.0], [368.0, 368.0, 6.0], "coping"),
            b([144.0, 144.0, 0.0], [160.0, 368.0, 6.0], "coping"),
            b([352.0, 144.0, 0.0], [368.0, 368.0, 6.0], "coping"),
            b([392.0, 200.0, 0.0], [440.0, 248.0, 48.0], "crate"),
            b([200.0, 104.0, 0.0], [312.0, 128.0, 20.0], "bench"),
            b([88.0, 88.0, 0.0], [120.0, 120.0, 64.0], "stump"),
        ];
        let (p, q) = (engine(0.0, 0.0, 0.0), engine(512.0, 512.0, 256.0));
        Scene::new(brushes, engine(256.0, 256.0, LIFT), p.min(q), p.max(q))
    })
}

/// The test map's room B (map x 928..1440, floor z 0, ceiling 384): the
/// basin trench y 112..400 with water at z −16, the pillar x 1168..1200,
/// y 240..272 standing in it, balcony, rail and stairs, the doorway west to
/// room A and room A behind it. The authored box and its capture point,
/// moved west of the pillar by `_capture_offset "-128 0"`.
pub(super) fn room_b() -> &'static Scene {
    static SCENE: OnceLock<Scene> = OnceLock::new();
    SCENE.get_or_init(|| {
        let b = brush;
        let mut brushes = vec![
            b([1440.0, -16.0, -112.0], [1456.0, 528.0, 400.0], "wall"),
            b([912.0, -16.0, -112.0], [1456.0, 0.0, 400.0], "wall"),
            b([912.0, 512.0, -112.0], [1456.0, 528.0, 400.0], "wall"),
            b([912.0, -16.0, 384.0], [1456.0, 528.0, 400.0], "ceiling"),
            b([912.0, 0.0, -112.0], [928.0, 144.0, 400.0], "wall"),
            b([912.0, 368.0, -112.0], [928.0, 512.0, 400.0], "wall"),
            b([912.0, 144.0, 128.0], [928.0, 368.0, 400.0], "wall"),
            b([928.0, 0.0, -112.0], [1440.0, 112.0, 0.0], "floor"),
            b([928.0, 400.0, -112.0], [1440.0, 512.0, 0.0], "floor"),
            b([928.0, 112.0, -112.0], [1440.0, 400.0, -96.0], "pitfloor"),
            b([1168.0, 240.0, -96.0], [1200.0, 272.0, 384.0], "pillar"),
            b([1312.0, 0.0, 176.0], [1440.0, 512.0, 192.0], "balcony"),
            b([1312.0, 80.0, 192.0], [1320.0, 512.0, 216.0], "rail"),
        ];
        for k in 0..12 {
            let x = 1024.0 + 24.0 * k as f32;
            brushes.push(b(
                [x, 0.0, 0.0],
                [x + 24.0, 64.0, 16.0 * (k + 1) as f32],
                "stair",
            ));
        }
        brushes.extend([
            b([512.0, -16.0, -112.0], [528.0, 528.0, 400.0], "wallA"),
            b([512.0, -16.0, 192.0], [912.0, 528.0, 400.0], "ceilingA"),
            b([512.0, -16.0, -112.0], [912.0, 0.0, 400.0], "wallA"),
            b([512.0, 512.0, -112.0], [912.0, 528.0, 400.0], "wallA"),
            b([528.0, 0.0, -112.0], [912.0, 208.0, 0.0], "floorA"),
            b([528.0, 304.0, -112.0], [912.0, 512.0, 0.0], "floorA"),
            b([528.0, 208.0, -112.0], [912.0, 304.0, -96.0], "floorA"),
        ]);
        let (p, q) = (engine(928.0, 0.0, -96.0), engine(1440.0, 512.0, 384.0));
        Scene::new(
            brushes,
            engine(1039.6, 256.0, -16.0 + LIFT),
            p.min(q),
            p.max(q),
        )
    })
}

/// The empty room with a 0.4 m square pillar standing in the water, floor
/// to ceiling, its capture-facing face `near` metres east (+x) of the
/// capture point: an occluder near or far from it.
pub(super) fn pillar_room(near: f32) -> Scene {
    let room = empty_room();
    let mut brushes = room.brushes.clone();
    brushes.push(SceneBox {
        min: Vec3::new(near, -1.0, -0.2),
        max: Vec3::new(near + 0.4, 4.0, 0.2),
        name: "pillar",
    });
    Scene::new(brushes, room.capture, room.box_min, room.box_max)
}

/// A canal 20 m long and 4 m wide over water wall to wall at y = 0, the
/// capture point 2 mm above its centre: its side walls seen from the capture
/// point at up to 79° from their normals.
pub(super) fn canal() -> &'static Scene {
    static SCENE: OnceLock<Scene> = OnceLock::new();
    SCENE.get_or_init(|| {
        let (min, max) = (Vec3::new(-10.0, -1.0, -2.0), Vec3::new(10.0, 4.0, 2.0));
        Scene::new(shell(min, max), Vec3::new(0.0, 0.002, 0.0), min, max)
    })
}
