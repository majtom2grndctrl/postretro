//! The texel-step detector through the production weight-map stage: warnings
//! on the caller's thread, cut faces, cache replay, the switch's and the
//! threshold's keys, worker counts, and no false warnings with the floor on.
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use glam::{DVec3, Vec3};
use log::Level;
use postretro_level_format::geometry::{FaceMeta, GeometrySection, Vertex};
use postretro_level_format::texture_names::TextureNamesSection;
use postretro_test_log_capture::{CapturedRecord, LogCapture};
use rayon::ThreadPoolBuilder;

use super::*;
use crate::animated_light_chunks::build_placed_animated_light_chunks;
use crate::animated_light_weight_maps::WeightMapInputs;
use crate::bvh_build::build_bvh;
use crate::cache::StageCache;
use crate::geometry::{FaceIndexRange, GeometryResult};
use crate::governor::Governor;
use crate::light_namespaces::{AnimatedBakedLights, StaticBakedLights};
use crate::lightmap_bake::{BlockOrdering, DEFAULT_AREA_SAMPLE_COUNT};
use crate::map_data::{
    DEFAULT_LIGHT_SIZE, FalloffModel, LightAnimation, LightType, MapLight, ShadowType,
};
use crate::reporter::StageProgress;
use crate::texel_step_detector::{StepSite, TEXEL_STEP_THRESHOLD};

/// Lightmap density of every fixture: 0.25 m texels.
const DENSITY: f32 = 0.25;

/// Height of a contact plate above the floor: above the 1 mm ray lift and
/// the 1 mm a shadow ray skips, so the separation and clip segments pass
/// beneath it.
const PLATE_HEIGHT: f32 = 0.005;

/// A pool edge that cuts a 24 m floor at 0.25 m texels 2 × 2.
const CUT_POOL_EDGE: u32 = 64;

fn fresh_cache_dir(label: &str) -> std::path::PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "postretro_weight_map_stage_cache_{label}_{}_{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed),
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

// --- geometry ---------------------------------------------------------------

fn empty_geometry() -> GeometryResult {
    GeometryResult {
        geometry: GeometrySection {
            vertices: Vec::new(),
            indices: Vec::new(),
            faces: Vec::new(),
        },
        texture_names: TextureNamesSection { names: Vec::new() },
        face_index_ranges: Vec::new(),
    }
}

/// Append a convex polygon as one fan-triangulated face.
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

/// An up-facing square `size` a side at height `y`, its corner at `(x, z)`.
fn push_square(geometry: &mut GeometryResult, x: f32, z: f32, size: f32, y: f32) {
    let v = |dx: f32, dz: f32| Vec3::new(x + dx * size, y, z + dz * size);
    push_polygon(
        geometry,
        &[v(0.0, 0.0), v(1.0, 0.0), v(1.0, 1.0), v(0.0, 1.0)],
        Vec3::Y,
    );
}

/// A solid box resting on whatever lies under it: its top and four sides.
fn push_box(geometry: &mut GeometryResult, a: Vec3, b: Vec3) {
    let v = Vec3::new;
    let faces = [
        (
            [
                v(a.x, b.y, a.z),
                v(b.x, b.y, a.z),
                v(b.x, b.y, b.z),
                v(a.x, b.y, b.z),
            ],
            Vec3::Y,
        ),
        (
            [
                v(a.x, a.y, a.z),
                v(a.x, a.y, b.z),
                v(a.x, b.y, b.z),
                v(a.x, b.y, a.z),
            ],
            Vec3::NEG_X,
        ),
        (
            [
                v(b.x, a.y, b.z),
                v(b.x, a.y, a.z),
                v(b.x, b.y, a.z),
                v(b.x, b.y, b.z),
            ],
            Vec3::X,
        ),
        (
            [
                v(b.x, a.y, a.z),
                v(a.x, a.y, a.z),
                v(a.x, b.y, a.z),
                v(b.x, b.y, a.z),
            ],
            Vec3::NEG_Z,
        ),
        (
            [
                v(a.x, a.y, b.z),
                v(b.x, a.y, b.z),
                v(b.x, b.y, b.z),
                v(a.x, b.y, b.z),
            ],
            Vec3::Z,
        ),
    ];
    for (corners, normal) in faces {
        push_polygon(geometry, &corners, normal);
    }
}

