//! The texel-step detector through the production stage: warnings on the
//! caller's thread, cut faces, memo replay on every cache path, the
//! threshold's key, and no false warnings with the floor on.
use super::*;
use crate::lightmap_bake::DEFAULT_AREA_SAMPLE_COUNT;
use crate::map_data::DEFAULT_LIGHT_SIZE;
use crate::texel_step_detector::{VisibilityGrid, texel_steps};
use postretro_test_log_capture::CapturedRecord;

/// Height of a contact plate above the floor. The detector's separation
/// segments, like the footprint's clip segments, run 1 mm up and pass beneath
/// it; a shadow ray starts 1 mm along its way from that lift, so a plate
/// under 2 mm up would let a near-vertical ray start above it.
const PLATE_HEIGHT: f32 = 0.005;

fn empty_geometry() -> GeometryResult {
    let mut geometry = quad_geometry();
    geometry.geometry.vertices.clear();
    geometry.geometry.indices.clear();
    geometry.geometry.faces.clear();
    geometry.face_index_ranges.clear();
    geometry
}

/// Append a convex polygon as one fan-triangulated face of leaf 0.
fn push_polygon(geometry: &mut GeometryResult, corners: &[Vec3], normal: Vec3) {
    let tangent = normal.any_orthonormal_vector();
    let base = geometry.geometry.vertices.len() as u32;
    let index_offset = geometry.geometry.indices.len() as u32;
    for corner in corners {
        geometry.geometry.vertices.push(Vertex::new(
            corner.to_array(),
            [0.0, 0.0],
            normal.to_array(),
            tangent.to_array(),
            true,
            [0.0, 0.0],
            0,
        ));
    }
    for i in 1..corners.len() as u32 - 1 {
        geometry
            .geometry
            .indices
            .extend([base, base + i, base + i + 1]);
    }
    geometry.geometry.faces.push(FaceMeta {
        leaf_index: 0,
        texture_index: 0,
    });
    geometry.face_index_ranges.push(FaceIndexRange {
        index_offset,
        index_count: geometry.geometry.indices.len() as u32 - index_offset,
    });
}

/// A solid box resting on whatever lies under it: its top and four sides,
/// each one face.
fn push_box(geometry: &mut GeometryResult, min: Vec3, max: Vec3) {
    let v = |x: f32, y: f32, z: f32| Vec3::new(x, y, z);
    let (a, b) = (min, max);
    push_polygon(
        geometry,
        &[
            v(a.x, b.y, a.z),
            v(b.x, b.y, a.z),
            v(b.x, b.y, b.z),
            v(a.x, b.y, b.z),
        ],
        Vec3::Y,
    );
    push_polygon(
        geometry,
        &[
            v(a.x, a.y, a.z),
            v(a.x, a.y, b.z),
            v(a.x, b.y, b.z),
            v(a.x, b.y, a.z),
        ],
        Vec3::NEG_X,
    );
    push_polygon(
        geometry,
        &[
            v(b.x, a.y, b.z),
            v(b.x, a.y, a.z),
            v(b.x, b.y, a.z),
            v(b.x, b.y, b.z),
        ],
        Vec3::X,
    );
    push_polygon(
        geometry,
        &[
            v(b.x, a.y, a.z),
            v(a.x, a.y, a.z),
            v(a.x, b.y, a.z),
            v(b.x, b.y, a.z),
        ],
        Vec3::NEG_Z,
    );
    push_polygon(
        geometry,
        &[
            v(a.x, a.y, b.z),
            v(b.x, a.y, b.z),
            v(b.x, b.y, b.z),
            v(a.x, b.y, b.z),
        ],
        Vec3::Z,
    );
}

fn floor(size: f32) -> GeometryResult {
    let mut geometry = empty_geometry();
    push_quad(&mut geometry, 0.0, 0.0, size, 0.0, 0);
    geometry
}

/// A 6 m floor, face 0, under a 2 m plate resting 5 mm above it, face 1.
fn contact_geometry() -> GeometryResult {
    let mut geometry = floor(6.0);
    push_quad(&mut geometry, 2.0, 2.0, 2.0, PLATE_HEIGHT, 0);
    geometry
}

fn hard(origin: [f64; 3], range: f32) -> MapLight {
    let mut light = point_light(DVec3::from_array(origin), [1.0, 0.9, 0.8]);
    light.falloff_range = range;
    light
}

/// The source labels a fixture's lights carry: entity `10 + i` for light
/// `i`, so a warning that names a layer-list index instead shows.
fn source_labels(lights: &[MapLight]) -> Vec<String> {
    (0..lights.len())
        .map(|i| format!("entity {} 'light'", 10 + i))
        .collect()
}

