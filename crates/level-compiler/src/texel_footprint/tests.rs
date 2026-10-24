// Receiver footprint tests: kernel layout, contact ramps, thin poles, sample-count sweeps, walls.
// See: context/lib/build_pipeline.md §Compiler pipeline (step 12)

use glam::Vec3;

use super::test_fixtures::*;
use super::*;
use crate::lightmap_bake::{DEFAULT_AREA_SAMPLE_COUNT, SOFT_PROBE_SAMPLES};
use crate::texel_step_detector::{TEXEL_STEP_THRESHOLD, VisibilityGrid, texel_steps};

// --- layout ---------------------------------------------------------------

/// Kolmogorov–Smirnov distance between the offsets' radii and the cone
/// kernel's radial CDF `3r² − 2r³`.
fn cone_ks_distance(offsets: &[[f32; 2]]) -> f64 {
    let mut radii: Vec<f64> = offsets
        .iter()
        .map(|&[x, y]| f64::from(x).hypot(f64::from(y)))
        .collect();
    radii.sort_by(f64::total_cmp);
    let n = radii.len() as f64;
    let cdf = |r: f64| {
        let r = r.min(1.0);
        3.0 * r * r - 2.0 * r * r * r
    };
    radii
        .iter()
        .enumerate()
        .map(|(i, &r)| {
            let f = cdf(r);
            (f - i as f64 / n).abs().max(((i + 1) as f64 / n - f).abs())
        })
        .fold(0.0, f64::max)
}

/// The row's two conditions: every offset within one texel pitch, and a
/// radial distribution within `ks_bound` of the cone kernel's.
fn matches_one_texel_cone(offsets: &[[f32; 2]], ks_bound: f64) -> bool {
    let within_pitch = offsets
        .iter()
        .all(|&[x, y]| f64::from(x).hypot(f64::from(y)) <= 1.0);
    within_pitch && cone_ks_distance(offsets) <= ks_bound
}

#[test]
fn footprint_offsets_lie_within_one_pitch_and_follow_the_cone_kernel() {
    let light = hard_point([0.0, 2.0, 0.0]);
    for count in [DEFAULT_AREA_SAMPLE_COUNT, 64] {
        let footprint = TexelFootprint::build(&light, count);
        let offsets = footprint.offsets();
        assert_eq!(offsets.len(), count as usize);
        let n = f64::from(count);
        let ks = cone_ks_distance(offsets);
        // Replacing stratum 0 by the exact centre puts 1/N of the mass at
        // r = 0, so the statistic is at least 1/N; the ring's minimum radius
        // sits below every ring stratum at these counts and moves nothing.
        let bound = 1.0 / n + 1.0e-6;
        println!(
            "count {count}: KS {ks:.5} (1/N {:.5}, bound {bound:.5})",
            1.0 / n
        );
        assert!(
            matches_one_texel_cone(offsets, bound),
            "count {count}: KS {ks} past {bound}"
        );

        // Every rotation of the pattern through a chart's pitch stays within
        // one pitch of the centre, on a non-square grid too.
        let frame = TexelFrame::new(Vec3::X * 0.04, Vec3::Z * 0.1);
        for turn in 0..16 {
            let rotation = (turn as f32 / 16.0 * std::f32::consts::TAU).sin_cos();
            for &offset in offsets {
                let world = frame.offset(offset, rotation);
                let texels = Vec3::new(world.x / 0.04, 0.0, world.z / 0.1);
                assert!(texels.length() <= 1.0 + 1.0e-6, "{offset:?} → {texels}");
            }
        }

        // Negative controls at the same count: a round kernel 1.5 texels
        // wide, and square footprints (one texel across, and one pitch out
        // to each side), fail the same check.
        let wider: Vec<[f32; 2]> = offsets.iter().map(|&[x, y]| [x * 1.5, y * 1.5]).collect();
        let side = (count as f64).sqrt().ceil() as usize;
        let square = |half: f32| -> Vec<[f32; 2]> {
            (0..count as usize)
                .map(|i| {
                    let (ix, iy) = (i % side, i / side);
                    let cell = |k: usize| ((k as f32 + 0.5) / side as f32 * 2.0 - 1.0) * half;
                    [cell(ix), cell(iy)]
                })
                .collect()
        };
        for (name, control) in [
            ("round, radius 1.5", wider),
            ("square, one texel across", square(0.5)),
            ("square, one pitch out", square(1.0)),
        ] {
            println!(
                "count {count}: control {name}: KS {:.4}",
                cone_ks_distance(&control)
            );
            assert!(
                !matches_one_texel_cone(&control, bound),
                "count {count}: {name} footprint passed the kernel check"
            );
        }
    }
}

#[test]
fn footprint_probes_are_the_centre_and_a_ring_within_reach() {
    let light = hard_point([0.0, 2.0, 0.0]);
    for count in [SOFT_PROBE_SAMPLES, 9, 12, 32, 64] {
        let footprint = TexelFootprint::build(&light, count);
        let offsets = footprint.offsets();
        assert_eq!(offsets.len(), (count as usize).max(FOOTPRINT_PROBES));
        assert_eq!(offsets[0], [0.0, 0.0], "probe 0 is the centre");
        let mut angles: Vec<f32> = offsets[1..FOOTPRINT_PROBES]
            .iter()
            .map(|&[x, y]| {
                let r = x.hypot(y);
                assert!(
                    (RING_MIN_RADIUS_TEXELS - 1.0e-5..1.0).contains(&r),
                    "ring radius {r}"
                );
                y.atan2(x).rem_euclid(std::f32::consts::TAU)
            })
            .collect();
        angles.sort_by(f32::total_cmp);
        for (k, angle) in angles.iter().enumerate() {
            let expected = k as f32 * std::f32::consts::FRAC_PI_4;
            assert!(
                (angle - expected).abs() < 1.0e-4,
                "ring angle {angle} vs {expected}"
            );
        }
    }
}

