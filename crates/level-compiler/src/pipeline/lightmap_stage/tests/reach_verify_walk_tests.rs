//! Reach cull walk order on the two-rooms fixture: verify mode's re-walk is a
//! kept pair's production walk, a culled pair's work stays in one permit and
//! one progress unit, and a texel traced after a run of
//! skipped ones records what the unculled walk and a brute-force trace do
//! though the remembered occluders differ.
//! See: context/lib/build_pipeline.md §Compiler pipeline step 12 (Exact skips)

use std::sync::atomic::AtomicUsize;

use super::reach_bake::{ReachRun, reach_bake, reach_bake_with, reach_off};
use super::*;
use crate::chart_raster::{chart_texel_seed, chart_texel_world_position};
use crate::governor::permits_held_on_this_thread;
use crate::lightmap_layer::{ChartTally, ReachPhase, SkipStats};
use crate::lightmap_reach::test_fixtures::{ReachFixture, Wall, point_light, room_a_centre};
use crate::ray_traversal::{TraceRay, segment_clear_brute_force, triangle_points};
use crate::texel_footprint::{ClipBand, ProbeEarlyOut, TexelFrame, TexelSampler, TexelSite};

fn two_rooms_light() -> [MapLight; 1] {
    [point_light(room_a_centre(), 6.0)]
}

/// Bake the two rooms with `exact_skips` tallied into a fresh tally, floor
/// on, four workers; the tally.
fn tallied_bake(
    fixture: &ReachFixture,
    exact_skips: lightmap_layer::ExactSkips,
) -> &'static SkipStats {
    let stats = SkipStats::leak();
    reach_bake(
        fixture,
        &two_rooms_light(),
        true,
        exact_skips.tallied(stats),
        4,
    )
    .expect("bake");
    stats
}

/// Each walked face's tallies, by face: the culled walk, then the re-walk,
/// for a culled pair; the one walk for a kept pair.
fn by_face(stats: &SkipStats) -> std::collections::BTreeMap<usize, Vec<ChartTally>> {
    let mut faces = std::collections::BTreeMap::<usize, Vec<ChartTally>>::new();
    for tally in stats.charts() {
        faces.entry(tally.face).or_default().push(tally);
    }
    for tallies in faces.values_mut() {
        // One thread walks a pair and then re-walks it, in that order.
        tallies.sort_by_key(|tally| !tally.culled);
    }
    faces
}

// Verify mode re-walks each culled pair with the shaft cull and the clip
// pre-pass on and both occluder caches empty at its first texel, exactly as
// production walks a kept pair: each tile's shaft tests and traced segments
// equal those of the bake with only the reach cull off, and so does every
// texel's record.
#[test]
fn reach_verify_rewalks_a_culled_pair_as_production_walks_a_kept_pair() {
    let fixture = ReachFixture::two_rooms(Wall::Solid);
    let culled = by_face(tallied_bake(&fixture, lightmap_layer::ExactSkips::ON));
    let kept = by_face(tallied_bake(&fixture, reach_off()));
    let (mut rewalks, mut shaft_tests) = (0, 0);
    for (face, tallies) in &culled {
        if !tallies[0].culled {
            assert_eq!(tallies.len(), 1, "face {face}: a kept pair walks once");
            continue;
        }
        assert_eq!(
            tallies.len(),
            2,
            "face {face}: a culled walk, then its re-walk"
        );
        let rewalk = &tallies[1];
        assert!(!rewalk.culled);
        let [production] = kept[face].as_slice() else {
            panic!("face {face}: one production walk");
        };
        assert_eq!(
            rewalk.texels.first().map(|texel| texel.occluders),
            Some([None, None]),
            "face {face}: the re-walk starts with both occluder caches empty"
        );
        assert_eq!(rewalk.tiles, production.tiles, "face {face}: tile tallies");
        let records = |tally: &ChartTally| -> Vec<_> {
            tally
                .texels
                .iter()
                .map(|texel| (texel.texel, texel.visibility.map(f32::to_bits)))
                .collect()
        };
        assert_eq!(records(rewalk), records(production), "face {face}");
        rewalks += 1;
        shaft_tests += rewalk.tiles.iter().map(|tile| tile.tests).sum::<u32>();
    }
    assert!(rewalks > 0, "the two rooms cull pairs");
    assert!(shaft_tests > 0, "the re-walks take the shaft cull");
}

