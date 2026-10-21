// Reflection-probe capture exclusion contracts: threshold, floor, size, thinness and the underside rule.
// See: context/lib/resource_management.md §5.2

use glam::{DVec3, Vec3};
use postretro_level_format::environment_volumes::EnvironmentVolumesSection;
use postretro_level_format::reflection_probes::MAX_REFLECTION_PROBES;
use postretro_render_cpu::fluid_surface::reflection_quality::FluidReflectionQuality;

use crate::bake_control::BakeControl;
use crate::bvh_build::build_bvh;
use crate::geometry::{GeometryResult, extract_geometry_with_brushes};
use crate::map_data::{BrushVolume, ReflectionCapture};
use crate::partition::Aabb;
use crate::{environment_volumes, partition, portals, visibility};

use super::box_fit::WorldBounds;
use super::capture_point::PointClassifier;
use super::exclusion::{
    BrushExtents, DEFAULT_HFOV_DEGREES, ExclusionScene, SSR_COARSEST_STRIDE_DIVISOR,
    THRESHOLD_KNEE_WIDTH_M, classify_probes, excluded_by_rule, floor_ratio, size_threshold,
    thin_floor, water_width,
};
use super::hit::SceneTracer;
use super::placement::{PlacedProbe, PlacementScene, place_probes};
use super::placement_test_fixtures::{
    UNIT, box_brush, engine_box, entity, fluid, parse_inline, reflection_volume, sealed_room,
};
use super::reflection_probes_test_fixtures::{GeometryBuilder, fluid_volume, up_rect, volumes};

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// A compiled map with what classification reads, every brush `Auto`.
struct ExclusionFixture {
    geometry: GeometryResult,
    face_brush: Vec<Option<u32>>,
    brush_volumes: Vec<BrushVolume>,
    environment: EnvironmentVolumesSection,
    probes: Vec<PlacedProbe>,
}

impl ExclusionFixture {
    /// Worldspawn with `world` brushes (entity 0), then `entities` as map
    /// entities 1, 2, … compiled through parse, BSP, geometry, id 52 and
    /// placement.
    fn compile(world: &[String], entities: &[String]) -> Self {
        let map = parse_inline(&format!(
            "{}\n{}\n",
            entity("worldspawn", "", world),
            entities.join("\n")
        ));
        let result = partition::partition(&map.brush_volumes).expect("partition");
        let generated = portals::generate_portals(&result.tree);
        let exterior = visibility::find_exterior_leaves(&result.tree, &generated);
        let (mut geometry, face_brush) =
            extract_geometry_with_brushes(&result.faces, &result.tree, &exterior);
        assert_eq!(face_brush.len(), geometry.face_index_ranges.len());
        let (bvh, primitives, _) = build_bvh(&geometry).expect("BVH build");
        let environment = environment_volumes::encode_environment_volumes_section(
            &map.environment_volumes,
            map.initial_gravity,
            &map.brush_volumes,
            &mut geometry.texture_names,
        );
        let probes = {
            let tracer = SceneTracer::new(&bvh, &primitives, &geometry);
            let bounds =
                WorldBounds::from_positions(geometry.geometry.vertices.iter().map(|v| &v.position))
                    .expect("the fixture has a static world");
            let classifier = PointClassifier::new(&result.tree, &exterior, &environment);
            let control = BakeControl::unrestricted();
            place_probes(
                &environment,
                &map.reflection_volumes,
                &PlacementScene {
                    tracer: &tracer,
                    bounds: &bounds,
                    classifier: &classifier,
                    governor: control.governor(),
                },
                MAX_REFLECTION_PROBES,
            )
            .expect("placement")
        };
        Self {
            geometry,
            face_brush,
            brush_volumes: map.brush_volumes,
            environment,
            probes,
        }
    }

    fn excluded(&self) -> Vec<Vec<u32>> {
        let overrides = vec![ReflectionCapture::Auto; self.brush_volumes.len()];
        classify_probes(
            &ExclusionScene {
                brush_volumes: &self.brush_volumes,
                capture_override: &overrides,
                face_brush: &self.face_brush,
                geometry: &self.geometry,
                environment_volumes: &self.environment,
            },
            &self.probes,
        )
    }

