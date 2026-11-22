// Area-light texels: ratio-estimator visibility over the polygon lattice, the centre probe on a
// polygon, targets in the emitter sphere, memo independence, the window and the weight maps.
// See: context/lib/build_pipeline.md §Compiler pipeline (step 12, Area lights)

use std::cell::Cell;
use std::sync::Arc;

use glam::{DVec3, Vec3};

use super::test_fixtures::*;
use super::*;
use crate::animated_light_weight_maps::contribution_to_weight;
use crate::area_light::test_lights::{area_light, ceiling_panel, pillar_sides, wall_rect_x};
use crate::area_light::{EmitterPolygon, polygon_distance, polygon_irradiance, reach_window};
use crate::bake_control::BakeControl;
use crate::chart_raster::{chart_texel_seed, chart_texel_world_position};
use crate::lightmap_bake::{
    DEFAULT_AREA_SAMPLE_COUNT, light_contribution_and_direction, light_texel_is_covered,
};
use crate::lightmap_layer::{
    ExactSkips, SkipStats, bake_light_layer, chart_may_cover, reconstruct_light_texel,
};
use crate::map_data::{LightAnimation, MapLight};
use crate::texel_step_detector::VisibilityGrid;

// --- scenes ---------------------------------------------------------------

/// A down-facing ceiling quad over `x` × `z` at height `y`.
fn ceiling(scene: &mut SceneBuilder, x: [f32; 2], z: [f32; 2], y: f32) {
    scene.quad(
        [
            Vec3::new(x[0], y, z[0]),
            Vec3::new(x[1], y, z[0]),
            Vec3::new(x[1], y, z[1]),
            Vec3::new(x[0], y, z[1]),
        ],
        Vec3::NEG_Y,
    );
}

/// A wall quad in the plane `x`, facing −X, over `y` × `z`.
fn wall_facing_neg_x(scene: &mut SceneBuilder, x: f32, y: [f32; 2], z: [f32; 2]) {
    scene.quad(
        [
            Vec3::new(x, y[0], z[0]),
            Vec3::new(x, y[0], z[1]),
            Vec3::new(x, y[1], z[1]),
            Vec3::new(x, y[1], z[0]),
        ],
        Vec3::NEG_X,
    );
}

/// A floor receiver `width × height` texels from `(x0, z0)`.
fn receiver_at(x0: f32, z0: f32, width: u32, height: u32) -> Receiver {
    Receiver {
        x0,
        y: 0.0,
        z0,
        width,
        height,
        pitch: PITCH,
    }
}

/// A 1 × 1 m panel flush in a ceiling 2 m over a 1.6 m receiver, with a
/// block 0.3 to 0.5 m up over x < 0: open texels see the whole panel,
/// blocked ones none, and a penumbra runs between. The block is thick
/// enough that no face of it is a sliver the shaft cull refuses.
fn plated_flush_panel() -> (Fixture, MapLight) {
    let receiver = Receiver::centred(40, 40);
    let mut scene = SceneBuilder::default();
    receiver.add_floor(&mut scene);
    ceiling(&mut scene, [-3.0, 3.0], [-3.0, 3.0], 2.0);
    scene.aabb(Vec3::new(-3.0, 0.3, -3.0), Vec3::new(0.0, 0.5, 3.0));
    let light = area_light(
        vec![ceiling_panel([-0.5, 0.5], [-0.5, 0.5], 2.0)],
        1.0,
        None,
    );
    (Fixture::new(scene, receiver), light)
}

/// A 1 × 1 m panel flush in a ceiling 2 m over an open 0.4 m receiver.
fn open_flush_panel() -> (Fixture, MapLight) {
    let receiver = Receiver::centred(10, 10);
    let mut scene = SceneBuilder::default();
    receiver.add_floor(&mut scene);
    ceiling(&mut scene, [-3.0, 3.0], [-3.0, 3.0], 2.0);
    let light = area_light(
        vec![ceiling_panel([-0.5, 0.5], [-0.5, 0.5], 2.0)],
        1.0,
        None,
    );
    (Fixture::new(scene, receiver), light)
}

