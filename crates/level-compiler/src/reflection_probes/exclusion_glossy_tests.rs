// Capture exclusion for probes binding glossy world faces: which probes classify, the thin rule's strides, and undersides over a glossy floor.
// See: context/lib/resource_management.md §5.2 (Exclusion), §5.4 (Exclusion)

use glam::{DVec3, Vec3};
use postretro_level_format::environment_volumes::EnvironmentVolumesSection;
use postretro_render_cpu::fluid_surface::reflection_quality::FluidReflectionQuality;

use crate::geometry::GeometryResult;
use crate::map_data::{BrushVolume, ReflectionCapture};
use crate::partition::Aabb;

use super::exclusion::{
    ExclusionConstants, ExclusionScene, FLOOR_LAYER_COARSEST_STRIDE_DIVISOR, ScreenSpaceLayers,
    classifies, classify_probes_within_boxes,
};
use super::facing::{Facing, Facings};
use super::placement::{FaceGeometry, PlacedProbe, ReflectorFace, ReflectorSource};
use super::reflection_probes_test_fixtures::{GeometryBuilder, volumes};

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// A hull with no planes or sides: the size and floor rule read its bounds.
fn hull(min: [f64; 3], max: [f64; 3]) -> BrushVolume {
    BrushVolume {
        planes: Vec::new(),
        sides: Vec::new(),
        aabb: Aabb {
            min: DVec3::from(min),
            max: DVec3::from(max),
        },
    }
}

/// A hull at the origin with engine extents `x` × `y` (along up) × `z`.
fn sized(x: f64, y: f64, z: f64) -> BrushVolume {
    hull([0.0; 3], [x, y, z])
}

/// A probe of `facing` with bound area `area` binding `fluid` and `world`,
/// its box holding every fixture brush.
fn probe(facing: Facing, area: f64, fluid: Vec<(u32, u32)>, world: Vec<u32>) -> PlacedProbe {
    PlacedProbe {
        source_entity: 1,
        authored: false,
        box_min: [-100.0; 3],
        box_max: [100.0; 3],
        capture_point: [0.0; 3],
        facing,
        bound_area: area,
        fluid_faces: fluid,
        world_faces: world,
    }
}