    /// The only probe's excluded set.
    fn only_probe_excluded(&self) -> Vec<u32> {
        assert_eq!(self.probes.len(), 1, "{:?}", self.probes);
        self.excluded().remove(0)
    }

    /// The brush whose hull is the map-unit box `min`..`max`, within the
    /// parser's f32 rounding of plane points.
    fn brush(&self, min: [f64; 3], max: [f64; 3]) -> u32 {
        let (lo, hi) = engine_box(min, max);
        self.brush_volumes
            .iter()
            .position(|b| {
                (b.aabb.min - lo).abs().max_element() < 1e-4
                    && (b.aabb.max - hi).abs().max_element() < 1e-4
            })
            .unwrap_or_else(|| panic!("no brush spans {min:?}..{max:?}")) as u32
    }

    /// The index of the probe a fluid or reflection-volume entity placed.
    fn probe_of(&self, source_entity: u32) -> usize {
        self.probes
            .iter()
            .position(|p| p.source_entity == source_entity)
            .unwrap_or_else(|| panic!("no probe for entity {source_entity}: {:?}", self.probes))
    }

    /// Whether any surviving face of `brush` points down.
    fn has_downward_face(&self, brush: u32) -> bool {
        let geom = &self.geometry.geometry;
        self.face_brush
            .iter()
            .enumerate()
            .filter(|&(_, &b)| b == Some(brush))
            .any(|(face, _)| {
                let range = &self.geometry.face_index_ranges[face];
                let start = range.index_offset as usize;
                let corner = |k: usize| {
                    DVec3::from(
                        geom.vertices[geom.indices[start + k] as usize]
                            .position
                            .map(f64::from),
                    )
                };
                (corner(1) - corner(0)).cross(corner(2) - corner(0)).y < 0.0
            })
    }
}

fn room_with(max: [f64; 3], extra: &[String]) -> Vec<String> {
    let mut world = sealed_room([0.0; 3], max);
    world.extend(extra.iter().cloned());
    world
}

fn water(brushes: &[([f64; 3], [f64; 3])]) -> String {
    let brushes: Vec<String> = brushes.iter().map(|&(a, b)| box_brush(a, b)).collect();
    fluid("water", &brushes)
}

/// Metres to map units.
fn units(meters: f64) -> f64 {
    meters / UNIT
}

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

/// A hull at the origin with engine extents `x` × `y` (along the water
/// normal) × `z`.
fn sized(x: f64, y: f64, z: f64) -> BrushVolume {
    hull([0.0; 3], [x, y, z])
}

fn probe(bound_area: f64, faces: Vec<(u32, u32)>) -> PlacedProbe {
    PlacedProbe {
        source_entity: 1,
        authored: false,
        box_min: [-100.0; 3],
        box_max: [100.0; 3],
        capture_point: [0.0; 3],
        bound_area,
        faces,
    }
}

/// One probe of water width `width` binding no faces, over hulls with no
/// surviving faces: the size and floor rule alone.
fn classify_by_size(width: f64, brushes: &[BrushVolume]) -> Vec<u32> {
    let overrides = vec![ReflectionCapture::Auto; brushes.len()];
    classify_with(brushes, &overrides, width)
}

fn classify_with(brushes: &[BrushVolume], overrides: &[ReflectionCapture], width: f64) -> Vec<u32> {
    let geometry = GeometryBuilder::default().finish(Vec::new());
    let environment = volumes(Vec::new());
    classify_probes(
        &ExclusionScene {
            brush_volumes: brushes,
            capture_override: overrides,
            face_brush: &[],
            geometry: &geometry,
            environment_volumes: &environment,
        },
        &[probe(width * width, Vec::new())],
    )
    .remove(0)
}