fn floor_config(penumbra_floor: bool) -> LightmapConfig {
    LightmapConfig {
        penumbra_floor,
        ..config(false)
    }
}

/// How one stage bake runs. Every bake runs on the calling thread, so log
/// capture reads its warnings; `permits` is the `-j` worker count.
#[derive(Clone, Copy)]
struct Run<'a> {
    cache: Option<&'a StageCache>,
    permits: usize,
    threshold: Option<f32>,
    selection: Option<&'a EntityShadowLightsSection>,
    /// A small pool edge cuts large faces; `None` keeps the production edge.
    pool_edge: Option<u32>,
    samples: u32,
}

impl Default for Run<'_> {
    fn default() -> Self {
        Self {
            cache: None,
            permits: 1,
            threshold: None,
            selection: None,
            pool_edge: None,
            samples: 4,
        }
    }
}

struct Baked {
    report: TexelStepReport,
    section: Vec<u8>,
    /// Every texel-step warning logged, in order.
    warnings: Vec<String>,
    records: Vec<CapturedRecord>,
}

impl Baked {
    fn logged(&self, level: Level, body: &str) -> usize {
        self.records
            .iter()
            .filter(|record| record.level == level && record.message.contains(body))
            .count()
    }
}

fn bake(
    mut geometry: GeometryResult,
    lights: &[MapLight],
    config: &LightmapConfig,
    run: Run,
) -> Baked {
    let mut args = test_args();
    args.soft_shadow_samples = run.samples;
    let static_lights = StaticBakedLights::from_lights(lights);
    let alpha_lights = AlphaLightsNs::from_lights(lights);
    let progress = StageProgress::indeterminate();
    let control = BakeControl::new(Arc::new(Governor::new(run.permits, false)), &progress);
    let prepared = match run.pool_edge {
        Some(edge) => lightmap_bake::prepare_atlas_within(
            &mut geometry,
            &static_lights,
            config.lightmap_density,
            &[],
            lightmap_bake::BlockOrdering::by_cell_id(config.direction_texel_scale),
            edge,
            &control,
        ),
        None => lightmap_bake::prepare_atlas(
            &mut geometry,
            &static_lights,
            config.lightmap_density,
            &[],
        ),
    }
    .expect("fixture atlas prepares");
    let (bvh, primitives, _) = build_bvh(&geometry).expect("fixture BVH builds");
    let window = PartitionWindow {
        step_threshold: run.threshold,
        ..PartitionWindow::default()
    };
    let labels = source_labels(lights);
    let logs = LogCapture::start();
    let output = bake_fused_windowed(
        &args,
        run.cache,
        &control,
        &BakeControl::unrestricted(),
        &mut geometry,
        &static_lights,
        &alpha_lights,
        run.selection,
        &labels,
        &crate::texel_footprint::NO_EMBEDDED_EMITTERS,
        &bvh,
        &primitives,
        config,
        prepared,
        &window,
    )
    .expect("a stepped bake still compiles");
    let records = logs.records();
    let warnings = records
        .iter()
        .filter(|record| record.level == Level::Warn && record.message.contains("texel step"))
        .map(|record| record.message.clone())
        .collect();
    Baked {
        report: output.texel_steps,
        section: output.lightmap.section.to_bytes(),
        warnings,
        records,
    }
}

/// One light's stored visibility over interior texels of chart `face`, read
/// from its layer bake: the grid the detector sees, with no surface test.
fn stored_grid(
    mut geometry: GeometryResult,
    lights: &[MapLight],
    light: usize,
    face: usize,
    penumbra_floor: bool,
) -> VisibilityGrid {
    let config = floor_config(penumbra_floor);
    let static_lights = StaticBakedLights::from_lights(lights);
    let prepared =
        lightmap_bake::prepare_atlas(&mut geometry, &static_lights, config.lightmap_density, &[])
            .expect("fixture atlas prepares");
    let (bvh, primitives, _) = build_bvh(&geometry).expect("fixture BVH builds");
    let shared = SharedAtlas {
        charts: &prepared.charts,
        placements: &prepared.placements,
        atlas_width: prepared.atlas_width,
        atlas_height: prepared.atlas_height,
        layout: &prepared.layout,
    };
    let partition = lightmap_layer::bake_light_layer_controlled(
        &lights[light],
        &shared,
        &bvh,
        &primitives,
        &geometry,
        prepared.placements[face].layer,
        test_args().soft_shadow_samples,
        penumbra_floor,
        lightmap_layer::ExactSkips::ON,
        &BakeControl::unrestricted(),
    );
    let (width, height) = crate::chart_raster::chart_interior_dims(&prepared.charts[face]);
    let mut grid = VisibilityGrid::new(width as u32, height as u32);
    lightmap_layer::for_each_light_layer_chart_texel(&shared, face, |sample| {
        if let Some(record) = partition.texels.iter().find(|r| r.idx == sample.idx) {
            grid.set(sample.tx as u32, sample.ty as u32, record.raw_visibility);
        }
    });
    grid
}

