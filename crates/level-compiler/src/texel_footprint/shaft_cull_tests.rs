// Tile shaft cull through the layer walk: byte parity with the culls off, the
// walk orderings it must survive, its margins at authored angles, and verify mode.
// See: context/lib/build_pipeline.md §Compiler pipeline (step 12)

use std::f64::consts::SQRT_2;

use glam::{DVec3, Vec3};

use super::test_fixtures::*;
use crate::lightmap_layer::{
    ChartTally, ExactSkips, LayerTexel, SHAFT_TILE_TEXELS, ShaftCull, SkipStats,
};
use crate::map_data::{FalloffModel, LightType, MapLight};

/// Emitter samples per texel: past the nine probes, so penumbrae escalate.
const SAMPLES: u32 = 16;

fn bytes(texels: &[LayerTexel]) -> &[u8] {
    bytemuck::cast_slice(texels)
}

/// Bake `light` over `fixture` at `floor` with the culls on (verified and
/// tallied) and off, and require identical records. Returns the culled
/// walk's tally after checking it ([`check_tally`]).
fn culled_matches_unculled(
    fixture: &Fixture,
    light: &MapLight,
    floor: bool,
    label: &str,
) -> &'static SkipStats {
    let stats = SkipStats::leak();
    let culled = fixture.bake_texels(light, SAMPLES, floor, ExactSkips::ON.tallied(stats));
    let unculled = fixture.bake_texels(light, SAMPLES, floor, ExactSkips::OFF);
    assert!(
        bytes(&culled) == bytes(&unculled),
        "{label}, floor {floor}: records differ with the culls off"
    );
    check_tally(stats, light, label);
    stats
}

/// What every culled walk must show: each tile with a covered texel was
/// tested exactly once (never, for a sun), a tile with none never was, a
/// clear tile traced no segment, and verify mode traced every covered texel
/// of every clear tile.
fn check_tally(stats: &SkipStats, light: &MapLight, label: &str) {
    let shafted = light.light_type != LightType::Directional;
    let mut clear_covered = 0;
    for chart in stats.charts() {
        for (index, tile) in chart.tiles.iter().enumerate() {
            let expected = u32::from(shafted && tile.covered > 0);
            assert_eq!(
                tile.tests, expected,
                "{label}: tile {index} of face {} (covered {})",
                chart.face, tile.covered
            );
            if tile.clear == Some(true) {
                assert_eq!(tile.segments(), 0, "{label}: clear tile {index} traced");
                clear_covered += tile.covered as usize;
            }
        }
    }
    assert_eq!(
        stats.verified_texels(),
        clear_covered,
        "{label}: verify mode traces every covered texel of each clear tile"
    );
}

/// The receiver's one chart in a tally.
fn only_chart(stats: &SkipStats) -> ChartTally {
    let charts = stats.charts();
    assert_eq!(charts.len(), 1, "one chart walked");
    charts.into_iter().next().unwrap()
}

/// A receiver `width × height` texels whose first texel's corner sits at
/// `(x0, y, z0)`.
fn receiver(x0: f32, y: f32, z0: f32, width: u32, height: u32) -> Receiver {
    Receiver {
        x0,
        y,
        z0,
        width,
        height,
        pitch: PITCH,
    }
}

fn floor_with(receiver: Receiver, add: impl FnOnce(&mut SceneBuilder)) -> Fixture {
    let mut scene = SceneBuilder::default();
    receiver.add_floor(&mut scene);
    add(&mut scene);
    Fixture::new(scene, receiver)
}

/// The quadrant occluder: a plate 0.3 m up over one quadrant of a centred
/// 32 × 32 receiver, moved by `offset`.
fn quadrant_plate(scene: &mut SceneBuilder, offset: Vec3) {
    scene.aabb(
        offset + Vec3::new(-0.7, 0.3, -0.7),
        offset + Vec3::new(-0.1, 0.35, 0.0),
    );
}

fn translated(receiver: Receiver, by: Vec3) -> Receiver {
    Receiver {
        x0: receiver.x0 + by.x,
        y: receiver.y + by.y,
        z0: receiver.z0 + by.z,
        ..receiver
    }
}

fn moved(light: &MapLight, by: Vec3) -> MapLight {
    let mut light = light.clone();
    light.origin += by.as_dvec3();
    light
}

