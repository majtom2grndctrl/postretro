// Mid-travel poses at the bounce half's seam: the near-field domain a mid
// pose gathers over, and a mid pose against a static-brush reference bake
// of the mover at that pose (direct q, bounce one and depth).
// See: context/lib/build_pipeline.md §Compiler pipeline step 12 (Mover probe channel, Poses)

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use glam::DVec3;
use postretro_level_format::mover_probe_channel::PoseMark;
use postretro_level_format::octahedral::{
    DEFAULT_IRRADIANCE_TILE_BORDER, DEFAULT_IRRADIANCE_TILE_DIMENSION,
};
use postretro_level_format::probe_depth_tiles::PROBE_DEPTH_TILE_TEXELS;

use super::{CONFIG, bake_base, closet, plan_half, with_ctx};
use crate::fixture_pipeline::{fixture_path, load_map};
use crate::mover_probe_channel::fixture_scene::Scene;
use crate::mover_probe_channel::reference_maps::{quake_offset, staticize_at};
use crate::mover_probe_channel::{
    MoverBounceRequest, ProbeCells, compact_slots, compact_tile, display_levels,
    max_abs_difference, round_trip_error,
};
use crate::sh_bake::{pack_octahedral_irradiance_tile, vec3_from};
use crate::surface_radiance::DEFAULT_BOUNCE_FILL;

/// Whether two f16 depth moments agree within f16 rounding: one unit in the
/// last place of the larger.
fn f16_close(a: u16, b: u16) -> bool {
    let (a, b) = (
        crate::bc6h::f16_bits_to_f32(a),
        crate::bc6h::f16_bits_to_f32(b),
    );
    let ulp = (a.abs().max(b.abs()) * 2.0f32.powi(-10)).max(2.0f32.powi(-24));
    (a - b).abs() <= ulp
}

