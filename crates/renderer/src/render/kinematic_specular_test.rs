// GPU readback tests for the mover pass's selected-static specular under the
// pixel and smooth styles (`kinematic_specular_harness.rs`).
// See: context/lib/rendering_pipeline.md §4 (Texel-centre highlight) ·
// context/lib/testing_guide.md

use std::collections::HashMap;

use super::gpu_test_harness::gpu_or_skip;
use super::kinematic_specular_harness::{
    DEPTH_SCALE_M, LATTICE, MoverCase, MoverLight, Observe, Output, PIXELS_PER_TEXEL, RAISED, SUNK,
    Style, TARGET, fragment_shadow, fragment_world, render, texel_height,
};

/// A point light and an eye off the floor's low corner, so the highlight
/// falls off monotonically across the floor along x and z.
fn flank_light() -> MoverLight {
    MoverLight::point([-1.0, 1.5, -1.0], 10.0)
}

/// A spot straight down over the floor's middle, its cone edge crossing
/// texels.
fn spot_light() -> MoverLight {
    MoverLight::spot([2.0, 2.0, 2.0], [0.0, -1.0, 0.0], 20.0, 30.0)
}

fn sun() -> MoverLight {
    MoverLight::directional([0.6, -0.8, 0.0])
}

/// A point light low over the floor's middle whose range circle crosses
/// texels.
fn ranged_light() -> MoverLight {
    MoverLight::point([2.0, 0.5, 2.0], 1.5)
}

/// A point light whose influence sphere is tighter than its range, so the
/// per-pixel influence test crosses texels the light still reaches.
fn influence_light() -> MoverLight {
    MoverLight {
        influence: 1.3,
        ..MoverLight::point([2.0, 1.0, 2.0], 10.0)
    }
}

/// A normal-mapped texel's tilted normal, and a light whose diffuse terminator
/// under it runs along x = 2.1, through the texel column at x in [2.0, 2.5]
/// clear of every fragment: on the floor n·L = 0 where x = light.x - light.y.
/// The column's centre lies on the lit side.
const TILTED: [f32; 3] = [
    -std::f32::consts::FRAC_1_SQRT_2,
    std::f32::consts::FRAC_1_SQRT_2,
    0.0,
];
fn terminator_light() -> MoverLight {
    MoverLight::point([5.1, 3.0, 2.0], 10.0)
}

fn index(x: u32, y: u32) -> usize {
    (y * TARGET + x) as usize
}

fn pixels() -> impl Iterator<Item = (u32, u32)> {
    (0..TARGET).flat_map(|y| (0..TARGET).map(move |x| (x, y)))
}

fn rgb(pixel: [f32; 4]) -> [f32; 3] {
    [pixel[0], pixel[1], pixel[2]]
}

fn bits(pixel: [f32; 4]) -> [u32; 3] {
    rgb(pixel).map(f32::to_bits)
}

/// A texel the fragment shows clear of every seam band: its reconstructed
/// albedo coordinate is a texel centre along both axes.
fn interior_texel(texel: [f32; 4]) -> Option<(i64, i64)> {
    let centre = |c: f32| c.fract().abs() == 0.5;
    (centre(texel[0]) && centre(texel[1]))
        .then(|| (texel[0].floor() as i64, texel[1].floor() as i64))
}

fn face(texel: [f32; 4]) -> u32 {
    texel[2] as u32
}

/// Interior fragments grouped by the texel and the face of it they show.
fn texel_faces(texels: &[[f32; 4]]) -> HashMap<(i64, i64, u32), Vec<usize>> {
    let mut groups: HashMap<(i64, i64, u32), Vec<usize>> = HashMap::new();
    for (i, texel) in texels.iter().enumerate() {
        if let Some((tx, ty)) = interior_texel(*texel) {
            groups.entry((tx, ty, face(*texel))).or_default().push(i);
        }
    }
    groups
}