/// A 0.5 m pillar wrapped on all four sides by one light, 0.5 to 1.5 m up,
/// its centroid on the pillar's axis inside it, and an open floor receiver
/// north of it that faces only the north side.
fn pillar_wrap() -> (Fixture, MapLight) {
    let receiver = receiver_at(-0.2, 0.6, 10, 10);
    let mut scene = SceneBuilder::default();
    receiver.add_floor(&mut scene);
    scene.aabb(Vec3::new(-0.25, 0.0, -0.25), Vec3::new(0.25, 2.0, 0.25));
    let light = area_light(pillar_sides(0.0, 0.0, 0.25, [0.5, 1.5]), 1.0, None);
    (Fixture::new(scene, receiver), light)
}

/// Every texel's stored visibility, row-major, `None` where unrecorded.
fn cells(grid: &VisibilityGrid) -> Vec<Option<f32>> {
    (0..grid.height())
        .flat_map(|y| (0..grid.width()).map(move |x| grid.get(x, y)))
        .collect()
}

/// The receiver texel's site as the walk builds it, every clip traced.
fn site_at(receiver: &Receiver, tx: i32, ty: i32) -> TexelSite {
    let chart = receiver.chart();
    TexelSite {
        world_p: chart_texel_world_position(&chart, tx, ty),
        normal: chart.normal,
        seed: chart_texel_seed(&chart, tx, ty),
        frame: TexelFrame::for_chart(&chart),
        may_clip: true,
    }
}

/// The stored lit value's red channel against `light`'s windowed polygon
/// term at the same texel, as a ratio.
fn lit_over_term(light: &MapLight, world_p: Vec3, visibility: f32) -> f64 {
    let (lit, _) = reconstruct_light_texel(light, world_p, Vec3::Y, visibility);
    let emitter = light.area.as_ref().expect("an area light");
    let (term, _) =
        emitter.windowed_irradiance(world_p.as_dvec3(), DVec3::Y, f64::from(light.falloff_range));
    assert!(term > 0.0, "the texel at {world_p} is lit");
    f64::from(lit.x) / (term * f64::from(light.intensity))
}

// --- visibility -----------------------------------------------------------

// AC 5: under a flush panel, open texels store exactly 1 and texels a plate
// hides store exactly 0, with a penumbra between and no NaN.
#[test]
fn a_flush_panel_stores_exactly_one_open_and_zero_occluded() {
    let (fixture, light) = plated_flush_panel();
    let grid = fixture.bake(&light, DEFAULT_AREA_SAMPLE_COUNT, true);
    assert_eq!(grid.get(0, 20), Some(0.0), "under the plate");
    assert_eq!(grid.get(39, 20), Some(1.0), "in the open");
    let stored: Vec<f32> = cells(&grid).into_iter().flatten().collect();
    assert!(
        stored
            .iter()
            .all(|v| v.is_finite() && (0.0..=1.0).contains(v))
    );
    let count = |test: fn(f32) -> bool| stored.iter().filter(|&&v| test(v)).count();
    let (zeros, ones, penumbra) = (
        count(|v| v == 0.0),
        count(|v| v == 1.0),
        count(|v| v > 0.0 && v < 1.0),
    );
    assert!(
        zeros > 200 && ones > 200 && penumbra > 0,
        "{zeros} zero, {ones} one, {penumbra} penumbra of {}",
        stored.len()
    );
}