fn floor(size: f32) -> GeometryResult {
    let mut geometry = empty_geometry();
    push_square(&mut geometry, 0.0, 0.0, size, 0.0);
    geometry
}

/// A 6 m floor, face 0, under a 2 m plate resting just above it, face 1.
fn contact_geometry() -> GeometryResult {
    let mut geometry = floor(6.0);
    push_square(&mut geometry, 2.0, 2.0, 2.0, PLATE_HEIGHT);
    geometry
}

/// A box resting on an uncut floor: an inside corner at its foot, and texels
/// under it inside the solid.
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

// --- lights -----------------------------------------------------------------

/// A hard animated point light.
fn animated(origin: [f64; 3], range: f32) -> MapLight {
    MapLight {
        origin: DVec3::from_array(origin),
        carrier: String::new(),
        light_type: LightType::Point,
        intensity: 1.0,
        color: [1.0, 0.9, 0.8],
        falloff_model: FalloffModel::Linear,
        falloff_range: range,
        light_size: 0.0,
        angular_diameter: 0.0,
        cone_angle_inner: None,
        cone_angle_outer: None,
        cone_direction: None,
        animation: Some(LightAnimation {
            period: 1.0,
            phase: 0.0,
            brightness: Some(vec![1.0, 0.5]),
            color: None,
            direction: None,
            start_active: true,
        }),
        bake_only: false,
        is_dynamic: false,
        casts_entity_shadows: false,
        is_animated: false,
        tags: Vec::new(),
        shadow_type: ShadowType::StaticLightMap,
    }
}

/// The static light every fixture adds last, so atlas preparation places
/// charts for the weight maps to sit on. It is not in the animated list, so
/// animated light indices follow the fixture's own order.
fn placing_light() -> MapLight {
    MapLight {
        animation: None,
        ..animated([0.5, 8.0, 0.5], 1.0)
    }
}

fn with_placing_light(mut lights: Vec<MapLight>) -> Vec<MapLight> {
    lights.push(placing_light());
    lights
}

// --- the stage --------------------------------------------------------------

/// How one weight-map stage run goes. It runs on the calling thread, so log
/// capture reads its warnings; `permits` is the `-j` worker count.
#[derive(Clone, Copy)]
struct Run<'a> {
    cache: Option<&'a StageCache>,
    permits: usize,
    floor: bool,
    threshold: f32,
    samples: u32,
    /// A small pool edge cuts large faces; `None` keeps the production edge.
    pool_edge: Option<u32>,
}

impl Default for Run<'_> {
    fn default() -> Self {
        Self {
            cache: None,
            permits: 1,
            floor: true,
            threshold: TEXEL_STEP_THRESHOLD,
            samples: 4,
            pool_edge: None,
        }
    }
}

fn floor_off() -> Run<'static> {
    Run {
        floor: false,
        ..Run::default()
    }
}

struct Compiled {
    section: Vec<u8>,
    report: TexelStepReport,
    /// Every texel-step warning logged, in order.
    warnings: Vec<String>,
    records: Vec<CapturedRecord>,
}

impl Compiled {
    fn logged(&self, level: Level, body: &str) -> usize {
        self.records
            .iter()
            .filter(|record| record.level == level && record.message.contains(body))
            .count()
    }
}