// --- AC 18: the production stage reports to the caller (P1, P17) ----------

// P1: a hard light's contact edge steps with the floor disabled; the stage
// warns once, on the caller's thread, naming the light by its source label
// and the face. An `sdf` light ahead of it takes no layer, so the label is
// looked up through the layer light's source index. With the floor on,
// nothing warns and the compile succeeds either way.
#[test]
fn a_hard_contact_edge_warns_from_the_stage_only_with_the_floor_disabled() {
    let mut sdf = hard([20.0, 3.0, 20.0], 1.0);
    sdf.shadow_type = ShadowType::Sdf;
    let lights = [sdf, hard([3.3, 3.0, 2.7], 8.0)];
    let off = bake(
        contact_geometry(),
        &lights,
        &floor_config(false),
        Run::default(),
    );
    assert_eq!(off.warnings.len(), 1, "{:?}", off.warnings);
    let warning = &off.warnings[0];
    assert!(
        warning
            .starts_with("[Lightmap] texel step: static light entity 11 'light' on face 0 near ("),
        "{warning}"
    );
    assert!(
        warning.contains("adjacent texel pair(s) differ by up to 1.00"),
        "{warning}"
    );
    let step = off.report.warnings[0];
    assert_eq!((step.light, step.site), (0, StepSite::Face(0)));
    assert!(
        step.pairs >= 8,
        "the plate's whole outline steps: {}",
        step.pairs
    );
    assert_eq!(step.worst_difference, 1.0);
    // The named texel lies on the shadow's outline, about the plate's edge.
    let [x, y, z] = step.position;
    assert!(y.abs() < 1.0e-3, "{y}");
    assert!(
        (1.5..=4.5).contains(&x) && (1.5..=4.5).contains(&z),
        "{x}, {z}"
    );

    let on = bake(
        contact_geometry(),
        &lights,
        &floor_config(true),
        Run::default(),
    );
    assert!(on.warnings.is_empty(), "{:?}", on.warnings);
    assert_eq!(on.report, TexelStepReport::default());
}

/// A 6 m floor; a soft light's wide penumbra from a raised occluder in the
/// middle, and a hard light, out of the soft light's reach, over a small
/// contact plate in one corner.
fn short_edge_scene() -> (GeometryResult, Vec<MapLight>) {
    let mut geometry = floor(6.0);
    push_quad(&mut geometry, 2.5, 2.5, 1.5, 1.0, 0);
    push_quad(&mut geometry, 0.75, 0.75, 0.5, PLATE_HEIGHT, 0);
    let mut soft = hard([3.25, 3.0, 3.25], 3.5);
    soft.light_size = 1.0;
    let corner = hard([1.0, 0.6, 1.0], 1.2);
    (geometry, vec![soft, corner])
}

// P17: floor disabled, a stepped edge two texels a side warns, naming its
// face, while the soft light's penumbra over the rest of the face ramps and
// draws nothing.
#[test]
fn a_short_stepped_edge_warns_on_a_face_whose_other_edges_ramp() {
    let (geometry, lights) = short_edge_scene();
    // The soft light's penumbra is real and smooth.
    let soft = stored_grid(geometry.clone(), &lights, 0, 0, false);
    let fractional = (0..soft.height())
        .flat_map(|y| (0..soft.width()).map(move |x| (x, y)))
        .filter(|&(x, y)| soft.get(x, y).is_some_and(|v| v > 0.0 && v < 1.0))
        .count();
    assert!(
        fractional > 20,
        "the soft penumbra spans {fractional} texels"
    );
    assert_eq!(texel_steps(&soft, TEXEL_STEP_THRESHOLD).pairs, 0);

    let run = Run {
        samples: 32,
        ..Run::default()
    };
    let off = bake(geometry, &lights, &floor_config(false), run);
    assert_eq!(off.warnings.len(), 1, "{:?}", off.warnings);
    assert!(
        off.warnings[0].contains("static light entity 11 'light' on face 0 near"),
        "{}",
        off.warnings[0]
    );
    let step = off.report.warnings[0];
    assert!(
        (1..=16).contains(&step.pairs),
        "a short edge: {} pairs",
        step.pairs
    );
}