// AC 21 (fixture): the plated panel's records are byte-identical with every
// exact skip on and off, verify mode checks every clear tile, and the shaft
// cull clears tiles under the ceiling the flush glass shares a plane with.
#[test]
fn a_flush_panel_bakes_identically_with_the_skips_on_and_clears_tiles() {
    let (fixture, light) = plated_flush_panel();
    let stats = SkipStats::leak();
    for count in [DEFAULT_AREA_SAMPLE_COUNT, 9] {
        let culled = fixture.bake_texels(&light, count, true, ExactSkips::ON.tallied(stats));
        let unculled = fixture.bake_texels(&light, count, true, ExactSkips::OFF);
        let bits = |texels: &[crate::lightmap_layer::LayerTexel]| {
            texels
                .iter()
                .map(|texel| (texel.idx, texel.raw_visibility.to_bits()))
                .collect::<Vec<_>>()
        };
        assert_eq!(bits(&culled), bits(&unculled), "{count} samples");
    }
    assert!(stats.tiles_cleared() > 0, "no tile cleared");
    assert!(
        stats.verified_texels() > 0,
        "verify mode traced the clear tiles"
    );
}

/// A 4 × 4 m panel flush in a ceiling 1 m up, and a thin plate just under
/// the glass over its near (x < 0) or far half, the receiver under the
/// panel's −x edge.
fn half_plated_panel(near: bool) -> (Fixture, MapLight) {
    let receiver = receiver_at(-2.2, -0.2, 10, 10);
    let mut scene = SceneBuilder::default();
    receiver.add_floor(&mut scene);
    ceiling(&mut scene, [-3.0, 3.0], [-3.0, 3.0], 1.0);
    let x = if near { [-2.5, 0.0] } else { [0.0, 2.5] };
    scene.aabb(Vec3::new(x[0], 0.998, -2.5), Vec3::new(x[1], 0.999, 2.5));
    let light = area_light(
        vec![ceiling_panel([-2.0, 2.0], [-2.0, 2.0], 1.0)],
        1.0,
        None,
    );
    (Fixture::new(scene, receiver), light)
}

// AC 6: occluding the near half of a large panel and occluding its far half
// store different visibility, each the integrand share of the half left
// open, though each half holds the same area, so an unweighted area
// fraction (0.5) ties them.
#[test]
fn near_and_far_half_occluders_are_ordered_by_integrand_weight() {
    let samples = 256;
    let near_plated = half_plated_panel(true);
    let far_plated = half_plated_panel(false);
    let (tx, ty) = (5, 5);
    let p = site_at(&near_plated.0.receiver, tx, ty).world_p.as_dvec3();
    let half = |x: [f64; 2]| ceiling_panel(x, [-2.0, 2.0], 1.0);
    let (near_half, far_half) = (half([-2.0, 0.0]), half([0.0, 2.0]));
    let term = |poly: &EmitterPolygon| polygon_irradiance(poly, p, DVec3::Y).0;
    let (near_term, far_term) = (term(&near_half), term(&far_half));
    let total = near_term + far_term;
    let stored = |(fixture, light): &(Fixture, MapLight)| {
        let grid = fixture.bake(light, samples, true);
        f64::from(grid.get(tx as u32, ty as u32).expect("the texel is lit"))
    };
    let (near_blocked, far_blocked) = (stored(&near_plated), stored(&far_plated));
    println!(
        "near half blocked {near_blocked:.4} (expected {:.4}), far half blocked \
         {far_blocked:.4} (expected {:.4})",
        far_term / total,
        near_term / total
    );
    assert!(
        near_blocked + 0.2 < far_blocked,
        "the near half carries more of the light"
    );
    assert!((near_blocked - far_term / total).abs() < 0.03);
    assert!((far_blocked - near_term / total).abs() < 0.03);
    // Plain area shares tie.
    assert_eq!(near_half.area, far_half.area);
}

