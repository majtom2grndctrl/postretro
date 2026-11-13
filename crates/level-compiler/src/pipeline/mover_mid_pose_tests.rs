// Mid-travel poses at the bounce half's seam: the near-field domain a mid
// pose gathers over, and a mid pose against a static-brush reference bake
// of the mover at that pose (direct q, bounce one, full bounce and depth).
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
    DomainProbe, MoverBounceRequest, ProbeCells, compact_slots, compact_tile, display_levels,
    max_abs_difference, round_trip_error,
};
use crate::sh_bake::{pack_octahedral_irradiance_tile, vec3_from};
use crate::surface_radiance::{DEFAULT_BOUNCE_FILL, PATCH_SIZE_M, ShBase};

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

/// One bounce comparison against the reference: domain probes compared,
/// those within the reference tile's BC6H bar, the worst excess past it,
/// the worst display-level difference, and the probes each patch-grid rule
/// sets aside with their difference.
#[derive(Default)]
struct BounceMatch {
    compared: usize,
    within_bar: usize,
    worst_bar: f32,
    worst_levels: f32,
    /// Set aside by a posed face off the patch grid: count, worst levels.
    by_posed_face: (usize, f32),
    /// Set aside by a reference floor cut off the patch grid, and by no
    /// posed face: each probe with its levels.
    by_floor_cut: Vec<(u32, f32)>,
}

/// Whether the second patch-grid rule, the reference's floor cuts, sets
/// probes aside.
#[derive(Clone, Copy, PartialEq)]
enum FloorCuts {
    SetAside,
    Compared,
}

/// What one mid pose's bake reads against the static-brush reference.
struct MidPoseReference {
    direct: (usize, f32),
    bounce_one: BounceMatch,
    full_bounce: BounceMatch,
    depth: (usize, usize),
}

/// Static patches are this wide, on an absolute plane grid.
const PATCH_M: f64 = PATCH_SIZE_M;

/// Whether a plane at coordinate `plane` on a horizontal axis lands off the
/// static patch grid. A floor face's patch frame runs along x and −z
/// (`surface_radiance::patches::FaceFrame`), so its cell edges sit at whole
/// multiples of the patch on both.
fn off_grid(plane: f64) -> bool {
    let cells = plane / PATCH_M;
    (cells - cells.round()).abs() > 1.0e-4
}

/// The patch-grid exception for posed faces: whether `probe` lies
/// within one patch of a face of `brushes` (posed, axis-aligned bounds) whose
/// plane lands off the static patch grid. There the posed bake's partly
/// buried floor patch differs from the reference's floor cut at the face.
fn near_an_off_grid_face(brushes: &[crate::mover_pose::PosedBrush], probe: DVec3) -> bool {
    brushes.iter().any(|brush| {
        let (lo, hi) = (brush.bounds.min, brush.bounds.max);
        (0..3).any(|axis| {
            let beside = (0..3).filter(|&other| other != axis).all(|other| {
                probe[other] >= lo[other] - PATCH_M && probe[other] <= hi[other] + PATCH_M
            });
            beside
                && [lo[axis], hi[axis]]
                    .into_iter()
                    .any(|plane| off_grid(plane) && (probe[axis] - plane).abs() <= PATCH_M)
        })
    })
}

/// A plane along which the reference's static slab cuts the floor off the
/// patch grid past the slab's own footprint: `axis = plane`, cutting floor
/// faces over `span` along the other horizontal axis.
#[derive(Debug)]
struct FloorCut {
    axis: usize,
    plane: f64,
    span: [f64; 2],
}

impl FloorCut {
    /// The patch-grid exception for the reference's floor cuts: within one
    /// patch of the plane, beside the cut. Deliberately narrower than "a
    /// plane along which the reference cuts the floor off the patch grid":
    /// only along the cut's span, and only planes of the static slab's own
    /// faces that also edge a floor face beyond the slab's footprint
    /// ([`reference_floor_cuts`]), so a probe beside a cut that reaches no
    /// floor past the slab stays compared.
    fn sets_aside(&self, probe: DVec3) -> bool {
        let along = 2 - self.axis;
        (probe[self.axis] - self.plane).abs() <= PATCH_M
            && probe[along] >= self.span[0] - PATCH_M
            && probe[along] <= self.span[1] + PATCH_M
    }
}