/// A 24 m floor that a 64-texel pool edge cuts 2 × 2 at 12 m, under a contact
/// plate straddling both cut lines.
fn cut_contact_geometry() -> GeometryResult {
    let mut geometry = floor(24.0);
    push_quad(&mut geometry, 11.0, 11.0, 2.0, PLATE_HEIGHT, 0);
    geometry
}

// P17: a contact edge crossing a face cut warns once, naming the pre-cut face,
// and counts exactly the pairs the uncut face counts: every pair straddling a
// cut line is checked, none in the overlap twice.
#[test]
fn a_stepped_edge_across_a_face_cut_warns_once_for_the_parent_face() {
    let lights = [hard([12.3, 3.0, 11.7], 40.0)];
    let config = floor_config(false);
    let cut = bake(
        cut_contact_geometry(),
        &lights,
        &config,
        Run {
            pool_edge: Some(CUT_POOL_EDGE),
            ..Run::default()
        },
    );
    let uncut = bake(cut_contact_geometry(), &lights, &config, Run::default());
    assert_eq!(cut.warnings.len(), 1, "{:?}", cut.warnings);
    assert!(
        cut.warnings[0].contains("on pre-cut face 0 (now faces 0..=3) near"),
        "{}",
        cut.warnings[0]
    );
    let (cut_step, uncut_step) = (cut.report.warnings[0], uncut.report.warnings[0]);
    assert_eq!(
        cut_step.site,
        StepSite::CutFace {
            parent: 0,
            first_face: 0,
            last_face: 3,
        }
    );
    assert_eq!(uncut_step.site, StepSite::Face(0));
    assert_eq!(cut_step.pairs, uncut_step.pairs, "each pair counts once");
    assert_eq!(cut_step.position, uncut_step.position);
    assert_eq!(cut_step.worst_difference, uncut_step.worst_difference);

    let on = bake(
        cut_contact_geometry(),
        &lights,
        &floor_config(true),
        Run {
            pool_edge: Some(CUT_POOL_EDGE),
            ..Run::default()
        },
    );
    assert!(on.warnings.is_empty(), "{:?}", on.warnings);
}

/// A 36 m floor whose corner the polygon cuts off along x + z = 31, so a
/// 64-texel pool edge cuts it 3 × 3 at 12 m and 24 m and its first piece,
/// x and z under 12 m, has no polygon and emits no sub-face. A contact plate
/// covers that piece, its edges on the two cut lines bounding it.
fn missed_piece_geometry() -> GeometryResult {
    let v = |x: f32, z: f32| Vec3::new(x, 0.0, z);
    let mut geometry = empty_geometry();
    push_polygon(
        &mut geometry,
        &[
            v(31.0, 0.0),
            v(36.0, 0.0),
            v(36.0, 36.0),
            v(0.0, 36.0),
            v(0.0, 31.0),
        ],
        Vec3::Y,
    );
    push_quad(&mut geometry, -4.0, -4.0, 16.0, PLATE_HEIGHT, 0);
    geometry
}

// Regression: a sub-face owned a pair only by its start texel, so a pair
// starting in a piece the polygon missed, its other texel in a sub-face's
// own segment, was never checked though the uncut face checks it. The plate's
// shadow edges run exactly along such pairs; cut and uncut bakes report alike.
#[test]
fn a_stepped_edge_beside_a_missed_cut_piece_reports_as_on_the_uncut_face() {
    let lights = [hard([14.0, 3.0, 14.0], 40.0)];
    let config = floor_config(false);
    let cut = bake(
        missed_piece_geometry(),
        &lights,
        &config,
        Run {
            pool_edge: Some(CUT_POOL_EDGE),
            ..Run::default()
        },
    );
    let uncut = bake(missed_piece_geometry(), &lights, &config, Run::default());
    let floor_steps = |baked: &Baked| {
        baked
            .report
            .warnings
            .iter()
            .copied()
            .find(|w| {
                matches!(
                    w.site,
                    StepSite::Face(0) | StepSite::CutFace { parent: 0, .. }
                )
            })
            .expect("the plate's edge steps on the floor")
    };
    let (cut_step, uncut_step) = (floor_steps(&cut), floor_steps(&uncut));
    let StepSite::CutFace {
        first_face,
        last_face,
        ..
    } = cut_step.site
    else {
        panic!("the floor is cut: {:?}", cut_step.site);
    };
    assert_eq!(
        last_face - first_face + 1,
        8,
        "nine pieces, one missed: {:?}",
        cut_step.site
    );
    assert_eq!(uncut_step.site, StepSite::Face(0));
    assert!(
        uncut_step.pairs >= 90,
        "both edges step: {}",
        uncut_step.pairs
    );
    assert_eq!(cut_step.pairs, uncut_step.pairs, "every pair counts once");
    assert_eq!(cut_step.position, uncut_step.position);
    assert_eq!(cut_step.worst_difference, uncut_step.worst_difference);
}

