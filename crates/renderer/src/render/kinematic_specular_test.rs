// GPU readback tests for the mover pass's selected-static specular under the
// pixel and smooth styles, with the specular slot's per-texel sharpness and
// tint (`kinematic_specular_harness.rs`).
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
    // alone, which the per-pixel gates no longer cut.
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

// --- Per-texel sharpness and tint through the mover's beauty entries ---

use super::kinematic_specular_harness::{MaterialPath, checker_height_byte, render_material};
use super::specular_slot_fixtures::{
    MaterialTextures, SlotFormat, material_textures, slot_mip_chain, white_material,
};

/// Selected-static point, spot, directional and ranged records.
fn material_lights() -> [MoverLight; 4] {
    [flank_light(), spot_light(), sun(), ranged_light()]
}

/// The whole loop the frozen comparison drives: a dynamic light the Surface
/// Depth self-shadow marches, an animated-tail spot, then selected-static
/// records of every kind.
fn whole_loop_lights() -> [MoverLight; 8] {
    [
        MoverLight::point([1.0, 0.6, 1.5], 3.0),
        MoverLight {
            color: [0.8, 0.6, 0.4],
            ..spot_light()
        },
        flank_light(),
        spot_light(),
        sun(),
        ranged_light(),
        influence_light(),
        terminator_light(),
    ]
}

fn whole_loop(case: MoverCase) -> MoverCase {
    MoverCase {
        dynamic_light_count: 1,
        scripted_light_count: 2,
        ..case
    }
}

fn rgba_bits(pixel: [f32; 4]) -> [u32; 4] {
    pixel.map(f32::to_bits)
}

/// The interior fragment of each of the `LATTICE`² texels, with its texel.
fn texel_interiors() -> impl Iterator<Item = ((u32, u32), usize)> {
    (0..LATTICE).flat_map(|ty| {
        (0..LATTICE).map(move |tx| {
            (
                (tx, ty),
                index(
                    tx * PIXELS_PER_TEXEL + PIXELS_PER_TEXEL / 2,
                    ty * PIXELS_PER_TEXEL + PIXELS_PER_TEXEL / 2,
                ),
            )
        })
    })
}

/// Smoothness bytes across the decode's bands, as the forward test uses.
fn smoothness_byte(tx: u32, ty: u32) -> u8 {
    const LISTED: [u8; 45] = [
        0, 1, 2, 12, 24, 25, 26, 30, 40, 50, 64, 80, 96, 100, 110, 120, 127, 128, 129, 140, 150,
        160, 170, 180, 190, 200, 205, 210, 215, 220, 222, 223, 224, 225, 226, 230, 235, 240, 245,
        250, 251, 252, 253, 254, 255,
    ];
    let i = (ty * LATTICE + tx) as usize;
    LISTED.get(i).copied().unwrap_or(((i * 37) % 256) as u8)
}

fn varied_albedo(tx: u32, ty: u32) -> [u8; 4] {
    let (tx, ty) = (tx % 8, ty % 8);
    [
        (30 + 25 * tx) as u8,
        (200 - 20 * ty) as u8,
        (90 + 13 * (tx ^ ty)) as u8,
        255,
    ]
}

fn assert_bitwise_equal(current: &[[f32; 4]], oracle: &[[f32; 4]], what: &str) {
    for (i, (c, o)) in current.iter().zip(oracle).enumerate() {
        assert_eq!(
            rgba_bits(*c),
            rgba_bits(*o),
            "{what}: fragment {i} is {c:?}, before tint {o:?}"
        );
    }
}

/// The float-reassociation bound the tint rows use, per channel.
fn reassociation_bound(expected: f32) -> f32 {
    2.0e-6 + expected.abs() * 2.0e-5
}

fn assert_within_reassociation(current: &[[f32; 4]], oracle: &[[f32; 4]], what: &str) {
    for (i, (c, o)) in current.iter().zip(oracle).enumerate() {
        for k in 0..3 {
            assert!(
                (c[k] - o[k]).abs() <= reassociation_bound(o[k]),
                "{what}: fragment {i} channel {k} is {c:?}, before the change {o:?}"
            );
        }
        assert_eq!(c[3].to_bits(), o[3].to_bits(), "{what}: fragment {i} alpha");
    }
}