// AC 9 (O19): a texel facing one emitting face, under a light wrapping a
// pillar (centroid inside it) and under a flush panel (centroid on the
// glass), stores exactly 1 with nothing between. With every segment but the
// centre probe's blocked, as by an occluder over every ring probe's sample
// but not the whole face, it stores the full set's weighted visibility,
// above 0, never the all-blocked early-out's 0.
#[test]
fn the_centre_probe_traces_a_polygon_sample_not_the_lights_origin() {
    for (name, (fixture, light)) in [
        ("pillar", pillar_wrap()),
        ("flush panel", open_flush_panel()),
    ] {
        let grid = fixture.bake(&light, DEFAULT_AREA_SAMPLE_COUNT, true);
        let stored: Vec<f32> = cells(&grid).into_iter().flatten().collect();
        assert_eq!(stored.len(), 100, "{name}: every texel is lit");
        assert!(stored.iter().all(|&v| v == 1.0), "{name}: {stored:?}");

        let site = site_at(&fixture.receiver, 5, 5);
        let footprint = footprint_for(&light, DEFAULT_AREA_SAMPLE_COUNT);
        let centre_origin = site.world_p + site.normal * RAY_EPSILON;
        let only_centre = |from: Vec3, _to: Vec3| from == centre_origin;
        let outcome = |early_out| {
            footprint_visibility(
                &light,
                &site,
                &footprint,
                only_centre,
                |_, _| true,
                |_, _| true,
                early_out,
            )
        };
        let (agreed, full) = (outcome(ProbeEarlyOut::Agreed), outcome(ProbeEarlyOut::Off));
        assert!(!agreed.early_out, "{name}: the clear centre escalates");
        assert!(agreed.visibility > 0.0, "{name}");
        assert_eq!(agreed.visibility, full.visibility, "{name}");
        // The single hard target is a lifted polygon sample, clear here, and
        // never the origin.
        assert_ne!(
            hard_ray_target(site.world_p, &light),
            light.origin.as_vec3(),
            "{name}"
        );
        assert!(!fixture.centre_ray_blocked(&light, 5, 5), "{name}");
    }
    // Aimed at the pillar light's origin, the centre ray would end in solid.
    let (fixture, light) = pillar_wrap();
    let site = site_at(&fixture.receiver, 5, 5);
    assert!(!crate::ray_traversal::segment_clear(
        &fixture.bvh,
        &fixture.primitives,
        &fixture.geometry,
        site.world_p + site.normal * RAY_EPSILON,
        light.origin.as_vec3(),
    ));
}

// AC 10 (O20): north of a light wrapping a pillar, with nothing between,
// every texel samples only the north side, stores exactly 1, and its lit
// value matches the unoccluded polygon term within 1%; the sides it is
// behind add nothing to that term.
#[test]
fn a_texel_north_of_a_wrapped_pillar_stores_one_and_the_polygon_term() {
    let (fixture, light) = pillar_wrap();
    let grid = fixture.bake(&light, DEFAULT_AREA_SAMPLE_COUNT, true);
    let chart = fixture.receiver.chart();
    let north = &light.area.as_ref().unwrap().polygons[0];
    let reach = f64::from(light.falloff_range);
    let mut checked = 0;
    for ty in 0..10 {
        for tx in 0..10 {
            let v = grid.get(tx, ty).expect("lit");
            assert_eq!(v, 1.0, "texel ({tx}, {ty})");
            let p = chart_texel_world_position(&chart, tx as i32, ty as i32);
            let ratio = lit_over_term(&light, p, v);
            assert!((ratio - 1.0).abs() < 0.01, "ratio {ratio}");
            let (e, _) = polygon_irradiance(north, p.as_dvec3(), DVec3::Y);
            let windowed = e * reach_window(polygon_distance(north, p.as_dvec3()), reach);
            let (whole, _) =
                light
                    .area
                    .as_ref()
                    .unwrap()
                    .windowed_irradiance(p.as_dvec3(), DVec3::Y, reach);
            assert!((whole - windowed).abs() <= 1.0e-12 * whole);
            checked += 1;
        }
    }
    assert_eq!(checked, 100);
}