/// The reference's floor cuts off the patch grid, from its own faces: the
/// static slab is the reference brush (or brushes) whose faces bound `slab`
/// (the posed brushes it was made static from). Each of its vertical face
/// planes off the patch grid that also edges a floor face beyond the slab's
/// footprint is a cut there: the reference's partition extends the slab's
/// plane across the floor, splitting static patches into slivers the posed
/// bake, whose floor stays whole, never has.
fn reference_floor_cuts(
    reference: &crate::fixture_pipeline::FixturePipeline,
    slab: &[crate::mover_pose::PosedBrush],
) -> Vec<FloorCut> {
    const EPS: f64 = 1.0e-4;
    let within = |p: DVec3, brush: &crate::mover_pose::PosedBrush| {
        (0..3).all(|a| p[a] >= brush.bounds.min[a] - EPS && p[a] <= brush.bounds.max[a] + EPS)
    };
    // The slab's reference brushes: every face vertex inside one posed
    // brush's bounds, the faces together reaching its bounds.
    let mut by_brush: BTreeMap<usize, Vec<&crate::map_data::Face>> = BTreeMap::new();
    for face in &reference.faces {
        by_brush.entry(face.brush_index).or_default().push(face);
    }
    let slab_brushes: BTreeSet<usize> = by_brush
        .iter()
        .filter(|(_, faces)| {
            slab.iter().any(|brush| {
                let (mut lo, mut hi) =
                    (DVec3::splat(f64::INFINITY), DVec3::splat(f64::NEG_INFINITY));
                for p in faces.iter().flat_map(|face| &face.vertices) {
                    lo = lo.min(*p);
                    hi = hi.max(*p);
                }
                faces
                    .iter()
                    .flat_map(|face| &face.vertices)
                    .all(|&p| within(p, brush))
                    && lo.abs_diff_eq(brush.bounds.min, EPS)
                    && hi.abs_diff_eq(brush.bounds.max, EPS)
            })
        })
        .map(|(&brush, _)| brush)
        .collect();
    assert!(
        !slab_brushes.is_empty(),
        "the reference holds the static slab's faces"
    );
    // Its vertical face planes, on x or z, off the patch grid.
    let mut planes: BTreeSet<(usize, i64)> = BTreeSet::new();
    for face in slab_brushes.iter().flat_map(|brush| &by_brush[brush]) {
        for axis in [0, 2] {
            if face.normal[axis].abs() > 1.0 - 1.0e-9 {
                let plane = face.distance / face.normal[axis];
                if off_grid(plane) {
                    planes.insert((axis, (plane / EPS).round() as i64));
                }
            }
        }
    }
    // The parts of `[lo, hi]` along `along` outside every slab brush.
    let beyond_footprint = |lo: f64, hi: f64, along: usize| {
        let mut pieces = vec![[lo, hi]];
        for brush in slab {
            let (f0, f1) = (brush.bounds.min[along], brush.bounds.max[along]);
            pieces = pieces
                .into_iter()
                .flat_map(|[a, b]| [[a, b.min(f0)], [a.max(f1), b]])
                .filter(|[a, b]| b - a > EPS)
                .collect();
        }
        pieces
    };
    let mut cuts = Vec::new();
    for (axis, key) in planes {
        let plane = key as f64 * EPS;
        let along = 2 - axis;
        let mut span = [f64::INFINITY, f64::NEG_INFINITY];
        for face in reference
            .faces
            .iter()
            .filter(|face| face.normal.y > 0.7 && !slab_brushes.contains(&face.brush_index))
        {
            let n = face.vertices.len();
            for k in 0..n {
                let (a, b) = (face.vertices[k], face.vertices[(k + 1) % n]);
                if (a[axis] - plane).abs() > EPS || (b[axis] - plane).abs() > EPS {
                    continue;
                }
                // The edge's reach beyond the slab's footprint.
                let (lo, hi) = (a[along].min(b[along]), a[along].max(b[along]));
                for [a, b] in beyond_footprint(lo, hi, along) {
                    span = [span[0].min(a), span[1].max(b)];
                }
            }
        }
        if span[0] <= span[1] {
            cuts.push(FloorCut { axis, plane, span });
        }
    }
    cuts
}