/// Some fragment's specular-on shade clears its specular-off shade by more
/// than the reassociation bound, so the bound cannot hide a lost highlight.
fn assert_specular_reaches(on: &[[f32; 4]], off: &[[f32; 4]], what: &str) {
    assert!(
        on.iter()
            .zip(off)
            .any(|(on, off)| (0..3).any(|k| on[k] - off[k] > reassociation_bound(off[k]))),
        "{what}: specular must clear the reassociation bound somewhere"
    );
}

/// The style's comparison against the pre-change oracle: pixel keeps its
/// bits; smooth sums specular apart from diffuse, so it holds within float
/// reassociation.
fn assert_matches_oracle(style: Style, current: &[[f32; 4]], oracle: &[[f32; 4]], what: &str) {
    match style {
        Style::Pixel => assert_bitwise_equal(current, oracle, what),
        Style::Smooth => assert_within_reassociation(current, oracle, what),
    }
}

fn render_both(
    ctx: &super::gpu_test_harness::GpuCtx,
    lights: &[MoverLight],
    case: MoverCase,
    material: &MaterialTextures,
    oracle_decodes: bool,
) -> (Vec<[f32; 4]>, Vec<[f32; 4]>) {
    let current = render_material(ctx, lights, case, Observe::Shade, material);
    let oracle = render_material(
        ctx,
        lights,
        MoverCase {
            material: MaterialPath::Oracle,
            oracle_decodes,
            ..case
        },
        Observe::Shade,
        material,
    );
    (current, oracle)
}

#[test]
fn sharpness_decode_matches_the_cpu_reference() {
    use postretro_render_cpu::specular_texel::texel_exponent;
    let Some(ctx) = gpu_or_skip("kinematic sharpness_decode_matches_the_cpu_reference") else {
        return;
    };
    let material = white_material(&ctx, LATTICE, SlotFormat::Rgba8, &|tx, ty| {
        [200, 127, smoothness_byte(tx, ty), 255]
    });
    let prefix = 64.0;
    let lights = material_lights();
    for style in [Style::Pixel, Style::Smooth] {
        let case = MoverCase {
            exponent: prefix,
            ..MoverCase::material(style, MaterialPath::Current)
        };
        let slot = render_material(&ctx, &lights, case, Observe::Slot, &material);
        let texel = render_material(&ctx, &lights, case, Observe::SpecularTexel, &material);
        for ((tx, ty), i) in texel_interiors() {
            let byte = smoothness_byte(tx, ty);
            let b = slot[i][2];
            let what = format!("{style:?}: texel ({tx}, {ty}), byte {byte}");
            assert!(
                (b - f32::from(byte) / 255.0).abs() <= 0.5 / 255.0,
                "{what}: B {b}"
            );
            let exponent = texel[i][0];
            let expected = texel_exponent(b, prefix);
            match byte {
                0 => assert_eq!(exponent, prefix, "{what}: no `_p` keeps the prefix"),
                1..=24 => assert_eq!(exponent, 1.0, "{what}"),
                224..=255 => assert_eq!(exponent, 8192.0, "{what}: past the cap"),
                _ => assert!(
                    (exponent - expected).abs() <= expected * 2.0e-6,
                    "{what}: {exponent} vs the CPU's {expected}"
                ),
            }
        }
        let (current, oracle) = render_both(&ctx, &lights, case, &material, true);
        assert_matches_oracle(style, &current, &oracle, &format!("{style:?}"));
        assert!(
            current.iter().any(|p| p[0] > 0.3),
            "{style:?}: the lights reach the floor"
        );
    }
}

/// Albedo rows (sRGB) and tint bytes by column, as the forward test uses.
const TINT_ALBEDOS: [[u8; 4]; LATTICE as usize] = [
    [28, 64, 40, 255],
    [150, 96, 44, 255],
    [128, 128, 128, 255],
    [0, 0, 0, 255],
    [255, 140, 60, 255],
    [128, 70, 30, 255],
    [20, 40, 230, 255],
    [255, 255, 255, 255],
];
const TINT_BYTES: [u8; LATTICE as usize] = [0, 64, 127, 190, 254, 255, 0, 254];