#[test]
fn footprint_emitter_pairing_uses_every_sample_once_and_spreads_the_probes() {
    let point = default_point([0.0, 2.0, 0.0]);
    let mut sun = hard_sun([0.3, -1.0, 0.2]);
    sun.angular_diameter = 4.0;
    for light in [&point, &sun] {
        for count in [9, 16, DEFAULT_AREA_SAMPLE_COUNT, 64] {
            let footprint = TexelFootprint::build(light, count);
            let mut emitter = footprint.emitter().to_vec();
            emitter.sort_unstable();
            assert_eq!(
                emitter,
                (0..count).collect::<Vec<_>>(),
                "{:?} at {count}: each emitter sample pairs with one footprint sample",
                light.light_type
            );
            // The probes span the emitter's polar range: some near each pole.
            let polar: Vec<f32> = footprint.emitter()[..FOOTPRINT_PROBES]
                .iter()
                .map(|&i| (i as f32 + 0.5) / count as f32)
                .collect();
            assert!(
                polar.iter().any(|&t| t < 0.2) && polar.iter().any(|&t| t > 0.8),
                "{:?} at {count}: probe emitter samples {:?}",
                light.light_type,
                &footprint.emitter()[..FOOTPRINT_PROBES]
            );
        }
    }
}

#[test]
fn footprint_memo_matches_a_fresh_build_for_interleaved_lights() {
    let point = default_point([0.0, 2.0, 0.0]);
    let mut spot = hard_spot([1.0, 3.0, -2.0]);
    spot.light_size = 0.5;
    let narrow_sun = {
        let mut sun = hard_sun([0.0, -1.0, 0.0]);
        sun.angular_diameter = 0.01;
        sun
    };
    let wide_sun = {
        let mut sun = hard_sun([0.4, -1.0, 0.1]);
        sun.angular_diameter = 8.0;
        sun
    };
    for count in [
        DEFAULT_AREA_SAMPLE_COUNT,
        16,
        4,
        DEFAULT_AREA_SAMPLE_COUNT,
        64,
    ] {
        for light in [&point, &narrow_sun, &spot, &wide_sun, &point] {
            assert_eq!(
                *footprint_for(light, count),
                TexelFootprint::build(light, count),
                "{:?} at {count}",
                light.light_type
            );
        }
    }
}

// --- fixtures -------------------------------------------------------------

/// A 48×48-texel floor and a plate resting `CONTACT_GAP` above it, turned
/// `yaw` about +Y, whose one edge in view passes through `anchor`. The plate
/// lies on the edge's −x side (local frame).
fn plate_fixture(anchor: Vec3, yaw: f32, height: f32) -> Fixture {
    let receiver = Receiver::centred(48, 48);
    let mut scene = SceneBuilder::default();
    receiver.add_floor(&mut scene);
    scene.oriented_box(
        Vec3::new(anchor.x, 0.0, anchor.z),
        yaw,
        Vec3::new(-4.0, height, -2.0),
        Vec3::new(0.0, height + 0.004, 2.0),
    );
    Fixture::new(scene, receiver)
}

/// The plate's edge axes: along the edge, and toward its lit side.
fn edge_axes(yaw: f32) -> (Vec3, Vec3) {
    let (sin, cos) = yaw.sin_cos();
    (Vec3::new(sin, 0.0, cos), Vec3::new(cos, 0.0, -sin))
}

/// The hard shadow the plate edge casts from `light`'s centre, in texel
/// coordinates: the edge at height `height` projected onto the floor.
fn plate_shadow_edge(
    receiver: &Receiver,
    light: &MapLight,
    anchor: Vec3,
    yaw: f32,
    height: f32,
) -> Edge {
    let (along, lit) = edge_axes(yaw);
    let edge_point = Vec3::new(anchor.x, height, anchor.z);
    let to_light = toward_light(light, edge_point);
    // Follow the ray from the edge away from the light down to the floor.
    let floor_point = edge_point - to_light * (height / to_light.y);
    Edge {
        point: receiver.texel_coords(floor_point),
        direction: [f64::from(along.x), f64::from(along.z)],
        lit_normal: [f64::from(lit.x), f64::from(lit.z)],
    }
}

/// A light on the plate edge's lit side, `offset` metres out and 2 m up.
fn light_beside(kind: &str, anchor: Vec3, yaw: f32, offset: f32) -> MapLight {
    let (_, lit) = edge_axes(yaw);
    let at = Vec3::new(anchor.x, 0.0, anchor.z) + lit * offset + Vec3::Y * 2.0;
    let origin = at.as_dvec3().to_array();
    match kind {
        "hard point" => hard_point(origin),
        "hard spot" => hard_spot(origin),
        "hard sun" => hard_sun((-(at - Vec3::new(anchor.x, 0.0, anchor.z))).to_array()),
        "default point" => default_point(origin),
        _ => unreachable!("{kind}"),
    }
}

const CONTACT_LIGHTS: [&str; 4] = ["hard point", "hard spot", "hard sun", "default point"];

/// Mean ramp of `grid` and of the exact cone-kernel grid across `edge`.
fn ramp_against_floor(fixture: &Fixture, grid: &VisibilityGrid, edge: &Edge) -> (f64, f64, usize) {
    let exact = exact_edge_grid(&fixture.receiver, edge);
    let (baked, profiles) = mean_ramp_width(grid, edge, (-30.0, 30.0), 3.0);
    let (floor, floor_profiles) = mean_ramp_width(&exact, edge, (-30.0, 30.0), 3.0);
    assert_eq!(
        profiles, floor_profiles,
        "both grids cover the same profiles"
    );
    (baked, floor, profiles)
}

// --- AC 1, 4: contact edges -------------------------------------------------