/// Every interior fragment of one texel face draws one value.
fn assert_constant_per_texel_face(shade: &[[f32; 4]], texels: &[[f32; 4]], what: &str) {
    let groups = texel_faces(texels);
    assert!(
        groups.values().filter(|members| members.len() > 1).count() >= 16,
        "{what}: the target must hold many multi-fragment texels",
    );
    for (key, members) in &groups {
        let first = bits(shade[members[0]]);
        for &member in members {
            assert_eq!(
                bits(shade[member]),
                first,
                "{what}: texel face {key:?} must draw one value, fragment {member}"
            );
        }
    }
}

fn render_with_texels(
    ctx: &super::gpu_test_harness::GpuCtx,
    lights: &[MoverLight],
    case: MoverCase,
) -> (Vec<[f32; 4]>, Vec<[f32; 4]>) {
    (
        render(ctx, lights, case, Observe::Shade),
        render(ctx, lights, case, Observe::Texel),
    )
}

/// Clearance from a boundary before the CPU calls a fragment's side of it,
/// so f32 rounding on the GPU cannot land it on the other.
const BOUNDARY_MARGIN: f64 = 1.0e-4;

/// A light's per-fragment boundary at a march-off fragment's plane position:
/// whether that position alone lies inside range, inside the outer cone and
/// in front of the terminator.
fn reached_per_pixel(light: MoverLight, normal: [f32; 3], x: u32, y: u32) -> bool {
    let world = fragment_world(x, y);
    if light.is_directional() {
        return true;
    }
    let to_light: [f64; 3] = std::array::from_fn(|i| f64::from(light.position[i]) - world[i]);
    let dist = to_light.iter().map(|c| c * c).sum::<f64>().sqrt();
    let l = to_light.map(|c| c / dist);
    let in_range = dist < f64::from(light.range);
    let in_cone = !light.is_spot()
        || -(0..3).map(|i| l[i] * f64::from(light.aim[i])).sum::<f64>()
            > f64::from(light.cos_outer());
    let front = (0..3).map(|i| l[i] * f64::from(normal[i])).sum::<f64>() > 0.0;
    in_range && in_cone && front
}

#[test]
fn mover_pixel_specular_is_constant_inside_a_magnified_texel() {
    let Some(ctx) = gpu_or_skip("mover_pixel_specular_is_constant_inside_a_magnified_texel") else {
        return;
    };
    let up = [0.0, 1.0, 0.0];
    let cases = [
        ("point", flank_light(), None),
        ("spot", spot_light(), None),
        ("directional", sun(), None),
        ("range", ranged_light(), None),
        ("terminator", terminator_light(), Some(TILTED)),
    ];
    for (name, light, fixed_normal) in cases {
        for march in [false, true] {
            let case = MoverCase {
                march,
                fixed_normal,
                ..MoverCase::new(Style::Pixel, Output::TexelTerm)
            };
            let what = format!("{name}, march {march}");
            let (unshadowed, texels) = render_with_texels(&ctx, &[light], case);
            assert_constant_per_texel_face(&unshadowed, &texels, &what);
            assert!(
                unshadowed.iter().any(|p| p[0] > 0.0),
                "{what}: the light must reach the floor"
            );
            // Each fragment draws its texel's value times its own shadow.
            let shadowed = render(
                &ctx,
                &[light],
                MoverCase {
                    shadow_pattern: true,
                    ..case
                },
                Observe::Shade,
            );
            for (x, y) in pixels() {
                let i = index(x, y);
                let shadow = fragment_shadow(x, y);
                assert_eq!(
                    rgb(shadowed[i]),
                    rgb(unshadowed[i]).map(|c| c * shadow),
                    "{what}: fragment ({x}, {y}) under shadow {shadow}"
                );
            }
        }

        // Range, cone edge and terminator cross texel interiors: per fragment
        // they split a texel, while its point decides the whole texel.
        if matches!(name, "spot" | "range" | "terminator") {
            let normal = fixed_normal.unwrap_or(up);
            let case = MoverCase {
                fixed_normal,
                ..MoverCase::new(Style::Pixel, Output::TexelTerm)
            };
            let (shade, texels) = render_with_texels(&ctx, &[light], case);
            let split_and_lit = texel_faces(&texels).values().any(|members| {
                let reached = |&m: &usize| {
                    let (x, y) = (m as u32 % TARGET, m as u32 / TARGET);
                    reached_per_pixel(light, normal, x, y)
                };
                members.iter().any(reached)
                    && !members.iter().all(reached)
                    && shade[members[0]][0] > 0.0
            });
            assert!(
                split_and_lit,
                "{name}: some lit texel must straddle the per-fragment boundary"
            );
        }
    }

    // Neighbouring texels sit at different points, so their highlights
    // differ, along both texture axes.
    for (name, light) in [("point", flank_light()), ("directional", sun())] {
        let shade = render(
            &ctx,
            &[light],
            MoverCase::new(Style::Pixel, Output::TexelTerm),
            Observe::Shade,
        );
        let centre = |tx: u32, ty: u32| {
            rgb(shade[index(
                tx * PIXELS_PER_TEXEL + PIXELS_PER_TEXEL / 2,
                ty * PIXELS_PER_TEXEL + PIXELS_PER_TEXEL / 2,
            )])
        };
        for a in 0..LATTICE {
            for b in 0..LATTICE - 1 {
                assert_ne!(centre(b, a), centre(b + 1, a), "{name}: ({b}, {a}) along u");
                assert_ne!(centre(a, b), centre(a, b + 1), "{name}: ({a}, {b}) along v");
            }
        }
    }
}