/// Whether a 2 × 2 × 2 m brush, hull `hull_min`..`hull_max`, whose only
/// surviving face is the unit quad centred at `center` with the given
/// outward normal tilted `degrees` below horizontal-outward (n·N = −sin), is
/// excluded by a probe binding a 10 × 10 m pool at y = 0 (t = 8.5 m).
fn quad_brush_excluded(hull_min: [f64; 3], hull_max: [f64; 3], center: Vec3, degrees: f32) -> bool {
    let a = degrees.to_radians();
    let normal = Vec3::new(a.cos(), -a.sin(), 0.0);
    // In the quad's plane: along z, and up its slope.
    let (across, up) = (Vec3::Z * 0.5, Vec3::new(a.sin(), a.cos(), 0.0) * 0.5);
    let corners = [
        center - across - up,
        center + across - up,
        center + across + up,
        center - across + up,
    ];
    let mut builder = GeometryBuilder::default();
    builder.push_face(normal, &corners, 0, Vec3::ZERO, 0, [[0.0; 2]; 4]);
    let geometry = builder.finish(vec!["test/wall".to_string()]);
    let environment = volumes(vec![fluid_volume(1, &[up_rect(0.0, 0.0, 10.0, 10.0, 0.0)])]);
    let brushes = [hull(hull_min, hull_max)];
    let excluded = classify_probes(
        &ExclusionScene {
            brush_volumes: &brushes,
            capture_override: &[ReflectionCapture::Auto],
            face_brush: &[Some(0)],
            geometry: &geometry,
            environment_volumes: &environment,
        },
        &[probe(100.0, vec![(0, 0)])],
    );
    excluded[0] == [0]
}

/// Widths 0.5 m to 500 m in 1 cm steps.
fn widths() -> impl Iterator<Item = f64> {
    (50..=50_000).map(|cm| f64::from(cm) * 0.01)
}

// ---------------------------------------------------------------------------
// Threshold, width and size
// ---------------------------------------------------------------------------

#[test]
fn size_threshold_meets_the_anchors_and_rises_continuously_with_width() {
    for (width, expected) in [(2.0, 1.7), (10.0, 8.5), (50.0, 16.5), (100.0, 21.956)] {
        let t = size_threshold(width);
        assert!(
            (t - expected).abs() < 1e-3,
            "t({width}) = {t}, not {expected}"
        );
    }
    let knee = THRESHOLD_KNEE_WIDTH_M;
    assert!((size_threshold(knee - 1e-9) - size_threshold(knee + 1e-9)).abs() < 1e-6);
    let mut last = f64::NEG_INFINITY;
    for width in widths() {
        let t = size_threshold(width);
        assert!(t > last, "t({width}) = {t} does not rise past {last}");
        last = t;
    }
}

#[test]
fn water_width_is_the_root_of_the_bound_area_for_an_l_pool_and_a_canal() {
    let room = room_with([512.0, 256.0, 192.0], &[]);
    // An L of a 128 × 64 u and a 64 × 64 u basin.
    let l_pool = ExclusionFixture::compile(
        &room,
        &[water(&[
            ([64.0, 64.0, 0.0], [192.0, 128.0, 32.0]),
            ([64.0, 128.0, 0.0], [128.0, 192.0, 32.0]),
        ])],
    );
    assert_eq!(l_pool.probes.len(), 1, "{:?}", l_pool.probes);
    let l_area = (128.0 * 64.0 + 64.0 * 64.0) * UNIT * UNIT;
    let width = water_width(&l_pool.probes[0]);
    assert!((width - l_area.sqrt()).abs() < 1e-4, "{width}");

    // A 10 × 2.4 m canal: W = √24 ≈ 4.90 m, not its 10 m length.
    let canal = ExclusionFixture::compile(
        &room,
        &[water(&[(
            [64.0, 64.0, 0.0],
            [64.0 + units(10.0), 64.0 + units(2.4), 32.0],
        )])],
    );
    assert_eq!(canal.probes.len(), 1, "{:?}", canal.probes);
    let width = water_width(&canal.probes[0]);
    assert!((width - 24.0_f64.sqrt()).abs() < 1e-4, "{width}");
}

#[test]
fn brush_just_under_the_threshold_is_excluded_and_just_over_it_kept() {
    for width in [5.0, 50.0] {
        let (t, eps) = (size_threshold(width), 1e-4);
        let brushes = [
            sized(t - eps, 1.0, 0.5),
            sized(t + eps, 1.0, 0.5),
            sized(0.5, 1.0, t + eps),
        ];
        assert_eq!(classify_by_size(width, &brushes), vec![0], "W = {width}");
    }
}

