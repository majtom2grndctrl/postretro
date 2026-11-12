// Geometry-knot placement against the bake's own enclosing rule: every
// enclosure end of every near-field probe gets a pose just inside and one
// clear outside, on lattice and off it, for several brushes, a bent path
// and a ramped mover.
// See: context/lib/build_pipeline.md §Compiler pipeline step 12 (Mover probe channel, Poses)

use glam::{DVec3, Vec3};
use postretro_level_format::mover_probe_channel::ramped_knot;
use postretro_level_format::texture_names::TextureNamesSection;
use postretro_test_log_capture::LogCapture;

use super::geometry_knots::{
    INSIDE_KNOT_MARGIN_M, KNOT_MERGE_TOLERANCE_M, OUTSIDE_KNOT_DISTANCE_M,
};
use super::placement::{AUTO_POSE_CAP, KnotCounts, even_knots};
use super::pose::{AnalysedMover, MoverPoses};
use super::tests::{box_brush, mover, waypoint};
use super::travel::PathTravel;
use crate::map_data::{BrushVolume, MapKinematicMover, MapKinematicWaypoint};
use crate::mover_probe_channel::{MoverRecords, NearFieldDomain, PosedSolid};
use crate::sh_bake::{ProbeGridLayout, vec3_from};

const SPACING: f32 = 1.0;

/// Slack on the knot distances the tests read back, in metres of travel:
/// the `f32` rounding of t and of the posed offset.
const SLOP_M: f64 = 1.0e-4;

/// A full lattice at `SPACING`, `dims` probes from `min`, every probe valid.
fn lattice(min: DVec3, dims: [u32; 3]) -> ProbeGridLayout {
    let mut probe_positions = Vec::new();
    for z in 0..dims[2] {
        for y in 0..dims[1] {
            for x in 0..dims[0] {
                let step = DVec3::new(f64::from(x), f64::from(y), f64::from(z));
                probe_positions.push(min + step * f64::from(SPACING));
            }
        }
    }
    let validity = vec![1; probe_positions.len()];
    ProbeGridLayout {
        world_min: min,
        dims,
        cell_size: [SPACING; 3],
        far_sentinel: 4.0 * SPACING * 3.0_f32.sqrt(),
        probe_positions,
        validity,
    }
}

/// A mover of `brushes` on a path through `points`, `w0`, `w1`, ….
fn on_path(
    brushes: Vec<BrushVolume>,
    points: &[DVec3],
    light_poses: Option<u32>,
) -> (MapKinematicMover, Vec<MapKinematicWaypoint>) {
    let mut slab = mover(0, "slab", "w0", brushes);
    slab.light_poses = light_poses;
    let waypoints = points
        .iter()
        .enumerate()
        .map(|(index, &origin)| {
            let next = if index + 1 < points.len() {
                format!("w{}", index + 1)
            } else {
                String::new()
            };
            waypoint(&format!("w{index}"), &next, origin)
        })
        .collect();
    (slab, waypoints)
}

fn placed(
    mover: &MapKinematicMover,
    waypoints: &[MapKinematicWaypoint],
    probes: &ProbeGridLayout,
) -> MoverPoses {
    let mut poses = MoverPoses::resolve(std::slice::from_ref(mover), waypoints);
    poses
        .place_poses(std::slice::from_ref(mover), SPACING, probes)
        .expect("the mover places");
    poses
}