/// Bake `mover` of dev map `map` at its middle mid pose and the map with the
/// mover made static there, then compare: direct q at every probe a light
/// reaches, valid in both, within one q step (B7's direct bar); at each
/// domain probe valid in both and not enclosed, the mid pose's bounce one
/// against the reference's first bounce alone (`_bounce_fill 0`) and its
/// full bounce against the reference's full bake; and its
/// closed-pose depth tile against the reference's id-59 tile where both
/// exist, texel by texel within f16 (B4's bar). Bounce probes the
/// patch-grid rules cover are set aside from both bounce bars; the
/// reference's floor cuts only under [`FloorCuts::SetAside`].
fn mid_pose_against_its_reference(
    map: &str,
    mover: &str,
    floor_cuts: FloorCuts,
) -> MidPoseReference {
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

    // Bounce and depth: the mid pose's near-field gathers.
    let kept = with_ctx(&today.fixture, |ctx, poses| {
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
    });
    let reference_base = |fill: f32| {
        with_ctx(&reference.fixture, |ctx, poses| {
            let plan = plan_half(&reference.fixture, ctx, poses, fill);
            bake_base(ctx, &plan, &MoverBounceRequest::default(), fill, None).1
        })
    };
    let (reference_one, reference_full) =
        (reference_base(0.0), reference_base(DEFAULT_BOUNCE_FILL));
    let slots = compact_slots(&reference.layout);
    let by_posed_face = |probe: usize| {
        near_an_off_grid_face(
            &posed.poses[pose].brushes,
            today.layout.probe_positions[probe],
        )
    };
    let cuts = match floor_cuts {
        FloorCuts::SetAside => reference_floor_cuts(&reference.fixture, &posed.poses[pose].brushes),
        FloorCuts::Compared => Vec::new(),
    };
    for cut in &cuts {
        eprintln!(
            "  reference floor cut off the patch grid: {} = {:.4} m over {} {:.4}–{:.4} m",
            ["x", "y", "z"][cut.axis],
            cut.plane,
            ["x", "y", "z"][2 - cut.axis],
            cut.span[0],
            cut.span[1]
        );
    }
    let by_floor_cut = |probe: usize| {
        cuts.iter()
            .any(|cut| cut.sets_aside(today.layout.probe_positions[probe]))
    };
    // A probe the pose encloses stores its dark posed state, which a static
    // reference holding the brush has no valid probe to compare against.
    let encloses = |probe: usize| {
        posed.poses[pose].encloses(crate::sh_bake::vec3_from(
            today.layout.probe_positions[probe],
        ))
    };
    let compare = |gathered: &[DomainProbe], reference: &ShBase, label: &str| {
        let dim = reference.section.tile_dimension as usize;
        let mut matched = BounceMatch::default();
        for probe in gathered {
            let at = probe.probe as usize;
            if encloses(probe.probe as usize) || !both_valid(at) {
                continue;
            }
            let tile = pack_octahedral_irradiance_tile(
                &probe.coefficients,
                true,
                DEFAULT_IRRADIANCE_TILE_DIMENSION,
                DEFAULT_IRRADIANCE_TILE_BORDER,
            );
            let reference_tile = compact_tile(&reference.section, slots[at] as usize);
            let levels = display_levels(&tile, &reference_tile);
            if by_posed_face(at) {
                matched.by_posed_face.0 += 1;
                matched.by_posed_face.1 = matched.by_posed_face.1.max(levels);
                continue;
            }
            if by_floor_cut(at) {
                matched.by_floor_cut.push((probe.probe, levels));
                continue;
            }
            let difference = max_abs_difference(&tile, &reference_tile);
            let bar = round_trip_error(&reference_tile, dim, false);
            matched.compared += 1;
            matched.within_bar += usize::from(difference <= bar);
            matched.worst_bar = matched.worst_bar.max(difference - bar);
            matched.worst_levels = matched.worst_levels.max(levels);
            if levels > 2.0 {
                eprintln!(
                    "  {label} probe {} at quake {:?}: {levels:.2} levels, |Δ| {difference:.4} \
                     bar {bar:.4}",
                    probe.probe,
                    quake_offset(today.layout.probe_positions[at])
                );
            }
        }
        matched
    };
    let bounce_one = compare(&kept.domain_bounce_one, &reference_one, "bounce one");
    let full_bounce = compare(&kept.domain, &reference_full, "full bounce");
    let reference_depth: BTreeMap<u32, &[[u16; 2]]> = reference_one
        .depth_tiles
        .section
        .probe_indices
        .iter()
        .copied()
        .zip(
            reference_one
                .depth_tiles
                .section
                .texels
                .chunks(PROBE_DEPTH_TILE_TEXELS),
        )
        .collect();
    let (mut depth_compared, mut depth_misses) = (0, 0);
    for probe in &kept.domain {
        if encloses(probe.probe as usize) || !both_valid(probe.probe as usize) {
            continue;
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
    let line = |label: &str, matched: &BounceMatch| {
        let cut_worst = matched
            .by_floor_cut
            .iter()
            .fold(0.0f32, |worst, &(_, levels)| worst.max(levels));
        format!(
            "{label} {} of {} domain probe(s) within the reference tile's BC6H bar, worst {:.5} \
             past it, worst {:.2} display levels; set aside: {} probe(s) by an off-grid posed \
             face, worst {:.2} levels; {} more by a reference floor cut{}, worst {:.2} levels \
             {:?}",
            matched.within_bar,
            matched.compared,
            matched.worst_bar,
            matched.worst_levels,
            matched.by_posed_face.0,
            matched.by_posed_face.1,
            matched.by_floor_cut.len(),
            match floor_cuts {
                FloorCuts::SetAside => "",
                FloorCuts::Compared => " (rule off)",
            },
            cut_worst,
            matched
                .by_floor_cut
                .iter()
                .map(|&(probe, levels)| format!("{probe}: {levels:.2}"))
                .collect::<Vec<_>>()
        )
    };
    eprintln!(
        "{map} {mover} pose {pose} of {}: direct {compared} reached pair(s), worst {worst:.3} q \
         steps; {}; {}; depth {depth_compared} tile(s) against the reference's id 59, \
         {depth_misses} past f16",
        posed.poses.len(),
        line("bounce one", &bounce_one),
        line("full bounce", &full_bounce),
    );
    MidPoseReference {
        direct: (compared, worst),
        bounce_one,
        full_bounce,
        depth: (depth_compared, depth_misses),
    }
}

/// The sweeper probe traced to the reference's floor cut: 0.91 m
/// ahead of the slab's leading face, inside one patch of its north end-face
/// plane, which the reference extends across the floor into 0.060 and
/// 0.190 m² slivers. It reads 4.01 levels in bounce one and 3.51 in the full
/// bounce at the middle mid pose.
const SWEEPER_FLOOR_CUT_PROBE: u32 = 2267;

/// Assert the reference bars at the mover's middle mid pose; returns the
/// comparison for the caller's own checks.
fn assert_mid_pose_matches(map: &str, mover: &str, floor_cuts: FloorCuts) -> MidPoseReference {
    let reference = mid_pose_against_its_reference(map, mover, floor_cuts);
    let MidPoseReference {
        direct,
        bounce_one,
        full_bounce,
        depth,
    } = &reference;
    assert!(direct.0 > 0, "{map}: direct compares reached probes");
    // B7's base bar: the reference's own patch seeds differ from the posed
    // sides', so its BC6H bar alone is noise-bound (probe-channel research
    // §B7 investigation); the restated bar is three display levels, outside
    // the patch-grid exceptions.
    for (label, matched) in [("bounce one", bounce_one), ("full bounce", full_bounce)] {
        assert!(
            matched.compared > 0,
            "{map}: the domain holds probes valid in both"
        );
        assert!(
            matched.worst_levels <= 3.0,
            "{map} {mover}: {label} {} display levels from the reference ({} of {} within its \
             BC6H bar)",
            matched.worst_levels,
            matched.within_bar,
            matched.compared
        );
    }
    assert!(depth.0 > 0, "{map}: depth compares tiles");
    assert_eq!(depth.1, 0, "{map} {mover}: every depth tile within f16");
    reference
}

// Bounce one, direct, depth and full bounce at the rising room's middle mid
// pose, with the posed-face exception alone.
#[test]
#[ignore = "cold bakes of mover-setpiece and its static-room reference at two fills: cargo test \
            -p postretro-level-compiler --bin prl-build \
            the_rising_rooms_mid_pose_matches_its_static_brush_reference -- --ignored --nocapture"]
fn the_rising_rooms_mid_pose_matches_its_static_brush_reference() {
    assert_mid_pose_matches("mover-setpiece", "setpiece_room", FloorCuts::Compared);
}

// The same bars at the sweeper's middle mid pose. Its west face lands
// part-way into a 0.5 m floor patch, which the posed slab buries part way
// where the reference's floor is cut at the face; the probes within one
// patch of that face are the posed-face exception. The reference's static
// slab also cuts the floor off the patch grid along its own face planes,
// past its footprint; the probes within one patch of such a cut are the
// floor-cut exception, and it must catch the probe that cut was traced to.
#[test]
#[ignore = "cold bakes of enemy-movers and its static-sweeper reference at two fills: cargo test \
            -p postretro-level-compiler --bin prl-build \
            the_sweepers_mid_pose_matches_its_static_brush_reference -- --ignored --nocapture"]
fn the_sweepers_mid_pose_matches_its_static_brush_reference() {
    let reference = assert_mid_pose_matches("enemy-movers", "em_sweeper", FloorCuts::SetAside);
    for (label, matched, read) in [
        ("bounce one", &reference.bounce_one, 4.0),
        ("full bounce", &reference.full_bounce, 3.5),
    ] {
        let caught = matched
            .by_floor_cut
            .iter()
            .find(|(probe, _)| *probe == SWEEPER_FLOOR_CUT_PROBE);
        assert!(
            caught.is_some_and(|&(_, levels)| levels >= read),
            "{label}: the floor-cut rule sets aside probe {SWEEPER_FLOOR_CUT_PROBE} at its \
             traced {read}+ levels; caught {caught:?}"
        );
    }
}