#[test]
fn brush_just_under_the_floor_is_kept_and_just_over_it_excluded() {
    for width in [5.0, 50.0] {
        let (f, eps) = (thin_floor(width), 1e-5);
        let brushes = [
            sized(1.0, f - eps, 1.0),
            sized(1.0, f + eps, 1.0),
            // A post thinner than f in plane but taller than it.
            sized(f * 0.5, 1.0, f * 0.5),
        ];
        assert_eq!(classify_by_size(width, &brushes), vec![1, 2], "W = {width}");
    }
    // The floor at the coarsest stride (divisor 180): 3.7 cm at 5 m, 0.37 m
    // at 50 m.
    assert!(
        (thin_floor(5.0) - 0.0372).abs() < 1e-4,
        "{}",
        thin_floor(5.0)
    );
    assert!(
        (thin_floor(50.0) - 0.372).abs() < 1e-3,
        "{}",
        thin_floor(50.0)
    );
    for width in widths() {
        assert!(
            thin_floor(width) < size_threshold(width),
            "f ≥ t at W = {width}"
        );
    }
}

#[test]
fn shore_rail_is_kept_and_standing_post_excluded_on_two_metre_water() {
    // t = 1.7 m: the rail's 0.1 m side is below t, its 2 m side is not.
    let rail = sized(2.0, 0.3, 0.1);
    let post = sized(0.2, 3.0, 0.2);
    assert_eq!(classify_by_size(2.0, &[rail, post]), vec![1]);
}

#[test]
fn brush_extents_against_a_vertical_normal_read_vertical_and_along_wall_size_and_depth_thinness() {
    // 5 cm deep from the wall (x), 3 m tall (y), 0.2 m along the wall (z).
    let pole = hull([0.0; 3], [0.05, 3.0, 0.2]);
    for normal in [DVec3::X, DVec3::NEG_X] {
        let extents = BrushExtents::of(&pole.aabb, normal);
        assert!((extents.size - 3.0).abs() < 1e-12, "{extents:?}");
        assert!((extents.thinness - 0.05).abs() < 1e-12, "{extents:?}");
    }
    let along_wall = BrushExtents::of(&hull([0.0; 3], [4.0, 0.5, 1.5]).aabb, DVec3::X);
    assert!((along_wall.size - 1.5).abs() < 1e-12, "{along_wall:?}");
    assert!((along_wall.thinness - 4.0).abs() < 1e-12, "{along_wall:?}");
    // The same pole against water: 0.2 m in plane, 3 m along up.
    let on_water = BrushExtents::of(&pole.aabb, DVec3::Y);
    assert!((on_water.size - 0.2).abs() < 1e-12, "{on_water:?}");
    assert!((on_water.thinness - 3.0).abs() < 1e-12, "{on_water:?}");
    // At W = 5 m on the wall (t 4.25 m, f 3.7 cm) the pole's 3 m height is
    // under t and its 5 cm depth over f.
    let width = 5.0;
    let wall = BrushExtents::of(&pole.aabb, DVec3::X);
    assert!(excluded_by_rule(
        wall,
        size_threshold(width),
        thin_floor(width),
        || false
    ));
}

#[test]
fn overrides_beat_the_rule_in_both_directions() {
    let width = 5.0;
    let brushes = [
        sized(10.0, 1.0, 1.0),
        sized(0.5, 1.0, 0.5),
        sized(0.5, 1.0, 0.5),
    ];
    let overrides = [
        ReflectionCapture::Never,
        ReflectionCapture::Always,
        ReflectionCapture::Auto,
    ];
    assert_eq!(classify_with(&brushes, &overrides, width), vec![0, 2]);
}

// ---------------------------------------------------------------------------
// Floor constants against their sources
// ---------------------------------------------------------------------------

#[test]
fn floor_stride_divisor_matches_the_coarsest_screen_space_tier() {
    let coarsest = FluidReflectionQuality::ALL
        .iter()
        .filter(|quality| quality.runs_screen_space())
        .map(|quality| {
            quality
                .preset()
                .expect("a tier that runs the screen-space layer has a preset")
                .march
                .rows_per_stride_pixel
        })
        .min()
        .expect("some tier runs the screen-space layer");
    assert_eq!(SSR_COARSEST_STRIDE_DIVISOR, coarsest);
}