/// Where, independently of placement's interval algebra, the posed mover
/// encloses `probe`: the bake's own predicate ([`super::pose::Pose::encloses`])
/// sampled along the runtime's path rule and each change refined by
/// bisection on `f32` t, as closed intervals of enclosed t. Two intervals
/// join where the probe sits inside the mover's solid between them: a seam
/// between two of its brushes, which no face crosses.
fn sampled_enclosure(
    mover: &MapKinematicMover,
    travel: &PathTravel,
    probe: Vec3,
) -> Vec<(f32, f32)> {
    let encloses = |t: f32| {
        super::pose::Pose::new("", &mover.brush_volumes, travel.mid_offset(t)).encloses(probe)
    };
    // Refine a change between `out` (not enclosed) and `inside` to the
    // nearest enclosed t.
    let refine = |mut inside: f32, mut out: f32| {
        loop {
            let middle = 0.5 * (inside + out);
            if middle == inside || middle == out {
                return inside;
            }
            if encloses(middle) {
                inside = middle;
            } else {
                out = middle;
            }
        }
    };
    const SAMPLES: u32 = 8_000;
    let mut intervals = Vec::new();
    let mut start: Option<f32> = None;
    let mut previous = (0.0_f32, false);
    for step in 0..=SAMPLES {
        let t = step as f32 / SAMPLES as f32;
        let now = encloses(t);
        match (previous.1, now) {
            (false, true) => {
                start = Some(if step == 0 {
                    0.0
                } else {
                    refine(t, previous.0)
                })
            }
            (true, false) => {
                intervals.push((start.take().expect("open"), refine(previous.0, t)));
            }
            _ => {}
        }
        previous = (t, now);
    }
    if let Some(start) = start {
        intervals.push((start, 1.0));
    }
    let in_solid = |t: f32| {
        super::pose::Pose::new("", &mover.brush_volumes, travel.mid_offset(t))
            .contains_by(probe.as_dvec3(), 0.0)
    };
    let mut joined: Vec<(f32, f32)> = Vec::new();
    for interval in intervals {
        match joined.last_mut() {
            Some(last) if in_solid(0.5 * (last.1 + interval.0)) => last.1 = interval.1,
            _ => joined.push(interval),
        }
    }
    joined
}

/// The knot checks every case shares. For each near-field probe and each
/// end of each of its enclosure intervals strictly inside the path, some
/// knot lies just inside, within the inside margin plus the merge
/// tolerance, and its posed solid encloses the probe; and some knot lies
/// outside, at the outside distance within the merge tolerance (nearer only
/// where a terminus or the middle of the gap to the next enclosure comes
/// first), and its posed solid does not. Returns the ends checked.
fn assert_every_enclosure_end_is_bracketed(
    mover: &MapKinematicMover,
    poses: &MoverPoses,
    probes: &ProbeGridLayout,
) -> usize {
    let analysed: &AnalysedMover = &poses.analysed[0];
    let travel = PathTravel::new(&analysed.path);
    let length = f64::from(travel.length());
    let solids: Vec<PosedSolid> = analysed
        .poses
        .iter()
        .map(|pose| PosedSolid::new(mover, pose, &TextureNamesSection { names: Vec::new() }))
        .collect();
    let knots: Vec<f64> = analysed.knots.iter().map(|&t| f64::from(t)).collect();
    let metres = |a: f64, b: f64| (a - b) * length;
    let domain = NearFieldDomain::new(analysed, SPACING);
    let mut ends = 0;
    for index in domain.valid_probes(probes) {
        let probe = vec3_from(probes.probe_positions[index as usize]);
        let intervals = sampled_enclosure(mover, &travel, probe);
        for (at, &(a, b)) in intervals.iter().enumerate() {
            let (a, b) = (f64::from(a), f64::from(b));
            let before = at.checked_sub(1).map(|i| f64::from(intervals[i].1));
            let after = intervals.get(at + 1).map(|next| f64::from(next.0));
            // (end, toward the inside, room to the terminus or gap middle)
            let mut checks = Vec::new();
            if a > 0.0 {
                let room = before.map_or(metres(a, 0.0), |b0| 0.5 * metres(a, b0));
                checks.push((a, 1.0, room, b));
            }
            if b < 1.0 {
                let room = after.map_or(metres(1.0, b), |a1| 0.5 * metres(a1, b));
                checks.push((b, -1.0, room, a));
            }
            for (end, inward, room, other_end) in checks {
                ends += 1;
                let span = metres(other_end, end).abs();
                let inside = knots.iter().enumerate().any(|(k, &t)| {
                    let depth = inward * metres(t, end);
                    depth >= 0.0
                        && depth
                            <= (INSIDE_KNOT_MARGIN_M + KNOT_MERGE_TOLERANCE_M).min(span) + SLOP_M
                        && solids[k].encloses(probe)
                });
                assert!(
                    inside,
                    "probe {probe}: no enclosing knot just inside the end at t {end:.6}; knots \
                     {knots:?}"
                );
                let want = OUTSIDE_KNOT_DISTANCE_M.min(room);
                let outside = knots.iter().enumerate().any(|(k, &t)| {
                    let distance = -inward * metres(t, end);
                    distance > 0.0
                        && distance >= want - KNOT_MERGE_TOLERANCE_M - SLOP_M
                        && distance <= want + KNOT_MERGE_TOLERANCE_M + SLOP_M
                        && !solids[k].encloses(probe)
                });
                assert!(
                    outside,
                    "probe {probe}: no clear knot {want:.4} m outside the end at t {end:.6}; \
                     knots {knots:?}"
                );
            }
        }
    }
    ends
}