fn compile(mut geometry: GeometryResult, lights: &[MapLight], run: Run) -> Compiled {
    let static_lights = StaticBakedLights::from_lights(lights);
    let animated_lights = AnimatedBakedLights::from_lights(lights);
    let unrestricted = BakeControl::unrestricted();
    let prepared = match run.pool_edge {
        Some(edge) => crate::lightmap_bake::prepare_atlas_within(
            &mut geometry,
            &static_lights,
            DENSITY,
            &[],
            BlockOrdering::by_cell_id(2),
            edge,
            &unrestricted,
        ),
        None => crate::lightmap_bake::prepare_atlas(&mut geometry, &static_lights, DENSITY, &[]),
    }
    .expect("fixture atlas prepares");
    let (bvh, primitives, bvh_section) = build_bvh(&geometry).expect("fixture BVH builds");
    let (chunks, _) = build_placed_animated_light_chunks(
        &bvh_section,
        &animated_lights,
        &prepared.charts,
        &prepared.placements,
        &geometry.face_index_ranges,
        DENSITY,
    );
    assert!(!chunks.chunks.is_empty(), "the fixture bakes weight maps");
    let (animated_list, _) = animated_lights.to_parallel_vecs();
    let inputs = WeightMapInputs {
        bvh: &bvh,
        primitives: &primitives,
        geometry: &geometry,
        chunk_section: &chunks,
        lights: &animated_list,
        face_charts: &prepared.charts,
        face_placements: &prepared.placements,
        layout: &prepared.layout,
        atlas_width: prepared.atlas_width,
        atlas_height: prepared.atlas_height,
        static_atlas_layer_count: prepared.layer_count,
        area_sample_count: run.samples,
        cut_parents: &prepared.cut_parents,
        penumbra_floor: run.floor,
        step_threshold: run.threshold,
    };
    let progress = StageProgress::indeterminate();
    let control = BakeControl::new(Arc::new(Governor::new(run.permits, false)), &progress);
    let logs = LogCapture::start();
    let bake = bake_or_load_weight_maps(&inputs, DENSITY, run.cache, &control)
        .expect("a stepped bake still compiles");
    let records = logs.records();
    let warnings = records
        .iter()
        .filter(|record| record.level == Level::Warn && record.message.contains("texel step"))
        .map(|record| record.message.clone())
        .collect();
    Compiled {
        section: bake.section.to_bytes(),
        report: bake.texel_steps,
        warnings,
        records,
    }
}

// --- AC 18: the weight-map stage reports to the caller (P1, P17) -------------