// An open floor under a soft point light clears every tile; with a plate
// shadowing one quadrant, every tile whose texels all bake fully lit still
// clears (the hull's own faces separate the plate beside its shaft) and the
// rest trace, and both bake the unculled records at each floor setting
// (verify on).
#[test]
fn shaft_cull_clears_an_open_floor_and_matches_the_unculled_walk_beside_an_occluder() {
    let centred = Receiver::centred(32, 32);
    let light = default_point([0.0, 2.0, 0.0]);
    let open = floor_with(centred, |_| {});
    let occluded = floor_with(centred, |scene| quadrant_plate(scene, Vec3::ZERO));
    // G, the candidate query's growth: under a millimetre here.
    let scene = open.shaft_scene();
    let growth = open
        .shaft_cull(&light, true, &scene)
        .tile_query_growth((0, 0))
        .expect("a finite growth");
    assert!(growth > 0.0 && growth < 2.0e-3, "query growth {growth}");
    for floor in [true, false] {
        let stats = culled_matches_unculled(&open, &light, floor, "open floor");
        let chart = only_chart(stats);
        assert!(
            chart.tiles.iter().all(|tile| tile.clear == Some(true)),
            "floor {floor}: every open tile clears: {:?}",
            chart.tiles
        );

        let stats = culled_matches_unculled(&occluded, &light, floor, "occluded floor");
        let chart = only_chart(stats);
        let clear = chart.clear_tiles().count();
        assert!(
            clear > 0 && clear < chart.tiles.len(),
            "floor {floor}: some tiles clear, those under the plate's shaft trace: {clear}"
        );
        let grid = occluded.bake(&light, SAMPLES, floor);
        assert!(
            (0..32).any(|y| (0..32).any(|x| grid.get(x, y).is_some_and(|v| v < 1.0))),
            "the plate shadows part of the chart"
        );
        for (index, tile) in chart.tiles.iter().enumerate() {
            let [x0, x1, y0, y1] = chart.tile_texels(index);
            let lit = (y0..=y1)
                .all(|y| (x0..=x1).all(|x| grid.get(x as u32, y as u32).is_some_and(|v| v == 1.0)));
            assert!(
                !lit || tile.clear == Some(true),
                "floor {floor}: fully lit tile {index} clears: {:?}",
                chart.tiles
            );
        }
    }
}

// Tile B between shadowed tiles A and C along the walk's rows, so the
// remembered occluder crosses a skipped clear run on every texel row. One
// triangle cannot shadow both and miss B's hull (a halfspace holding points
// of A's and C's slabs holds the point of B's slab between them), so the one
// occluder is a gate: two posts beside B's shaft joined by a lintel over the
// light. Each texel row walks A's run, skips B's with the post's triangle
// remembered, and walks C's.
#[test]
fn shaft_cull_skips_a_clear_tile_between_two_tiles_its_occluder_shadows() {
    let receiver = receiver(0.0, 0.0, 0.0, 24, 8);
    let light = default_point([0.48, 2.0, 0.16]);
    let gate = floor_with(receiver, |scene| {
        scene.aabb(Vec3::new(0.10, 0.0, -0.3), Vec3::new(0.20, 2.6, 0.6));
        scene.aabb(Vec3::new(0.76, 0.0, -0.3), Vec3::new(0.86, 2.6, 0.6));
        scene.aabb(Vec3::new(0.10, 2.4, -0.3), Vec3::new(0.86, 2.6, 0.6));
    });
    for floor in [true, false] {
        let stats = culled_matches_unculled(&gate, &light, floor, "gate");
        let chart = only_chart(stats);
        let verdicts: Vec<_> = chart.tiles.iter().map(|tile| tile.clear).collect();
        assert_eq!(
            verdicts,
            [Some(false), Some(true), Some(false)],
            "floor {floor}: the middle tile clears between its shadowed neighbours"
        );
        let grid = gate.bake(&light, SAMPLES, floor);
        for tile in [0, 2] {
            let shadowed = (0..8)
                .any(|y| (tile * 8..tile * 8 + 8).any(|x| grid.get(x, y).is_some_and(|v| v < 1.0)));
            assert!(shadowed, "floor {floor}: tile {tile} is shadowed");
        }
    }
}