/// A slab 0.1 m thick along x and 3 m square, its near face at x = 0.
fn slab() -> BrushVolume {
    box_brush(DVec3::new(-0.1, 0.0, 0.0), DVec3::new(0.0, 3.0, 3.0))
}

fn geometry_count(poses: &MoverPoses) -> u32 {
    poses.analysed[0].knot_counts.geometry
}

#[test]
fn a_slab_moving_along_a_lattice_axis_gets_a_knot_inside_and_outside_every_enclosure_end() {
    // Four metres along +x through probe layers at x 0.5 … 3.5: each layer
    // is enclosed for the slab's thickness less the inset at both faces.
    let probes = lattice(DVec3::new(-1.5, -1.5, -1.5), [7, 7, 7]);
    let (slab, waypoints) = on_path(
        vec![slab()],
        &[DVec3::ZERO, DVec3::new(4.0, 0.0, 0.0)],
        None,
    );
    let poses = placed(&slab, &waypoints, &probes);
    let ends = assert_every_enclosure_end_is_bracketed(&slab, &poses, &probes);
    assert!(
        ends >= 8,
        "every layer the slab crosses has two ends ({ends})"
    );
    // Every probe of a layer shares its ends, so a layer's eight knots merge
    // into four: inside and outside at both faces. Four layers, sixteen
    // geometry knots; the five even poses stay, none within the tolerance.
    let analysed = &poses.analysed[0];
    assert_eq!(
        analysed.knot_counts,
        KnotCounts {
            even: 5,
            geometry: 16
        }
    );
    assert_eq!(analysed.knots.len(), 21);
    assert_eq!(analysed.poses.len(), 21);
    assert!(analysed.knots.windows(2).all(|pair| pair[0] < pair[1]));
    for k in even_knots(5) {
        assert!(analysed.knots.contains(&k), "even knot {k} stays");
    }
}

#[test]
fn a_slab_off_the_lattice_phase_still_brackets_every_enclosure_end() {
    // The lattice shifted 0.06 m along x and the path 3.3 m: no end lands
    // on a round t, and the first layer sits 0.06 m past the start, nearer
    // than the outside distance, so the first terminus is its outside knot.
    let probes = lattice(DVec3::new(-1.94, -1.5, -1.5), [7, 7, 7]);
    let (slab, waypoints) = on_path(
        vec![slab()],
        &[DVec3::ZERO, DVec3::new(3.3, 0.0, 0.0)],
        None,
    );
    let poses = placed(&slab, &waypoints, &probes);
    let ends = assert_every_enclosure_end_is_bracketed(&slab, &poses, &probes);
    assert!(ends >= 8, "{ends}");
    assert!(geometry_count(&poses) > 0);
}