// AC 17: a floor texel at the edge of a panel flush in the wall it meets
// stores visibility 1 and matches the unoccluded polygon term within 1%.
// The floor stands for the wall and the wall for the ceiling: only the
// flush edge matters.
#[test]
fn a_texel_at_a_flush_emitters_edge_stores_one() {
    let receiver = receiver_at(-0.4, -0.2, 10, 10);
    let mut scene = SceneBuilder::default();
    receiver.add_floor(&mut scene);
    wall_facing_neg_x(&mut scene, 0.0, [0.0, 2.0], [-1.0, 1.0]);
    let light = area_light(
        vec![wall_rect_x(0.0, [0.0, 0.5], [-0.25, 0.25], DVec3::NEG_X)],
        1.0,
        None,
    );
    let fixture = Fixture::new(scene, receiver);
    let grid = fixture.bake(&light, DEFAULT_AREA_SAMPLE_COUNT, true);
    let chart = receiver.chart();
    for ty in 0..10 {
        for tx in 0..10 {
            let v = grid.get(tx, ty).expect("lit");
            assert_eq!(v, 1.0, "texel ({tx}, {ty})");
            let p = chart_texel_world_position(&chart, tx as i32, ty as i32);
            let ratio = lit_over_term(&light, p, v);
            assert!((ratio - 1.0).abs() < 0.01, "texel ({tx}, {ty}): {ratio}");
        }
    }
    // The edge column lies half a texel from the glass's plane.
    let edge = chart_texel_world_position(&chart, 9, 5);
    assert!(edge.x > -PITCH && edge.x < 0.0);
}

// AC 18: every target the polygon lattice yields, the floor's centre probe
// target first among them, the centre sampler's and the single hard target,
// lies inside the light's emitter sphere grown by the 1 mm slack its readers
// add, for a 4 × 4 m ceiling panel, a 4 × 0.1 m strip and a light wrapping a
// pillar.
#[test]
fn every_area_target_lies_in_the_emitter_sphere() {
    let shapes = [
        (
            "4 x 4 panel",
            area_light(
                vec![ceiling_panel([-2.0, 2.0], [-2.0, 2.0], 3.0)],
                1.0,
                None,
            ),
        ),
        (
            "4 x 0.1 strip",
            area_light(
                vec![ceiling_panel([-2.0, 2.0], [-0.05, 0.05], 3.0)],
                1.0,
                None,
            ),
        ),
        (
            "pillar wrap",
            area_light(pillar_sides(0.0, 0.0, 0.25, [0.5, 1.5]), 1.0, None),
        ),
    ];
    let receivers = [
        Vec3::new(0.0, 0.0, 0.0),
        Vec3::new(1.7, 0.0, -1.9),
        Vec3::new(3.0, 0.0, 1.0),
        Vec3::new(0.0, 0.0, 0.8),
        Vec3::new(0.8, 0.0, 0.1),
        Vec3::new(-0.6, 0.0, -0.7),
    ];
    let frame = TexelFrame::new(Vec3::X * PITCH, Vec3::Z * PITCH);
    for (name, light) in &shapes {
        let (centre, radius) = light.emitter_sphere();
        let inside = |target: Vec3, what: &str| {
            let distance = target.as_dvec3().distance(centre);
            assert!(
                distance <= radius + 1.0e-3,
                "{name}: {what} target {target} lies {distance} from the centroid, past {radius}"
            );
        };
        let mut traced = 0;
        for (i, &world_p) in receivers.iter().enumerate() {
            let site = TexelSite {
                world_p,
                normal: Vec3::Y,
                seed: 0x9E37_79B9_7F4A_7C15_u64.wrapping_mul(i as u64 + 1),
                frame,
                may_clip: true,
            };
            if !light_texel_is_covered(light, world_p, Vec3::Y) {
                continue;
            }
            let footprint = footprint_for(light, DEFAULT_AREA_SAMPLE_COUNT);
            let targets = std::cell::RefCell::new(Vec::new());
            let record = |_from: Vec3, to: Vec3| {
                targets.borrow_mut().push(to);
                true
            };
            footprint_visibility(
                light,
                &site,
                &footprint,
                record,
                |_, _| true,
                |_, _| true,
                ProbeEarlyOut::Off,
            );
            let targets = targets.into_inner();
            assert_eq!(targets.len(), footprint.offsets.len(), "{name}");
            inside(targets[0], "centre probe");
            for &target in &targets {
                inside(target, "footprint");
            }
            // The centre sampler, made to escalate by alternating answers.
            let flip = Cell::new(false);
            let sampler = TexelSampler::new(light, DEFAULT_AREA_SAMPLE_COUNT, false, false);
            sampler.visibility(
                light,
                &site,
                |_, to| {
                    inside(to, "centre sampler");
                    flip.set(!flip.get());
                    flip.get()
                },
                |_, _| true,
                |_, _| true,
            );
            inside(hard_ray_target(world_p, light), "hard");
            traced += targets.len();
        }
        assert!(traced > 0, "{name}: no receiver was lit");
    }
}