#[test]
fn contact_edges_ramp_over_the_floor_width_for_hard_and_default_lights() {
    for (anchor, yaw_deg) in [
        (Vec3::new(0.013, 0.0, 0.021), 20.0f32),
        (Vec3::new(-0.031, 0.0, 0.007), 37.0),
    ] {
        let yaw = yaw_deg.to_radians();
        let fixture = plate_fixture(anchor, yaw, CONTACT_GAP);
        for kind in CONTACT_LIGHTS {
            let light = light_beside(kind, anchor, yaw, 0.35);
            let grid = fixture.bake(&light, DEFAULT_AREA_SAMPLE_COUNT, true);
            let edge = plate_shadow_edge(&fixture.receiver, &light, anchor, yaw, CONTACT_GAP);
            let (baked, floor, profiles) = ramp_against_floor(&fixture, &grid, &edge);
            let jumps = zero_one_jumps(&grid);
            let steps = texel_steps(&grid, TEXEL_STEP_THRESHOLD);
            println!(
                "{yaw_deg}° {kind}: ramp {baked:.3} vs floor {floor:.3} over {profiles} profiles; \
                 0→1 jumps {jumps}; worst pair {:.3}",
                steps.worst_difference
            );
            assert!(profiles > 100, "{kind}: the edge crosses the grid");
            assert_eq!(jumps, 0, "{yaw_deg}° {kind}: single-texel 0→1 jump");
            assert!(
                (baked - floor).abs() <= 0.2,
                "{yaw_deg}° {kind}: mean ramp {baked} vs floor width {floor}"
            );
            assert_eq!(steps.pairs, 0, "{yaw_deg}° {kind}: detector fired");

            // The floor-off bake of a hard light still steps: the fixture's
            // edge is genuinely sharper than a texel.
            if kind != "default point" {
                assert!(
                    zero_one_jumps(&fixture.bake(&light, DEFAULT_AREA_SAMPLE_COUNT, false)) > 0
                );
            }
        }
    }
}

#[test]
fn texels_beyond_the_footprint_keep_exact_zero_and_one() {
    let (anchor, yaw) = (Vec3::new(0.013, 0.0, 0.021), 20.0f32.to_radians());
    let fixture = plate_fixture(anchor, yaw, CONTACT_GAP);
    for kind in CONTACT_LIGHTS {
        let light = light_beside(kind, anchor, yaw, 0.35);
        let edge = plate_shadow_edge(&fixture.receiver, &light, anchor, yaw, CONTACT_GAP);
        for count in [SOFT_PROBE_SAMPLES, DEFAULT_AREA_SAMPLE_COUNT] {
            let grid = fixture.bake(&light, count, true);
            let (mut lit, mut dark) = (0, 0);
            for y in 0..grid.height() {
                for x in 0..grid.width() {
                    let Some(v) = grid.get(x, y) else { continue };
                    let d = edge.signed_distance([f64::from(x) + 0.5, f64::from(y) + 0.5]);
                    // 0.02 texel absorbs the default light's 1/80-texel spread.
                    if d > 1.02 {
                        assert_eq!(v, 1.0, "{kind} @{count}: lit texel ({x}, {y}) at {d:.3}");
                        lit += 1;
                    } else if d < -1.02 {
                        assert_eq!(v, 0.0, "{kind} @{count}: dark texel ({x}, {y}) at {d:.3}");
                        dark += 1;
                    }
                }
            }
            assert!(lit > 100 && dark > 100, "{kind}: {lit} lit, {dark} dark");
        }
    }
}

// --- AC 3: a clear-standing occluder ---------------------------------------

#[test]
fn clear_standing_occluder_ramp_changes_under_one_texel_with_the_floor() {
    let (anchor, yaw) = (Vec3::new(0.013, 0.0, 0.021), 20.0f32.to_radians());
    let height = 0.5;
    let fixture = plate_fixture(anchor, yaw, height);
    let light = light_beside("default point", anchor, yaw, 0.35);
    let edge = plate_shadow_edge(&fixture.receiver, &light, anchor, yaw, height);
    let ramp = |floor| {
        let grid = fixture.bake(&light, DEFAULT_AREA_SAMPLE_COUNT, floor);
        mean_ramp_width(&grid, &edge, (-30.0, 30.0), 8.0)
    };
    let ((on, profiles), (off, _)) = (ramp(true), ramp(false));
    println!(
        "clear-standing occluder: ramp {on:.3} floored vs {off:.3} centre-point over {profiles} profiles"
    );
    assert!(profiles > 50);
    assert!(
        off > 2.0,
        "the penumbra spans several texels already ({off})"
    );
    assert!((on - off).abs() < 1.0, "ramp moved {} texels", on - off);
}

// --- AC 5, 6: thin pole -----------------------------------------------------

/// A pole `0.4` texel square in section, lying `bottom` above the floor,
/// turned `yaw` about +Y, its axis `phase` texels off the grid centre.
fn pole_fixture(yaw: f32, phase: f32, bottom: f32) -> (Fixture, Vec3) {
    const WIDTH: f32 = 0.4 * PITCH;
    let receiver = Receiver::centred(40, 40);
    let mut scene = SceneBuilder::default();
    receiver.add_floor(&mut scene);
    let (_, across) = edge_axes(yaw);
    let anchor = across * (phase * PITCH);
    scene.oriented_box(
        anchor,
        yaw,
        Vec3::new(-WIDTH / 2.0, bottom, -2.0),
        Vec3::new(WIDTH / 2.0, bottom + WIDTH, -2.0 + 4.0),
    );
    (
        Fixture::new(scene, receiver),
        Vec3::new(anchor.x, bottom + WIDTH / 2.0, anchor.z),
    )
}

/// Centre-blocked texels storing 1.0, and points along the shadow line where
/// every texel bilinear reconstruction blends stores 1.0 (a lit gap).
fn pole_failures(
    fixture: &Fixture,
    light: &MapLight,
    grid: &VisibilityGrid,
    axis: Vec3,
    yaw: f32,
) -> (usize, usize, usize, usize) {
    let mut blocked = 0;
    let mut blocked_lit = 0;
    for y in 0..grid.height() {
        for x in 0..grid.width() {
            if fixture.centre_ray_blocked(light, x, y) {
                blocked += 1;
                if grid.get(x, y) == Some(1.0) {
                    blocked_lit += 1;
                }
            }
        }
    }
    let (along, _) = edge_axes(yaw);
    let to_light = toward_light(light, axis);
    let on_floor = axis - to_light * (axis.y / to_light.y);
    let mut points = 0;
    let mut gaps = 0;
    for k in -1500..=1500 {
        let p = fixture
            .receiver
            .texel_coords(on_floor + along * (k as f32 * 0.001));
        let inner = |c: f64, n: u32| (3.0..f64::from(n) - 3.0).contains(&c);
        if !inner(p[0], grid.width()) || !inner(p[1], grid.height()) {
            continue;
        }
        let Some(taps) = bilinear_taps(grid, p) else {
            continue;
        };
        points += 1;
        if taps.iter().all(|&(v, w)| w == 0.0 || v == 1.0) {
            gaps += 1;
        }
    }
    (blocked, blocked_lit, points, gaps)
}