#[test]
fn a_two_brush_mover_brackets_each_brush_and_shares_the_gap_between_them() {
    // Two slabs 1.5 outside distances apart along x: each probe they pass
    // is enclosed twice, with a gap shorter than two outside distances, so
    // both outside knots sit at the gap's middle and merge.
    let gap = 1.5 * OUTSIDE_KNOT_DISTANCE_M;
    let brushes = vec![
        slab(),
        box_brush(
            DVec3::new(-0.2 - gap, 0.0, 0.0),
            DVec3::new(-0.1 - gap, 3.0, 3.0),
        ),
    ];
    let probes = lattice(DVec3::new(-1.5, -1.5, -1.5), [7, 7, 7]);
    let (slab, waypoints) = on_path(brushes, &[DVec3::ZERO, DVec3::new(3.0, 0.0, 0.0)], None);
    let poses = placed(&slab, &waypoints, &probes);
    let ends = assert_every_enclosure_end_is_bracketed(&slab, &poses, &probes);
    assert!(ends >= 12, "{ends}");
    // Per layer the slab pair crosses whole: four inside knots, two outside
    // the pair and one shared in the gap.
    let analysed = &poses.analysed[0];
    let crossed = 3;
    assert_eq!(analysed.knot_counts.geometry, crossed * 7);
}

#[test]
fn a_seam_between_two_touching_brushes_places_no_knot_inside_the_mover() {
    // Two 0.1 m slabs stacked along the travel, touching at x = −0.1: the
    // inset leaves each probe unenclosed for 2 mm of travel at the seam, but
    // it never leaves the solid there, so each layer gets the four knots of
    // one 0.2 m slab and none at the seam.
    let brushes = vec![
        slab(),
        box_brush(DVec3::new(-0.2, 0.0, 0.0), DVec3::new(-0.1, 3.0, 3.0)),
    ];
    let probes = lattice(DVec3::new(-1.5, -1.5, -1.5), [7, 7, 7]);
    let (slab, waypoints) = on_path(brushes, &[DVec3::ZERO, DVec3::new(4.0, 0.0, 0.0)], None);
    let poses = placed(&slab, &waypoints, &probes);
    let ends = assert_every_enclosure_end_is_bracketed(&slab, &poses, &probes);
    assert_eq!(ends, 4 * 9 * 2, "two ends per probe of each crossed layer");
    assert_eq!(poses.analysed[0].knot_counts.geometry, 4 * 4);
}

#[test]
fn a_bent_two_segment_path_brackets_enclosure_ends_on_both_legs() {
    // A 0.4 m post 3 m tall moving 2 m along +x, then 2 m along +z: it
    // crosses probe columns on both legs, and the corner joins its legs.
    let post = box_brush(DVec3::new(-0.2, 0.0, -0.2), DVec3::new(0.2, 3.0, 0.2));
    // Columns at whole x and z: the post leaves (0, 0) and crosses (1, 0) on
    // the first leg, turns the corner inside (2, 0), crosses (2, 1) and
    // arrives over (2, 2).
    let probes = lattice(DVec3::new(-1.0, -1.5, -1.0), [5, 7, 5]);
    let (post, waypoints) = on_path(
        vec![post],
        &[
            DVec3::ZERO,
            DVec3::new(2.0, 0.0, 0.0),
            DVec3::new(2.0, 0.0, 2.0),
        ],
        None,
    );
    let poses = placed(&post, &waypoints, &probes);
    let ends = assert_every_enclosure_end_is_bracketed(&post, &poses, &probes);
    // Per probe row: one end leaving (0, 0), two at (1, 0), two across the
    // corner at (2, 0), two at (2, 1) and one arriving at (2, 2).
    assert_eq!(ends, 3 * 8);
    assert!(geometry_count(&poses) > 0);
}