// --- memo independence ----------------------------------------------------

/// A point light's footprint and probe subset at one count on this thread.
fn point_sets(point: &MapLight) -> (TexelFootprint, SoftProbes) {
    (
        (*footprint_for(point, DEFAULT_AREA_SAMPLE_COUNT)).clone(),
        SoftProbes::new(point, DEFAULT_AREA_SAMPLE_COUNT),
    )
}

/// Everything an area light's texel walk builds at the same count, and one
/// texel traced through each sampler.
fn build_area_sets(area: &MapLight) {
    let receiver = Receiver::centred(4, 4);
    let site = site_at(&receiver, 1, 1);
    for floor in [true, false] {
        let sampler = TexelSampler::new(area, DEFAULT_AREA_SAMPLE_COUNT, floor, false);
        let flip = Cell::new(false);
        sampler.visibility(
            area,
            &site,
            |_, _| {
                flip.set(!flip.get());
                flip.get()
            },
            |_, _| true,
            |_, _| true,
        );
    }
    let _ = SoftProbes::new(area, DEFAULT_AREA_SAMPLE_COUNT);
}

// AC 29 (O9): on one thread, a point light's footprint and probe subset are
// the same whether or not an area light's were built first at the same
// count, and an area light's build leaves the point light's memoized
// footprint in place.
#[test]
fn an_area_lights_lattice_leaves_the_point_light_memos_alone() {
    let point = default_point([0.0, 2.0, 0.0]);
    let area = area_light(
        vec![ceiling_panel([-0.5, 0.5], [-0.5, 0.5], 2.0)],
        1.0,
        None,
    );
    let on_thread = |work: Box<dyn FnOnce() -> (TexelFootprint, SoftProbes) + Send>| {
        std::thread::spawn(work)
            .join()
            .expect("the thread finishes")
    };
    let reference = on_thread(Box::new({
        let point = point.clone();
        move || point_sets(&point)
    }));
    let area_first = on_thread(Box::new({
        let (point, area) = (point.clone(), area.clone());
        move || {
            build_area_sets(&area);
            point_sets(&point)
        }
    }));
    assert_eq!(area_first, reference, "area light first");
    let interleaved = on_thread(Box::new(move || {
        let memo = footprint_for(&point, DEFAULT_AREA_SAMPLE_COUNT);
        let probes = SoftProbes::new(&point, DEFAULT_AREA_SAMPLE_COUNT);
        build_area_sets(&area);
        let after = footprint_for(&point, DEFAULT_AREA_SAMPLE_COUNT);
        assert!(
            Arc::ptr_eq(&memo, &after),
            "the area light evicted the memo"
        );
        assert_eq!(probes, SoftProbes::new(&point, DEFAULT_AREA_SAMPLE_COUNT));
        point_sets(&point)
    }));
    assert_eq!(interleaved, reference, "area light between");
}

// --- window and weight maps -----------------------------------------------

/// A 1 m² `light 300` panel `height` metres over an open floor: its reach
/// derives to 16 m.
fn panel_over_floor(height: f64) -> (Fixture, MapLight) {
    let receiver = Receiver::centred(10, 10);
    let mut scene = SceneBuilder::default();
    receiver.add_floor(&mut scene);
    let light = area_light(
        vec![ceiling_panel([-0.5, 0.5], [-0.5, 0.5], height)],
        1.0,
        None,
    );
    assert_eq!(light.falloff_range, 16.0);
    (Fixture::new(scene, receiver), light)
}