fn assert_rgb_close(actual: [f32; 4], expected: [f32; 3], what: &str) {
    for c in 0..3 {
        assert!(
            (actual[c] - expected[c]).abs() <= 2.0e-6 + expected[c].abs() * 2.0e-5,
            "{what}: channel {c} is {actual:?}, expected {expected:?}"
        );
    }
}

#[test]
fn tint_mix_matches_the_cpu_reference() {
    use postretro_render_cpu::specular_texel::{tint_amount, tint_color};
    let Some(ctx) = gpu_or_skip("kinematic tint_mix_matches_the_cpu_reference") else {
        return;
    };
    let material = material_textures(
        &ctx,
        LATTICE,
        &|_, ty| TINT_ALBEDOS[ty as usize],
        SlotFormat::Rgba8,
        &|tx, _| {
            [
                if tx < 6 { 255 } else { 96 },
                127,
                0,
                TINT_BYTES[tx as usize],
            ]
        },
    );
    let lights = material_lights();
    for style in [Style::Pixel, Style::Smooth] {
        let case = MoverCase::material(style, MaterialPath::Current);
        let observe = |observe| render_material(&ctx, &lights, case, observe, &material);
        let slot = observe(Observe::Slot);
        let texel = observe(Observe::SpecularTexel);
        let albedo = observe(Observe::Albedo);
        let raw = observe(Observe::RawTinted);
        let out = observe(Observe::TintedOut);
        for ((tx, ty), i) in texel_interiors() {
            let what = format!(
                "{style:?}: texel ({tx}, {ty}), tint byte {}",
                TINT_BYTES[tx as usize]
            );
            match tint_amount(slot[i][3]) {
                Some(t) => {
                    assert_eq!(texel[i][2], 1.0, "{what}: tinted");
                    assert!((texel[i][3] - t).abs() <= 1.0e-6, "{what}");
                    assert!(raw[i][0] > 0.0, "{what}: the lights reach the texel");
                    let tint = tint_color(rgb(albedo[i]), t);
                    let expected = std::array::from_fn(|c| raw[i][c] * tint[c]);
                    assert_rgb_close(out[i], expected, &what);
                    if TINT_BYTES[tx as usize] == 0 || matches!(ty, 2 | 3 | 7) {
                        assert_rgb_close(out[i], rgb(raw[i]), &format!("{what}: glints white"));
                    }
                }
                None => {
                    assert_eq!(texel[i][2], 0.0, "{what}: untinted");
                    match style {
                        // An untinted texel's specular stays in the pixel
                        // loop's lit sum; nothing reaches the tinted one.
                        Style::Pixel => {
                            assert_eq!(rgb(raw[i]), [0.0; 3], "{what}");
                            assert_eq!(rgb(out[i]), [0.0; 3], "{what}");
                        }
                        // Smooth's one sum takes the raw albedo.
                        Style::Smooth => {
                            assert!(raw[i][0] > 0.0, "{what}: the lights reach the texel");
                            let albedo = rgb(albedo[i]);
                            let expected = std::array::from_fn(|c| raw[i][c] * albedo[c]);
                            assert_rgb_close(out[i], expected, &what);
                        }
                    }
                }
            }
        }
    }
}

/// A slot of `format` whose intensity, height (the checker) and four-channel
/// smoothness vary per texel; never tinted.
fn untinted_slot(format: SlotFormat) -> impl Fn(u32, u32) -> [u8; 4] {
    move |tx, ty| {
        let intensity = (40 + 29 * tx + 11 * ty) as u8;
        let smoothness = if format == SlotFormat::Rgba8 {
            smoothness_byte(tx % LATTICE, ty % LATTICE).max(1)
        } else {
            0
        };
        [
            intensity,
            checker_height_byte(tx % LATTICE, ty % LATTICE),
            smoothness,
            255,
        ]
    }
}