#[test]
fn a_ramped_movers_geometry_knots_are_its_unramped_ones_and_map_through_the_ramp() {
    let probes = lattice(DVec3::new(-1.5, -1.5, -1.5), [7, 7, 7]);
    let path = [DVec3::ZERO, DVec3::new(4.0, 0.0, 0.0)];
    let (plain, waypoints) = on_path(vec![slab()], &path, None);
    let (mut ramped, _) = on_path(vec![slab()], &path, None);
    (ramped.light_ramp_start, ramped.light_ramp_end) = (0.3, 0.8);
    let plain_poses = placed(&plain, &waypoints, &probes);
    let ramped_poses = placed(&ramped, &waypoints, &probes);
    assert_every_enclosure_end_is_bracketed(&ramped, &ramped_poses, &probes);
    let knots = &ramped_poses.analysed[0].knots;
    assert_eq!(
        knots, &plain_poses.analysed[0].knots,
        "knots stay path fractions t"
    );
    let records = MoverRecords::new(std::slice::from_ref(&ramped), &ramped_poses);
    let effective = records
        .get(0)
        .expect("the mover's record")
        .effective_knots();
    // Every knot inside the ramp maps to ramp(t); those squeezed onto
    // either end are never read.
    let inside_ramp: Vec<f32> = knots
        .iter()
        .copied()
        .filter(|&t| t > 0.3 && t < 0.8)
        .collect();
    assert!(inside_ramp.len() > 4, "geometry knots fall inside the ramp");
    for t in inside_ramp {
        let pose = knots.iter().position(|&k| k == t).expect("a knot") as u32;
        let knot = effective
            .iter()
            .find(|knot| knot.pose == pose)
            .unwrap_or_else(|| panic!("knot {t} is read"));
        assert_eq!(knot.t, ramped_knot(t, 0.3, 0.8));
    }
}

#[test]
fn an_authored_light_poses_places_its_even_poses_and_no_geometry_knot() {
    let probes = lattice(DVec3::new(-1.5, -1.5, -1.5), [7, 7, 7]);
    let (slab, waypoints) = on_path(
        vec![slab()],
        &[DVec3::ZERO, DVec3::new(4.0, 0.0, 0.0)],
        Some(5),
    );
    let poses = placed(&slab, &waypoints, &probes);
    let analysed = &poses.analysed[0];
    assert_eq!(analysed.knots, even_knots(5));
    assert_eq!(
        analysed.knot_counts,
        KnotCounts {
            even: 5,
            geometry: 0
        }
    );
    // `light_poses 2` stays the termini alone.
    let (two, waypoints) = on_path(
        vec![self::slab()],
        &[DVec3::ZERO, DVec3::new(4.0, 0.0, 0.0)],
        Some(2),
    );
    let poses = placed(&two, &waypoints, &probes);
    assert_eq!(poses.analysed[0].knots, vec![0.0, 1.0]);
    assert_eq!(poses.analysed[0].knot_counts, KnotCounts::TERMINI);
}

#[test]
fn a_path_past_the_cap_warns_about_its_even_poses_and_keeps_every_geometry_knot() {
    let capture = LogCapture::start();
    // 20.25 m along +x, probe layers at x 0.5 … 4.5 over its first metres:
    // 22 even poses wanted, 16 baked, and four geometry knots for each of
    // the five layers the slab crosses, none counted against the cap.
    let probes = lattice(DVec3::new(-1.5, -1.5, -1.5), [7, 7, 7]);
    let (slab, waypoints) = on_path(
        vec![slab()],
        &[DVec3::ZERO, DVec3::new(20.25, 0.0, 0.0)],
        None,
    );
    let poses = placed(&slab, &waypoints, &probes);
    assert_every_enclosure_end_is_bracketed(&slab, &poses, &probes);
    let analysed = &poses.analysed[0];
    let counts = analysed.knot_counts;
    assert_eq!(counts.geometry, 5 * 4);
    assert_eq!(
        analysed.poses.len(),
        (counts.even + counts.geometry) as usize
    );
    assert!(analysed.poses.len() > AUTO_POSE_CAP as usize);
    // An even knot leaves only where it merged into a geometry knot.
    let tolerance = KNOT_MERGE_TOLERANCE_M / 20.25;
    for even in even_knots(AUTO_POSE_CAP) {
        if !analysed.knots.contains(&even) {
            assert!(
                analysed
                    .knots
                    .iter()
                    .any(|&g| (f64::from(g) - f64::from(even)).abs() <= tolerance),
                "even knot {even} left with no geometry knot beside it"
            );
        }
    }
    capture.assert_logged_once(log::Level::Warn, "kinematic_mover 'slab'");
    capture.assert_logged_once(log::Level::Warn, "wants 22 even poses");
    capture.assert_logged_once(log::Level::Warn, "baking 16 even poses");
}