/// Up-facing probes of bound area `area` binding water alone, a glossy
/// floor alone, and both: the three kinds the rule classifies.
fn up_probes(area: f64) -> [(&'static str, PlacedProbe); 3] {
    [
        ("water", probe(Facing::Up, area, vec![(0, 0)], Vec::new())),
        ("glossy floor", probe(Facing::Up, area, Vec::new(), vec![0])),
        ("both", probe(Facing::Up, area, vec![(0, 0)], vec![0])),
    ]
}

/// Probes binding a wall, a ceiling and a 45° slope.
fn other_probes(area: f64) -> [(&'static str, PlacedProbe); 3] {
    let mut facings = Facings::default();
    let mut other = |normal: DVec3| facings.facing_of(normal.normalize());
    [
        ("wall", probe(other(DVec3::X), area, Vec::new(), vec![0])),
        (
            "ceiling",
            probe(other(DVec3::NEG_Y), area, Vec::new(), vec![0]),
        ),
        (
            "slope",
            probe(other(DVec3::new(1.0, 1.0, 0.0)), area, Vec::new(), vec![0]),
        ),
    ]
}

/// What classification reads, its brushes' faces (`face_brush`) in
/// `geometry` and the glossy floors placement offered in `world_faces`.
struct Scene {
    brushes: Vec<BrushVolume>,
    overrides: Vec<ReflectionCapture>,
    geometry: GeometryResult,
    face_brush: Vec<Option<u32>>,
    environment: EnvironmentVolumesSection,
    world_faces: Vec<ReflectorFace>,
}

impl Scene {
    /// `brushes` with no surviving faces, each `Auto` unless `overrides`
    /// says otherwise, and no reflector faces.
    fn of(brushes: Vec<BrushVolume>, overrides: Vec<ReflectionCapture>) -> Self {
        assert_eq!(brushes.len(), overrides.len());
        Self {
            brushes,
            overrides,
            geometry: GeometryBuilder::default().finish(Vec::new()),
            face_brush: Vec::new(),
            environment: volumes(Vec::new()),
            world_faces: Vec::new(),
        }
    }

    /// `probe`'s excluded set under `constants`.
    fn excluded(&self, probe: &PlacedProbe, constants: &ExclusionConstants) -> Vec<u32> {
        classify_probes_within_boxes(
            &ExclusionScene {
                brush_volumes: &self.brushes,
                capture_override: &self.overrides,
                face_brush: &self.face_brush,
                geometry: &self.geometry,
                environment_volumes: &self.environment,
                world_faces: &self.world_faces,
            },
            std::slice::from_ref(probe),
            constants,
        )
        .remove(0)
    }
}

/// A glossy floor world face `face`, the square `min`..`max` in `(x, z)` at
/// height `y`, facing up.
fn glossy_floor(face: u32, min: [f64; 2], max: [f64; 2], y: f64) -> ReflectorFace {
    let points = [
        DVec3::new(min[0], y, min[1]),
        DVec3::new(min[0], y, max[1]),
        DVec3::new(max[0], y, max[1]),
        DVec3::new(max[0], y, min[1]),
    ];
    let geometry = FaceGeometry::from_polygon(&points).expect("a square has area");
    assert!(geometry.is_up_facing());
    ReflectorFace::new(
        ReflectorSource::World { face },
        geometry,
        Facing::Up,
        &points,
    )
}

// ---------------------------------------------------------------------------
// Which probes classify
// ---------------------------------------------------------------------------

// Row 13: a probe binding up-facing faces alone, water, a glossy floor or
// both, excludes a brush small relative to its bound area by water's rule
// and keeps a large one; a probe binding a wall, a ceiling or a slope
// captures that brush. `never` excludes from, and `always` keeps in, every
// up-facing probe; a wall, ceiling or slope probe captures a `never` brush
// too, as it captures every static brush.
#[test]
fn up_facing_probes_exclude_by_waters_rule_and_wall_ceiling_and_slope_probes_capture_every_brush() {
    // W = 10 m: t = 8.5 m, the thin floor ≈ 7.4 cm.
    let scene = Scene::of(
        vec![
            sized(2.0, 1.0, 2.0),
            sized(9.0, 1.0, 2.0),
            sized(2.0, 1.0, 2.0),
            sized(2.0, 1.0, 2.0),
        ],
        vec![
            ReflectionCapture::Auto,
            ReflectionCapture::Auto,
            ReflectionCapture::Never,
            ReflectionCapture::Always,
        ],
    );
    let production = ExclusionConstants::PRODUCTION;
    for (what, probe) in up_probes(100.0) {
        assert!(classifies(&probe), "{what}");
        assert_eq!(
            scene.excluded(&probe, &production),
            vec![0, 2],
            "{what}: the small brush and the `never` one leave"
        );
    }
    for (what, probe) in other_probes(100.0) {
        assert!(!classifies(&probe), "{what}");
        assert_eq!(
            scene.excluded(&probe, &production),
            Vec::<u32>::new(),
            "{what} captures every brush"
        );
    }
}

// Row 13, underside half: a brush whose only downward face hangs over a
// probe's glossy floor stays in the probe, as one over its water does; moved
// off the floor, the same brush leaves.
#[test]
fn an_up_facing_probe_keeps_a_brush_with_an_underside_over_its_glossy_floor() {
    // A 2 m cube 1–3 m over the floor whose bottom, facing down, survives.
    let bottom = |x: f32| {
        let corners = [
            Vec3::new(x, 1.0, 4.0),
            Vec3::new(x + 2.0, 1.0, 4.0),
            Vec3::new(x + 2.0, 1.0, 6.0),
            Vec3::new(x, 1.0, 6.0),
        ];
        let mut builder = GeometryBuilder::default();
        builder.push_face(Vec3::NEG_Y, &corners, 0, Vec3::ZERO, 0, [[0.0; 2]; 4]);
        builder.finish(vec!["test/wall".to_string()])
    };
    let floor = glossy_floor(7, [0.0, 0.0], [10.0, 10.0], 0.0);
    for (x, kept) in [(4.0, true), (14.0, false)] {
        let scene = Scene {
            geometry: bottom(x),
            face_brush: vec![Some(0)],
            world_faces: vec![floor.clone()],
            ..Scene::of(
                vec![hull(
                    [f64::from(x), 1.0, 4.0],
                    [f64::from(x) + 2.0, 3.0, 6.0],
                )],
                vec![ReflectionCapture::Auto],
            )
        };
        for (what, probe) in [
            (
                "glossy floor",
                probe(Facing::Up, 100.0, Vec::new(), vec![7]),
            ),
            ("water and glossy floor", {
                // Water bound elsewhere: only the floor lies under the brush.
                probe(Facing::Up, 100.0, vec![(0, 0)], vec![7])
            }),
        ] {
            let excluded = scene.excluded(&probe, &ExclusionConstants::PRODUCTION);
            assert_eq!(excluded.is_empty(), kept, "{what} at x = {x}: {excluded:?}");
        }
    }
}

// ---------------------------------------------------------------------------
// The thin rule's strides
// ---------------------------------------------------------------------------

/// The production rule with the floor layer's coarsest stride divisor at
/// `divisor`: a test-only floor stride.
fn floor_stride(divisor: u32) -> ExclusionConstants {
    ExclusionConstants {
        floor_stride_divisor: divisor,
        ..ExclusionConstants::PRODUCTION
    }
}

// Row 14 (P24): a probe binding water and a glossy floor keeps a brush
// thinner than the coarser of the two layers' thin floors at its view
// distance. Under a test-only floor stride coarser than water's, the floor's
// thin floor is the coarser; under one finer, water's is; either way the
// probe keeps a brush between the two.
#[test]
fn a_water_and_glossy_floor_probe_keeps_a_brush_under_the_coarser_strides_thin_floor() {
    let width = 10.0;
    let water_floor = ExclusionConstants::PRODUCTION.thin_floor(width);
    let [water, floor, both] = up_probes(width * width).map(|(_, probe)| probe);
    // Floor stride divisor 90, half water's 180: a stride twice as coarse.
    let coarser = floor_stride(90);
    let floor_floor = coarser.thin_floor_for(ScreenSpaceLayers::Floor, width);
    assert!((floor_floor - 2.0 * water_floor).abs() < 1e-12);
    assert_eq!(
        coarser.thin_floor_for(ScreenSpaceLayers::Both, width),
        floor_floor
    );
    let between = Scene::of(
        vec![sized(2.0, 1.5 * water_floor, 2.0)],
        vec![ReflectionCapture::Auto],
    );
    assert_eq!(between.excluded(&both, &coarser), Vec::<u32>::new());
    assert_eq!(between.excluded(&floor, &coarser), Vec::<u32>::new());
    assert_eq!(between.excluded(&water, &coarser), vec![0]);

    // Divisor 360: the floor's stride is finer, so water's thin floor holds.
    let finer = floor_stride(360);
    let floor_floor = finer.thin_floor_for(ScreenSpaceLayers::Floor, width);
    assert!((floor_floor - 0.5 * water_floor).abs() < 1e-12);
    assert_eq!(
        finer.thin_floor_for(ScreenSpaceLayers::Both, width),
        water_floor
    );
    let between = Scene::of(
        vec![sized(2.0, 0.75 * water_floor, 2.0)],
        vec![ReflectionCapture::Auto],
    );
    assert_eq!(between.excluded(&both, &finer), Vec::<u32>::new());
    assert_eq!(between.excluded(&water, &finer), Vec::<u32>::new());
    assert_eq!(between.excluded(&floor, &finer), vec![0]);
}

// Row 15: a glossy-floor probe's thin rule keys to the floor layer's
// coarsest stride and a water-only probe's to water's. Under a test-only
// floor stride coarser than water's, a brush whose thickness lies between
// the two thin floors stays in the glossy-floor probe and leaves the water
// probe of the same bound width; at the production strides, both agree.
#[test]
fn a_glossy_floor_probes_thin_rule_keys_to_the_floor_stride_and_waters_to_its_own() {
    let width = 20.0;
    let [water, floor, _] = up_probes(width * width).map(|(_, probe)| probe);
    let coarser = floor_stride(120);
    let (water_floor, floor_floor) = (
        coarser.thin_floor_for(ScreenSpaceLayers::Water, width),
        coarser.thin_floor_for(ScreenSpaceLayers::Floor, width),
    );
    assert!(water_floor < floor_floor);
    assert_eq!(
        water_floor,
        ExclusionConstants::PRODUCTION.thin_floor(width),
        "water's thin floor never reads the floor layer's stride"
    );
    let between = Scene::of(
        vec![sized(3.0, 0.5 * (water_floor + floor_floor), 3.0)],
        vec![ReflectionCapture::Auto],
    );
    assert_eq!(between.excluded(&floor, &coarser), Vec::<u32>::new());
    assert_eq!(between.excluded(&water, &coarser), vec![0]);
    let production = ExclusionConstants::PRODUCTION;
    assert_eq!(between.excluded(&floor, &production), vec![0]);
    assert_eq!(between.excluded(&water, &production), vec![0]);
}

// Row 16: the compiler's floor-layer stride is the coarsest stride among the
// floor layer's runtime tiers, as water's is pinned to water's. The floor
// layer's tiers start as water's steps and divisors (`plan.md` Delegated
// answers) and live in render-cpu's reflection presets until the floor
// layer gives them their own table, which this pin then reads; retuning a
// tier coarser fails it either way.
#[test]
fn floor_layer_stride_divisor_matches_the_coarsest_floor_layer_tier() {
    let coarsest = FluidReflectionQuality::ALL
        .iter()
        .filter_map(|quality| quality.preset())
        .map(|preset| preset.march.rows_per_stride_pixel)
        .min()
        .expect("some tier runs a screen-space layer");
    assert_eq!(FLOOR_LAYER_COARSEST_STRIDE_DIVISOR, coarsest);
}