// Near-field domain: on the closet, every mid pose gathers exactly the
// door's domain probes, and a probe just outside the domain (one lattice
// step past it) stores no mid-pose bounce or depth: it blends across. A
// domain probe the door changes is marked at a mid pose. The direct half
// still bakes every probe a gated light reaches: on the setpiece, the rising
// room's mid poses gate probes outside its domain.
#[test]
fn a_mid_pose_gathers_its_domain_alone_and_direct_reaches_past_it() {
    let maps = tempfile::tempdir().expect("maps dir");
    let fixture = load_map(&closet(maps.path(), None));
    with_ctx(&fixture, |ctx, poses| {
        let mut plan = plan_half(&fixture, ctx, poses, DEFAULT_BOUNCE_FILL);
        let mut request = std::mem::take(&mut plan.request);
        let job = &request.jobs[0];
        let (pose_count, domain): (usize, BTreeSet<u32>) = (
            job.scenes.len(),
            job.domain_probes.iter().copied().collect(),
        );
        assert!(pose_count > 2, "the closet door bakes mid poses");
        let layout = crate::sh_bake::probe_grid_layout_of(
            ctx.geometry,
            ctx.tree,
            ctx.exterior_leaves,
            CONFIG.probe_spacing,
        );
        // Just outside: a valid probe off the domain, one lattice step from
        // a domain probe.
        let neighbours = |probe: u32| {
            let [nx, ny, nz] = layout.dims.map(|dim| dim as i64);
            let p = i64::from(probe);
            let (x, y, z) = (p % nx, p / nx % ny, p / (nx * ny));
            [
                (1, 0, 0),
                (-1, 0, 0),
                (0, 1, 0),
                (0, -1, 0),
                (0, 0, 1),
                (0, 0, -1),
            ]
            .into_iter()
            .map(move |(dx, dy, dz)| (x + dx, y + dy, z + dz))
            .filter(move |&(x, y, z)| {
                (0..nx).contains(&x) && (0..ny).contains(&y) && (0..nz).contains(&z)
            })
            .map(move |(x, y, z)| (x + y * nx + z * nx * ny) as u32)
        };
        let just_outside: BTreeSet<u32> = domain
            .iter()
            .flat_map(|&probe| neighbours(probe))
            .filter(|probe| layout.validity[*probe as usize] != 0 && !domain.contains(probe))
            .collect();
        assert!(!just_outside.is_empty(), "the domain has a valid rim");

        let (_, _, outcome) = bake_base(ctx, &plan, &request, DEFAULT_BOUNCE_FILL, None);
        let [states] = outcome.movers.as_slice() else {
            panic!("one door");
        };
        let mid = 1..pose_count - 1;
        let marked_mid =
            |marks: &[PoseMark]| marks[mid.clone()].iter().any(|&m| m != PoseMark::Unmarked);
        let mut inside_marked = 0;
        for (probe, marks) in states
            .bounce
            .iter()
            .map(|p| (p.probe, &p.marks))
            .chain(states.depth.iter().map(|p| (p.probe, &p.marks)))
        {
            if domain.contains(&probe) {
                inside_marked += usize::from(marked_mid(marks));
            } else {
                assert!(
                    !marked_mid(marks),
                    "probe {probe} outside the domain{}: no mid-pose state",
                    if just_outside.contains(&probe) {
                        ", just past it"
                    } else {
                        ""
                    }
                );
            }
        }
        assert!(inside_marked > 0, "a domain probe is marked at a mid pose");

        // Every mid pose gathers the domain's probes and no other.
        request.keep_poses = true;
        let (_, _, kept) = bake_base(ctx, &plan, &request, DEFAULT_BOUNCE_FILL, None);
        let mut mids = 0;
        for pose in kept.poses.iter().filter(|pose| mid.contains(&pose.pose)) {
            let gathered: BTreeSet<u32> = pose.domain.iter().map(|probe| probe.probe).collect();
            assert_eq!(
                gathered, domain,
                "mid pose {} gathers the domain",
                pose.pose
            );
            mids += 1;
        }
        assert_eq!(mids, pose_count - 2);
        eprintln!(
            "closet door: {} pose(s), {} domain probe(s), {} valid probe(s) just outside, \
             {inside_marked} domain (record, probe) pair(s) marked at a mid pose",
            pose_count,
            domain.len(),
            just_outside.len()
        );
    });

    // The direct half bakes every reached probe, inside the domain or not.
    let scene = Scene::of(load_map(&fixture_path("mover-setpiece")));
    let analysed = scene.mover_poses();
    let room = analysed
        .analysed
        .iter()
        .find(|mover| mover.label.contains("setpiece_room"))
        .expect("the rising room");
    let domain = crate::mover_probe_channel::NearFieldDomain::new(room, CONFIG.probe_spacing);
    let lights = scene.static_channel_lights();
    let cells = ProbeCells::of_layout(&scene.layout);
    let bake = scene.bake(&lights, &cells);
    let mut outside = 0;
    for entry in bake
        .lights
        .iter()
        .flat_map(|light| &light.entries)
        .filter(|entry| entry.mover_id == room.mover_id)
    {
        for (slot, probe) in cells
            .valid_probes(entry.cell as usize)
            .into_iter()
            .enumerate()
        {
            let q = entry.poses_at(slot);
            let mid_gated = q[1..q.len() - 1].iter().any(|&q| q < 255);
            if mid_gated && !domain.contains(vec3_from(scene.layout.probe_positions[probe])) {
                outside += 1;
            }
        }
    }
    eprintln!(
        "setpiece room: {outside} (entry, probe) pair(s) gated at a mid pose outside its domain"
    );
    assert!(outside > 0, "direct reaches past the near-field domain");
}

/// What one mid pose's bake reads against the static-brush reference.
struct MidPoseReference {
    direct: (usize, f32),
    bounce: (usize, usize, f32, f32),
    depth: (usize, usize),
}