#[test]
fn floor_field_of_view_matches_the_engine_default() {
    // The binary's `camera::HFOV`, which level-compiler cannot depend on:
    // read its source, written `<degrees> * PI / 180`.
    let camera = include_str!("../../../postretro/src/camera.rs");
    let line = camera
        .lines()
        .find(|line| line.trim_start().starts_with("pub const HFOV: f32 ="))
        .expect("crates/postretro/src/camera.rs declares HFOV");
    let degrees: f64 = line
        .split('=')
        .nth(1)
        .and_then(|value| value.split('*').next())
        .and_then(|degrees| degrees.trim().parse().ok())
        .unwrap_or_else(|| panic!("HFOV is not written as `<degrees> * PI / 180`: {line}"));
    assert_eq!(DEFAULT_HFOV_DEGREES, degrees);
    assert_eq!(DEFAULT_HFOV_DEGREES, 100.0);
    // θv = 2·atan(tan(50°)·9/16), so 2·tan(θv/2) = 2·tan(50°)·9/16.
    let expected = 2.0 * 50.0_f64.to_radians().tan() * 9.0 / 16.0 / 180.0;
    assert!(
        (floor_ratio() - expected).abs() < 1e-12,
        "{}",
        floor_ratio()
    );
}

// ---------------------------------------------------------------------------
// Undersides
// ---------------------------------------------------------------------------

/// A 50 m lake's room: the lake spans 64..2032.5 u on both axes, with dry
/// floor past x = 2032.5.
const LAKE_ROOM: [f64; 3] = [2600.0, 2100.0, 512.0];
const LAKE: ([f64; 3], [f64; 3]) = ([64.0, 64.0, 0.0], [2032.5, 2032.5, 32.0]);

/// A 10 m dock 1 m thick and a 12 m beam 0.6 m deep, with x offset `x`.
fn dock_and_beam(x: f64) -> [([f64; 3], [f64; 3]); 2] {
    [
        (
            [x, 200.0, 64.0],
            [x + units(2.0), 200.0 + units(10.0), 64.0 + units(1.0)],
        ),
        (
            [x + 400.0, 200.0, 200.0],
            [
                x + 400.0 + units(0.3),
                200.0 + units(12.0),
                200.0 + units(0.6),
            ],
        ),
    ]
}

#[test]
fn lake_dock_and_beam_are_kept_over_the_water_and_excluded_off_it() {
    for (x, over_water) in [(400.0, true), (2040.0, false)] {
        let brushes = dock_and_beam(x);
        let extra: Vec<String> = brushes.iter().map(|&(a, b)| box_brush(a, b)).collect();
        let fixture = ExclusionFixture::compile(&room_with(LAKE_ROOM, &extra), &[water(&[LAKE])]);
        let width = water_width(&fixture.probes[0]);
        assert!((size_threshold(width) - 16.5).abs() < 1e-2, "W = {width}");
        assert!((thin_floor(width) - 0.372).abs() < 1e-3, "W = {width}");
        let excluded = fixture.only_probe_excluded();
        for (min, max) in brushes {
            let brush = fixture.brush(min, max);
            assert_eq!(
                excluded.contains(&brush),
                !over_water,
                "brush {min:?}..{max:?}, over water {over_water}: {excluded:?}"
            );
        }
    }
}

#[test]
fn pillar_and_post_standing_in_the_water_are_excluded() {
    let pillar = ([140.0, 140.0, 0.0], [160.0, 160.0, 128.0]);
    let post = ([190.0, 140.0, 0.0], [198.0, 148.0, 96.0]);
    let fixture = ExclusionFixture::compile(
        &room_with(
            [512.0, 384.0, 192.0],
            &[box_brush(pillar.0, pillar.1), box_brush(post.0, post.1)],
        ),
        &[water(&[([64.0, 64.0, 0.0], [256.0, 256.0, 32.0])])],
    );
    // The water around the pillar and the post binds more than one derived
    // probe; each excludes both.
    let excluded = fixture.excluded();
    assert!(!excluded.is_empty());
    for (min, max) in [pillar, post] {
        let brush = fixture.brush(min, max);
        assert!(!fixture.has_downward_face(brush));
        for set in &excluded {
            assert!(set.contains(&brush), "{min:?}..{max:?}: {excluded:?}");
        }
    }
}