// A tile's first covered texel is not its first texel: a range edge and a
// spot's cone edge cross tiles, so the walk reaches that texel after the
// tile's first. The hull still spans the whole tile; such a tile clears and
// bakes the unculled bytes.
#[test]
fn shaft_cull_clears_a_tile_whose_first_covered_texel_is_not_its_first() {
    let receiver = receiver(0.0, 0.0, 0.0, 16, 16);
    let fixture = floor_with(receiver, |_| {});
    let mut ranged = default_point([0.62, 0.5, 0.62]);
    ranged.falloff_range = 0.62;
    let mut spot = default_point([0.2, 1.0, 0.2]);
    spot.light_type = LightType::Spot;
    spot.cone_direction = Some([0.0, -1.0, 0.0]);
    spot.cone_angle_inner = Some(0.09);
    spot.cone_angle_outer = Some(0.12);
    for (light, label) in [(&ranged, "range edge"), (&spot, "cone edge")] {
        for floor in [true, false] {
            let stats = culled_matches_unculled(&fixture, light, floor, label);
            let chart = only_chart(stats);
            let crossed = chart.clear_tiles().find(|&index| {
                let [tx0, _, ty0, _] = chart.tile_texels(index);
                chart.tiles[index].first_covered != Some((tx0, ty0))
            });
            assert!(
                crossed.is_some(),
                "{label}, floor {floor}: a clear tile whose first texel is uncovered: {:?}",
                chart.tiles
            );
            assert!(
                chart.tiles.iter().any(|tile| tile.covered == 0),
                "{label}: some tile lies wholly outside the light"
            );
        }
    }
}

// Narrow last tiles: an interior that is not a multiple of 8 on either axis,
// and a one-texel chart. Their last tiles are narrower than 8, and the hull
// holds exactly their texels: a post inside the pad past the last column
// blocks the last tile column on the floored sampler, and once moved past the
// pad it clears.
#[test]
fn shaft_cull_narrow_last_tiles_block_on_an_occluder_inside_the_pad_and_clear_past_it() {
    struct Case {
        width: u32,
        height: u32,
        light: [f64; 3],
        /// The post's x span inside the floored pad, then past it.
        inside: [f32; 2],
        past: [f32; 2],
    }
    let cases = [
        Case {
            width: 11,
            height: 13,
            light: [0.2, 1.0, 0.26],
            inside: [0.445, 0.455],
            past: [0.47, 0.49],
        },
        Case {
            width: 1,
            height: 1,
            light: [-0.3, 1.0, 0.02],
            inside: [0.045, 0.055],
            past: [0.07, 0.09],
        },
    ];
    for case in cases {
        let receiver = receiver(0.0, 0.0, 0.0, case.width, case.height);
        let light = point_light(case.light, 0.1);
        let label = format!("{} x {}", case.width, case.height);
        for (span, expect_clear) in [(case.inside, false), (case.past, true)] {
            let fixture = floor_with(receiver, |scene| {
                scene.aabb(Vec3::new(span[0], 0.0, -0.3), Vec3::new(span[1], 0.5, 0.8));
            });
            for floor in [true, false] {
                let stats = culled_matches_unculled(&fixture, &light, floor, &label);
                let chart = only_chart(stats);
                let last_column = chart.tiles_wide as usize - 1;
                let narrow = chart.tile_texels(last_column);
                assert_eq!(
                    narrow[1] - narrow[0] + 1,
                    (case.width as i32 - 1) % SHAFT_TILE_TEXELS + 1,
                    "{label}: the last tile column holds only the chart's texels"
                );
                let last_tiles: Vec<_> = (0..chart.tiles.len())
                    .filter(|index| index % chart.tiles_wide as usize == last_column)
                    .map(|index| chart.tiles[index].clear)
                    .collect();
                if floor || expect_clear {
                    assert!(
                        last_tiles.iter().all(|&clear| clear == Some(expect_clear)),
                        "{label}, floor {floor}, post at {span:?}: last tiles {last_tiles:?}"
                    );
                }
            }
        }
    }
}

/// The light and tile layout of the margin fixtures: tiles A (0, 0) and
/// B (1, 0) of a 16 × 8 receiver, the light over B.
fn margin_receiver() -> Receiver {
    receiver(0.0, 0.0, 0.0, 16, 8)
}

const TILE_B: (i32, i32) = (1, 0);