// Pixel's untinted texels keep their pre-tint bits; smooth's whole-loop row
// is `smooth_shade_matches_the_frozen_pre_change_loop_within_reassociation`.
#[test]
fn untinted_pixel_specular_matches_the_frozen_pre_change_function() {
    let Some(ctx) =
        gpu_or_skip("kinematic untinted_pixel_specular_matches_the_frozen_pre_change_function")
    else {
        return;
    };
    let lights = whole_loop_lights();
    for format in [SlotFormat::R8, SlotFormat::Rg8, SlotFormat::Rgba8] {
        let material = material_textures(
            &ctx,
            LATTICE,
            &varied_albedo,
            format,
            &untinted_slot(format),
        );
        // A one-channel slot never marches: has-depth is clear for it.
        let marches: &[bool] = if format == SlotFormat::R8 {
            &[false]
        } else {
            &[false, true]
        };
        for &march in marches {
            for shadow_pattern in [false, true] {
                let case = MoverCase {
                    march,
                    shadow_pattern,
                    ..whole_loop(MoverCase::material(Style::Pixel, MaterialPath::Current))
                };
                let (current, oracle) =
                    render_both(&ctx, &lights, case, &material, format == SlotFormat::Rgba8);
                assert_bitwise_equal(
                    &current,
                    &oracle,
                    &format!("{format:?}, march {march}, shadows {shadow_pattern}"),
                );
            }
        }
    }
}

/// The four-channel checker the whole-loop cases shade over: full intensity,
/// the height checker, no `_p` and no `_t`, under a varied albedo.
fn whole_loop_material(ctx: &super::gpu_test_harness::GpuCtx) -> MaterialTextures {
    material_textures(
        ctx,
        LATTICE,
        &varied_albedo,
        SlotFormat::Rgba8,
        &|tx, ty| [255, checker_height_byte(tx % LATTICE, ty % LATTICE), 0, 255],
    )
}

// The smooth pipeline sums specular apart from diffuse and colours it once
// after the loop, so its shade matches the 1c7fc9e3a loop composed the old
// way (one lit sum the albedo multiplies) only up to float reassociation
// (rendering_pipeline.md §4, Tint and output). The current side is the smooth
// entry's own loop and composition, spliced from the shader; the bound is the
// tint rows'. Every case but specular-off proves the bound hides no highlight.
#[test]
fn smooth_shade_matches_the_frozen_pre_change_loop_within_reassociation() {
    let Some(ctx) = gpu_or_skip(
        "kinematic smooth_shade_matches_the_frozen_pre_change_loop_within_reassociation",
    ) else {
        return;
    };
    let lights = whole_loop_lights();
    let smooth = whole_loop(MoverCase::material(Style::Smooth, MaterialPath::Current));
    let check = |name: &str, case: MoverCase, material: &MaterialTextures, decodes: bool| {
        let (current, oracle) = render_both(&ctx, &lights, case, material, decodes);
        assert_within_reassociation(&current, &oracle, name);
        if case.use_specular {
            let off = render_material(
                &ctx,
                &lights,
                MoverCase {
                    use_specular: false,
                    ..case
                },
                Observe::Shade,
                material,
            );
            assert_specular_reaches(&current, &off, name);
        }
    };

    let checker = whole_loop_material(&ctx);
    for march in [false, true] {
        for shadow_pattern in [false, true] {
            check(
                &format!("march {march}, shadows {shadow_pattern}"),
                MoverCase {
                    march,
                    shadow_pattern,
                    ..smooth
                },
                &checker,
                false,
            );
        }
    }
    for (name, case) in [
        (
            "specular off",
            MoverCase {
                march: true,
                use_specular: false,
                ..smooth
            },
        ),
        (
            "sharp exponent",
            MoverCase {
                march: true,
                exponent: 96.0,
                ..smooth
            },
        ),
        (
            "tilted normal",
            MoverCase {
                fixed_normal: Some(TILTED),
                ..smooth
            },
        ),
        (
            "low eye",
            MoverCase {
                march: true,
                eye: [0.25, 0.03, 0.25],
                ..smooth
            },
        ),
    ] {
        check(name, case, &checker, false);
    }

    for format in [SlotFormat::R8, SlotFormat::Rg8, SlotFormat::Rgba8] {
        let material = material_textures(
            &ctx,
            LATTICE,
            &varied_albedo,
            format,
            &untinted_slot(format),
        );
        // A one-channel slot never marches: has-depth is clear for it.
        let marches: &[bool] = if format == SlotFormat::R8 {
            &[false]
        } else {
            &[false, true]
        };
        for &march in marches {
            for shadow_pattern in [false, true] {
                check(
                    &format!("{format:?} untinted, march {march}, shadows {shadow_pattern}"),
                    MoverCase {
                        march,
                        shadow_pattern,
                        ..smooth
                    },
                    &material,
                    format == SlotFormat::Rgba8,
                );
            }
        }
    }
}