#[test]
fn mover_pixel_specular_ignores_per_pixel_gates() {
    let Some(ctx) = gpu_or_skip("mover_pixel_specular_ignores_per_pixel_gates") else {
        return;
    };
    // Outside a fragment's influence sphere, and past its diffuse terminator,
    // the loop adds no diffuse: what it draws there is the texel's specular
    // alone, which the per-pixel gates no longer cut (P10).
    for march in [false, true] {
        for (name, light, fixed_normal) in [
            ("influence", influence_light(), None),
            ("terminator", terminator_light(), Some(TILTED)),
        ] {
            let what = format!("{name}, march {march}");
            let case = |style, output| MoverCase {
                march,
                fixed_normal,
                shadow_pattern: true,
                ..MoverCase::new(style, output)
            };
            let looped = render(
                &ctx,
                &[light],
                case(Style::Pixel, Output::Loop),
                Observe::Shade,
            );
            let term = render(
                &ctx,
                &[light],
                case(Style::Pixel, Output::TexelTerm),
                Observe::Shade,
            );
            let smooth = render(
                &ctx,
                &[light],
                case(Style::Smooth, Output::Loop),
                Observe::Shade,
            );
            let points = render(
                &ctx,
                &[light],
                case(Style::Pixel, Output::Loop),
                Observe::Point,
            );
            let mut gated = 0;
            let mut cut_by_smooth = 0;
            for (x, y) in pixels() {
                let i = index(x, y);
                let world = fragment_world(x, y);
                let dist = |p: [f64; 3]| {
                    (0..3)
                        .map(|a| (p[a] - f64::from(light.position[a])).powi(2))
                        .sum::<f64>()
                        .sqrt()
                };
                let radius = f64::from(light.influence);
                let gate_closed = match name {
                    // The plane position the loop tests influence at.
                    "influence" => {
                        let point = [0, 1, 2].map(|a| f64::from(points[i][a]));
                        dist(world) > radius + BOUNDARY_MARGIN
                            && dist(point) < radius - BOUNDARY_MARGIN
                    }
                    // With the march off the hit is the plane position. The
                    // terminator runs well clear of every fragment column.
                    _ => !march && !reached_per_pixel(light, TILTED, x, y),
                };
                if !gate_closed {
                    continue;
                }
                assert_eq!(
                    bits(looped[i]),
                    bits(term[i]),
                    "{what}: gated fragment ({x}, {y}) draws its texel's specular alone"
                );
                if fragment_shadow(x, y) > 0.0 && term[i][0] > 0.0 {
                    gated += 1;
                    if smooth[i][0] == 0.0 {
                        cut_by_smooth += 1;
                    }
                }
            }
            if name == "influence" || !march {
                assert!(
                    gated > 0,
                    "{what}: some lit fragment must sit past the gate"
                );
                assert!(
                    cut_by_smooth > 0,
                    "{what}: smooth keeps the per-pixel gate there and draws nothing"
                );
            }
        }
    }
}