/// Place the triangle `at(offset)` builds so its gap from tile B's hull,
/// along its own normal (`normal`) or along `axis`, is `ratio` times its
/// margin, to within 5%. Returns the triangle and its (gap, margin).
fn place(
    fixture: &Fixture,
    light: &MapLight,
    floor: bool,
    ratio: f64,
    axis: Option<DVec3>,
    at: impl Fn(f64) -> [Vec3; 3],
) -> ([Vec3; 3], (f64, f64)) {
    let scene = fixture.shaft_scene();
    let shaft = fixture.shaft_cull(light, floor, &scene);
    let measure = |points: [Vec3; 3]| {
        let (normal, along) = shaft.certificates(TILE_B, points, axis.unwrap_or(DVec3::X));
        match axis {
            None => normal,
            Some(_) => along.expect("a unit axis"),
        }
    };
    let mut offset = 1.0e-3;
    for _ in 0..8 {
        let (gap, margin) = measure(at(offset));
        let wanted = ratio * margin;
        if (gap / margin - ratio).abs() < 0.05 * ratio {
            return (at(offset), (gap, margin));
        }
        offset += wanted - gap;
    }
    panic!("could not place the triangle at {ratio} of its margin");
}

/// The box `(lo, hi)` of `tile`'s hull, read through the hull's support
/// function along the world axes.
fn hull_box(shaft: &ShaftCull, tile: (i32, i32)) -> (DVec3, DVec3) {
    let support = |axis: DVec3| shaft.tile_support(tile, axis);
    (
        DVec3::new(
            -support(-DVec3::X),
            -support(-DVec3::Y),
            -support(-DVec3::Z),
        ),
        DVec3::new(support(DVec3::X), support(DVec3::Y), support(DVec3::Z)),
    )
}

/// An independent `f64` recomputation of the axis margin the cull asks of
/// `points` (index order) against a hull with box `(lo, hi)`:
///
/// m_A = κ·[5√2·u·ρ·(3·S_T + L + ‖e1‖ + ‖e2‖) / sin θ0 + u·(S_T + 5·e_T) + e_H]
///
/// with κ = 2, θ0 = 2°, u = 2⁻²⁴, e1 = b − a, e2 = c − a, ρ = ‖e1‖‖e2‖ /
/// ‖e1 × e2‖, e_T = max(‖e1‖, ‖e2‖), L the box's diagonal, S_T = ‖a −
/// centre‖ + L/2, and e_H the axis-weighted sum of 10u·R + 6u·L per world
/// axis. R is taken from the box alone: the chart's own coordinate terms stay
/// under a metre here, and e_H is under one percent of the margin.
fn expected_axis_margin(points: [Vec3; 3], (lo, hi): (DVec3, DVec3), axis: DVec3) -> f64 {
    let u = 2.0_f64.powi(-24);
    let sin_floor = 2.0_f64.to_radians().sin();
    let [a, b, c] = points.map(|p| p.as_dvec3());
    let (e1, e2) = (b - a, c - a);
    let (l1, l2) = (e1.length(), e2.length());
    let rho = l1 * l2 / e1.cross(e2).length();
    let diag = (hi - lo).length();
    let s_t = a.distance((lo + hi) * 0.5) + 0.5 * diag;
    let epsilon = lo.abs().max(hi.abs()) * (10.0 * u) + DVec3::splat(6.0 * u * diag);
    let e_h = axis.abs().dot(epsilon);
    2.0 * (5.0 * SQRT_2 * u * rho * (3.0 * s_t + diag + l1 + l2) / sin_floor
        + u * (s_t + 5.0 * l1.max(l2))
        + e_h)
}

/// Floor bakes of a margin fixture: the occluder triangle beside tile B.
fn margin_fixture(occluder: [Vec3; 3]) -> Fixture {
    floor_with(margin_receiver(), |scene| {
        let normal = (occluder[1] - occluder[0])
            .cross(occluder[2] - occluder[0])
            .normalize();
        scene.polygon(&occluder, normal);
    })
}