// --- AC 19: a warm compile replays the cold compile's warnings -------------

/// Two floors, each under a raised occluder, lit by hard point, spot and
/// directional lights and a soft light; floor disabled, several (light, face)
/// pairs step.
fn stepped_scene() -> (GeometryResult, Vec<MapLight>) {
    let mut geometry = floor(6.0);
    push_quad(&mut geometry, 2.0, 2.0, 2.0, 1.0, 0);
    push_quad(&mut geometry, 10.0, 0.0, 6.0, 0.0, 1);
    push_quad(&mut geometry, 12.0, 2.0, 2.0, 1.0, 1);
    let point = hard([3.3, 3.0, 2.7], 6.0);
    let mut sun = point_light(DVec3::new(0.0, 50.0, 0.0), [0.6, 0.5, 0.4]);
    sun.light_type = LightType::Directional;
    sun.cone_direction = Some(Vec3::new(0.3, -1.0, 0.2).normalize().to_array());
    let mut soft = hard([13.0, 2.5, 3.4], 6.0);
    soft.light_size = DEFAULT_LIGHT_SIZE;
    let mut spot = hard([12.8, 3.5, 3.1], 8.0);
    spot.light_type = LightType::Spot;
    spot.cone_direction = Some([0.0, -1.0, 0.0]);
    spot.cone_angle_inner = Some(1.2);
    spot.cone_angle_outer = Some(1.4);
    (geometry, vec![point, sun, soft, spot])
}

// P2, P11, P12, P15: every warm path reports exactly the cold warnings, each
// once — when every memo hits; when the section memo misses but every layer
// partition hits; when only the shadowmask memo re-keys (a promotion), so the
// shadowmask pass reads partitions after a section hit; and when the
// threshold changes, the section memo misses and the warnings follow it.
#[test]
fn warm_compiles_replay_the_cold_warnings_on_every_cache_path() {
    let (geometry, lights) = stepped_scene();
    let off = floor_config(false);
    let cold = bake(geometry.clone(), &lights, &off, Run::default());
    assert!(
        cold.report.warnings.len() >= 3,
        "the scene steps for several lights: {:?}",
        cold.warnings
    );
    let mut lit: Vec<u32> = cold.report.warnings.iter().map(|w| w.light).collect();
    lit.dedup();
    assert!(lit.len() >= 2, "more than one light steps: {lit:?}");
    assert!(
        cold.report
            .warnings
            .iter()
            .any(|w| w.site == StepSite::Face(2)),
        "the second floor steps too"
    );

    let dir = fresh_cache_dir("texel_step_replay");
    let cache = StageCache::new(&dir).expect("create replay cache");
    let selection = EntityShadowLightsSection {
        light_indices: vec![0],
    };
    let warm = |run: Run, config: &LightmapConfig| {
        bake(
            geometry.clone(),
            &lights,
            config,
            Run {
                cache: Some(&cache),
                ..run
            },
        )
    };
    let once = |baked: &Baked, body: &str| {
        assert_eq!(baked.logged(Level::Info, body), 1, "{body}");
    };

    // The cache-writing build is the cold build.
    let written = warm(Run::default(), &off);
    assert_eq!(written.warnings, cold.warnings);
    assert_eq!(written.section, cold.section);

    // P2: every memo hits.
    cache.clear_test_accesses();
    let hit = warm(Run::default(), &off);
    once(&hit, "[cache] lightmap_section hit");
    once(&hit, "[cache] shadowmask_atlas hit");
    assert_eq!(cache.test_access("lightmap_layer").read_attempts, 0);
    assert_eq!(hit.warnings, cold.warnings, "a memo hit replays the report");
    assert_eq!(hit.report, cold.report);

    // P11: the encode setting re-keys only the section memo.
    let uncompressed = LightmapConfig {
        uncompressed_irradiance: true,
        ..off
    };
    cache.clear_test_accesses();
    let layers_only = warm(Run::default(), &uncompressed);
    once(&layers_only, "[cache] lightmap_section miss");
    let layers = cache.test_access("lightmap_layer");
    assert!(layers.read_hits > 0);
    assert_eq!(
        layers.read_hits, layers.read_attempts,
        "every partition hits"
    );
    assert_eq!(layers.writes, 0, "no partition re-bakes");
    assert_eq!(
        layers_only.warnings, cold.warnings,
        "loaded partitions report"
    );

    // P12: a promotion re-keys only the shadowmask memo.
    let promoted = Run {
        selection: Some(&selection),
        ..Run::default()
    };
    cache.clear_test_accesses();
    let shadowmask_only = warm(promoted, &off);
    once(&shadowmask_only, "[cache] lightmap_section hit");
    once(&shadowmask_only, "[cache] shadowmask_atlas miss");
    assert!(
        cache.test_access("lightmap_layer").read_hits > 0,
        "the shadowmask pass reads partitions"
    );
    assert_eq!(
        shadowmask_only.warnings, cold.warnings,
        "the shadowmask pass runs no detector"
    );

    // P15: a threshold above every difference misses the section memo and
    // reports nothing; one below the soft penumbra's steps reports more.
    for threshold in [1.0, 0.2] {
        cache.clear_test_accesses();
        let changed = warm(
            Run {
                threshold: Some(threshold),
                ..Run::default()
            },
            &off,
        );
        once(&changed, "[cache] lightmap_section miss");
        let layers = cache.test_access("lightmap_layer");
        assert_eq!(layers.read_hits, layers.read_attempts);
        assert_eq!(changed.section, cold.section, "the threshold moves no byte");
        let uncached = bake(
            geometry.clone(),
            &lights,
            &off,
            Run {
                threshold: Some(threshold),
                ..Run::default()
            },
        );
        assert_eq!(changed.report, uncached.report);
        assert_eq!(changed.warnings, uncached.warnings);
        if threshold == 1.0 {
            assert!(changed.warnings.is_empty(), "{:?}", changed.warnings);
        } else {
            let pairs = |report: &TexelStepReport| {
                report
                    .warnings
                    .iter()
                    .map(|w| u64::from(w.pairs))
                    .sum::<u64>()
            };
            assert!(pairs(&changed.report) > pairs(&cold.report));
        }
    }
    drop(cache);
    let _ = std::fs::remove_dir_all(&dir);
}