#[test]
fn diffuse_is_identical_with_and_without_tint() {
    let Some(ctx) = gpu_or_skip("kinematic diffuse_is_identical_with_and_without_tint") else {
        return;
    };
    let lights = whole_loop_lights();
    let slots: [(&str, SlotFormat, u8); 4] = [
        ("one-channel", SlotFormat::R8, 255),
        ("no `_t`", SlotFormat::Rgba8, 255),
        ("t = 0", SlotFormat::Rgba8, 0),
        ("t = 1", SlotFormat::Rgba8, 254),
    ];
    for style in [Style::Pixel, Style::Smooth] {
        let case = MoverCase {
            use_specular: false,
            shadow_pattern: true,
            ..whole_loop(MoverCase::material(style, MaterialPath::Current))
        };
        let mut renders = Vec::new();
        for (name, format, tint) in slots {
            let material = material_textures(&ctx, LATTICE, &varied_albedo, format, &|tx, ty| {
                [200, 127, smoothness_byte(tx % LATTICE, ty % LATTICE), tint]
            });
            let (current, oracle) = render_both(&ctx, &lights, case, &material, false);
            assert_bitwise_equal(&current, &oracle, &format!("{style:?}, {name}"));
            renders.push((name, current));
        }
        for (name, shade) in &renders[1..] {
            assert_bitwise_equal(
                shade,
                &renders[0].1,
                &format!("{style:?}: {name} vs one-channel"),
            );
        }
    }
}

/// How the floor maps onto the target, as the forward test's footprints.
const FOOTPRINTS: [(&str, u32, [f32; 2]); 3] = [
    ("magnified", LATTICE, [1.0 / TARGET as f32; 2]),
    ("minified", 64, [2.5 / TARGET as f32; 2]),
    ("grazing", 64, [0.5 / TARGET as f32, 7.0 / TARGET as f32]),
];

#[test]
fn marker_slots_keep_their_meaning_through_filtering() {
    let Some(ctx) = gpu_or_skip("kinematic marker_slots_keep_their_meaning_through_filtering")
    else {
        return;
    };
    let lights = material_lights();
    let prefix = 64.0;
    for (footprint, size, uv_per_pixel) in FOOTPRINTS {
        for (name, marker) in [
            ("`_p` at B = 1", [255, 127, 1, 255]),
            ("`_t` at A = 254", [255, 127, 0, 254]),
        ] {
            let material = white_material(&ctx, size, SlotFormat::Rgba8, &|_, _| marker);
            let case = MoverCase {
                exponent: prefix,
                uv_per_pixel,
                ..MoverCase::material(Style::Pixel, MaterialPath::Current)
            };
            let texel = render_material(&ctx, &lights, case, Observe::SpecularTexel, &material);
            for (i, [exponent, intensity, tinted, tint]) in texel.into_iter().enumerate() {
                let what = format!("{footprint}, {name}: fragment {i}");
                assert!(
                    (intensity - 1.0).abs() <= 0.5 / 255.0,
                    "{what}: intensity {intensity}"
                );
                if marker[2] == 1 {
                    assert_eq!(exponent, 1.0, "{what}: smoothness byte 1 is exponent 1");
                    assert_eq!(tinted, 0.0, "{what}: raw albedo");
                } else {
                    assert_eq!(exponent, prefix, "{what}: no `_p` keeps the prefix");
                    assert_eq!(tinted, 1.0, "{what}: tinted");
                    assert!(
                        (tint - 1.0).abs() <= 0.5 / 254.0,
                        "{what}: t = 1, read {tint}"
                    );
                }
            }
        }
    }
}