fn sweep_pole(count: u32) -> (usize, usize) {
    let mut configurations = 0;
    let mut blocked_total = 0;
    for kind in ["hard point", "default point"] {
        for (place, bottom) in [("contact", CONTACT_GAP), ("raised", 0.03)] {
            for yaw_deg in [0.0f32, 13.0, 30.0, 45.0] {
                for phase in [0.0, 0.25, 0.5, 0.75] {
                    let yaw = yaw_deg.to_radians();
                    let (fixture, axis) = pole_fixture(yaw, phase, bottom);
                    let light = light_beside(kind, axis, yaw, 0.3);
                    let grid = fixture.bake(&light, count, true);
                    let (blocked, blocked_lit, points, gaps) =
                        pole_failures(&fixture, &light, &grid, axis, yaw);
                    assert!(points > 500, "{kind} {place}: the line is in view");
                    assert_eq!(
                        blocked_lit, 0,
                        "{kind} {place} {yaw_deg}° phase {phase} @{count}: {blocked_lit} of \
                         {blocked} centre-blocked texels stored 1.0"
                    );
                    assert_eq!(
                        gaps, 0,
                        "{kind} {place} {yaw_deg}° phase {phase} @{count}: {gaps} lit gaps in \
                         {points} line points"
                    );
                    assert_eq!(
                        texel_steps(&grid, TEXEL_STEP_THRESHOLD).pairs,
                        0,
                        "{kind} {place} {yaw_deg}° phase {phase} @{count}: detector fired"
                    );
                    configurations += 1;
                    blocked_total += blocked;
                }
            }
        }
    }
    (configurations, blocked_total)
}

#[test]
fn thin_pole_shadow_survives_the_probe_early_out() {
    let (configurations, blocked) = sweep_pole(DEFAULT_AREA_SAMPLE_COUNT);
    assert!(blocked > 0);
    println!(
        "thin pole @32: {configurations} configurations, {blocked} centre-blocked texels, none lit, no gaps"
    );
}

#[test]
fn minimum_sample_count_keeps_the_pole_line_and_a_silent_detector() {
    let (configurations, blocked) = sweep_pole(SOFT_PROBE_SAMPLES);
    assert!(blocked > 0);
    println!(
        "thin pole @4: {configurations} configurations, {blocked} centre-blocked texels, none lit, no gaps"
    );

    // At the minimum the nine probes are the whole set. The contact ramp
    // spans at least the floor width and the detector stays silent.
    let (fixture, light, edge) = minimum_count_contact_fixture();
    let grid = fixture.bake(&light, SOFT_PROBE_SAMPLES, true);
    let (baked, floor, _) = ramp_against_floor(&fixture, &grid, &edge);
    let steps = texel_steps(&grid, TEXEL_STEP_THRESHOLD);
    println!(
        "contact ramp @4: {baked:.3} vs floor {floor:.3}; worst pair {:.3}",
        steps.worst_difference
    );
    assert!(baked >= floor, "ramp {baked} vs floor {floor}");
    assert_eq!(steps.pairs, 0);
    assert_eq!(zero_one_jumps(&grid), 0);
}

fn minimum_count_contact_fixture() -> (Fixture, MapLight, Edge) {
    let (anchor, yaw) = (Vec3::new(0.013, 0.0, 0.021), 20.0f32.to_radians());
    let fixture = plate_fixture(anchor, yaw, CONTACT_GAP);
    let light = light_beside("hard point", anchor, yaw, 0.35);
    let edge = plate_shadow_edge(&fixture.receiver, &light, anchor, yaw, CONTACT_GAP);
    (fixture, light, edge)
}

// At nine samples the layout is the centre and the ring alone, so the ring's
// minimum radius sets the kernel's spread there.
#[test]
fn minimum_sample_count_contact_ramp_stays_within_the_floor_tolerance() {
    let (fixture, light, edge) = minimum_count_contact_fixture();
    let grid = fixture.bake(&light, SOFT_PROBE_SAMPLES, true);
    let (baked, floor, _) = ramp_against_floor(&fixture, &grid, &edge);
    assert!(
        (baked - floor).abs() <= 0.2,
        "ramp {baked} vs floor {floor}"
    );
}

// --- AC 7: sample-count sweep on a mid-phase edge ---------------------------

#[test]
fn mid_phase_contact_edge_never_snaps_at_any_sample_count() {
    let receiver = Receiver::centred(48, 48);
    // A texel boundary: midway between two texel centres.
    let boundary_x = receiver.x0 + 24.0 * PITCH;
    for (yaw_deg, anchor) in [
        (0.0f32, Vec3::new(boundary_x, 0.0, 0.0)),
        (20.0, Vec3::new(boundary_x, 0.0, 0.0)),
    ] {
        let yaw = yaw_deg.to_radians();
        let fixture = plate_fixture(anchor, yaw, CONTACT_GAP);
        // Straight above the edge line: its shadow falls exactly under it.
        let light = hard_point([f64::from(anchor.x), 2.0, f64::from(anchor.z)]);
        let edge = plate_shadow_edge(&fixture.receiver, &light, anchor, yaw, CONTACT_GAP);
        for count in [4, 5, 6, 7, 8, 12, 16, 24, DEFAULT_AREA_SAMPLE_COUNT, 64] {
            let grid = fixture.bake(&light, count, true);
            let steps = texel_steps(&grid, TEXEL_STEP_THRESHOLD);
            let mut snapped = 0;
            let mut in_reach = 0;
            for y in 0..grid.height() {
                for x in 0..grid.width() {
                    let exact =
                        cone_esf(edge.signed_distance([f64::from(x) + 0.5, f64::from(y) + 0.5]));
                    if exact > 0.1 && exact < 0.9 {
                        in_reach += 1;
                        if matches!(grid.get(x, y), Some(v) if v == 0.0 || v == 1.0) {
                            snapped += 1;
                        }
                    }
                }
            }
            println!(
                "{yaw_deg}° mid-phase edge @{count}: worst axis pair {:.3}, {snapped} of {in_reach} in-reach texels snapped",
                steps.worst_difference
            );
            assert!(in_reach > 40);
            assert_eq!(
                steps.pairs, 0,
                "{yaw_deg}° @{count}: worst {}",
                steps.worst_difference
            );
            assert_eq!(snapped, 0, "{yaw_deg}° @{count}");
        }
    }
}