// --- AC 15 (P9): the same warnings in the same order at any worker count ----

#[test]
fn texel_step_warnings_keep_their_order_across_worker_counts() {
    let (geometry, lights) = stepped_scene();
    let off = floor_config(false);
    let at = |permits: usize| {
        bake(
            geometry.clone(),
            &lights,
            &off,
            Run {
                permits,
                ..Run::default()
            },
        )
    };
    let one = at(1);
    assert!(one.warnings.len() >= 3);
    for permits in [4, 1, 8] {
        let other = at(permits);
        assert_eq!(other.warnings, one.warnings, "{permits} worker(s)");
        assert_eq!(other.report, one.report);
    }
    // Rayon pools of one and several threads run the detector on different
    // workers in a different order; the report does not move.
    for threads in [1, 4] {
        let report = ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .expect("build detector pool")
            .install(|| {
                bake(
                    geometry.clone(),
                    &lights,
                    &off,
                    Run {
                        permits: threads,
                        ..Run::default()
                    },
                )
                .report
            });
        assert_eq!(report, one.report, "{threads}-thread pool");
    }
    // Warnings come in light order, then face order.
    let order: Vec<(u32, u32)> = one
        .report
        .warnings
        .iter()
        .map(|w| match w.site {
            StepSite::Face(face) => (w.light, face),
            StepSite::CutFace { first_face, .. } => (w.light, first_face),
        })
        .collect();
    let mut sorted = order.clone();
    sorted.sort_unstable();
    assert_eq!(order, sorted);
}

// --- AC 20 (P8): no false warnings with the floor on -----------------------