// AC 14 (texel and weight map): on the axis of a 1 m² `light 300` panel at
// half its 16 m reach, the lightmap texel and the animated weight map at
// curve 1 each read 0.879 of their unwindowed term within 1e-3, and at
// reach neither records the texel.
#[test]
fn texels_and_weight_maps_read_the_window_at_half_reach_and_nothing_at_reach() {
    let window_at_half = (1.0f64 - 1.0 / 16.0).powi(2);
    let (fixture, light) = panel_over_floor(8.0);
    let panel = &light.area.as_ref().unwrap().polygons[0];
    let p = site_at(&fixture.receiver, 5, 5).world_p;
    let unwindowed = polygon_irradiance(panel, p.as_dvec3(), DVec3::Y).0;
    let grid = fixture.bake(&light, DEFAULT_AREA_SAMPLE_COUNT, true);
    let v = grid.get(5, 5).expect("lit");
    assert_eq!(v, 1.0);
    let (lit, _) = reconstruct_light_texel(&light, p, Vec3::Y, v);
    let texel_ratio = f64::from(lit.x) / unwindowed;
    let weights = fixture.bake_weights(&light, DEFAULT_AREA_SAMPLE_COUNT, true);
    let weight_ratio = f64::from(weights.get(5, 5).expect("weighted")) / unwindowed;
    for (what, ratio) in [("texel", texel_ratio), ("weight map", weight_ratio)] {
        assert!(
            (ratio - window_at_half).abs() < 1.0e-3,
            "{what}: {ratio} vs {window_at_half}"
        );
    }

    let (fixture, light) = panel_over_floor(16.0);
    let grid = fixture.bake(&light, DEFAULT_AREA_SAMPLE_COUNT, true);
    let weights = fixture.bake_weights(&light, DEFAULT_AREA_SAMPLE_COUNT, true);
    assert!(cells(&grid).iter().all(Option::is_none), "texels at reach");
    assert!(
        cells(&weights).iter().all(Option::is_none),
        "weights at reach"
    );
}

// AC 33: an animated area light's weight map at curve 1 equals its static
// bake over a penumbra: each weight is the unshadowed weight times the
// static texel's visibility, bit for bit.
#[test]
fn an_animated_area_lights_weight_map_equals_its_static_bake() {
    let (fixture, light) = plated_flush_panel();
    let animated = MapLight {
        animation: Some(LightAnimation {
            period: 1.0,
            phase: 0.0,
            brightness: Some(vec![1.0, 0.5]),
            color: None,
            direction: None,
            start_active: true,
        }),
        is_animated: true,
        ..light.clone()
    };
    let chart = fixture.receiver.chart();
    let stat = fixture.bake(&light, DEFAULT_AREA_SAMPLE_COUNT, true);
    let weights = fixture.bake_weights(&animated, DEFAULT_AREA_SAMPLE_COUNT, true);
    let (mut compared, mut penumbra) = (0, 0);
    for ty in 0..fixture.receiver.height {
        for tx in 0..fixture.receiver.width {
            let (Some(v), Some(weight)) = (stat.get(tx, ty), weights.get(tx, ty)) else {
                continue;
            };
            let p = chart_texel_world_position(&chart, tx as i32, ty as i32);
            let (contribution, _) = light_contribution_and_direction(&animated, p, chart.normal);
            let unshadowed =
                contribution_to_weight(contribution, animated.color, animated.intensity);
            let expected = if v > 0.0 { unshadowed * v } else { 0.0 };
            assert_eq!(
                weight.to_bits(),
                expected.to_bits(),
                "texel ({tx}, {ty}): weight {weight}, static {v}"
            );
            compared += 1;
            penumbra += usize::from(v > 0.0 && v < 1.0);
        }
    }
    assert!(
        compared > 1000 && penumbra > 20,
        "{compared} compared, {penumbra} penumbra"
    );
}