// --- AC 8, 10: walls ---------------------------------------------------------

/// A floor `width × 40` texels ending at the face of a wall standing at
/// `wall_x`, `thickness` thick and `wall_height` tall.
fn wall_fixture(width: u32, thickness: f32, wall_height: f32) -> (SceneBuilder, Receiver, f32) {
    let mut receiver = Receiver::centred(width, 40);
    receiver.x0 = -(width as f32) * PITCH;
    let wall_x = 0.0;
    let mut scene = SceneBuilder::default();
    receiver.add_floor(&mut scene);
    scene.aabb(
        Vec3::new(wall_x, 0.0, -2.0),
        Vec3::new(wall_x + thickness, wall_height, 2.0),
    );
    (scene, receiver, wall_x)
}

#[test]
fn inside_corner_edges_trace_full_sets_where_clipped() {
    let mut clipped_texels = 0;
    for yaw_deg in [15.0f32, 30.0, 45.0, 60.0, 75.0] {
        for phase in [0.0f32, 0.33, 0.67] {
            let (mut scene, receiver, wall_x) = wall_fixture(32, 0.2, 1.0);
            let yaw = yaw_deg.to_radians();
            // The plate's edge crosses the wall's foot at `yaw` from the wall.
            let anchor = Vec3::new(wall_x, 0.0, (phase - 3.0) * PITCH);
            scene.oriented_box(
                anchor,
                yaw,
                Vec3::new(-4.0, CONTACT_GAP, -2.0),
                Vec3::new(0.0, CONTACT_GAP + 0.004, 2.0),
            );
            let fixture = Fixture::new(scene, receiver);
            // On the floor's side, high: the wall shadows none of it.
            let light = hard_point([-0.5, 2.5, f64::from(anchor.z)]);
            for count in [SOFT_PROBE_SAMPLES, DEFAULT_AREA_SAMPLE_COUNT] {
                let grid = fixture.bake(&light, count, true);
                let outcomes = fixture.footprint_outcomes(&light, count, None);
                for (i, outcome) in outcomes.iter().enumerate() {
                    let (x, y) = (i as u32 % receiver.width, i as u32 / receiver.width);
                    assert_eq!(
                        outcome.map(|o| o.visibility),
                        grid.get(x, y),
                        "replay matches bake"
                    );
                    if let Some(outcome) = outcome
                        && outcome.clipped_probe
                    {
                        clipped_texels += 1;
                        assert!(
                            !outcome.early_out,
                            "{yaw_deg}° phase {phase} @{count}: clipped texel ({x}, {y}) early-out"
                        );
                    }
                }
                // Axis pairs with a texel within one texel of the wall face.
                let near_wall = |x: u32| receiver.width - x <= 1;
                let mut worst = 0.0f32;
                for y in 0..grid.height() {
                    for x in 0..grid.width() {
                        let here = grid.get(x, y).unwrap();
                        let mut pair = |other: f32| worst = worst.max((here - other).abs());
                        if x + 1 < grid.width() && (near_wall(x) || near_wall(x + 1)) {
                            pair(grid.get(x + 1, y).unwrap());
                        }
                        if y + 1 < grid.height() && near_wall(x) {
                            pair(grid.get(x, y + 1).unwrap());
                        }
                    }
                }
                assert!(
                    worst <= TEXEL_STEP_THRESHOLD,
                    "{yaw_deg}° phase {phase} @{count}: pair {worst} at the wall"
                );
                assert_eq!(texel_steps(&grid, TEXEL_STEP_THRESHOLD).pairs, 0);
            }
        }
    }
    println!("inside corner: {clipped_texels} clipped texels, each traced its full set");
    assert!(clipped_texels > 0);
}

#[test]
fn inside_corner_floor_gains_no_shadow_from_inside_the_wall() {
    // A thick wall and a thin one (0.3 texel, under the footprint radius),
    // the light on the floor's side.
    for thickness in [0.2, 0.3 * PITCH] {
        let (scene, receiver, _) = wall_fixture(24, thickness, 1.0);
        let fixture = Fixture::new(scene, receiver);
        let light = hard_point([-0.5, 2.5, 0.0]);
        let grid = fixture.bake(&light, DEFAULT_AREA_SAMPLE_COUNT, true);
        for y in 0..grid.height() {
            for x in receiver.width - 2..receiver.width {
                assert_eq!(grid.get(x, y), Some(1.0), "{thickness} m wall: ({x}, {y})");
            }
        }
        // Control: without the clip, samples inside the wall darken the foot.
        let leaked = fixture
            .footprint_outcomes(&light, DEFAULT_AREA_SAMPLE_COUNT, Some(true))
            .into_iter()
            .flatten()
            .filter(|o| o.visibility < 1.0)
            .count();
        println!("{thickness} m wall, lit side: unclipped control darkens {leaked} texels");
        assert!(leaked > 0);
    }
}