/// A floor whose CSG cut leaves a diagonal edge at a prism standing on it:
/// the floor's chart rectangle reaches past its polygon, under the prism,
/// where texels store 0 beside lit texels at the prism's foot.
fn csg_cut_prism_geometry() -> GeometryResult {
    let v = |x: f32, y: f32, z: f32| Vec3::new(x, y, z);
    let mut geometry = empty_geometry();
    push_polygon(
        &mut geometry,
        &[
            v(0.0, 0.0, 0.0),
            v(6.0, 0.0, 0.0),
            v(6.0, 0.0, 3.1),
            v(3.1, 0.0, 6.0),
            v(0.0, 0.0, 6.0),
        ],
        Vec3::Y,
    );
    let inward = Vec3::new(-1.0, 0.0, -1.0).normalize();
    push_polygon(
        &mut geometry,
        &[
            v(6.0, 0.0, 3.1),
            v(3.1, 0.0, 6.0),
            v(3.1, 1.0, 6.0),
            v(6.0, 1.0, 3.1),
        ],
        inward,
    );
    push_polygon(
        &mut geometry,
        &[v(6.0, 1.0, 3.1), v(3.1, 1.0, 6.0), v(6.0, 1.0, 6.0)],
        Vec3::Y,
    );
    push_polygon(
        &mut geometry,
        &[
            v(6.0, 0.0, 3.1),
            v(6.0, 0.0, 6.0),
            v(6.0, 1.0, 6.0),
            v(6.0, 1.0, 3.1),
        ],
        Vec3::X,
    );
    push_polygon(
        &mut geometry,
        &[
            v(6.0, 0.0, 6.0),
            v(3.1, 0.0, 6.0),
            v(3.1, 1.0, 6.0),
            v(6.0, 1.0, 6.0),
        ],
        Vec3::Z,
    );
    geometry
}

/// A box resting on an uncut floor: texels under it lie inside its polygon
/// and inside the solid.
fn uncut_box_geometry() -> GeometryResult {
    let mut geometry = floor(6.0);
    push_box(
        &mut geometry,
        Vec3::new(2.5, 0.0, 2.5),
        Vec3::new(3.5, 1.0, 3.5),
    );
    geometry
}

/// A wall 5 cm thick, thinner than a texel, standing across an uncut floor.
fn thin_wall_geometry() -> GeometryResult {
    let mut geometry = floor(6.0);
    push_box(
        &mut geometry,
        Vec3::new(2.95, 0.0, 0.0),
        Vec3::new(3.0, 1.0, 6.0),
    );
    geometry
}

// The trap these fixtures set is real: with no surface test, the floor's
// stored grid steps 0/1 at the solid's foot, floor on or off.
#[test]
fn texels_under_a_resting_solid_step_against_its_lit_foot_without_the_surface_test() {
    for (name, geometry, light) in [
        (
            "csg cut",
            csg_cut_prism_geometry(),
            hard([2.5, 2.5, 2.5], 10.0),
        ),
        (
            "uncut box",
            uncut_box_geometry(),
            hard([1.0, 3.0, 1.2], 10.0),
        ),
        (
            "thin wall",
            thin_wall_geometry(),
            hard([1.5, 2.0, 3.1], 10.0),
        ),
    ] {
        for floored in [true, false] {
            let grid = stored_grid(
                geometry.clone(),
                std::slice::from_ref(&light),
                0,
                0,
                floored,
            );
            let steps = texel_steps(&grid, TEXEL_STEP_THRESHOLD);
            assert!(
                steps.pairs > 0,
                "{name}, floor {floored}: the bare grid must step at the foot"
            );
        }
    }
}