#[test]
fn narrow_specular_slots_shade_in_raw_albedo() {
    let Some(ctx) = gpu_or_skip("kinematic narrow_specular_slots_shade_in_raw_albedo") else {
        return;
    };
    let lights = material_lights();
    for (footprint, size, uv_per_pixel) in FOOTPRINTS {
        for format in [SlotFormat::R8, SlotFormat::Rg8, SlotFormat::Placeholder] {
            let material =
                material_textures(&ctx, size, &varied_albedo, format, &untinted_slot(format));
            for style in [Style::Pixel, Style::Smooth] {
                let case = MoverCase {
                    uv_per_pixel,
                    ..MoverCase::material(style, MaterialPath::Current)
                };
                let what = format!("{footprint}, {format:?}, {style:?}");
                let texel = render_material(&ctx, &lights, case, Observe::SpecularTexel, &material);
                for (i, [exponent, _, tinted, _]) in texel.into_iter().enumerate() {
                    assert_eq!(exponent, case.exponent, "{what}: fragment {i}: the prefix");
                    assert_eq!(tinted, 0.0, "{what}: fragment {i}: raw albedo");
                }
                let (current, oracle) = render_both(&ctx, &lights, case, &material, false);
                assert_matches_oracle(style, &current, &oracle, &what);
            }
        }
    }
}

/// The material uniform the renderer uploads for `material` over a loaded
/// four-channel slot, at `quality`: shininess from the prefix, has-depth from
/// the format, the relief band measured from the uploaded mips.
fn four_channel_uniform(
    material: postretro_render_data::material::Material,
    slot: &dyn Fn(u32, u32) -> [u8; 4],
    quality: postretro_render_cpu::surface_depth::SurfaceDepthQuality,
) -> [u8; 32] {
    use postretro_render_cpu::material_plan::MaterialUniformPlan;
    use postretro_render_cpu::surface_depth::surface_relief_from_surface_map;
    let chain = slot_mip_chain(LATTICE, SlotFormat::Rgba8, slot);
    let levels: Vec<(u32, u32, &[u8])> = chain
        .iter()
        .map(|(side, bytes)| (*side, *side, bytes.as_slice()))
        .collect();
    let relief = surface_relief_from_surface_map(&levels, 4);
    MaterialUniformPlan::new(material, 0.0, true, chain.len() as u32, relief).uniform_bytes(quality)
}