// --- reach readers --------------------------------------------------------

// AC 15 (lightmap, O21): a floor within reach of a 4 × 4 m panel's corner
// but beyond reach of its centroid is planned, walked and lit at the
// windowed polygon term within 1e-5 relative at every texel.
#[test]
fn a_floor_within_reach_of_a_corner_takes_the_windowed_term() {
    let receiver = receiver_at(2.8, 2.8, 10, 10);
    let mut scene = SceneBuilder::default();
    receiver.add_floor(&mut scene);
    let light = area_light(
        vec![ceiling_panel([-2.0, 2.0], [-2.0, 2.0], 1.0)],
        1.0,
        Some(3.0),
    );
    let fixture = Fixture::new(scene, receiver);
    let chart = receiver.chart();
    for (tx, ty) in [(0, 0), (9, 9)] {
        let p = chart_texel_world_position(&chart, tx, ty).as_dvec3();
        assert!(
            p.distance(light.origin) > 3.0,
            "beyond the centroid's reach"
        );
    }
    let layers = fixture.with_atlas(|shared| {
        bake_light_layer(
            &light,
            shared,
            &fixture.bvh,
            &fixture.primitives,
            &fixture.geometry,
            DEFAULT_AREA_SAMPLE_COUNT,
            true,
            ExactSkips::ON,
            &BakeControl::unrestricted(),
        )
    });
    let grid = fixture.grid_of(&layers[0].texels);
    for ty in 0..10 {
        for tx in 0..10 {
            let v = grid.get(tx, ty).expect("every texel is walked and lit");
            assert_eq!(v, 1.0);
            let p = chart_texel_world_position(&chart, tx as i32, ty as i32);
            let ratio = lit_over_term(&light, p, v);
            assert!((ratio - 1.0).abs() < 1.0e-5, "texel ({tx}, {ty}): {ratio}");
        }
    }
}

// The chart cull drops a pair only where the polygon term is exactly zero at
// every texel: a receiver behind the panel's plane, one whose horizon holds
// the panel below it, and one past the reach sphere. A receiver under the
// panel, and one within reach of its corner only, stay.
#[test]
fn the_area_chart_cull_drops_only_charts_the_polygons_cannot_light() {
    let light = area_light(
        vec![ceiling_panel([-2.0, 2.0], [-2.0, 2.0], 3.0)],
        1.0,
        Some(3.0),
    );
    let up_facing = area_light(
        vec![crate::area_light::test_lights::horizontal_rect(
            [-2.0, 2.0],
            [-2.0, 2.0],
            3.0,
            DVec3::Y,
        )],
        1.0,
        Some(3.0),
    );
    let floor_at = |x0: f32, y: f32, z0: f32| Receiver {
        x0,
        y,
        z0,
        width: 20,
        height: 20,
        pitch: PITCH,
    };
    let cases = [
        ("under the panel", &light, floor_at(-0.4, 0.5, -0.4), true),
        ("by the corner", &light, floor_at(2.5, 1.0, 2.5), true),
        ("behind the plane", &light, floor_at(-0.4, 3.5, -0.4), false),
        (
            "panel below the horizon",
            &up_facing,
            floor_at(-0.4, 3.5, -0.4),
            false,
        ),
        (
            "past the reach sphere",
            &light,
            floor_at(8.0, 0.0, 8.0),
            false,
        ),
    ];
    for (name, light, receiver, kept) in cases {
        let chart = receiver.chart();
        assert_eq!(chart_may_cover(light, &chart), kept, "{name}");
        let lit = (0..20).any(|ty| {
            (0..20).any(|tx| {
                light_texel_is_covered(
                    light,
                    chart_texel_world_position(&chart, tx, ty),
                    chart.normal,
                )
            })
        });
        assert_eq!(lit, kept, "{name}: the cull agrees with the bake's term");
    }
}