#[test]
fn downward_faces_off_the_water_or_meeting_it_only_along_its_edge_are_excluded() {
    let pool = water(&[([64.0, 64.0, 0.0], [256.0, 256.0, 32.0])]);
    // Floating beside the pool, over dry floor.
    let beside = ([320.0, 300.0, 64.0], [352.0, 332.0, 96.0]);
    // A shore ledge whose bottom meets the water's x = 256 edge.
    let flush_ledge = ([256.0, 96.0, 64.0], [320.0, 224.0, 96.0]);
    // The same ledge overhanging the water by 6 u (15 cm) is kept.
    let overhanging_ledge = ([250.0, 96.0, 64.0], [320.0, 224.0, 96.0]);
    for (ledge, ledge_kept) in [(flush_ledge, false), (overhanging_ledge, true)] {
        let fixture = ExclusionFixture::compile(
            &room_with(
                [512.0, 384.0, 192.0],
                &[box_brush(beside.0, beside.1), box_brush(ledge.0, ledge.1)],
            ),
            std::slice::from_ref(&pool),
        );
        let excluded = fixture.only_probe_excluded();
        let beside_brush = fixture.brush(beside.0, beside.1);
        let ledge_brush = fixture.brush(ledge.0, ledge.1);
        assert!(fixture.has_downward_face(beside_brush) && fixture.has_downward_face(ledge_brush));
        assert!(excluded.contains(&beside_brush), "{excluded:?}");
        assert_eq!(
            !excluded.contains(&ledge_brush),
            ledge_kept,
            "{ledge:?}: {excluded:?}"
        );
    }
}

#[test]
fn deck_over_an_authored_capture_offset_stays_in_the_capture() {
    let deck = ([192.0, 96.0, 48.0], [256.0, 224.0, 56.0]);
    let fixture = ExclusionFixture::compile(
        &room_with([512.0, 384.0, 192.0], &[box_brush(deck.0, deck.1)]),
        &[
            water(&[([64.0, 64.0, 0.0], [256.0, 256.0, 32.0])]),
            reflection_volume(
                "\"_capture_offset\" \"64 0\"",
                [48.0, 48.0, 0.0],
                [272.0, 272.0, 128.0],
            ),
        ],
    );
    assert_eq!(fixture.probes.len(), 1, "{:?}", fixture.probes);
    let probe = &fixture.probes[0];
    assert!(probe.authored);
    // The capture point sits under the deck.
    let point = DVec3::from(probe.capture_point.map(f64::from));
    let (lo, hi) = engine_box(deck.0, deck.1);
    assert!(
        point.x > lo.x && point.x < hi.x && point.z > lo.z && point.z < hi.z && point.y < lo.y,
        "{point} is not under the deck {lo}..{hi}"
    );
    let deck = fixture.brush(deck.0, deck.1);
    assert!(!fixture.only_probe_excluded().contains(&deck));
}

#[test]
fn crate_sunk_into_a_dock_over_the_water_is_excluded_and_the_dock_kept() {
    let dock = ([96.0, 96.0, 64.0], [224.0, 160.0, 76.0]);
    // Its bottom 5 cm inside the dock.
    let sunk_crate = ([140.0, 110.0, 76.0 - units(0.05)], [160.0, 130.0, 94.0]);
    let fixture = ExclusionFixture::compile(
        &room_with(
            [512.0, 384.0, 192.0],
            &[
                box_brush(dock.0, dock.1),
                box_brush(sunk_crate.0, sunk_crate.1),
            ],
        ),
        &[water(&[([64.0, 64.0, 0.0], [256.0, 256.0, 32.0])])],
    );
    let excluded = fixture.only_probe_excluded();
    let (dock, sunk_crate) = (
        fixture.brush(dock.0, dock.1),
        fixture.brush(sunk_crate.0, sunk_crate.1),
    );
    // CSG removed the crate's bottom; its hull's bottom side is no underside.
    assert!(!fixture.has_downward_face(sunk_crate));
    assert!(excluded.contains(&sunk_crate), "{excluded:?}");
    assert!(!excluded.contains(&dock), "{excluded:?}");
}