// A triangle on the plane tangent to tile B's hull on the side facing A,
// between A and most of the emitter, so it shadows A, whose rays meet nothing
// else, and is A's remembered occluder when the walk reaches B. Outside B's
// hull by 0.95 of its normal margin it keeps B blocked; by 1.05 it clears B.
#[test]
fn shaft_cull_blocks_a_triangle_inside_its_margin_and_clears_one_outside_it() {
    let light = default_point([0.48, 1.5, 0.16]);
    let bare = floor_with(margin_receiver(), |_| {});
    for floor in [true, false] {
        let scene = bare.shaft_scene();
        let shaft = bare.shaft_cull(&light, floor, &scene);
        // From B's left slab edge, the line tangent to the emitter sphere on
        // its left, and that line's normal pointing away from B.
        let left = DVec3::new(-shaft.tile_support(TILE_B, -DVec3::X), 0.0025, 0.16);
        let to_light = light.origin.as_vec3().as_dvec3() - left;
        let tilt = (f64::from(light.light_size) / to_light.length()).asin();
        let (sin, cos) = tilt.sin_cos();
        let to_light = to_light.normalize();
        let up = DVec3::new(
            to_light.x * cos - to_light.y * sin,
            to_light.x * sin + to_light.y * cos,
            0.0,
        );
        let away = DVec3::new(-up.y, up.x, 0.0);
        let support = shaft.tile_support(TILE_B, away);
        let at = |gap: f64| {
            let base = left + away * (support + gap - away.dot(left));
            [
                base - up * 1.0 - DVec3::Z * 1.5,
                base - up * 1.0 + DVec3::Z * 1.5,
                base + up * 2.0,
            ]
            .map(|p| p.as_vec3())
        };
        for (ratio, expect_clear) in [(0.95, false), (1.05, true)] {
            let (occluder, (gap, margin)) = place(&bare, &light, floor, ratio, None, at);
            let fixture = margin_fixture(occluder);
            let label = format!("tangent triangle at {:.3} of its margin", gap / margin);
            let stats = culled_matches_unculled(&fixture, &light, floor, &label);
            let chart = only_chart(stats);
            assert_eq!(chart.tiles[0].clear, Some(false), "{label}: A is crossed");
            assert_eq!(
                chart.tiles[1].clear,
                Some(expect_clear),
                "{label}, floor {floor}: gap {gap}, margin {margin}"
            );
            let grid = fixture.bake(&light, SAMPLES, floor);
            assert!(
                (0..8).any(|y| (0..8).any(|x| grid.get(x, y).is_some_and(|v| v < 1.0))),
                "{label}: the triangle shadows A"
            );
        }
    }
}

// Margins near the origin: a wall the tile's rays meet at about 10°,
// and a trim triangle whose narrowest angle (at its first corner) is 5°, each
// with an edge along the chart's u axis, crossing B's shaft only if extended,
// separated from B's hull along the v axis by 1.05 of that axis's margin.
// Tile B clears, and each margin is within a quarter of an independent
// recomputation from the triangle and the hull's box.
#[test]
fn shaft_cull_clears_beside_a_grazing_wall_and_a_narrow_trim_just_past_their_margins() {
    let light = default_point([0.48, 2.0, 0.16]);
    let bare = floor_with(margin_receiver(), |_| {});
    let (sin, cos) = 10.0_f64.to_radians().sin_cos();
    // Up the wall's plane, away from the shaft: 10° off the vertical rays.
    let rise = DVec3::new(0.0, cos, sin);
    type Shape = fn(DVec3, DVec3) -> [DVec3; 3];
    let shapes: [(&str, Shape); 2] = [
        ("10° wall", |a, rise| {
            let b = a + DVec3::X * 3.0;
            [a, b, (a + b) * 0.5 + rise * 1.5]
        }),
        ("5° trim", |a, rise| {
            let (sin, cos) = 5.0_f64.to_radians().sin_cos();
            [
                a,
                a + DVec3::X * 3.0,
                a + (DVec3::X * cos + rise * sin) * 3.0,
            ]
        }),
    ];
    for floor in [true, false] {
        let scene = bare.shaft_scene();
        let shaft = bare.shaft_cull(&light, floor, &scene);
        let reach = shaft.tile_support(TILE_B, DVec3::Z);
        let hull = hull_box(&shaft, TILE_B);
        for (label, shape) in shapes {
            let at = |gap: f64| {
                let a = DVec3::new(0.48 - 1.5, 2.0, reach + gap);
                shape(a, rise).map(|p| p.as_vec3())
            };
            let (occluder, (gap, margin)) = place(&bare, &light, floor, 1.05, Some(DVec3::Z), at);
            let expected = expected_axis_margin(occluder, hull, DVec3::Z);
            assert!(
                (0.75..=1.25).contains(&(margin / expected)),
                "{label}, floor {floor}: margin {margin}, recomputed {expected}"
            );
            // Its own plane crosses the hull: only the off-normal axis
            // separates it.
            let (normal, _) = shaft.certificates(TILE_B, occluder, DVec3::Z);
            assert!(
                normal.0 < 0.0,
                "{label}: the plane cuts the shaft: {normal:?}"
            );
            let fixture = margin_fixture(occluder);
            let stats = culled_matches_unculled(&fixture, &light, floor, label);
            assert_eq!(
                only_chart(stats).tiles[1].clear,
                Some(true),
                "{label}, floor {floor}: gap {gap}, margin {margin}"
            );
        }
    }
}