#[test]
fn sharpness_and_tint_hold_through_surface_depth_off_and_on() {
    use postretro_render_cpu::specular_texel::texel_exponent;
    use postretro_render_cpu::surface_depth::SurfaceDepthQuality::{Off, On};
    use postretro_render_data::material::Material;
    let Some(ctx) =
        gpu_or_skip("kinematic sharpness_and_tint_hold_through_surface_depth_off_and_on")
    else {
        return;
    };
    let lights = material_lights();
    // (case, prefix, slot, marches at On). Bytes as the bake writes them: G
    // the stored plane (127) without `_h`, B = 0 without `_p`.
    type Slot = fn(u32, u32) -> [u8; 4];
    let with_height: Slot = |tx, ty| {
        [
            255,
            checker_height_byte(tx, ty),
            smoothness_byte(tx, ty).max(1),
            255,
        ]
    };
    let without_height: Slot = |tx, ty| [255, 127, smoothness_byte(tx, ty).max(1), 255];
    let tinted_with_height: Slot = |tx, ty| {
        [
            255,
            checker_height_byte(tx, ty),
            smoothness_byte(tx, ty).max(1),
            254,
        ]
    };
    let tint_only: Slot = |tx, ty| [255, 127, 0, if (tx + ty) % 2 == 0 { 254 } else { 0 }];
    let cases: [(&str, Material, Slot, bool); 5] = [
        ("`_p`+`_h`", Material::Concrete, with_height, true),
        ("`_p` alone", Material::Concrete, without_height, false),
        (
            "`_p`+`_h`, untuned prefix",
            Material::Glass,
            with_height,
            false,
        ),
        (
            "`_p`+`_t`+`_h`",
            Material::Concrete,
            tinted_with_height,
            true,
        ),
        ("`_t` alone", Material::Metal, tint_only, false),
    ];
    for (name, prefix, slot, marches) in cases {
        let material = material_textures(&ctx, LATTICE, &varied_albedo, SlotFormat::Rgba8, &slot);
        // Built at Off, then On, then Off again: per-texel sharpness and tint must hold through the Surface Depth toggle.
        for (step, quality) in [Off, On, Off].into_iter().enumerate() {
            for style in [Style::Pixel, Style::Smooth] {
                let what = format!("{name}, step {step} ({quality:?}), {style:?}");
                let case = MoverCase {
                    material_uniform: Some(four_channel_uniform(prefix, &slot, quality)),
                    ..MoverCase::material(style, MaterialPath::Current)
                };
                let sampled = render_material(&ctx, &lights, case, Observe::Slot, &material);
                let texel = render_material(&ctx, &lights, case, Observe::SpecularTexel, &material);
                let carved = render_material(&ctx, &lights, case, Observe::Texel, &material);
                let prefix_exponent = prefix.shininess().max(1.0);
                for (i, p) in texel.iter().enumerate() {
                    let b = sampled[i][2];
                    let expected = texel_exponent(b, prefix_exponent);
                    assert!(
                        (p[0] - expected).abs() <= expected * 2.0e-6,
                        "{what}: fragment {i} exponent {} vs {expected}",
                        p[0]
                    );
                    assert_eq!(
                        p[2],
                        f32::from(sampled[i][3] < 254.5 / 255.0),
                        "{what}: fragment {i} tint"
                    );
                }
                let any_sharp = texel.iter().any(|p| p[0] != prefix_exponent);
                let any_tinted = texel.iter().any(|p| p[2] == 1.0);
                assert_eq!(
                    any_sharp,
                    slot(0, 0)[2] != 0 || slot(1, 0)[2] != 0,
                    "{what}: per-texel sharpness"
                );
                assert_eq!(any_tinted, slot(0, 0)[3] != 255, "{what}: tint");
                let any_carved = carved.iter().any(|p| p[3] != 0.0);
                assert_eq!(
                    any_carved,
                    marches && quality == On,
                    "{what}: the march runs only for `_h` at On"
                );
                // Highlights survive the toggle: specular clears the
                // reassociation bound somewhere at every step.
                let shade = render_material(&ctx, &lights, case, Observe::Shade, &material);
                let unlit = render_material(
                    &ctx,
                    &lights,
                    MoverCase {
                        use_specular: false,
                        ..case
                    },
                    Observe::Shade,
                    &material,
                );
                assert_specular_reaches(&shade, &unlit, &what);
            }
        }
    }
}

#[test]
fn tint_only_bundles_shade_at_each_prefix_exponent() {
    use postretro_render_cpu::surface_depth::SurfaceDepthQuality;
    use postretro_render_data::material::Material;
    let Some(ctx) = gpu_or_skip("kinematic tint_only_bundles_shade_at_each_prefix_exponent") else {
        return;
    };
    let slot = |tx: u32, _: u32| [255, 127, 0, TINT_BYTES[tx as usize].min(254)];
    let material = white_material(&ctx, LATTICE, SlotFormat::Rgba8, &slot);
    let lights = material_lights();
    let mut raws = Vec::new();
    for prefix in [Material::Metal, Material::Concrete] {
        let exponent = prefix.shininess().max(1.0);
        let case = MoverCase {
            material_uniform: Some(four_channel_uniform(prefix, &slot, SurfaceDepthQuality::On)),
            ..MoverCase::material(Style::Pixel, MaterialPath::Current)
        };
        let texel = render_material(&ctx, &lights, case, Observe::SpecularTexel, &material);
        for (i, p) in texel.iter().enumerate() {
            assert_eq!(
                p[0], exponent,
                "{prefix:?}: fragment {i} shades at its prefix"
            );
            assert_eq!(p[2], 1.0, "{prefix:?}: fragment {i} is tinted");
        }
        raws.push(render_material(
            &ctx,
            &lights,
            case,
            Observe::RawTinted,
            &material,
        ));
    }
    assert!(
        raws[0].iter().zip(&raws[1]).any(|(a, b)| a != b),
        "the two prefixes' exponents shape different highlights"
    );
}
