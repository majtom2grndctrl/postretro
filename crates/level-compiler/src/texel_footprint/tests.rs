// Receiver footprint tests: kernel layout, contact ramps, thin poles, sample-count sweeps, walls.
// See: context/lib/build_pipeline.md §Compiler pipeline (step 12)

use glam::{Vec2, Vec3};

use super::pairing::{RING_DIRECTIONS, ring_sector};
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
        // Stratum 0 at the exact centre puts 1/N of the mass at r = 0. The
        // ring puts the eight strata of a quantile band at one radius, so
        // just inside and just outside it the empirical CDF sits half the
        // band, 4/N, from the cone's, plus up to 1/(2N) for how far the
        // band's centre falls from F(0.6).
        let bound = 4.0 / n + 0.5 / n + 1.0e-6;
        println!("count {count}: KS {ks:.5} (bound {bound:.5})");
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
                    (r - RING_RADIUS_TEXELS).abs() < 1.0e-6,
                    "count {count}: ring radius {r}"
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

// Per texel, each ring probe pairs with the emitter sample reaching farthest
// within its 45° sector of the receiver plane, each counter-ring stratum with
// the unclaimed sample reaching farthest against its own offset, and the
// centre's ray to the emitter's centre stands in for the sample nearest it.
// Emitter offsets come from the live sampler at the texel's seed, carried onto
// the receiver along the light, so this also pins the lattice spin and the
// projection the search folds into two vectors.
#[test]
fn footprint_samples_pair_with_emitter_samples_along_or_against_their_offsets() {
    use crate::lightmap_bake::{
        RAY_EPSILON, area_sample_target, emitter_lattice_spin, hard_ray_target,
        probe_sample_direction,
    };
    let point = default_point([0.4, 2.0, -0.3]);
    let mut sun = hard_sun([0.3, -1.0, 0.2]);
    sun.angular_diameter = 4.0;
    // A wall-like receiver too, lit obliquely.
    let floor = (Vec3::Y, TexelFrame::new(Vec3::X * PITCH, Vec3::Z * PITCH));
    let wall = (
        -Vec3::X,
        TexelFrame::new(Vec3::Z * PITCH, Vec3::Y * (1.5 * PITCH)),
    );
    let sector_slack = 0.05f32.to_radians().cos();
    for light in [&point, &sun] {
        for count in [9, 16, DEFAULT_AREA_SAMPLE_COUNT, 64] {
            let footprint = TexelFootprint::build(light, count);
            let (mut worst_angle, mut empty_sectors) = (0.0f32, 0);
            for texel in 0..64u64 {
                let (normal, frame) = if texel % 2 == 0 { floor } else { wall };
                let world_p = Vec3::new(texel as f32 * 0.07 - 2.0, 0.1, (texel % 7) as f32 * 0.3);
                let site = TexelSite {
                    world_p,
                    normal,
                    seed: texel.wrapping_mul(0x9E37_79B9_7F4A_7C15),
                    frame,
                    may_clip: false,
                };
                let axis = toward_light(light, world_p + normal * RAY_EPSILON);
                if normal.dot(axis) < 0.05 {
                    continue;
                }
                let centre = hard_ray_target(world_p, light);
                let live = |i: u32| area_sample_target(world_p, light, site.seed, i, count);
                let spin = emitter_lattice_spin(light, site.seed);
                for i in 0..count {
                    let turned = spin * probe_sample_direction(light, i, count);
                    let live = match light.light_type {
                        LightType::Directional => (live(i) - world_p).normalize(),
                        LightType::Point | LightType::Spot => (live(i) - centre).normalize(),
                    };
                    assert!(
                        turned.distance(live) < 1.0e-4,
                        "{:?} sample {i}: spun lattice {turned} vs live {live}",
                        light.light_type
                    );
                }
                // Sample `i`'s emitter offset carried onto the receiver along
                // the light, in the footprint's unrotated texel frame.
                let (sin, cos) = footprint_rotation(site.seed);
                let position = |i: u32| {
                    let offset = live(i) - centre;
                    let on_receiver = offset - axis * (normal.dot(offset) / normal.dot(axis));
                    let texels = |pitch: Vec3| on_receiver.dot(pitch) / pitch.length_squared();
                    let (u, v) = (texels(frame.pitch_u), texels(frame.pitch_v));
                    Vec2::new(u * cos + v * sin, v * cos - u * sin)
                };

                let indices = footprint.emitter_indices(light, &site);
                let mut sorted = indices.clone();
                sorted.sort_unstable();
                assert_eq!(
                    sorted,
                    (0..count).collect::<Vec<_>>(),
                    "{:?} at {count}: each emitter sample pairs with one footprint sample",
                    light.light_type
                );
                let scale = (0..count).map(|i| position(i).length()).fold(0.0, f32::max);
                let tolerance = 1.0e-3 * scale;
                let nearest = (0..count)
                    .map(|i| position(i).length())
                    .fold(f32::INFINITY, f32::min);
                assert!(
                    position(indices[0]).length() <= nearest + tolerance,
                    "{:?} at {count}: the centre stands in for a sample other than the nearest",
                    light.light_type
                );
                for k in 1..FOOTPRINT_PROBES {
                    let sector = (k - 1) * RING_POSITION_STRIDE % RING_SAMPLES;
                    let way = Vec2::from(RING_DIRECTIONS[sector]);
                    let inside = |i: u32| {
                        let p = position(i);
                        i != indices[0]
                            && p.dot(way)
                                >= p.length() * (22.5f32.to_radians().cos() * sector_slack)
                    };
                    let well_inside = |i: u32| {
                        let p = position(i);
                        i != indices[0]
                            && p.dot(way) * sector_slack >= p.length() * 22.5f32.to_radians().cos()
                    };
                    let best = (0..count)
                        .filter(|&i| well_inside(i))
                        .map(|i| position(i).dot(way))
                        .fold(f32::NEG_INFINITY, f32::max);
                    if best == f32::NEG_INFINITY {
                        empty_sectors += 1;
                        continue;
                    }
                    assert!(
                        inside(indices[k]) && position(indices[k]).dot(way) >= best - tolerance,
                        "{:?} at {count}: ring probe {k} does not take its sector's farthest sample",
                        light.light_type
                    );
                    worst_angle =
                        worst_angle.max(position(indices[k]).angle_to(way).abs().to_degrees());
                }
                // The counter ring, from the largest stratum inward, takes the
                // unclaimed sample reaching farthest against each offset.
                let len = indices.len();
                let mut claimed = indices[..FOOTPRINT_PROBES].to_vec();
                for j in 0..(len - FOOTPRINT_PROBES).min(8) {
                    let k = len - 1 - j;
                    let against = -Vec2::from(footprint.offsets()[k]).normalize();
                    let reach = |i: u32| position(i).dot(against);
                    let best = (0..count)
                        .filter(|i| !claimed.contains(i))
                        .map(reach)
                        .fold(f32::NEG_INFINITY, f32::max);
                    assert!(
                        reach(indices[k]) >= best - tolerance,
                        "{:?} at {count}: counter-ring stratum {k} does not take the farthest \
                         unclaimed sample against its offset",
                        light.light_type
                    );
                    claimed.push(indices[k]);
                }
            }
            println!(
                "{:?} at {count}: worst ring-probe emitter offset {worst_angle:.1}° off its \
                 direction; {empty_sectors} empty sectors",
                light.light_type
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

// --- probe claim: keyed pass against the reference scan ---------------------

/// Branching reference sector the keyed pass must reproduce: a scan of the
/// eight signed directions, keeping the first strictly farther one.
fn ring_sector_reference([x, y]: [f32; 2]) -> (usize, f32) {
    let diagonal = std::f32::consts::FRAC_1_SQRT_2;
    let along = [x, (x + y) * diagonal, y, (y - x) * diagonal];
    let mut sector = 0;
    let mut reach = x;
    for (position, value) in along.into_iter().enumerate() {
        if value > reach {
            (sector, reach) = (position, value);
        }
        if -value > reach {
            (sector, reach) = (position + RING_SAMPLES / 2, -value);
        }
    }
    (sector, reach)
}

/// Branching reference claim the keyed pass must reproduce sample for
/// sample: [`TexelFootprint::claim_probes`] with a branch on every comparison.
fn claim_probes_reference(footprint: &TexelFootprint, pairing: &mut EmitterPairing) {
    let mut nearest = (f32::INFINITY, 0u32);
    let mut first = [(f32::NEG_INFINITY, u32::MAX); RING_SAMPLES];
    let mut second = [(f32::NEG_INFINITY, u32::MAX); RING_SAMPLES];
    for (index, offset) in footprint.lattice.iter().enumerate() {
        let index = index as u32;
        let position = pairing.frame_position(*offset);
        let radius_sq = position[0] * position[0] + position[1] * position[1];
        if radius_sq < nearest.0 {
            nearest = (radius_sq, index);
        }
        let (sector, reach) = ring_sector_reference(position);
        if reach > first[sector].0 {
            second[sector] = first[sector];
            first[sector] = (reach, index);
        } else if reach > second[sector].0 {
            second[sector] = (reach, index);
        }
    }
    pairing.probes[0] = nearest.1;
    pairing.claimed[0] = nearest.1;
    pairing.claimed_len = 1;
    let mut unfilled = 0u16;
    for k in 1..FOOTPRINT_PROBES {
        let sector = (k - 1) * RING_POSITION_STRIDE % RING_SAMPLES;
        let (_, index) = if first[sector].1 == nearest.1 {
            second[sector]
        } else {
            first[sector]
        };
        if index == u32::MAX {
            unfilled |= 1 << k;
        } else {
            pairing.probes[k] = index;
            pairing.claimed[pairing.claimed_len] = index;
            pairing.claimed_len += 1;
        }
    }
    for k in (1..FOOTPRINT_PROBES).filter(|k| unfilled & (1 << k) != 0) {
        let sector = (k - 1) * RING_POSITION_STRIDE % RING_SAMPLES;
        let index = footprint.farthest_unclaimed(pairing, RING_DIRECTIONS[sector]);
        pairing.probes[k] = index;
        pairing.claimed[pairing.claimed_len] = index;
        pairing.claimed_len += 1;
    }
}

/// A deterministic xorshift stream of floats in `[-1, 1)`.
struct Stream(u64);

impl Stream {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 40) as f32 / (1u64 << 23) as f32 - 1.0
    }

    fn unit(&mut self) -> Vec3 {
        loop {
            let v = Vec3::new(self.next(), self.next(), self.next());
            if (0.01..1.0).contains(&v.length_squared()) {
                return v.normalize();
            }
        }
    }
}

// The keyed, branch-free probe claim selects the reference scan's samples
// for point, spot and sun lights at every count from 4 to 64, over random
// seeds, receivers and pitches, with the light from straight overhead down
// to grazing and below the projection floor; and the counter ring and the
// interior strata built on it follow.
#[test]
fn keyed_probe_claim_selects_the_reference_scans_samples() {
    let mut stream = Stream(0x5eed_1234_abcd_0001);
    let mut lights = vec![point_light([0.3, 1.7, -0.4], 0.25), {
        let mut spot = hard_spot([-0.8, 2.4, 0.6]);
        spot.light_size = 0.4;
        spot
    }];
    for (aim, diameter) in [
        ([0.3, -1.0, 0.2], 4.0),
        ([0.0, -1.0, 0.0], 0.5),
        ([1.0, -0.02, 0.3], 20.0),
        ([-0.6, -0.8, 0.0], 0.01),
    ] {
        let mut sun = hard_sun(aim);
        sun.angular_diameter = diameter;
        lights.push(sun);
    }
    // Cosines between the receiver normal and the light: overhead to grazing,
    // straddling the projection floor, and from behind.
    let facings: [f32; 9] = [1.0, 0.7, 0.2, 0.01, 2.0e-3, 1.0e-3, 5.0e-4, 0.0, -0.3];
    let mut compared = 0;
    for light in &lights {
        for count in 4..=64 {
            let footprint = TexelFootprint::build(light, count);
            for (site_index, &facing) in facings.iter().cycle().take(36).enumerate() {
                let world_p = Vec3::new(stream.next(), stream.next() * 0.2, stream.next()) * 2.0;
                let axis = toward_light(light, world_p);
                let random = stream.unit();
                let across = (random - axis * axis.dot(random)).normalize_or_zero();
                let across = if across == Vec3::ZERO {
                    axis.any_orthonormal_vector()
                } else {
                    across
                };
                let side = (1.0 - facing * facing).max(0.0).sqrt();
                let normal = (axis * facing + across * side).normalize();
                let tangent = normal.any_orthonormal_vector();
                let turn = stream.next() * std::f32::consts::PI;
                let tangent = tangent * turn.cos() + normal.cross(tangent) * turn.sin();
                let pitch = PITCH * (1.0 + 0.5 * stream.next().abs());
                let frame = TexelFrame::new(
                    tangent * pitch,
                    normal.cross(tangent) * (pitch * (1.0 + 0.3 * stream.next().abs())),
                );
                let seed = stream.0 ^ (site_index as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
                let site = TexelSite {
                    world_p,
                    normal,
                    seed,
                    frame,
                    may_clip: false,
                };
                let rotation = footprint_rotation(seed);
                let mut keyed = footprint.pairing_frame(light, &site, rotation);
                let mut reference = keyed;
                footprint.claim_probes(&mut keyed);
                claim_probes_reference(&footprint, &mut reference);
                assert_eq!(
                    keyed.probes, reference.probes,
                    "{:?} at {count}, facing {facing}, seed {seed:#x}",
                    light.light_type
                );
                footprint.claim_counter_ring(&mut keyed);
                footprint.claim_counter_ring(&mut reference);
                for k in 0..footprint.offsets.len() {
                    assert_eq!(
                        footprint.emitter_index(&keyed, k),
                        footprint.emitter_index(&reference, k),
                        "{:?} at {count}, sample {k}",
                        light.light_type
                    );
                }
                compared += 1;
            }
        }
    }
    println!("keyed probe claim matches the reference scan on {compared} texels");
}

// Exact ties, which random geometry almost never produces: a lattice drawn
// from a few points on the axes and diagonals, zero and signed zero among
// them, repeated, so samples tie on reach, on radius and on sector borders.
// The keyed pass breaks every tie as the reference scan does.
#[test]
fn keyed_probe_claim_breaks_ties_as_the_reference_scan_does() {
    let points = [
        [1.0, 0.0],
        [0.0, 1.0],
        [-1.0, 0.0],
        [0.0, -1.0],
        [1.0, 1.0],
        [-1.0, 1.0],
        [-1.0, -1.0],
        [1.0, -1.0],
        [2.0, 0.0],
        [0.0, 0.0],
        [-0.0, -0.0],
        [0.5, 0.5],
        [-2.0, 2.0],
        [0.0, -0.5],
    ];
    for &[x, y] in &points {
        let (sector, reach) = ring_sector([x, y]);
        let (expected, expected_reach) = ring_sector_reference([x, y]);
        assert_eq!(sector, expected, "sector of ({x}, {y})");
        assert_eq!(reach, expected_reach, "reach of ({x}, {y})");
    }
    let light = default_point([0.0, 2.0, 0.0]);
    let mut stream = Stream(0x7135_0000_0000_0001);
    let mut compared = 0;
    for count in 9..=40 {
        let mut footprint = TexelFootprint::build(&light, count);
        for _ in 0..32 {
            for offset in footprint.lattice.iter_mut() {
                let pick = ((stream.next() + 1.0) * 0.5 * points.len() as f32) as usize;
                let [x, y] = points[pick.min(points.len() - 1)];
                *offset = Vec3::new(x, stream.next(), y);
            }
            let mut keyed = EmitterPairing {
                x_axis: Vec3::X,
                y_axis: Vec3::Z,
                probes: [0; FOOTPRINT_PROBES],
                counter: [0; COUNTER_RING_SAMPLES],
                claimed: [0; FOOTPRINT_PROBES + COUNTER_RING_SAMPLES],
                claimed_len: 0,
            };
            let mut reference = keyed;
            footprint.claim_probes(&mut keyed);
            claim_probes_reference(&footprint, &mut reference);
            assert_eq!(keyed.probes, reference.probes, "count {count}");
            compared += 1;
        }
    }
    println!("tie-heavy lattices: {compared} claims match");
}

// --- fixtures -------------------------------------------------------------

/// A 48×48-texel floor and a plate resting `CONTACT_GAP` above it, turned
/// `yaw` about +Y, whose one edge in view passes through `anchor`. The plate
/// lies on the edge's −x side (local frame).
fn plate_fixture(anchor: Vec3, yaw: f32, height: f32) -> Fixture {
    plate_fixture_on(Receiver::centred(48, 48), anchor, yaw, height)
}

/// [`plate_fixture`] over `receiver`.
fn plate_fixture_on(receiver: Receiver, anchor: Vec3, yaw: f32, height: f32) -> Fixture {
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

// --- AC 12: one light baked static and animated ----------------------------

// The same light over the contact plate, baked as a static layer and as an
// animated weight map: both ramp over the floor width, and their tracers
// differ only on box-boundary hits, so they agree per texel within one
// shadow-sample fraction. Weight maps never hold a sun (the chunk builder
// skips directional lights).
#[test]
fn a_light_baked_static_and_animated_ramps_over_the_floor_width_alike() {
    let (anchor, yaw) = (Vec3::new(0.013, 0.0, 0.021), 20.0f32.to_radians());
    let fixture = plate_fixture(anchor, yaw, CONTACT_GAP);
    for kind in ["hard point", "hard spot", "default point"] {
        let light = light_beside(kind, anchor, yaw, 0.35);
        let edge = plate_shadow_edge(&fixture.receiver, &light, anchor, yaw, CONTACT_GAP);
        let layer = fixture.bake(&light, DEFAULT_AREA_SAMPLE_COUNT, true);
        let weights = fixture.bake_weight_map(&light, DEFAULT_AREA_SAMPLE_COUNT, true);
        let (layer_ramp, floor, profiles) = ramp_against_floor(&fixture, &layer, &edge);
        let (weight_ramp, _, weight_profiles) = ramp_against_floor(&fixture, &weights, &edge);
        let fraction = 1.0 / DEFAULT_AREA_SAMPLE_COUNT.max(FOOTPRINT_PROBES as u32) as f32;
        let (mut compared, mut differing, mut worst) = (0, 0, 0.0f32);
        for y in 0..layer.height() {
            for x in 0..layer.width() {
                let (Some(a), Some(b)) = (layer.get(x, y), weights.get(x, y)) else {
                    continue;
                };
                compared += 1;
                differing += usize::from(a != b);
                worst = worst.max((a - b).abs());
            }
        }
        println!(
            "{kind}: layer ramp {layer_ramp:.3}, weight-map ramp {weight_ramp:.3}, floor \
             {floor:.3} over {profiles} profiles; {differing} of {compared} texels differ, \
             worst {worst:.4}"
        );
        assert!(profiles > 100 && profiles == weight_profiles, "{kind}");
        assert!(
            (layer_ramp - floor).abs() <= 0.2,
            "{kind}: layer ramp {layer_ramp}"
        );
        assert!(
            (weight_ramp - floor).abs() <= 0.2,
            "{kind}: weight-map ramp {weight_ramp}"
        );
        assert!(compared > 1000, "{kind}: {compared} texels in common");
        // A rounding of the weight division, far under a sample fraction.
        assert!(
            worst <= fraction + 1.0e-5,
            "{kind}: worst per-texel difference {worst}"
        );
        assert_eq!(zero_one_jumps(&weights), 0, "{kind}");
    }
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

// --- AC 7 for soft lights: the early-out across penumbra widths ------------

/// Penumbra widths at the receiver, in texels: under, about and over the
/// footprint's own one-texel radius.
const SOFT_PENUMBRA_TEXELS: [f32; 3] = [0.3, 1.0, 3.0];

/// The plate's height above the floor for each soft light.
fn soft_plate_height(kind: &str) -> f32 {
    match kind {
        "soft point" => 0.15,
        "soft sun" => 0.5,
        _ => unreachable!("{kind}"),
    }
}

/// A soft light on the plate edge's lit side, 0.3 m out and 2 m up (the sun
/// shines from there), sized so its penumbra across an edge `height` above the
/// floor spans about `width` texels at the receiver: a sphere of radius `ρ`
/// at height `D` spreads it over `2ρ·h / (D − h)`, a sun of angular diameter
/// `α` over `h·α`.
fn soft_light_beside(kind: &str, anchor: Vec3, yaw: f32, height: f32, width: f32) -> MapLight {
    let (_, lit) = edge_axes(yaw);
    let floor = Vec3::new(anchor.x, 0.0, anchor.z);
    let at = floor + lit * 0.3 + Vec3::Y * 2.0;
    let spread = width * PITCH;
    match kind {
        "soft point" => point_light(
            at.as_dvec3().to_array(),
            spread * (at.y - height) / (2.0 * height),
        ),
        "soft sun" => {
            let mut sun = hard_sun((floor - at).to_array());
            sun.angular_diameter = (spread / height).to_degrees();
            sun
        }
        _ => unreachable!("{kind}"),
    }
}

// The probe early-out on soft lights: a ray from footprint point `p + δ` to
// emitter point `L + ε` crosses an occluder a fraction `t` of the way to the
// light displaced by `(1 − t)·δ + t·ε` from the centre ray, so the probes
// split on an edge only if they form a ring in that crossing plane too.
// Straight plate edges, swept in angle and phase, under soft point lights and
// suns whose penumbrae span about 0.3, 1 and 3 texels. The reference is the
// same footprint at 256 samples with every sample traced.
#[test]
fn soft_light_penumbra_edges_never_snap_at_any_sample_count() {
    const COUNTS: [u32; 5] = [SOFT_PROBE_SAMPLES, 8, 16, DEFAULT_AREA_SAMPLE_COUNT, 64];
    const REFERENCE_SAMPLES: u32 = 256;
    let receiver = Receiver::centred(32, 32);
    let mut failures = Vec::new();
    for kind in ["soft point", "soft sun"] {
        let height = soft_plate_height(kind);
        for width in SOFT_PENUMBRA_TEXELS {
            let mut snapped = [0usize; COUNTS.len()];
            let mut pairs = [0usize; COUNTS.len()];
            let mut worst = [0.0f32; COUNTS.len()];
            let mut fractional = 0usize;
            for yaw_deg in [0.0f32, 15.0, 30.0, 45.0] {
                for phase in [0.0f32, 0.25, 0.5, 0.75] {
                    let yaw = yaw_deg.to_radians();
                    let (_, lit) = edge_axes(yaw);
                    let anchor = Vec3::new(0.013, 0.0, 0.021) + lit * (phase * PITCH);
                    let fixture = plate_fixture_on(receiver, anchor, yaw, height);
                    let light = soft_light_beside(kind, anchor, yaw, height, width);
                    let edge = plate_shadow_edge(&receiver, &light, anchor, yaw, height);
                    // Beyond the penumbra plus the footprint's reach no
                    // sample can split, so the reference skips those texels.
                    let band = f64::from(width) + 2.0;
                    let reference =
                        fixture.footprint_reference(&light, REFERENCE_SAMPLES, |x, y| {
                            edge.signed_distance([f64::from(x) + 0.5, f64::from(y) + 0.5])
                                .abs()
                                < band
                        });
                    let in_penumbra =
                        |x, y| matches!(reference.get(x, y), Some(v) if v > 0.1 && v < 0.9);
                    let mut config_fractional = 0;
                    for y in 0..receiver.height {
                        for x in 0..receiver.width {
                            config_fractional += usize::from(in_penumbra(x, y));
                        }
                    }
                    assert!(
                        config_fractional > 20,
                        "{kind} {width} {yaw_deg}° phase {phase}: {config_fractional} fractional texels"
                    );
                    fractional += config_fractional;
                    for (c, &count) in COUNTS.iter().enumerate() {
                        let grid = fixture.bake(&light, count, true);
                        for y in 0..grid.height() {
                            for x in 0..grid.width() {
                                if in_penumbra(x, y)
                                    && matches!(grid.get(x, y), Some(v) if v == 0.0 || v == 1.0)
                                {
                                    snapped[c] += 1;
                                }
                            }
                        }
                        let steps = texel_steps(&grid, TEXEL_STEP_THRESHOLD);
                        pairs[c] += steps.pairs;
                        worst[c] = worst[c].max(steps.worst_difference);
                    }
                }
            }
            for (c, &count) in COUNTS.iter().enumerate() {
                let line = format!(
                    "{kind}, {width}-texel penumbra @{count}: {} of {fractional} fractional \
                     texels snapped; {} step pairs, worst pair {:.3}",
                    snapped[c], pairs[c], worst[c]
                );
                println!("{line}");
                if snapped[c] > 0 || pairs[c] > 0 {
                    failures.push(line);
                }
            }
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

// --- thin strips and small discs off the centre --------------------------------

/// Counts the strip and disc sweeps run at: the minimum, the probe set alone,
/// and up to 64.
const SNAP_SWEEP_COUNTS: [u32; 5] = [SOFT_PROBE_SAMPLES, 9, 16, DEFAULT_AREA_SAMPLE_COUNT, 64];

/// Hard and default-size lights, point and sun, over the sweep receivers.
fn snap_sweep_lights() -> [(&'static str, MapLight); 4] {
    let aim = [0.3, -1.0, 0.2];
    let mut sun = hard_sun(aim);
    sun.angular_diameter = crate::map_data::DEFAULT_ANGULAR_DIAMETER_DEG;
    [
        ("hard point", hard_point([0.3, 2.0, -0.2])),
        ("default point", default_point([0.3, 2.0, -0.2])),
        ("hard sun", hard_sun(aim)),
        ("default sun", sun),
    ]
}

/// Flat convex occluders, `CONTACT_GAP` above a receiver: corners on the
/// floor plane, in metres.
struct ContactOccluders {
    receiver: Receiver,
    polygons: Vec<Vec<Vec2>>,
}

impl ContactOccluders {
    fn fixture(&self) -> Fixture {
        let mut scene = SceneBuilder::default();
        self.receiver.add_floor(&mut scene);
        for polygon in &self.polygons {
            let corners: Vec<Vec3> = polygon
                .iter()
                .map(|p| Vec3::new(p.x, CONTACT_GAP, p.y))
                .collect();
            scene.polygon(&corners, -Vec3::Y);
        }
        Fixture::new(scene, self.receiver)
    }

    /// Exact cone-kernel visibility of the hard shadow `light`'s centre
    /// casts from the occluders, per texel. Default-size lights spread a
    /// contact edge by under 1/80 texel, so it stands for theirs too.
    fn exact(&self, light: &MapLight) -> VisibilityGrid {
        let shadow: Vec<Vec<[f64; 2]>> = self
            .polygons
            .iter()
            .map(|polygon| {
                polygon
                    .iter()
                    .map(|p| {
                        let corner = Vec3::new(p.x, CONTACT_GAP, p.y);
                        let to_light = toward_light(light, corner);
                        let on_floor = corner - to_light * (CONTACT_GAP / to_light.y);
                        self.receiver.texel_coords(on_floor)
                    })
                    .collect()
            })
            .collect();
        let mut grid = VisibilityGrid::new(self.receiver.width, self.receiver.height);
        for y in 0..self.receiver.height {
            for x in 0..self.receiver.width {
                let centre = [f64::from(x) + 0.5, f64::from(y) + 0.5];
                grid.set(x, y, cone_visibility_of_shadow(centre, &shadow) as f32);
            }
        }
        grid
    }
}

/// Per light and count: texels whose exact visibility lies in `(0.1, 0.9)`
/// and, of those, the ones baked to exactly 0 or 1.
type SnapTally = [[(usize, usize); SNAP_SWEEP_COUNTS.len()]; 4];

fn tally_snaps(occluders: &ContactOccluders) -> SnapTally {
    // Integration error stays far inside this.
    const TOLERANCE: f32 = 0.002;
    let fixture = occluders.fixture();
    let lights = snap_sweep_lights();
    // Each default-size light shares its hard twin's centre, so its shadow.
    let exacts = [occluders.exact(&lights[0].1), occluders.exact(&lights[2].1)];
    let mut tally = SnapTally::default();
    for (l, (_, light)) in lights.iter().enumerate() {
        let exact = &exacts[l / 2];
        let in_window =
            |x, y| matches!(exact.get(x, y), Some(v) if v > 0.1 + TOLERANCE && v < 0.9 - TOLERANCE);
        for (c, &count) in SNAP_SWEEP_COUNTS.iter().enumerate() {
            let grid = fixture.bake(light, count, true);
            for y in 0..grid.height() {
                for x in 0..grid.width() {
                    if !in_window(x, y) {
                        continue;
                    }
                    tally[l][c].0 += 1;
                    if matches!(grid.get(x, y), Some(v) if v == 0.0 || v == 1.0) {
                        tally[l][c].1 += 1;
                    }
                }
            }
        }
    }
    tally
}

/// [`tally_snaps`] summed over `configurations`, traced in parallel.
fn tally_all(configurations: &[ContactOccluders]) -> SnapTally {
    use rayon::prelude::*;
    configurations
        .par_iter()
        .map(tally_snaps)
        .reduce(SnapTally::default, |mut sum, tally| {
            for (row, other) in sum.iter_mut().zip(tally) {
                for (cell, (window, snaps)) in row.iter_mut().zip(other) {
                    cell.0 += window;
                    cell.1 += snaps;
                }
            }
            sum
        })
}

/// Prints `tally` for one occluder shape and size, and returns its snapped
/// texels over every light and count.
fn report_snaps(shape: &str, tally: &SnapTally) -> usize {
    let lights = snap_sweep_lights();
    let mut snapped = 0;
    for (l, row) in tally.iter().enumerate() {
        let cells: Vec<String> = row
            .iter()
            .zip(SNAP_SWEEP_COUNTS)
            .map(|(&(window, snaps), count)| format!("@{count} {snaps}/{window}"))
            .collect();
        println!(
            "{shape}, {}: snapped/in-window {}",
            lights[l].0,
            cells.join(", ")
        );
        snapped += row.iter().map(|&(_, snaps)| snaps).sum::<usize>();
    }
    snapped
}

// Agreeing probes snap a texel to 0 or 1, so an occluder slipping between
// the centre and the ring snaps one the kernel filters to a fraction. A
// strip covering `[a, b]` texels from the centre holds a ring point wherever
// `cos θ ∈ [a / 0.6, b / 0.6]`, so every strip 0.4 texel wide that the kernel
// filters to a fraction meets a probe, at any count, for hard and soft
// lights. Narrower strips, and discs a quarter to a third of a texel across
// that sit between the centre and the ring, can still slip past all nine
// probes; their counts are printed, not asserted. Strips are swept in width,
// angle and offset from the texel centres; discs in radius and sub-texel
// place.
#[test]
fn off_centre_strips_and_discs_never_snap_where_the_ring_reaches_them() {
    let receiver = Receiver::centred(16, 16);
    let mut failures = Vec::new();
    for width in [0.2f32, 0.3, 0.4, 0.5] {
        let mut configurations = Vec::new();
        for angle_deg in [0.0f32, 15.0, 30.0, 45.0] {
            let (sin, cos) = angle_deg.to_radians().sin_cos();
            let (along, across) = (Vec2::new(sin, cos), Vec2::new(cos, -sin));
            for step in 0..8 {
                let offset = step as f32 / 8.0;
                // A phase along the strip as well as across it.
                let anchor = across * (offset * PITCH) + along * (0.37 * offset * PITCH);
                let half = across * (0.5 * width * PITCH);
                let reach = along * 1.0;
                let strip = vec![
                    anchor - half - reach,
                    anchor + half - reach,
                    anchor + half + reach,
                    anchor - half + reach,
                ];
                configurations.push(ContactOccluders {
                    receiver,
                    polygons: vec![strip],
                });
            }
        }
        let tally = tally_all(&configurations);
        let snapped = report_snaps(&format!("{width}-texel strip"), &tally);
        assert!(tally.iter().flatten().all(|&(window, _)| window > 100));
        if width >= 0.4 && snapped > 0 {
            failures.push(format!("{width}-texel strip: {snapped} snapped"));
        }
    }

    let mut stream = Stream(0x0d15_c0de_0000_0001);
    for radius in [0.25f32, 0.31, 0.35, 0.4] {
        let mut configurations = Vec::new();
        for _ in 0..8 {
            // Discs three texels apart, each at its own sub-texel place, so
            // no two share a texel's kernel.
            let mut polygons = Vec::new();
            for i in 0..5 {
                for j in 0..5 {
                    let centre = Vec2::new(
                        receiver.x0 + (1.5 + 3.0 * i as f32 + stream.next()) * PITCH,
                        receiver.z0 + (1.5 + 3.0 * j as f32 + stream.next()) * PITCH,
                    );
                    polygons.push(
                        (0..16)
                            .map(|k| {
                                let angle = k as f32 / 16.0 * std::f32::consts::TAU;
                                centre + Vec2::from_angle(angle) * (radius * PITCH)
                            })
                            .collect(),
                    );
                }
            }
            configurations.push(ContactOccluders { receiver, polygons });
        }
        let tally = tally_all(&configurations);
        let snapped = report_snaps(&format!("{radius}-texel-radius disc"), &tally);
        assert!(tally.iter().flatten().all(|&(window, _)| window > 100));
        if !(0.25..=0.31).contains(&radius) && snapped > 0 {
            failures.push(format!("{radius}-texel-radius disc: {snapped} snapped"));
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
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

/// A floor whose column `WALL_COLUMN` of texel centres stands `gap` in front
/// of a wall face at `x = 0`, facing −x: a solid on an uncut floor, whose
/// face cuts the texel grid wherever it stands.
fn wall_within_a_millimetre(gap: f32, culled: bool) -> Fixture {
    const WALL_COLUMN: f32 = 20.0;
    let mut receiver = Receiver::centred(24, 40);
    receiver.x0 = -gap - (WALL_COLUMN + 0.5) * PITCH;
    let mut scene = SceneBuilder::default();
    receiver.add_floor(&mut scene);
    if culled {
        // Only the side facing the floor survives, as at a corridor's seams.
        scene.quad(
            [
                Vec3::new(0.0, 0.0, -4.0),
                Vec3::new(0.0, 0.0, 4.0),
                Vec3::new(0.0, 3.0, 4.0),
                Vec3::new(0.0, 3.0, -4.0),
            ],
            -Vec3::X,
        );
    } else {
        scene.aabb(Vec3::new(0.0, 0.0, -2.0), Vec3::new(0.2, 1.0, 2.0));
    }
    Fixture::new(scene, receiver)
}

// A texel centre, like a footprint point, can lie within the trace's skipped
// first `RAY_EPSILON` of a wall: wherever a face's edge, or a solid standing
// on it, cuts the texel grid off its axes. Clip segments from such a centre
// cross the wall inside that stretch. With the light on the floor's side the
// samples across the wall are clipped, so the column at its foot stays exactly
// lit (P4); with only the wall's floor side surviving and the light behind it,
// no sample across it takes light through it.
#[test]
fn samples_across_a_wall_within_a_millimetre_of_their_centre_never_count() {
    let column = 20;
    let counts = [SOFT_PROBE_SAMPLES, DEFAULT_AREA_SAMPLE_COUNT];
    let mut failures = Vec::new();
    let (mut lit_checked, mut dark_checked) = (0, 0);
    for gap in [0.25e-3, 0.5e-3, 0.9e-3] {
        let fixture = wall_within_a_millimetre(gap, false);
        let mut room_lights = vec![
            hard_point([-0.5, 2.5, 0.0]),
            default_point([-0.5, 2.5, 0.0]),
        ];
        room_lights.extend(suns_toward(&[160.0, 200.0], &[40.0]));
        for light in &room_lights {
            for count in counts {
                let grid = fixture.bake(light, count, true);
                for y in 0..grid.height() {
                    lit_checked += 1;
                    if grid.get(column, y) != Some(1.0) {
                        failures.push(format!(
                            "{gap} m, light on the floor's side, {:?} @{count}: ({column}, {y}) \
                             stores {:?}",
                            light.light_type,
                            grid.get(column, y)
                        ));
                    }
                }
            }
        }

        let fixture = wall_within_a_millimetre(gap, true);
        // Points far enough behind that no emitter sample reaches the
        // floor's side of the wall.
        let mut behind = suns_toward(&[-30.0, 0.0, 30.0], &[70.0, 80.0]);
        for origin in [[0.4, 2.0, 0.0], [0.5, 1.5, 0.5]] {
            behind.extend([hard_point(origin), default_point(origin)]);
        }
        for light in &behind {
            for count in counts {
                let grid = fixture.bake(light, count, true);
                for y in 0..grid.height() {
                    // A centre ray the trace lets past the wall is the
                    // floor-off bake's own reading; only samples are at issue.
                    if !fixture.centre_ray_blocked(light, column, y) {
                        continue;
                    }
                    dark_checked += 1;
                    if grid.get(column, y) != Some(0.0) {
                        failures.push(format!(
                            "{gap} m, light behind the wall, {:?} @{count}: ({column}, {y}) \
                             stores {:?}",
                            light.light_type,
                            grid.get(column, y)
                        ));
                    }
                }
            }
        }
    }
    println!(
        "wall within a millimetre: {lit_checked} lit and {dark_checked} dark texels checked, \
         {} wrong",
        failures.len()
    );
    assert!(lit_checked > 0 && dark_checked > 0);
    assert!(
        failures.is_empty(),
        "{} texels, e.g. {:#?}",
        failures.len(),
        &failures[..failures.len().min(8)]
    );
}

// --- sealed rooms: no light from outside -------------------------------------

/// The faces a compile keeps of a sealed room built from six brushes with
/// real thickness, spanning `lo..hi` inside: floor, ceiling and four walls,
/// each meeting its neighbours at a shared edge, as `gate-heavily-lit`'s
/// corridor does. Only each brush's side facing the room survives; its sides
/// in solid and in the void are culled, so a ray that gets inside a brush
/// meets no face on its way out.
fn sealed_room(scene: &mut SceneBuilder, lo: Vec3, hi: Vec3) {
    let p = Vec3::new;
    scene.quad(
        [
            p(lo.x, lo.y, lo.z),
            p(lo.x, lo.y, hi.z),
            p(hi.x, lo.y, hi.z),
            p(hi.x, lo.y, lo.z),
        ],
        Vec3::Y,
    );
    scene.quad(
        [
            p(lo.x, hi.y, lo.z),
            p(hi.x, hi.y, lo.z),
            p(hi.x, hi.y, hi.z),
            p(lo.x, hi.y, hi.z),
        ],
        -Vec3::Y,
    );
    scene.quad(
        [
            p(lo.x, lo.y, lo.z),
            p(hi.x, lo.y, lo.z),
            p(hi.x, hi.y, lo.z),
            p(lo.x, hi.y, lo.z),
        ],
        Vec3::Z,
    );
    scene.quad(
        [
            p(lo.x, lo.y, hi.z),
            p(lo.x, hi.y, hi.z),
            p(hi.x, hi.y, hi.z),
            p(hi.x, lo.y, hi.z),
        ],
        -Vec3::Z,
    );
    scene.quad(
        [
            p(lo.x, lo.y, lo.z),
            p(lo.x, hi.y, lo.z),
            p(lo.x, hi.y, hi.z),
            p(lo.x, lo.y, hi.z),
        ],
        Vec3::X,
    );
    scene.quad(
        [
            p(hi.x, lo.y, lo.z),
            p(hi.x, lo.y, hi.z),
            p(hi.x, hi.y, hi.z),
            p(hi.x, hi.y, lo.z),
        ],
        -Vec3::X,
    );
}

/// A hard sun and one at the default angular diameter, toward each of
/// `azimuths` (degrees about +Y from +X) at each of `elevations`.
fn suns_toward(azimuths: &[f32], elevations: &[f32]) -> Vec<MapLight> {
    let mut suns = Vec::new();
    for &elevation_deg in elevations {
        for &azimuth_deg in azimuths {
            let (e, a) = (elevation_deg.to_radians(), azimuth_deg.to_radians());
            let to_light = Vec3::new(e.cos() * a.cos(), e.sin(), e.cos() * a.sin());
            let hard = hard_sun((-to_light).to_array());
            let mut soft = hard.clone();
            soft.angular_diameter = crate::map_data::DEFAULT_ANGULAR_DIAMETER_DEG;
            suns.extend([hard, soft]);
        }
    }
    suns
}

/// Bakes every chart of `geometry` for each light through the production
/// layer walk, floor on, and returns every texel record whose visibility is
/// not exactly 0 (light index, face, raw visibility), with the number of
/// records checked.
fn lit_records(
    geometry: &mut crate::geometry::GeometryResult,
    lights: &[MapLight],
    counts: &[u32],
) -> (Vec<(usize, usize, f32)>, usize) {
    let baked = crate::light_namespaces::StaticBakedLights::from_lights(lights);
    let prepared = crate::lightmap_bake::prepare_atlas(geometry, &baked, PITCH, &[]).unwrap();
    let (bvh, primitives, _) = crate::bvh_build::build_bvh(geometry).unwrap();
    let atlas = crate::lightmap_layer::SharedAtlas {
        charts: &prepared.charts,
        placements: &prepared.placements,
        atlas_width: prepared.atlas_width,
        atlas_height: prepared.atlas_height,
        layout: &prepared.layout,
    };
    let control = crate::bake_control::BakeControl::unrestricted();
    let (mut lit, mut checked) = (Vec::new(), 0);
    for (index, light) in lights.iter().enumerate() {
        let embedded = super::has_embedded_emitter(light, &bvh, &primitives, geometry);
        for &count in counts {
            for face in 0..atlas.charts.len() {
                let texels = crate::lightmap_layer::bake_light_layer_chart_controlled(
                    light,
                    &atlas,
                    face,
                    &bvh,
                    &primitives,
                    geometry,
                    count,
                    true,
                    embedded,
                    crate::lightmap_layer::ExactSkips::ON,
                    &crate::lightmap_layer::ShaftScene::new(&primitives, geometry),
                    &control,
                );
                checked += texels.len();
                lit.extend(
                    texels
                        .iter()
                        .filter(|texel| texel.raw_visibility != 0.0)
                        .map(|texel| (index, face, texel.raw_visibility)),
                );
            }
        }
    }
    (lit, checked)
}

#[test]
fn a_sealed_room_takes_no_light_from_outside() {
    // Off the origin, at the corridor's distance, so positions round as they
    // do there.
    let (lo, hi) = (Vec3::new(-3.6, 0.0, -41.3), Vec3::new(-2.0, 1.2, -40.3));
    let mut scene = SceneBuilder::default();
    sealed_room(&mut scene, lo, hi);
    let mut geometry = scene.build();
    // Suns from every side; the room's own walls stand between each and
    // every texel.
    let mut lights = suns_toward(&[15.0, 45.0, 70.0, 135.0, 225.0, 300.0], &[30.0, 60.0]);
    // Point lights beside and above the room, outside its walls.
    let mid = (lo + hi) * 0.5;
    for outside in [
        Vec3::new(hi.x + 0.5, mid.y, mid.z),
        Vec3::new(mid.x, hi.y + 0.4, mid.z),
    ] {
        let origin = outside.as_dvec3().to_array();
        lights.extend([hard_point(origin), default_point(origin)]);
    }
    let (lit, checked) = lit_records(
        &mut geometry,
        &lights,
        &[SOFT_PROBE_SAMPLES, DEFAULT_AREA_SAMPLE_COUNT],
    );
    println!("sealed room: {checked} records checked, {} lit", lit.len());
    assert!(checked > 0);
    assert!(
        lit.is_empty(),
        "{} records take light from outside the room, e.g. {:?}",
        lit.len(),
        &lit[..lit.len().min(8)]
    );
}

#[test]
fn lead_ins_enter_only_front_faces_in_the_band_from_their_first_point() {
    use crate::lightmap_bake::{RAY_EPSILON, segment_clear};
    // A floor, a wall standing on it at x = 0 facing −x, and a plate three
    // lifts up facing down: wholly above the band.
    let receiver = Receiver::centred(8, 8);
    let mut scene = SceneBuilder::default();
    receiver.add_floor(&mut scene);
    scene.quad(
        [
            Vec3::new(0.0, 0.0, -1.0),
            Vec3::new(0.0, 0.0, 1.0),
            Vec3::new(0.0, 1.0, 1.0),
            Vec3::new(0.0, 1.0, -1.0),
        ],
        -Vec3::X,
    );
    let plate = 3.0 * RAY_EPSILON;
    scene.quad(
        [
            Vec3::new(-0.15, plate, -0.05),
            Vec3::new(-0.05, plate, -0.05),
            Vec3::new(-0.05, plate, 0.05),
            Vec3::new(-0.15, plate, 0.05),
        ],
        -Vec3::Y,
    );
    let fixture = Fixture::new(scene, receiver);
    let band = ClipBand::for_chart(&receiver.chart());
    let lead_in = |from, to| {
        lead_in_clear(
            &fixture.bvh,
            &fixture.primitives,
            &fixture.geometry,
            &band,
            from,
            to,
        )
    };
    let (near, past) = (
        Vec3::new(-1.0e-4, RAY_EPSILON, 0.0),
        Vec3::new(5.0e-4, RAY_EPSILON, 0.0),
    );
    // Into the wall's front a tenth of a millimetre on: a trace skips it.
    assert!(segment_clear(
        &fixture.bvh,
        &fixture.primitives,
        &fixture.geometry,
        near,
        past
    ));
    assert!(!lead_in(near, past));
    // Out through its back: leaving solid, clear.
    assert!(lead_in(past, near));
    // Into the plate above the band, and down onto the receiver below it.
    let open = Vec3::new(-0.1, RAY_EPSILON, 0.0);
    assert!(lead_in(open, open + Vec3::Y * 3.0 * RAY_EPSILON));
    assert!(lead_in(open, open - Vec3::Y * 2.0 * RAY_EPSILON));
}

/// A floor ending at the foot of a wall `height` tall, of which only the side
/// facing the floor survives the compile, as at the corridor's seams.
fn culled_wall_fixture(pitch: f32, height: f32) -> Fixture {
    let mut receiver = Receiver::centred(24, 64);
    receiver.pitch = pitch;
    receiver.x0 = -24.0 * pitch;
    receiver.z0 = -32.0 * pitch;
    let mut scene = SceneBuilder::default();
    receiver.add_floor(&mut scene);
    scene.quad(
        [
            Vec3::new(0.0, 0.0, -4.0),
            Vec3::new(0.0, 0.0, 4.0),
            Vec3::new(0.0, height, 4.0),
            Vec3::new(0.0, height, -4.0),
        ],
        -Vec3::X,
    );
    Fixture::new(scene, receiver)
}

/// Lights behind [`culled_wall_fixture`]'s wall: suns low enough that the
/// wall shades the floor's last texels, and points beyond it.
fn lights_behind_the_culled_wall() -> Vec<MapLight> {
    let mut lights = suns_toward(&[-50.0, -20.0, 0.0, 25.0, 45.0], &[30.0, 50.0]);
    for origin in [[0.5, 0.6, 0.0], [0.3, 1.2, 0.4]] {
        lights.extend([hard_point(origin), default_point(origin)]);
    }
    lights
}

#[test]
fn floor_at_a_culled_wall_foot_takes_no_light_through_the_wall() {
    let (mut lit, mut checked) = (Vec::new(), 0);
    // Pitches off the default too, so samples land at other distances from
    // the wall.
    for pitch in [PITCH, PITCH * 1.013, PITCH * 0.987] {
        let fixture = culled_wall_fixture(pitch, 3.0);
        let receiver = fixture.receiver;
        for light in &lights_behind_the_culled_wall() {
            for count in [SOFT_PROBE_SAMPLES, DEFAULT_AREA_SAMPLE_COUNT] {
                let grid = fixture.bake(light, count, true);
                // The last two columns: the wall blocks their centre rays
                // and every footprint sample's.
                for y in 0..receiver.height {
                    for x in receiver.width - 2..receiver.width {
                        let Some(visibility) = grid.get(x, y) else {
                            continue;
                        };
                        assert!(fixture.centre_ray_blocked(light, x, y));
                        checked += 1;
                        if visibility != 0.0 {
                            lit.push((pitch, x, y, visibility));
                        }
                    }
                }
            }
        }
    }
    println!(
        "culled wall foot: {checked} texels checked, {} lit",
        lit.len()
    );
    assert!(checked > 0);
    assert!(
        lit.is_empty(),
        "{} wall-foot texels take light through the wall, e.g. {:?}",
        lit.len(),
        &lit[..lit.len().min(8)]
    );

    // Control: with every lead-in taken as clear, samples within the ray
    // lift of the wall start past it and light the wall's foot.
    let fixture = culled_wall_fixture(PITCH, 3.0);
    let leaked: usize = lights_behind_the_culled_wall()
        .iter()
        .map(|light| {
            fixture
                .footprint_outcomes_without_lead_in(light, DEFAULT_AREA_SAMPLE_COUNT)
                .into_iter()
                .flatten()
                .filter(|outcome| outcome.visibility > 0.0)
                .count()
        })
        .sum();
    println!("culled wall foot: without the lead-in, {leaked} texels take light");
    assert!(leaked > 0);
}

#[test]
fn clip_prepass_reproduces_a_sealed_room_bake_bit_for_bit() {
    // Where lead-ins meet walls, the pre-pass still skips only texels whose
    // clip segments and lead-ins are all clear.
    let (lo, hi) = (Vec3::new(-3.6, 0.0, -41.3), Vec3::new(-2.0, 1.2, -40.3));
    let mut scene = SceneBuilder::default();
    sealed_room(&mut scene, lo, hi);
    let mut geometry = scene.build();
    let mut lights = suns_toward(&[45.0, 225.0], &[30.0]);
    let origin = Vec3::new(hi.x + 0.5, 0.6, -40.8).as_dvec3().to_array();
    lights.extend([hard_point(origin), default_point(origin)]);
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
    println!("sealed room: pre-pass skips {skipped} of {total} texels");
    assert!(skipped > 0 && skipped < total);
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
/// Each light bakes with its own embedded-emitter flag, then, if clear, with
/// the flag forced on, so the full-set walk is checked on every fixture.
/// Returns how many texels the pre-pass let skip their clip segments.
fn assert_prepass_matches_full_clip(
    geometry: &crate::geometry::GeometryResult,
    atlas: &crate::lightmap_layer::SharedAtlas<'_>,
    lights: &[MapLight],
    counts: &[u32],
) -> (usize, usize) {
    let (bvh, primitives, _) = crate::bvh_build::build_bvh(geometry).unwrap();
    let control = crate::bake_control::BakeControl::unrestricted();
    let own_flags: Vec<bool> = lights
        .iter()
        .map(|light| super::has_embedded_emitter(light, &bvh, &primitives, geometry))
        .collect();
    let scene = crate::lightmap_layer::ShaftScene::new(&primitives, geometry);
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
        for (light, &own_flag) in lights.iter().zip(&own_flags) {
            // Its own flag, then forced on; a flagged light bakes only flagged.
            for embedded in [false, true] {
                if own_flag && !embedded {
                    continue;
                }
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
                        embedded,
                        crate::lightmap_layer::ExactSkips::ON,
                        &scene,
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
                        embedded,
                        crate::lightmap_layer::ExactSkips::ON,
                        &scene,
                        &control,
                    );
                    assert_eq!(
                        bytemuck::cast_slice::<_, u8>(&fast),
                        bytemuck::cast_slice::<_, u8>(&full),
                        "face {face}, {:?} light, count {count}, embedded {embedded}",
                        light.light_type
                    );
                }
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
        y: 0.0,
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
                        false,
                        crate::lightmap_layer::ExactSkips::ON,
                        &crate::lightmap_layer::ShaftScene::new(
                            &fixture.primitives,
                            &fixture.geometry,
                        ),
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