// The occluder fixture at 4 km on every axis: the receiver's normal margin
// passes its 0.5 mm clearance, so the cull may decline every tile; the bytes
// still match.
#[test]
fn shaft_cull_matches_the_unculled_walk_four_kilometres_out() {
    let by = Vec3::splat(4096.0);
    let receiver = translated(Receiver::centred(32, 32), by);
    let fixture = floor_with(receiver, |scene| quadrant_plate(scene, by));
    let light = moved(&default_point([0.0, 2.0, 0.0]), by);
    for floor in [true, false] {
        culled_matches_unculled(&fixture, &light, floor, "4 km");
    }
}

// A hard point light, a spot and a light whose emitter reaches into the
// plate (flagged embedded) each bake the unculled bytes. The embedded light's
// sphere crosses every tile's hull, so no tile clears.
#[test]
fn shaft_cull_matches_for_a_hard_point_a_spot_and_an_embedded_emitter() {
    let fixture = floor_with(Receiver::centred(32, 32), |scene| {
        quadrant_plate(scene, Vec3::ZERO)
    });
    let embedded = default_point([-0.4, 0.45, -0.35]);
    assert!(
        fixture.has_embedded_emitter(&embedded),
        "the sphere enters the plate"
    );
    for floor in [true, false] {
        for (light, label) in [
            (hard_point([0.1, 2.0, 0.2]), "hard point"),
            (hard_spot([0.1, 2.0, 0.2]), "spot"),
        ] {
            let stats = culled_matches_unculled(&fixture, &light, floor, label);
            assert!(
                stats.tiles_cleared() > 0,
                "{label}, floor {floor}: tiles clear"
            );
        }
        let stats = culled_matches_unculled(&fixture, &embedded, floor, "embedded");
        assert!(
            stats.tiles_tested() > 0,
            "the embedded light's tiles are tested"
        );
        assert_eq!(
            stats.tiles_cleared(),
            0,
            "floor {floor}: no embedded tile clears"
        );
    }
}

// Suns take no shaft cull: a sun bakes the unculled bytes and tests no tile.
#[test]
fn shaft_cull_tests_no_tile_for_a_sun() {
    let fixture = floor_with(Receiver::centred(32, 32), |scene| {
        quadrant_plate(scene, Vec3::ZERO)
    });
    let mut soft_sun = hard_sun([0.3, -1.0, 0.2]);
    soft_sun.angular_diameter = 2.0;
    for light in [hard_sun([0.3, -1.0, 0.2]), soft_sun] {
        for floor in [true, false] {
            let stats = culled_matches_unculled(&fixture, &light, floor, "sun");
            assert!(!stats.charts().is_empty(), "the walk ran");
            assert_eq!(stats.tiles_tested(), 0, "floor {floor}: no tile tested");
        }
    }
}