#[test]
fn thin_wall_keeps_its_shadowed_floor_exactly_dark() {
    let thickness = 0.3 * PITCH;
    // The floor on the wall's far side, from its back face outward, and the
    // light 0.5 m before its front face: the wall shadows the first 0.5 m.
    let mut receiver = Receiver::centred(24, 40);
    receiver.x0 = thickness;
    let mut scene = SceneBuilder::default();
    receiver.add_floor(&mut scene);
    scene.aabb(Vec3::new(0.0, 0.0, -2.0), Vec3::new(thickness, 0.5, 2.0));
    let fixture = Fixture::new(scene, receiver);
    let light = hard_point([-0.5, 1.0, 0.0]);
    let grid = fixture.bake(&light, DEFAULT_AREA_SAMPLE_COUNT, true);
    for y in 0..grid.height() {
        for x in 0..2 {
            assert_eq!(grid.get(x, y), Some(0.0), "({x}, {y}) behind the thin wall");
        }
    }
    let leaked = fixture
        .footprint_outcomes(&light, DEFAULT_AREA_SAMPLE_COUNT, Some(true))
        .into_iter()
        .flatten()
        .filter(|o| o.visibility > 0.0 && o.visibility < 1.0)
        .count();
    println!("thin wall, shadowed side: unclipped control leaks light into {leaked} texels");
    assert!(leaked > 0);

    // One floor under both sides: the shadowed side's texels outside the
    // wall still read exactly 0.0 within a texel of it.
    let mut receiver = Receiver::centred(24, 40);
    receiver.x0 = -12.0 * PITCH + 0.1 * PITCH;
    let mut scene = SceneBuilder::default();
    receiver.add_floor(&mut scene);
    scene.aabb(Vec3::new(0.0, 0.0, -2.0), Vec3::new(thickness, 0.5, 2.0));
    let fixture = Fixture::new(scene, receiver);
    let grid = fixture.bake(&light, DEFAULT_AREA_SAMPLE_COUNT, true);
    let mut checked = 0;
    for y in 0..grid.height() {
        for x in 0..grid.width() {
            let centre_x = receiver.x0 + (x as f32 + 0.5) * PITCH;
            let beyond = centre_x - thickness;
            if beyond > 0.0 && beyond <= PITCH {
                assert_eq!(
                    grid.get(x, y),
                    Some(0.0),
                    "({x}, {y}) {beyond} m past the wall"
                );
                checked += 1;
            }
        }
    }
    assert!(checked > 0);
}

// --- switch -----------------------------------------------------------------

#[test]
fn floor_disabled_bakes_centre_point_soft_visibility() {
    let (anchor, yaw) = (Vec3::new(0.013, 0.0, 0.021), 20.0f32.to_radians());
    let fixture = plate_fixture(anchor, yaw, 0.2);
    let chart = fixture.receiver.chart();
    for kind in CONTACT_LIGHTS {
        let light = light_beside(kind, anchor, yaw, 0.35);
        let grid = fixture.bake(&light, DEFAULT_AREA_SAMPLE_COUNT, false);
        let probes = crate::lightmap_bake::SoftProbes::new(&light, DEFAULT_AREA_SAMPLE_COUNT);
        let last_occluder = std::cell::Cell::new(None);
        for y in 0..grid.height() {
            for x in 0..grid.width() {
                let world_p =
                    crate::chart_raster::chart_texel_world_position(&chart, x as i32, y as i32);
                if !crate::lightmap_bake::light_texel_is_covered(&light, world_p, chart.normal) {
                    assert_eq!(grid.get(x, y), None);
                    continue;
                }
                let expected = crate::lightmap_bake::soft_visibility_with(
                    world_p,
                    chart.normal,
                    &light,
                    crate::chart_raster::chart_texel_seed(&chart, x as i32, y as i32),
                    || probes,
                    |from, to| {
                        crate::lightmap_bake::segment_clear_remembering(
                            &fixture.bvh,
                            &fixture.primitives,
                            &fixture.geometry,
                            from,
                            to,
                            &last_occluder,
                        )
                    },
                );
                assert_eq!(
                    grid.get(x, y).map(f32::to_bits),
                    Some(expected.to_bits()),
                    "{kind} ({x}, {y})"
                );
            }
        }
    }
}

// --- clip pre-pass ------------------------------------------------------------

/// Every chart of `atlas`, baked for each light with the clip pre-pass and
/// with every clip segment traced: the layer records must match bit for bit.
/// Returns how many texels the pre-pass let skip their clip segments.
fn assert_prepass_matches_full_clip(
    geometry: &crate::geometry::GeometryResult,
    atlas: &crate::lightmap_layer::SharedAtlas<'_>,
    lights: &[MapLight],
    counts: &[u32],
) -> (usize, usize) {
    let (bvh, primitives, _) = crate::bvh_build::build_bvh(geometry).unwrap();
    let control = crate::bake_control::BakeControl::unrestricted();
    let (mut skipped, mut total) = (0, 0);
    for (face, chart) in atlas.charts.iter().enumerate() {
        if chart.uv_extent[0] <= 0.0 || chart.uv_extent[1] <= 0.0 {
            continue;
        }
        let reach = ClipReach::rasterized(chart, &bvh, &primitives, geometry);
        let (w, h) = crate::chart_raster::chart_interior_dims(chart);
        for ty in 0..h {
            for tx in 0..w {
                total += 1;
                skipped += usize::from(!reach.may_clip(tx, ty));
            }
        }
        for light in lights {
            for &count in counts {
                let fast = crate::lightmap_layer::bake_light_layer_chart_controlled(
                    light,
                    atlas,
                    face,
                    &bvh,
                    &primitives,
                    geometry,
                    count,
                    true,
                    &control,
                );
                let full = crate::lightmap_layer::bake_light_layer_chart_without_clip_prepass(
                    light,
                    atlas,
                    face,
                    &bvh,
                    &primitives,
                    geometry,
                    count,
                    &control,
                );
                assert_eq!(
                    bytemuck::cast_slice::<_, u8>(&fast),
                    bytemuck::cast_slice::<_, u8>(&full),
                    "face {face}, {:?} light, count {count}",
                    light.light_type
                );
            }
        }
    }
    (skipped, total)
}