#[test]
fn mover_texel_top_and_side_faces_share_one_point() {
    let Some(ctx) = gpu_or_skip("mover_texel_top_and_side_faces_share_one_point") else {
        return;
    };
    let lights = [flank_light()];
    let case = MoverCase {
        march: true,
        ..MoverCase::new(Style::Pixel, Output::TexelTerm)
    };
    let points = render(&ctx, &lights, case, Observe::Point);
    let texels = render(&ctx, &lights, case, Observe::Texel);
    let real = render(&ctx, &lights, case, Observe::Shade);
    let up = render(
        &ctx,
        &lights,
        MoverCase {
            fixed_normal: Some([0.0, 1.0, 0.0]),
            ..case
        },
        Observe::Shade,
    );

    let mut by_texel: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
    for (i, texel) in texels.iter().enumerate() {
        let carved = texel[3] == 1.0;
        if let Some(key) = interior_texel(*texel).filter(|_| carved) {
            by_texel.entry(key).or_default().push(i);
        }
    }
    let mut shared = 0;
    let mut normal_differs = 0;
    for ((tx, ty), members) in &by_texel {
        let top = members.iter().find(|&&m| face(texels[m]) == 0);
        let side = members.iter().find(|&&m| face(texels[m]) != 0);
        // One point per texel: its centre raised by its own top height.
        let height = texel_height(
            tx.rem_euclid(i64::from(LATTICE)) as u32,
            ty.rem_euclid(i64::from(LATTICE)) as u32,
        ) * DEPTH_SCALE_M;
        for &m in members {
            assert_eq!(
                points[m][..3]
                    .iter()
                    .map(|c| c.to_bits())
                    .collect::<Vec<_>>(),
                points[members[0]][..3]
                    .iter()
                    .map(|c| c.to_bits())
                    .collect::<Vec<_>>(),
                "texel ({tx}, {ty}): fragment {m} on face {} takes another point",
                face(texels[m])
            );
            assert_eq!(points[m][1], height, "texel ({tx}, {ty}) point height");
            // With every face shaded by one normal they draw one value.
            assert_eq!(
                bits(up[m]),
                bits(up[members[0]]),
                "texel ({tx}, {ty}) under one normal"
            );
        }
        if let (Some(&top), Some(&side)) = (top, side) {
            shared += 1;
            if bits(real[top]) != bits(real[side]) {
                normal_differs += 1;
            }
        }
    }
    assert!(
        shared >= 8,
        "many texels must show a top and a side face, got {shared}"
    );
    assert!(
        normal_differs > 0,
        "a side face differs from its top through its normal"
    );
}