// A spot covering part of the open floor. Each tile with a covered
// texel is tested once, on its first, and a tile with none never is; a clear
// tile's texels trace no shadow segment. With the clip pre-pass off, the
// unculled walk traces clip segments and lead-ins at those texels, and the
// culled walk traces none.
#[test]
fn shaft_cull_tests_each_covered_tile_once_and_traces_nothing_from_a_clear_one() {
    let fixture = floor_with(Receiver::centred(32, 32), |_| {});
    let mut spot = default_point([0.0, 2.0, 0.0]);
    spot.light_type = LightType::Spot;
    spot.cone_direction = Some([0.0, -1.0, 0.0]);
    spot.cone_angle_inner = Some(0.15);
    spot.cone_angle_outer = Some(0.197);
    for floor in [true, false] {
        let stats = culled_matches_unculled(&fixture, &spot, floor, "spot on open floor");
        let chart = only_chart(stats);
        assert!(chart.tiles.iter().any(|tile| tile.covered == 0));
        assert!(chart.clear_tiles().count() > 0);
        assert_eq!(stats.segments_in_clear_tiles(), 0);
    }

    let culled = SkipStats::leak();
    let unculled = SkipStats::leak();
    let with_cull =
        fixture.bake_texels_without_clip_prepass(&spot, SAMPLES, ExactSkips::ON.tallied(culled));
    let without =
        fixture.bake_texels_without_clip_prepass(&spot, SAMPLES, ExactSkips::OFF.tallied(unculled));
    assert!(
        bytes(&with_cull) == bytes(&without),
        "no clip pre-pass: records differ"
    );
    check_tally(culled, &spot, "no clip pre-pass");
    let (culled, unculled) = (only_chart(culled), only_chart(unculled));
    let mut clear = 0;
    for index in culled.clear_tiles() {
        clear += 1;
        let (skipped, traced) = (culled.tiles[index], unculled.tiles[index]);
        assert_eq!((skipped.shadow, skipped.clip, skipped.lead_in), (0, 0, 0));
        assert!(
            traced.shadow > 0 && traced.clip > 0 && traced.lead_in > 0,
            "the unculled walk traces tile {index}: {traced:?}"
        );
    }
    assert!(clear > 0);
}

// Permit side: the open floor records clear tiles at the origin (above) and
// ~104 m out on every axis (Quake's ±4096-unit grid), on a floor that is a
// single triangle with 60 m legs, whose normal margin stays under the
// receiver's clearance.
#[test]
fn shaft_cull_clears_tiles_on_one_long_floor_triangle_104_metres_out() {
    let by = Vec3::splat(104.0);
    let receiver = translated(Receiver::centred(32, 32), by);
    let mut scene = SceneBuilder::default();
    let corner = by + Vec3::new(-20.0, 0.0, -20.0);
    scene.polygon(
        &[
            corner,
            corner + Vec3::new(0.0, 0.0, 60.0),
            corner + Vec3::new(60.0, 0.0, 0.0),
        ],
        Vec3::Y,
    );
    let fixture = Fixture::new(scene, receiver);
    let light = moved(&default_point([0.0, 2.0, 0.0]), by);
    for floor in [true, false] {
        let stats = culled_matches_unculled(&fixture, &light, floor, "104 m");
        assert!(
            stats.tiles_cleared() > 0,
            "floor {floor}: tiles clear 104 m out on a 60 m floor triangle"
        );
    }
}

// Verify mode is the cull's oracle: with it, every clear tile's covered
// texels are traced as well, so a texel the unculled walk would shade below
// 1.0, or an occluder cache it would move, fails the bake (`check_tally`
// counts the traced texels in every test above). Here: a falloff model that
// is strictly positive at range clears and verifies too.
#[test]
fn shaft_cull_verify_mode_traces_every_covered_texel_of_each_clear_tile() {
    let fixture = floor_with(Receiver::centred(32, 32), |_| {});
    let mut light = default_point([0.0, 2.0, 0.0]);
    light.falloff_model = FalloffModel::InverseSquared;
    let stats = culled_matches_unculled(&fixture, &light, true, "verify");
    assert_eq!(stats.verified_texels(), 32 * 32, "every texel verified");
}

// Negative control for the oracle above: verify mode must fail a bake that
// skips a tile the geometry shadows. Forcing every tested tile clear under the
// quadrant plate's shadow (the `force_clear` test seam) has to trip it.
#[test]
#[should_panic(expected = "shaft cull:")]
fn shaft_cull_verify_mode_fails_a_shadowed_tile_forced_clear() {
    let fixture = floor_with(Receiver::centred(32, 32), |scene| {
        quadrant_plate(scene, Vec3::ZERO)
    });
    let light = default_point([0.0, 2.0, 0.0]);
    let forced = ExactSkips {
        force_clear: true,
        ..ExactSkips::ON
    };
    fixture.bake_texels(&light, SAMPLES, true, forced);
}