/// Bake `mover` of dev map `map` at its middle mid pose and the map with the
/// mover made static there, then compare: direct q at every probe a light
/// reaches, valid in both, within one q step (B7's direct bar); the mid
/// pose's bounce one at each domain probe valid in both and not enclosed
/// against the reference's first bounce alone (`_bounce_fill 0`); and its
/// closed-pose depth tile against the reference's id-59 tile where both
/// exist, texel by texel within f16 (B4's bar).
fn mid_pose_against_its_reference(map: &str, mover: &str) -> MidPoseReference {
    let text = std::fs::read_to_string(fixture_path(map)).expect("dev map");
    let today = Scene::of(load_map(&fixture_path(map)));
    let analysed = today.mover_poses();
    let posed = analysed
        .analysed
        .iter()
        .find(|analysed| analysed.label.contains(mover))
        .unwrap_or_else(|| panic!("{map}: no mover {mover}"));
    let pose = posed.poses.len() / 2;
    assert!(
        pose > 0 && pose + 1 < posed.poses.len(),
        "{map}: a mid pose"
    );
    let offset: DVec3 = posed.poses[pose].bounds.min - posed.poses[0].bounds.min;
    let dir = tempfile::tempdir().expect("reference directory");
    let path = dir.path().join(format!("{map}-{mover}-pose-{pose}.map"));
    std::fs::write(&path, staticize_at(&text, mover, quake_offset(offset))).expect("write");
    let reference = Scene::of(load_map(Path::new(&path)));
    assert_eq!(
        (today.layout.dims, today.layout.world_min),
        (reference.layout.dims, reference.layout.world_min),
        "{map}: both builds lay one probe grid"
    );
    let both_valid =
        |probe: usize| today.layout.validity[probe] != 0 && reference.layout.validity[probe] != 0;

    // Direct: q at the pose scales today's term to the reference's.
    let lights = today.static_channel_lights();
    let cells = ProbeCells::of_layout(&today.layout);
    let bake = today.bake(&lights, &cells);
    let mut q_at: BTreeMap<(u32, usize), u8> = BTreeMap::new();
    for light in &bake.lights {
        for entry in light
            .entries
            .iter()
            .filter(|e| e.mover_id == posed.mover_id)
        {
            for (slot, probe) in cells
                .valid_probes(entry.cell as usize)
                .into_iter()
                .enumerate()
            {
                q_at.insert((light.light_index, probe), entry.poses_at(slot)[pose]);
            }
        }
    }
    let max_abs = |terms: &[f32; 27]| terms.iter().fold(0.0f32, |max, v| max.max(v.abs()));
    let (mut compared, mut worst) = (0, 0.0f32);
    let mut misses = Vec::new();
    for probe in (0..today.layout.probe_positions.len()).filter(|&p| both_valid(p)) {
        for light in &lights {
            let term = today.term(light, probe);
            if max_abs(&term) == 0.0 {
                continue;
            }
            let reference_term = reference.term(light, probe);
            let step = max_abs(&term) / 255.0;
            let q = f32::from(
                q_at.get(&(light.light_index, probe))
                    .copied()
                    .unwrap_or(255),
            );
            let mut gap = [0f32; 27];
            for (k, value) in gap.iter_mut().enumerate() {
                *value = q / 255.0 * term[k] - reference_term[k];
            }
            let miss = max_abs(&gap) / step;
            worst = worst.max(miss);
            compared += 1;
            if miss > 1.0 + 1.0e-4 {
                misses.push(format!(
                    "probe {probe}, light {}: {miss:.2} q steps",
                    light.light_index
                ));
            }
        }
    }
    assert!(
        misses.is_empty(),
        "{map} {mover} pose {pose}: {} of {compared} reached (probe, light) pair(s) miss one q \
         step:\n{}",
        misses.len(),
        misses.join("\n")
    );

    // Bounce one and depth: the mid pose's near-field gather.
    let gathered = with_ctx(&today.fixture, |ctx, poses| {
        let mut plan = plan_half(&today.fixture, ctx, poses, DEFAULT_BOUNCE_FILL);
        let mut request = std::mem::take(&mut plan.request);
        request.jobs.retain(|job| job.mover_id == posed.mover_id);
        request.keep_poses = true;
        let (_, _, outcome) = bake_base(ctx, &plan, &request, DEFAULT_BOUNCE_FILL, None);
        outcome
            .poses
            .into_iter()
            .find(|baked| baked.mover_id == posed.mover_id && baked.pose == pose)
            .expect("the mid pose's gather")
            .domain
    });
    let reference_base = with_ctx(&reference.fixture, |ctx, poses| {
        let plan = plan_half(&reference.fixture, ctx, poses, 0.0);
        bake_base(ctx, &plan, &MoverBounceRequest::default(), 0.0, None).1
    });
    let slots = compact_slots(&reference.layout);
    let dim = reference_base.section.tile_dimension as usize;
    let reference_depth: BTreeMap<u32, &[[u16; 2]]> = reference_base
        .depth_tiles
        .section
        .probe_indices
        .iter()
        .copied()
        .zip(
            reference_base
                .depth_tiles
                .section
                .texels
                .chunks(PROBE_DEPTH_TILE_TEXELS),
        )
        .collect();
    let (mut bounce_compared, mut within_bar, mut worst_bar, mut worst_levels) =
        (0, 0, 0.0f32, 0.0f32);
    let (mut depth_compared, mut depth_misses) = (0, 0);
    for probe in &gathered {
        let at = probe.probe as usize;
        if probe.enclosed || !both_valid(at) {
            continue;
        }
        let one = pack_octahedral_irradiance_tile(
            &probe.coefficients,
            true,
            DEFAULT_IRRADIANCE_TILE_DIMENSION,
            DEFAULT_IRRADIANCE_TILE_BORDER,
        );
        let reference_tile = compact_tile(&reference_base.section, slots[at] as usize);
        let difference = max_abs_difference(&one, &reference_tile);
        let bar = round_trip_error(&reference_tile, dim, false);
        bounce_compared += 1;
        within_bar += usize::from(difference <= bar);
        worst_bar = worst_bar.max(difference - bar);
        let levels = display_levels(&one, &reference_tile);
        worst_levels = worst_levels.max(levels);
        if levels > 2.0 {
            eprintln!(
                "  probe {} at quake {:?}: {levels:.2} levels, |Δ| {difference:.4} bar {bar:.4}",
                probe.probe,
                quake_offset(today.layout.probe_positions[at])
            );
        }
        if let (Some(tile), Some(reference_tile)) =
            (&probe.depth, reference_depth.get(&probe.probe))
        {
            depth_compared += 1;
            let close = tile
                .iter()
                .zip(reference_tile.iter())
                .all(|(a, b)| f16_close(a[0], b[0]) && f16_close(a[1], b[1]));
            depth_misses += usize::from(!close);
        }
    }
    eprintln!(
        "{map} {mover} pose {pose} of {}: direct {compared} reached pair(s), worst {worst:.3} q \
         steps; bounce one {within_bar} of {bounce_compared} domain probe(s) within the reference \
         tile's BC6H bar, worst {worst_bar:.5} past it, worst {worst_levels:.2} display levels; \
         depth {depth_compared} tile(s) against the reference's id 59, {depth_misses} past f16",
        posed.poses.len()
    );
    MidPoseReference {
        direct: (compared, worst),
        bounce: (bounce_compared, within_bar, worst_bar, worst_levels),
        depth: (depth_compared, depth_misses),
    }
}