#[test]
fn mover_point_height_is_the_hit_height_on_every_top_hit() {
    let Some(ctx) = gpu_or_skip("mover_point_height_is_the_hit_height_on_every_top_hit") else {
        return;
    };
    // The far eye leaves every ray starting at the band's peak; the low eye
    // stands inside the raised texel at (0, 0), whose column the march reads
    // as the band's floor.
    for (name, eye) in [
        ("far eye", [-1.0, 3.0, -1.0]),
        ("low eye", [0.25, 0.03, 0.25]),
    ] {
        let case = MoverCase {
            march: true,
            eye,
            ..MoverCase::new(Style::Pixel, Output::TexelTerm)
        };
        let points = render(&ctx, &[flank_light()], case, Observe::Point);
        let texels = render(&ctx, &[flank_light()], case, Observe::Texel);
        let mut tops = 0;
        let mut eye_column = 0;
        for (i, texel) in texels.iter().enumerate() {
            let carved = texel[3] == 1.0;
            if !carved || face(*texel) != 0 {
                continue;
            }
            tops += 1;
            let [_, point_height, _, hit_height] = points[i];
            assert!(
                (point_height - hit_height).abs() <= 1.0e-6,
                "{name}: top-hit fragment {i} has its point at {point_height}, the hit at {hit_height}"
            );
            let (tx, ty) = (
                (texel[0].floor() as i64).rem_euclid(i64::from(LATTICE)) as u32,
                (texel[1].floor() as i64).rem_euclid(i64::from(LATTICE)) as u32,
            );
            if texel_height(tx, ty) == RAISED && point_height == SUNK * DEPTH_SCALE_M {
                eye_column += 1;
            }
        }
        assert!(tops > 0, "{name}: the march must take top hits");
        if name == "low eye" {
            assert!(
                eye_column > 0,
                "{name}: the eye's own column must take top hits"
            );
        }
    }
}

#[test]
fn smooth_specular_matches_the_frozen_pre_change_function() {
    let Some(ctx) = gpu_or_skip("kinematic smooth_specular_matches_the_frozen_pre_change_function")
    else {
        return;
    };
    // The whole loop: a dynamic light the Surface Depth self-shadow marches,
    // an animated-tail spot under its brightness curve, then selected-static
    // point, spot, directional, ranged and influence-limited records.
    let dynamic = MoverLight::point([1.0, 0.6, 1.5], 3.0);
    let animated = MoverLight {
        color: [0.8, 0.6, 0.4],
        ..spot_light()
    };
    let lights = [
        dynamic,
        animated,
        flank_light(),
        spot_light(),
        sun(),
        ranged_light(),
        influence_light(),
        terminator_light(),
    ];
    let smooth = MoverCase {
        dynamic_light_count: 1,
        scripted_light_count: 2,
        ..MoverCase::new(Style::Smooth, Output::Loop)
    };
    let mut cases = Vec::new();
    for march in [false, true] {
        for shadow_pattern in [false, true] {
            cases.push((
                format!("march {march}, shadows {shadow_pattern}"),
                MoverCase {
                    march,
                    shadow_pattern,
                    ..smooth
                },
            ));
        }
    }
    cases.extend([
        (
            "specular off".to_owned(),
            MoverCase {
                march: true,
                use_specular: false,
                ..smooth
            },
        ),
        (
            "sharp exponent".to_owned(),
            MoverCase {
                march: true,
                exponent: 96.0,
                ..smooth
            },
        ),
        (
            "tilted normal".to_owned(),
            MoverCase {
                fixed_normal: Some(TILTED),
                ..smooth
            },
        ),
        (
            "low eye".to_owned(),
            MoverCase {
                march: true,
                eye: [0.25, 0.03, 0.25],
                ..smooth
            },
        ),
    ]);
    for (name, case) in cases {
        let current = render(&ctx, &lights, case, Observe::Shade);
        let frozen = render(
            &ctx,
            &lights,
            MoverCase {
                output: Output::Frozen,
                ..case
            },
            Observe::Shade,
        );
        for (i, (c, f)) in current.iter().zip(&frozen).enumerate() {
            assert_eq!(
                bits(*c),
                bits(*f),
                "{name}: fragment {i} is {c:?}, the 1c7fc9e3a loop drew {f:?}"
            );
        }
        assert!(
            current.iter().any(|p| p[0] > 0.0),
            "{name}: the lights must reach the floor"
        );
    }
}