#[test]
fn footbridge_over_one_pool_is_kept_by_its_probe_and_excluded_by_the_other() {
    // 3.25 m long, 1 m wide, 0.6 m deep, over pool A only.
    let bridge = ([96.0, 140.0, 64.0], [224.0, 180.0, 64.0 + units(0.6)]);
    let fixture = ExclusionFixture::compile(
        &room_with([768.0, 384.0, 192.0], &[box_brush(bridge.0, bridge.1)]),
        &[
            water(&[([64.0, 64.0, 0.0], [256.0, 256.0, 32.0])]),
            water(&[([384.0, 64.0, 0.0], [608.0, 288.0, 32.0])]),
        ],
    );
    let (a, b) = (fixture.probe_of(1), fixture.probe_of(2));
    let bridge = fixture.brush(bridge.0, bridge.1);
    for probe in [a, b] {
        let width = water_width(&fixture.probes[probe]);
        let extents = BrushExtents::of(&fixture.brush_volumes[bridge as usize].aabb, DVec3::Y);
        assert!(extents.size < size_threshold(width) && extents.thinness >= thin_floor(width));
    }
    let excluded = fixture.excluded();
    assert!(!excluded[a].contains(&bridge), "{excluded:?}");
    assert!(excluded[b].contains(&bridge), "{excluded:?}");
}

#[test]
fn deck_between_two_bound_planes_is_kept_over_the_lower_water_and_excluded_under_the_upper() {
    // Lower water tops at 32 u; upper water at 144 u, on a slab over open
    // space. One authored box binds both.
    let pools = water(&[
        ([64.0, 64.0, 0.0], [192.0, 256.0, 32.0]),
        ([256.0, 64.0, 112.0], [384.0, 256.0, 144.0]),
    ]);
    let slab = box_brush([256.0, 64.0, 96.0], [384.0, 256.0, 112.0]);
    let over_lower = ([96.0, 96.0, 64.0], [160.0, 224.0, 72.0]);
    let under_upper = ([288.0, 96.0, 64.0], [352.0, 224.0, 72.0]);
    for (deck, kept) in [(over_lower, true), (under_upper, false)] {
        let fixture = ExclusionFixture::compile(
            &room_with(
                [512.0, 384.0, 256.0],
                &[slab.clone(), box_brush(deck.0, deck.1)],
            ),
            &[
                pools.clone(),
                reflection_volume("", [48.0, 48.0, 0.0], [400.0, 272.0, 240.0]),
            ],
        );
        assert_eq!(fixture.probes.len(), 1, "{:?}", fixture.probes);
        assert!(fixture.probes[0].authored);
        assert_eq!(fixture.probes[0].faces.len(), 2, "{:?}", fixture.probes[0]);
        let brush = fixture.brush(deck.0, deck.1);
        let excluded = fixture.only_probe_excluded();
        assert_eq!(!excluded.contains(&brush), kept, "{deck:?}: {excluded:?}");
    }
}

#[test]
fn faces_sloping_down_past_one_degree_over_the_water_are_undersides() {
    let (hull_min, hull_max) = ([4.0, 1.0, 4.0], [6.0, 3.0, 6.0]);
    let center = Vec3::new(5.0, 2.0, 5.0);
    // A wedge's 45° bottom.
    assert!(!quad_brush_excluded(hull_min, hull_max, center, 45.0));
    // 2° past vertical toward the water is past the 1° line; 0.5° is not.
    assert!(!quad_brush_excluded(hull_min, hull_max, center, 2.0));
    assert!(quad_brush_excluded(hull_min, hull_max, center, 0.5));
}

#[test]
fn face_crossing_the_water_plane_counts_by_its_part_above() {
    let (hull_min, hull_max) = ([4.0, -1.0, 4.0], [6.0, 1.0, 6.0]);
    // Sloping 30°, half above the plane: over the water when centred at
    // x = 5. Centred on the pool's x = 10 edge, its part above lies past the
    // edge and only its part below overhangs the water.
    assert!(!quad_brush_excluded(
        hull_min,
        hull_max,
        Vec3::new(5.0, 0.0, 5.0),
        30.0
    ));
    assert!(quad_brush_excluded(
        hull_min,
        hull_max,
        Vec3::new(10.0, 0.0, 5.0),
        30.0
    ));
}