#[test]
fn a_still_or_unbounded_probe_set_places_no_geometry_knot() {
    // A path with no length, and a map with no probes, place nothing.
    let probes = lattice(DVec3::new(-1.5, -1.5, -1.5), [7, 7, 7]);
    let (still, waypoints) = on_path(vec![slab()], &[DVec3::ONE, DVec3::ONE], None);
    let poses = placed(&still, &waypoints, &probes);
    assert_eq!(poses.analysed[0].knots, vec![0.0, 1.0]);
    let (moving, waypoints) = on_path(
        vec![slab()],
        &[DVec3::ZERO, DVec3::new(4.0, 0.0, 0.0)],
        None,
    );
    let poses = placed(&moving, &waypoints, &ProbeGridLayout::no_probes());
    assert_eq!(poses.analysed[0].knots, even_knots(5));
}

// The plan's riskiest premise: geometry knot counts stay sane on real maps.
// Placement alone, from the map's own probe grid; no bake.
#[test]
#[ignore = "loads the stress hallway; prints each mover's knot counts: cargo test -p \
            postretro-level-compiler --bin prl-build knot_counts_on_the_reference_maps -- \
            --ignored --nocapture"]
fn knot_counts_on_the_reference_maps() {
    use crate::fixture_pipeline::{fixture_path, load_map};
    for name in [
        "mover-closet",
        "mover-setpiece",
        "enemy-movers",
        "stress-warren-hallway-inspection",
    ] {
        let started = std::time::Instant::now();
        let fixture = load_map(&fixture_path(name));
        let loaded = started.elapsed();
        let spacing = crate::sh_bake::DEFAULT_PROBE_SPACING;
        let layout = fixture.probe_layout(spacing);
        let placing = std::time::Instant::now();
        let poses = fixture.placed_mover_poses(spacing);
        eprintln!(
            "== {name}: loaded in {:.1} s, placed in {:.2} s",
            loaded.as_secs_f64(),
            placing.elapsed().as_secs_f64(),
        );
        let patches = crate::surface_radiance::PatchSet::build(&fixture.geometry);
        let posed = crate::mover_probe_channel::PosedMovers::resolve(
            &poses,
            &fixture.kinematic_movers,
            &fixture.geometry.texture_names,
        )
        .expect("the map's movers pose");
        for (analysed, solids) in poses.analysed.iter().zip(posed.movers()) {
            let travel = PathTravel::new(&analysed.path);
            let near = NearFieldDomain::new(analysed, spacing);
            let domain = near.valid_probes(&layout);
            let domain_patches = near.static_patches(&patches).len();
            let posed_patches = crate::mover_probe_channel::PosedScene::new(
                std::sync::Arc::clone(&solids.solids[0]),
                0,
            )
            .expect("posed sides")
            .patches
            .len();
            // `captures/t6b-results.md`'s mid-pose model: ~70 µs per
            // gathered static or posed domain patch for the later bounces,
            // plus ~60 µs per domain probe of gather.
            let per_mid_pose =
                70.0e-6 * (domain_patches + posed_patches) as f64 + 60.0e-6 * domain.len() as f64;
            eprintln!(
                "  {}: path {:.2} m, {} brush(es), {} domain probe(s), {} domain + {} posed \
                 patch(es): {} pose(s) = {} even + {} geometry; projected {:.2} s per mid pose, \
                 geometry knots add {:.1} s; knots {:?}",
                analysed.label,
                travel.length(),
                analysed.poses[0].brushes.len(),
                domain.len(),
                domain_patches,
                posed_patches,
                analysed.poses.len(),
                analysed.knot_counts.even,
                analysed.knot_counts.geometry,
                per_mid_pose,
                per_mid_pose * f64::from(analysed.knot_counts.geometry),
                analysed
                    .knots
                    .iter()
                    .map(|t| format!("{t:.4}"))
                    .collect::<Vec<_>>(),
            );
        }
        for skipped in &poses.not_analysed {
            eprintln!("  {}: not analysed ({})", skipped.label, skipped.reason);
        }
    }
}