// P8: with the floor on, no warning where a shadow edge meets a chart border
// or texels with no record, at an N·L terminator over a convex edge, at a
// spot cone's outer edge, at a light's reach limit, at an inside corner, at a
// wall thinner than the footprint, under a solid resting on a CSG-cut floor
// or an uncut one; nor for a light with no shadow edge. Each scene that casts
// a hard shadow across open floor warns with the floor disabled, so its edges
// are real.
#[test]
fn the_floored_stage_warns_about_no_border_terminator_cone_reach_corner_or_wall() {
    let mut spot = hard([3.0, 2.0, 3.0], 8.0);
    spot.light_type = LightType::Spot;
    spot.cone_direction = Some([0.0, -1.0, 0.0]);
    spot.cone_angle_inner = Some(0.3);
    spot.cone_angle_outer = Some(0.5);

    // A contact plate hanging over the floor's edge: its shadow runs into
    // the chart border and the padding beyond.
    let mut border = floor(6.0);
    push_quad(&mut border, -1.0, 2.0, 2.0, PLATE_HEIGHT, 0);
    // A block whose top and sides meet at convex edges, lit low and from one
    // side so a side is back-facing and the block casts a long shadow.
    let mut block = floor(6.0);
    push_box(
        &mut block,
        Vec3::new(2.0, 0.0, 2.0),
        Vec3::new(3.0, 1.0, 3.0),
    );
    // A raised occluder whose shadow crosses the spot cone's outer edge.
    let mut cone = floor(6.0);
    push_quad(&mut cone, 3.2, 2.6, 0.8, 1.0, 0);

    let scenes: Vec<(&str, GeometryResult, Vec<MapLight>, bool)> = vec![
        ("border", border, vec![hard([0.8, 2.0, 3.1], 8.0)], true),
        ("terminator", block, vec![hard([0.5, 1.2, 2.4], 8.0)], true),
        ("spot cone", cone, vec![spot], true),
        (
            "reach limit",
            contact_geometry(),
            vec![hard([3.0, 0.8, 3.0], 2.0)],
            true,
        ),
        (
            "inside corner",
            uncut_box_geometry(),
            vec![hard([1.0, 3.0, 1.2], 10.0)],
            true,
        ),
        (
            "thin wall",
            thin_wall_geometry(),
            vec![hard([1.5, 2.0, 3.1], 10.0)],
            true,
        ),
        (
            "csg cut",
            csg_cut_prism_geometry(),
            vec![hard([2.5, 2.5, 2.5], 10.0)],
            false,
        ),
        (
            "open floor",
            floor(6.0),
            vec![hard([3.0, 2.0, 3.0], 8.0)],
            false,
        ),
    ];
    for (name, geometry, lights, casts_a_shadow) in scenes {
        for samples in [4, DEFAULT_AREA_SAMPLE_COUNT] {
            let run = Run {
                samples,
                ..Run::default()
            };
            let on = bake(geometry.clone(), &lights, &floor_config(true), run);
            assert!(
                on.warnings.is_empty(),
                "{name}, {samples} samples: {:?}",
                on.warnings
            );
            let off = bake(geometry.clone(), &lights, &floor_config(false), run);
            assert_eq!(
                !off.warnings.is_empty(),
                casts_a_shadow,
                "{name}, {samples} samples, floor off: {:?}",
                off.warnings
            );
        }
    }
}

// With the floor disabled, the wall and cut fixtures still warn about the
// shadows they cast across open floor, never about their feet.
#[test]
fn a_resting_solid_warns_only_for_the_shadow_it_casts_on_open_floor() {
    // The light stands in front of the prism's wall: it casts no shadow on
    // the floor, so its foot is the only 0/1 boundary and nothing warns.
    let prism = bake(
        csg_cut_prism_geometry(),
        &[hard([2.5, 2.5, 2.5], 10.0)],
        &floor_config(false),
        Run::default(),
    );
    assert!(prism.warnings.is_empty(), "{:?}", prism.warnings);

    // The thin wall shades the floor behind it out to x = 4.5: that far edge
    // steps; the wall's own foot, at x = 3, does not.
    let wall = bake(
        thin_wall_geometry(),
        &[hard([1.5, 2.0, 3.1], 10.0)],
        &floor_config(false),
        Run::default(),
    );
    assert_eq!(wall.report.warnings.len(), 1, "{:?}", wall.warnings);
    let step = wall.report.warnings[0];
    assert_eq!(step.site, StepSite::Face(0));
    assert!(
        step.position[0] > 4.0,
        "the earliest stepped texel lies on the far edge, at x {}",
        step.position[0]
    );
}

// --- AC 6 (P13): silent at the minimum sample count ------------------------

// At four soft-shadow samples the floored stage is silent over the contact
// plate and a strip thinner than half a texel; with the floor disabled both
// step.
#[test]
fn the_floored_stage_is_silent_at_the_minimum_sample_count() {
    let mut strip = floor(6.0);
    push_quad(&mut strip, 1.0, 3.08, 0.1, PLATE_HEIGHT, 0);
    // `push_quad` makes squares; stretch this one into a 4 m × 0.1 m strip
    // over one row of texel centres (z = 3.125).
    let first = strip.geometry.vertices.len() - 4;
    for vertex in &mut strip.geometry.vertices[first..] {
        if vertex.position[0] > 1.05 {
            vertex.position[0] = 5.0;
        }
    }
    let lights = [hard([3.1, 2.5, 3.6], 8.0)];
    for (name, geometry) in [("contact", contact_geometry()), ("thin strip", strip)] {
        let run = Run {
            samples: 4,
            ..Run::default()
        };
        let on = bake(geometry.clone(), &lights, &floor_config(true), run);
        assert!(on.warnings.is_empty(), "{name}: {:?}", on.warnings);
        let off = bake(geometry, &lights, &floor_config(false), run);
        assert_eq!(off.warnings.len(), 1, "{name}: {:?}", off.warnings);
    }
}