// Verify mode on, one governor permit, four threads: the two rooms bake
// to completion. Every admission is the thread's only permit; each culled
// pair's walk, its re-walk and its chart's mask build run holding exactly
// one; the re-walk starts with progress where the culled walk started it;
// and progress ends exact, one unit per pair.
#[test]
fn reach_cull_holds_one_permit_and_one_progress_unit_per_culled_pair() {
    let fixture = ReachFixture::two_rooms(Wall::Solid);
    let stats = SkipStats::leak();
    let progress = StageProgress::indeterminate();
    stats.watch_progress(progress.completed_handle());
    let nested = Arc::new(AtomicUsize::new(0));
    let hook_nested = Arc::clone(&nested);
    let mut run = ReachRun::new(true, lightmap_layer::ExactSkips::ON.tallied(stats), 4);
    run.permits = 1;
    run.progress = Some(&progress);
    run.enter_hook = Some(Arc::new(move || {
        if permits_held_on_this_thread() != 1 {
            hook_nested.fetch_add(1, Ordering::SeqCst);
        }
    }));
    let bake = reach_bake_with(&fixture, &two_rooms_light(), &run).expect("the bake completes");
    let counts = bake.counts;
    assert!(
        counts.culled > 0 && counts.rewalked == counts.culled,
        "{counts:?}"
    );
    assert_eq!(nested.load(Ordering::SeqCst), 0, "no admission nests");

    let notes = stats.reach_notes();
    for note in &notes {
        assert_eq!(note.permits, 1, "{note:?}");
    }
    let of = |phase| -> Vec<_> { notes.iter().filter(|note| note.phase == phase).collect() };
    let (walks, rewalks, masks) = (
        of(ReachPhase::CulledWalk),
        of(ReachPhase::Rewalk),
        of(ReachPhase::MaskBuild),
    );
    assert_eq!(walks.len() as u64, counts.culled);
    assert_eq!(rewalks.len(), walks.len());
    for walk in &walks {
        assert!(
            masks.iter().any(|mask| mask.face == walk.face),
            "face {}: its mask is built inside a walk",
            walk.face
        );
        let rewalk = rewalks
            .iter()
            .find(|rewalk| rewalk.face == walk.face)
            .expect("the pair is re-walked");
        assert_eq!(
            rewalk.progress, walk.progress,
            "face {}: the culled walk advances no progress before its re-walk",
            walk.face
        );
    }
    assert_eq!(progress.total(), Some(progress.completed()));
}

/// A brute-force trace of the record the walk stores at `(tx, ty)` of
/// `face`: every segment tested against every triangle, no BVH, every clip
/// segment traced.
fn brute_force_visibility(
    geometry: &GeometryResult,
    chart: &crate::lightmap_bake::Chart,
    light: &MapLight,
    (tx, ty): (i32, i32),
) -> f32 {
    let band = ClipBand::for_chart(chart);
    let triangles = || {
        geometry.face_index_ranges.iter().flat_map(|range| {
            let start = range.index_offset as usize;
            (start..start + range.index_count as usize).step_by(3)
        })
    };
    let front_faces_clear = |from: Vec3, to: Vec3| {
        let delta = to - from;
        let length = delta.length();
        if length <= 0.0 {
            return true;
        }
        let ray = TraceRay::new(from, delta / length);
        !triangles().any(|tri| {
            let points = triangle_points(geometry, tri);
            band.admits(points) && ray.hit(points, 0.0, length).is_some_and(|hit| hit.front)
        })
    };
    let sampler = TexelSampler::with_early_out(
        light,
        test_args().soft_shadow_samples,
        true,
        ProbeEarlyOut::for_light(false),
    );
    let site = TexelSite {
        world_p: chart_texel_world_position(chart, tx, ty),
        normal: chart.normal,
        seed: chart_texel_seed(chart, tx, ty),
        frame: TexelFrame::for_chart(chart),
        may_clip: true,
    };
    sampler.visibility(
        light,
        &site,
        |from, to| segment_clear_brute_force(geometry, from, to),
        |from, to| segment_clear_brute_force(geometry, from, to),
        front_faces_clear,
    )
}

// Inside a culled pair, a texel traced right after a run of skipped ones
// meets a remembered occluder the unculled walk does not hold there, since
// skipped texels write none. Its record equals the unculled walk's and a
// brute-force trace of every triangle. The wall is built of 4 x 4 brushes,
// so rays from distant texels are blocked by different triangles.
#[test]
fn reach_cull_traced_texel_after_skipped_run_matches_unculled_and_brute_force() {
    let fixture = ReachFixture::two_rooms(Wall::Grid(4, 4));
    let lights = two_rooms_light();
    let culled = by_face(tallied_bake(&fixture, lightmap_layer::ExactSkips::ON));
    let kept = by_face(tallied_bake(&fixture, reach_off()));
    let mut geometry = fixture.compiled.geometry.clone();
    let atlas = lightmap_bake::prepare_atlas(
        &mut geometry,
        &StaticBakedLights::from_lights(&lights),
        config(false).lightmap_density,
        &[],
    )
    .expect("atlas");
    let (mut after_runs, mut differing) = (0, 0);
    for (face, tallies) in &culled {
        let walk = &tallies[0];
        if !walk.culled {
            continue;
        }
        let production = &kept[face][0];
        assert_eq!(walk.texels.len(), production.texels.len());
        for i in 1..walk.texels.len() {
            let (texel, previous) = (&walk.texels[i], &walk.texels[i - 1]);
            if texel.skipped || !previous.skipped || texel.visibility.is_none() {
                continue;
            }
            let unculled = &production.texels[i];
            assert_eq!(unculled.texel, texel.texel);
            let brute =
                brute_force_visibility(&geometry, &atlas.charts[*face], &lights[0], texel.texel);
            let bits = |v: Option<f32>| v.map(f32::to_bits);
            assert_eq!(
                bits(texel.visibility),
                bits(unculled.visibility),
                "face {face} texel {:?}: culled against unculled",
                texel.texel
            );
            assert_eq!(
                bits(texel.visibility),
                Some(brute.to_bits()),
                "face {face} texel {:?}: culled against brute force",
                texel.texel
            );
            after_runs += 1;
            differing += usize::from(texel.occluders[0] != unculled.occluders[0]);
        }
    }
    assert!(
        after_runs > 0,
        "a culled pair traces a texel after skipped ones"
    );
    assert!(
        differing > 0,
        "of {after_runs} such texels, one meets a different remembered occluder"
    );
}