fn single_chart_atlas<R>(
    receiver: &Receiver,
    check: impl FnOnce(&crate::lightmap_layer::SharedAtlas<'_>) -> R,
) -> R {
    let charts = [receiver.chart()];
    let placements = [crate::chart_raster::ChartPlacement {
        x: 0,
        y: 0,
        layer: 0,
    }];
    let (w, h) = (charts[0].width_texels, charts[0].height_texels);
    let layout = crate::lightmap_bake::BlockLayout::whole_layers(w, h, &placements, 2);
    check(&crate::lightmap_layer::SharedAtlas {
        charts: &charts,
        placements: &placements,
        atlas_width: w,
        atlas_height: h,
        layout: &layout,
    })
}

#[test]
fn clip_prepass_reproduces_the_footprint_fixtures_bit_for_bit() {
    let mut soft_sun = hard_sun([0.3, -1.0, 0.2]);
    soft_sun.angular_diameter = 2.0;
    let lights = |anchor: Vec3, yaw: f32| -> Vec<MapLight> {
        let mut lights: Vec<MapLight> = CONTACT_LIGHTS
            .iter()
            .map(|kind| light_beside(kind, anchor, yaw, 0.35))
            .collect();
        lights.push(soft_sun.clone());
        lights
    };
    let counts = [SOFT_PROBE_SAMPLES, DEFAULT_AREA_SAMPLE_COUNT];
    let mut fixtures = Vec::new();
    let (anchor, yaw) = (Vec3::new(0.013, 0.0, 0.021), 20.0f32.to_radians());
    fixtures.push((plate_fixture(anchor, yaw, CONTACT_GAP), lights(anchor, yaw)));
    for bottom in [CONTACT_GAP, 0.03] {
        let (fixture, axis) = pole_fixture(30.0f32.to_radians(), 0.25, bottom);
        fixtures.push((fixture, lights(axis, 30.0f32.to_radians())));
    }
    for thickness in [0.2, 0.3 * PITCH] {
        let (mut scene, receiver, wall_x) = wall_fixture(24, thickness, 1.0);
        let yaw = 30.0f32.to_radians();
        let anchor = Vec3::new(wall_x, 0.0, -0.1);
        scene.oriented_box(
            anchor,
            yaw,
            Vec3::new(-4.0, CONTACT_GAP, -2.0),
            Vec3::new(0.0, CONTACT_GAP + 0.004, 2.0),
        );
        let mut wall_lights = lights(anchor, yaw);
        wall_lights.push(hard_point([-0.5, 2.5, -0.1]));
        wall_lights.push(hard_point([0.5, 1.0, 0.0]));
        fixtures.push((Fixture::new(scene, receiver), wall_lights));
    }
    for (fixture, lights) in &fixtures {
        let (skipped, total) = single_chart_atlas(&fixture.receiver, |atlas| {
            assert_prepass_matches_full_clip(&fixture.geometry, atlas, lights, &counts)
        });
        println!("fixture: pre-pass skips {skipped} of {total} texels");
    }
}

#[test]
fn clip_prepass_reproduces_a_room_bake_bit_for_bit() {
    // A room: floor, four walls, a box and a turned box at contact, a ramp,
    // a plate just above the floor and a fin thinner than the footprint.
    let mut scene = SceneBuilder::default();
    let (lo, hi, top) = (-2.0f32, 2.0f32, 2.0f32);
    let p = |x: f32, y: f32, z: f32| Vec3::new(x, y, z);
    scene.quad(
        [
            p(lo, 0.0, lo),
            p(lo, 0.0, hi),
            p(hi, 0.0, hi),
            p(hi, 0.0, lo),
        ],
        Vec3::Y,
    );
    scene.quad(
        [
            p(lo, 0.0, lo),
            p(hi, 0.0, lo),
            p(hi, top, lo),
            p(lo, top, lo),
        ],
        Vec3::Z,
    );
    scene.quad(
        [
            p(lo, 0.0, hi),
            p(lo, top, hi),
            p(hi, top, hi),
            p(hi, 0.0, hi),
        ],
        -Vec3::Z,
    );
    scene.quad(
        [
            p(lo, 0.0, lo),
            p(lo, top, lo),
            p(lo, top, hi),
            p(lo, 0.0, hi),
        ],
        Vec3::X,
    );
    scene.quad(
        [
            p(hi, 0.0, lo),
            p(hi, 0.0, hi),
            p(hi, top, hi),
            p(hi, top, lo),
        ],
        -Vec3::X,
    );
    scene.aabb(p(-1.2, 0.0, -1.2), p(-0.7, 0.5, -0.7));
    scene.oriented_box(
        p(0.6, 0.0, -0.8),
        0.6,
        p(-0.25, 0.0, -0.25),
        p(0.25, 0.3, 0.25),
    );
    let ramp_normal = Vec3::new(0.0, 1.0, -0.5).normalize();
    scene.quad(
        [
            p(-0.5, 0.0, 0.5),
            p(-0.5, 0.5, 1.5),
            p(0.5, 0.5, 1.5),
            p(0.5, 0.0, 0.5),
        ],
        ramp_normal,
    );
    scene.oriented_box(
        p(1.0, 0.0, 0.8),
        0.35,
        p(-0.3, CONTACT_GAP, -0.3),
        p(0.3, CONTACT_GAP + 0.004, 0.3),
    );
    scene.aabb(p(-1.5, 0.0, 0.2), p(-1.5 + 0.3 * PITCH, 0.6, 1.4));
    let mut geometry = scene.build();

    let mut soft_sun = hard_sun([0.4, -1.0, 0.25]);
    soft_sun.angular_diameter = 2.0;
    let lights = vec![
        hard_point([0.3, 1.8, 0.2]),
        default_point([-0.6, 1.5, -0.3]),
        hard_spot([1.2, 1.9, 1.0]),
        hard_sun([0.3, -1.0, -0.4]),
        soft_sun,
    ];
    let baked = crate::light_namespaces::StaticBakedLights::from_lights(&lights);
    let prepared = crate::lightmap_bake::prepare_atlas(&mut geometry, &baked, PITCH, &[]).unwrap();
    let atlas = crate::lightmap_layer::SharedAtlas {
        charts: &prepared.charts,
        placements: &prepared.placements,
        atlas_width: prepared.atlas_width,
        atlas_height: prepared.atlas_height,
        layout: &prepared.layout,
    };
    let (skipped, total) = assert_prepass_matches_full_clip(
        &geometry,
        &atlas,
        &lights,
        &[SOFT_PROBE_SAMPLES, DEFAULT_AREA_SAMPLE_COUNT],
    );
    println!(
        "room: pre-pass skips {skipped} of {total} texels over {} charts",
        prepared.charts.len()
    );
    assert!(skipped > 0 && skipped < total);
}

// --- AC 9: a `_lightmap_scale` face ------------------------------------------

/// A `_lightmap_scale` region changes only a chart's texel pitch; this face's
/// is four times the default.
const SCALED_PITCH: f32 = 4.0 * PITCH;

/// A 48×48-texel floor at `pitch`, centred on `(cx, 0, 0)`.
fn receiver_at(cx: f32, pitch: f32) -> Receiver {
    Receiver {
        x0: cx - 24.0 * pitch,
        z0: -24.0 * pitch,
        width: 48,
        height: 48,
        pitch,
    }
}

// P14: one hard light over an unscaled face and a face four times coarser,
// each with a contact plate, baked on one worker in both walk orders so one
// thread's footprint memo is built on one face's pitch and read on the
// other's. Each face's contact ramp is within 0.2 texels of the floor width,
// so both span the same texel count and the world width scales with the
// pitch; every layer record is byte-identical across the two orders.
#[test]
fn a_scaled_face_ramps_over_the_same_texels_in_either_walk_order() {
    use crate::bake_control::BakeControl;
    use crate::chart_raster::{CHART_PADDING_TEXELS, ChartPlacement};
    use crate::lightmap_bake::BlockLayout;
    use crate::lightmap_layer::{LayerTexel, SharedAtlas, bake_light_layer_chart_controlled};

    let faces = [
        (receiver_at(-1.5, PITCH), 20.0f32),
        (receiver_at(5.0, SCALED_PITCH), 200.0),
    ];
    let light = hard_point([0.0, 2.0, 0.0]);
    let mut scene = SceneBuilder::default();
    let mut edges = Vec::new();
    for (receiver, yaw_deg) in &faces {
        receiver.add_floor(&mut scene);
        // The plate scales with the face, and its edge crosses both faces at
        // the same sub-texel phase, lit side toward the light.
        let s = receiver.pitch / PITCH;
        let yaw = yaw_deg.to_radians();
        let anchor = Vec3::new(
            receiver.x0 + 24.3 * receiver.pitch,
            0.0,
            receiver.z0 + 24.6 * receiver.pitch,
        );
        scene.oriented_box(
            anchor,
            yaw,
            Vec3::new(-4.0 * s, CONTACT_GAP, -2.0 * s),
            Vec3::new(0.0, CONTACT_GAP + 0.004, 2.0 * s),
        );
        edges.push(plate_shadow_edge(
            receiver,
            &light,
            anchor,
            yaw,
            CONTACT_GAP,
        ));
    }
    let fixture = Fixture::new(scene, faces[0].0);

    let charts = [faces[0].0.chart(), faces[1].0.chart()];
    let placements = [
        ChartPlacement {
            x: 0,
            y: 0,
            layer: 0,
        },
        ChartPlacement {
            x: charts[0].width_texels,
            y: 0,
            layer: 0,
        },
    ];
    let atlas_width = charts[0].width_texels + charts[1].width_texels;
    let atlas_height = charts[0].height_texels.max(charts[1].height_texels);
    let layout = BlockLayout::whole_layers(atlas_width, atlas_height, &placements, 2);
    let shared = SharedAtlas {
        charts: &charts,
        placements: &placements,
        atlas_width,
        atlas_height,
        layout: &layout,
    };
    // A fresh one-thread pool per order, so its footprint memo starts empty
    // and the first face walked builds it.
    let bake_in = |order: [usize; 2]| -> [Vec<LayerTexel>; 2] {
        rayon::ThreadPoolBuilder::new()
            .num_threads(1)
            .build()
            .expect("one-worker pool")
            .install(|| {
                let mut records: [Vec<LayerTexel>; 2] = Default::default();
                for face in order {
                    records[face] = bake_light_layer_chart_controlled(
                        &light,
                        &shared,
                        face,
                        &fixture.bvh,
                        &fixture.primitives,
                        &fixture.geometry,
                        DEFAULT_AREA_SAMPLE_COUNT,
                        true,
                        &BakeControl::unrestricted(),
                    );
                }
                records
            })
    };
    let (forward, backward) = (bake_in([0, 1]), bake_in([1, 0]));

    let mut ramps = Vec::new();
    for (face, ((receiver, _), edge)) in faces.iter().zip(&edges).enumerate() {
        assert_eq!(
            bytemuck::cast_slice::<_, u8>(&forward[face]),
            bytemuck::cast_slice::<_, u8>(&backward[face]),
            "face {face}: layer records depend on walk order"
        );
        let mut grid = VisibilityGrid::new(receiver.width, receiver.height);
        for texel in &forward[face] {
            let x = texel.idx % atlas_width - placements[face].x - CHART_PADDING_TEXELS;
            let y = texel.idx / atlas_width - CHART_PADDING_TEXELS;
            grid.set(x, y, texel.raw_visibility);
        }
        let exact = exact_edge_grid(receiver, edge);
        let (baked, profiles) = mean_ramp_width(&grid, edge, (-30.0, 30.0), 3.0);
        let (floor, floor_profiles) = mean_ramp_width(&exact, edge, (-30.0, 30.0), 3.0);
        println!(
            "face {face} at {} m pitch: ramp {baked:.3} vs floor {floor:.3} texels over \
             {profiles} profiles; {:.4} m in the world",
            receiver.pitch,
            baked * f64::from(receiver.pitch)
        );
        assert_eq!(
            profiles, floor_profiles,
            "both grids cover the same profiles"
        );
        assert!(profiles > 100, "face {face}: the edge crosses the grid");
        assert_eq!(
            zero_one_jumps(&grid),
            0,
            "face {face}: single-texel 0→1 jump"
        );
        assert!(
            (baked - floor).abs() <= 0.2,
            "face {face}: mean ramp {baked} vs floor width {floor}"
        );
        ramps.push((baked, floor, f64::from(receiver.pitch)));
    }
    // The same edge phase and angle give both faces one floor width, so the
    // two ramps span the same texels and their world widths differ by the
    // pitch ratio.
    let [
        (unscaled, floor, pitch),
        (scaled, scaled_floor, scaled_pitch),
    ] = ramps[..]
    else {
        unreachable!("two faces");
    };
    assert!(
        (floor - scaled_floor).abs() < 0.01,
        "{floor} vs {scaled_floor}"
    );
    assert!(
        (unscaled - scaled).abs() <= 0.4,
        "{unscaled} vs {scaled} texels"
    );
    let world_ratio = (scaled * scaled_pitch) / (unscaled * pitch);
    let pitch_ratio = scaled_pitch / pitch;
    assert!(
        (world_ratio / pitch_ratio - 1.0).abs() <= 0.4 / unscaled,
        "world ramp ratio {world_ratio} vs pitch ratio {pitch_ratio}"
    );
}