fn assert_mid_pose_matches(map: &str, mover: &str) {
    let MidPoseReference {
        direct,
        bounce,
        depth,
    } = mid_pose_against_its_reference(map, mover);
    assert!(direct.0 > 0, "{map}: direct compares reached probes");
    let (compared, within, _, worst_levels) = bounce;
    assert!(compared > 0, "{map}: the domain holds probes valid in both");
    // B7's base bar: the reference's own patch seeds differ from the posed
    // sides', so its BC6H bar alone is noise-bound (probe-channel research
    // §B7 investigation); the restated bar is three display levels.
    assert!(
        worst_levels <= 3.0,
        "{map} {mover}: bounce one {worst_levels} display levels from the reference ({within} \
         of {compared} within its BC6H bar)"
    );
    assert!(depth.0 > 0, "{map}: depth compares tiles");
    assert_eq!(depth.1, 0, "{map} {mover}: every depth tile within f16");
}

#[test]
#[ignore = "cold bakes of mover-setpiece and its static-room reference; run on demand"]
fn the_rising_rooms_mid_pose_matches_its_static_brush_reference() {
    assert_mid_pose_matches("mover-setpiece", "setpiece_room");
}

// Known to fail (task 3, 2026-11-10): the sweeper's middle pose puts its
// west face 0.23 m into a 0.5 m floor patch, which the posed slab buries part
// way where the reference's floor is cut at the face; the column of probes
// 0.21 m in front of it reads bounce one up to 4.7 display levels off. The
// error follows each pose's edge phase in the patch grid (1.7 to 4.7 levels
// over the six mid poses; termini, on patch boundaries, 1.8 and 2.0), and a
// path snapped to whole metres brings every mid pose to 2.2 to 3.6. The
// bar is the owner's to restate.
#[test]
#[ignore = "cold bakes of enemy-movers and its static-sweeper reference; run on demand. Known to \
            fail B7's three-level bounce bar on floor-patch phase: see the comment"]
fn the_sweepers_mid_pose_matches_its_static_brush_reference() {
    assert_mid_pose_matches("enemy-movers", "em_sweeper");
}