// P1: a hard animated light's contact edge steps with the floor disabled; the
// weight-map stage warns once, on the caller's thread, naming the light and
// the face. With the floor on, nothing warns and the compile succeeds.
#[test]
fn a_hard_animated_contact_edge_warns_from_the_weight_map_stage_only_with_the_floor_disabled() {
    let lights = with_placing_light(vec![animated([3.3, 3.0, 2.7], 8.0)]);
    let off = compile(contact_geometry(), &lights, floor_off());
    assert_eq!(off.warnings.len(), 1, "{:?}", off.warnings);
    let warning = &off.warnings[0];
    assert!(
        warning.starts_with(
            "[Lightmap] texel step: animated light #0 at (3.30, 3.00, 2.70) on face 0 near ("
        ),
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
    let [x, y, z] = step.position;
    assert!(y.abs() < 1.0e-3, "{y}");
    assert!(
        (1.5..=4.5).contains(&x) && (1.5..=4.5).contains(&z),
        "{x}, {z}"
    );

    let on = compile(contact_geometry(), &lights, Run::default());
    assert!(on.warnings.is_empty(), "{:?}", on.warnings);
    assert_eq!(on.report, TexelStepReport::default());
}

/// A 24 m floor that a 64-texel pool edge cuts 2 × 2 at 12 m, under a contact
/// plate straddling both cut lines.
fn cut_contact_geometry() -> GeometryResult {
    let mut geometry = floor(24.0);
    push_square(&mut geometry, 11.0, 11.0, 2.0, PLATE_HEIGHT);
    geometry
}

// P17: animated faces are cut like any other. A contact edge crossing the cut
// warns once, naming the pre-cut face, and counts exactly the pairs the uncut
// face counts: every pair straddling a cut line is checked, none twice.
#[test]
fn a_stepped_animated_edge_across_a_face_cut_warns_once_for_the_parent_face() {
    let lights = with_placing_light(vec![animated([12.3, 3.0, 11.7], 40.0)]);
    let cut_run = Run {
        pool_edge: Some(CUT_POOL_EDGE),
        ..floor_off()
    };
    let cut = compile(cut_contact_geometry(), &lights, cut_run);
    let uncut = compile(cut_contact_geometry(), &lights, floor_off());
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

    let on = compile(
        cut_contact_geometry(),
        &lights,
        Run {
            floor: true,
            ..cut_run
        },
    );
    assert!(on.warnings.is_empty(), "{:?}", on.warnings);
}

// --- AC 19, 14, 15: cache replay, keys, workers --------------------------

/// Two floors, each under a raised occluder, lit by hard point and spot
/// lights and a soft light; floor disabled, several (light, face) pairs step.
fn stepped_scene() -> (GeometryResult, Vec<MapLight>) {
    let mut geometry = floor(6.0);
    push_square(&mut geometry, 2.0, 2.0, 2.0, 1.0);
    push_square(&mut geometry, 10.0, 0.0, 6.0, 0.0);
    push_square(&mut geometry, 12.0, 2.0, 2.0, 1.0);
    let point = animated([3.3, 3.0, 2.7], 6.0);
    let mut soft = animated([13.0, 2.5, 3.4], 6.0);
    soft.light_size = DEFAULT_LIGHT_SIZE;
    let mut spot = animated([12.8, 3.5, 3.1], 8.0);
    spot.light_type = LightType::Spot;
    spot.cone_direction = Some([0.0, -1.0, 0.0]);
    spot.cone_angle_inner = Some(1.2);
    spot.cone_angle_outer = Some(1.4);
    (geometry, with_placing_light(vec![point, soft, spot]))
}

fn pairs(report: &TexelStepReport) -> u64 {
    report.warnings.iter().map(|w| u64::from(w.pairs)).sum()
}

// AC 19 (P15): a warm compile whose weight-map entry hits reports exactly the
// cold compile's warnings, each once. A changed threshold misses the entry
// and the warnings follow it; the old threshold then hits its own entry.
#[test]
fn a_weight_map_cache_hit_replays_the_cold_warnings_and_a_threshold_change_misses() {
    let (geometry, lights) = stepped_scene();
    let cold = compile(geometry.clone(), &lights, floor_off());
    assert!(
        cold.report.warnings.len() >= 2,
        "the scene steps for several (light, face): {:?}",
        cold.warnings
    );
    let mut lit: Vec<u32> = cold.report.warnings.iter().map(|w| w.light).collect();
    lit.dedup();
    assert!(lit.len() >= 2, "more than one light steps: {lit:?}");

    let dir = fresh_cache_dir("replay");
    let cache = StageCache::new(&dir).expect("create replay cache");
    let warm = |threshold: f32| {
        compile(
            geometry.clone(),
            &lights,
            Run {
                cache: Some(&cache),
                threshold,
                ..floor_off()
            },
        )
    };
    let written = warm(TEXEL_STEP_THRESHOLD);
    assert_eq!(
        written.logged(Level::Info, "[cache] animated_lm_weight_maps miss"),
        1
    );
    assert_eq!(written.warnings, cold.warnings);
    assert_eq!(written.section, cold.section);

    let hit = warm(TEXEL_STEP_THRESHOLD);
    assert_eq!(
        hit.logged(Level::Info, "[cache] animated_lm_weight_maps hit"),
        1
    );
    assert_eq!(hit.warnings, cold.warnings, "a hit replays the report once");
    assert_eq!(hit.report, cold.report);
    assert_eq!(hit.section, cold.section);

    for threshold in [1.0, 0.2] {
        let changed = warm(threshold);
        assert_eq!(
            changed.logged(Level::Info, "[cache] animated_lm_weight_maps miss"),
            1,
            "threshold {threshold}"
        );
        assert_eq!(changed.section, cold.section, "the threshold moves no byte");
        let uncached = compile(
            geometry.clone(),
            &lights,
            Run {
                threshold,
                ..floor_off()
            },
        );
        assert_eq!(changed.report, uncached.report);
        assert_eq!(changed.warnings, uncached.warnings);
        if threshold == 1.0 {
            assert!(changed.warnings.is_empty(), "{:?}", changed.warnings);
        } else {
            assert!(pairs(&changed.report) > pairs(&cold.report));
        }
    }
    let back = warm(TEXEL_STEP_THRESHOLD);
    assert_eq!(
        back.logged(Level::Info, "[cache] animated_lm_weight_maps hit"),
        1
    );
    assert_eq!(back.warnings, cold.warnings);
    drop(cache);
    let _ = std::fs::remove_dir_all(&dir);
}

// AC 14 (P6): toggling the floor misses the weight-map entry, and a warm
// cache written at the other setting is never served: each build equals a
// cold build at its own setting.
#[test]
fn toggling_the_floor_misses_the_weight_map_entry_and_never_serves_the_other_setting() {
    let (geometry, lights) = stepped_scene();
    let cold_on = compile(geometry.clone(), &lights, Run::default());
    let cold_off = compile(geometry.clone(), &lights, floor_off());
    assert_ne!(cold_on.section, cold_off.section, "the floor moves bytes");

    let dir = fresh_cache_dir("toggle");
    let cache = StageCache::new(&dir).expect("create toggle cache");
    let warm = |floor: bool| {
        compile(
            geometry.clone(),
            &lights,
            Run {
                cache: Some(&cache),
                floor,
                ..Run::default()
            },
        )
    };
    // The first build at each setting writes its entry; the second reads it.
    for (floor, cold, outcome) in [
        (true, &cold_on, "miss"),
        (false, &cold_off, "miss"),
        (true, &cold_on, "hit"),
        (false, &cold_off, "hit"),
    ] {
        let built = warm(floor);
        let body = format!("[cache] animated_lm_weight_maps {outcome}");
        assert_eq!(built.logged(Level::Info, &body), 1, "floor {floor}: {body}");
        assert_eq!(built.section, cold.section, "floor {floor}");
        assert_eq!(built.warnings, cold.warnings, "floor {floor}");
    }
    drop(cache);
    let _ = std::fs::remove_dir_all(&dir);
}

// AC 15 (P9): weight maps, and with the floor off their warnings and order,
// are byte-identical at one worker and several, and on Rayon pools of one
// and several threads.
#[test]
fn weight_maps_and_their_warnings_match_across_worker_counts() {
    let (geometry, lights) = stepped_scene();
    for floor in [false, true] {
        let at = |permits: usize| {
            compile(
                geometry.clone(),
                &lights,
                Run {
                    permits,
                    floor,
                    samples: DEFAULT_AREA_SAMPLE_COUNT,
                    ..Run::default()
                },
            )
        };
        let one = at(1);
        assert_eq!(one.warnings.is_empty(), floor, "{:?}", one.warnings);
        for permits in [4, 1, 8] {
            let other = at(permits);
            assert_eq!(
                other.section, one.section,
                "floor {floor}, {permits} worker(s)"
            );
            assert_eq!(
                other.warnings, one.warnings,
                "floor {floor}, {permits} worker(s)"
            );
            assert_eq!(other.report, one.report);
        }
        for threads in [1, 4] {
            let (section, report) = ThreadPoolBuilder::new()
                .num_threads(threads)
                .build()
                .expect("build bake pool")
                .install(|| {
                    let compiled = compile(
                        geometry.clone(),
                        &lights,
                        Run {
                            permits: threads,
                            floor,
                            samples: DEFAULT_AREA_SAMPLE_COUNT,
                            ..Run::default()
                        },
                    );
                    (compiled.section, compiled.report)
                });
            assert_eq!(section, one.section, "floor {floor}, {threads}-thread pool");
            assert_eq!(report, one.report, "floor {floor}, {threads}-thread pool");
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
}

// --- AC 20 (P8): no false warnings with the floor on ------------------------

// With the floor on, an animated light over a contact edge, an inside corner,
// a wall thinner than the footprint and an open floor draws no warning, at
// the minimum and default sample counts. With the floor disabled, the scenes
// that cast a hard shadow across open floor warn, so their edges are real.
#[test]
fn the_floored_weight_map_stage_warns_about_no_contact_corner_wall_or_open_floor() {
    let scenes: Vec<(&str, GeometryResult, MapLight, bool)> = vec![
        (
            "contact",
            contact_geometry(),
            animated([3.3, 3.0, 2.7], 8.0),
            true,
        ),
        (
            "inside corner",
            uncut_box_geometry(),
            animated([1.0, 3.0, 1.2], 10.0),
            true,
        ),
        (
            "thin wall",
            thin_wall_geometry(),
            animated([1.5, 2.0, 3.1], 10.0),
            true,
        ),
        (
            "open floor",
            floor(6.0),
            animated([3.0, 2.0, 3.0], 8.0),
            false,
        ),
    ];
    for (name, geometry, light, casts_a_shadow) in scenes {
        let lights = with_placing_light(vec![light]);
        for samples in [4, DEFAULT_AREA_SAMPLE_COUNT] {
            let on = compile(
                geometry.clone(),
                &lights,
                Run {
                    samples,
                    ..Run::default()
                },
            );
            assert!(
                on.warnings.is_empty(),
                "{name}, {samples} samples: {:?}",
                on.warnings
            );
            let off = compile(
                geometry.clone(),
                &lights,
                Run {
                    samples,
                    ..floor_off()
                },
            );
            assert_eq!(
                !off.warnings.is_empty(),
                casts_a_shadow,
                "{name}, {samples} samples, floor off: {:?}",
                off.warnings
            );
        }
    }
}
