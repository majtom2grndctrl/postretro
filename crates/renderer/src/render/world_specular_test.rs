// GPU readback tests of forward.wgsl's world static specular: per-face owners,
// the SDF selection, and the pixel style's one point per albedo texel.
// See: context/lib/rendering_pipeline.md §4 (Static specular ownership,
// Texel-centre highlight)
//
// Harness and its skip policy: `world_specular_harness.rs`.

use std::collections::HashMap;

use super::gpu_test_harness::{gpu_or_skip, wgsl_const_line, wgsl_function};
use super::world_specular_harness::{
    Case, EMPTY, FORWARD_WGSL, Face, Geometry, LATTICE, Light, MATERIAL_SHADING_WGSL, March,
    NO_OWNERS, PIXELS_PER_TEXEL, Style, TEXEL_TARGET, TexelCase, WIDTH, fragment_visibility,
    key_is_centre, reconstructed_texel_key, render, render_texels,
};

fn one_face(owners: [u16; 4]) -> [Face; 1] {
    [Face {
        columns: (0, WIDTH),
        owners,
    }]
}

fn assert_close(actual: [f32; 3], expected: [f32; 3], what: &str) {
    for c in 0..3 {
        assert!(
            (actual[c] - expected[c]).abs() <= 1.0e-5 + expected[c].abs() * 1.0e-4,
            "{what}: channel {c} is {actual:?}, expected {expected:?}"
        );
    }
}

/// `assert_close` for every column of two renders.
fn assert_columns_close(actual: &[[f32; 3]], expected: &[[f32; 3]], what: &str) {
    assert_eq!(actual.len(), expected.len(), "{what}: column count");
    for (column, (a, e)) in actual.iter().zip(expected).enumerate() {
        assert_close(*a, *e, &format!("{what}, column {column}"));
    }
}

fn scaled(value: [f32; 3], scale: f32) -> [f32; 3] {
    value.map(|c| c * scale)
}

const RED: [f32; 3] = [1.0, 0.0, 0.0];
const GREEN: [f32; 3] = [0.0, 1.0, 0.0];
const BLUE: [f32; 3] = [0.0, 0.0, 1.0];
const WHITE: [f32; 3] = [1.0, 1.0, 1.0];
/// Columns either side of the edge shared by the two faces in `two_faces`.
const LEFT_OF_EDGE: usize = 7;
const RIGHT_OF_EDGE: usize = 8;

fn two_faces(left: [u16; 4], right: [u16; 4]) -> [Face; 2] {
    [
        Face {
            columns: (0, 8),
            owners: left,
        },
        Face {
            columns: (8, WIDTH),
            owners: right,
        },
    ]
}

#[test]
fn owner_specular_lights_only_the_face_that_owns_the_light() {
    let Some(ctx) = gpu_or_skip("owner_specular_lights_only_the_face_that_owns_the_light") else {
        return;
    };
    // Two point lights at the same spot over the shared edge, both in range of
    // every column. Light 0 owns the left face, light 1 the right face.
    let lights = [Light::point(RED), Light::point(GREEN)];
    let out = render(
        &ctx,
        &lights,
        &two_faces([0, EMPTY, EMPTY, EMPTY], [1, EMPTY, EMPTY, EMPTY]),
        Case::default(),
    );
    let (left, right) = (out[LEFT_OF_EDGE], out[RIGHT_OF_EDGE]);
    assert!(
        left[0] > 0.0 && left[1] == 0.0,
        "left of the edge: {left:?}"
    );
    assert!(
        right[1] > 0.0 && right[0] == 0.0,
        "right of the edge: {right:?}"
    );
    // The flat varying carries each face's owners whole: the two edge columns
    // mirror each other, so neither face blends in the other's light.
    assert_close([right[1], 0.0, 0.0], [left[0], 0.0, 0.0], "edge symmetry");

    // A face with no owners adds no static specular, whatever reaches it.
    let none = render(&ctx, &lights, &one_face(NO_OWNERS), Case::default());
    assert!(none.iter().all(|px| *px == [0.0; 3]), "{none:?}");
}

#[test]
fn owner_specular_follows_the_baked_mask_of_its_channel() {
    let Some(ctx) = gpu_or_skip("owner_specular_follows_the_baked_mask_of_its_channel") else {
        return;
    };
    let lights = [Light::point(WHITE)];
    let face = one_face([0, EMPTY, EMPTY, EMPTY]);
    let lit = render(&ctx, &lights, &face, Case::default());
    let occluded = render(
        &ctx,
        &lights,
        &face,
        Case {
            mask: [0.0, 1.0, 1.0, 1.0],
            ..Case::default()
        },
    );
    let half = render(
        &ctx,
        &lights,
        &face,
        Case {
            mask: [0.5, 1.0, 1.0, 1.0],
            ..Case::default()
        },
    );
    for column in 0..WIDTH as usize {
        assert!(
            lit[column][0] > 0.0,
            "column {column} lit: {:?}",
            lit[column]
        );
        assert_eq!(occluded[column], [0.0; 3], "column {column} occluded");
        assert_close(half[column], scaled(lit[column], 0.5), "half-lit channel");
    }

    // A block miss, or no usable mask, drops every owner.
    let unreadable = render(
        &ctx,
        &lights,
        &face,
        Case {
            owners_readable: false,
            ..Case::default()
        },
    );
    assert!(
        unreadable.iter().all(|px| *px == [0.0; 3]),
        "{unreadable:?}"
    );
    // The debug force-one toggle lights owners only, never non-owners.
    let forced = render(
        &ctx,
        &lights,
        &face,
        Case {
            mask: [0.0; 4],
            force_one: true,
            ..Case::default()
        },
    );
    assert_eq!(forced, lit, "force-one reads every owner channel as lit");
}

#[test]
fn owner_position_selects_its_own_channel_in_either_mask_group() {
    let Some(ctx) = gpu_or_skip("owner_position_selects_its_own_channel_in_either_mask_group")
    else {
        return;
    };
    let lights = [
        Light::point(RED),
        Light::point(GREEN),
        Light::point(BLUE),
        Light::point(WHITE),
    ];
    let all = one_face([0, 1, 2, 3]);
    for position in 0..4usize {
        let mut mask = [0.0; 4];
        mask[position] = 1.0;
        let through_position = render(
            &ctx,
            &lights,
            &all,
            Case {
                mask,
                ..Case::default()
            },
        );
        let alone = render(
            &ctx,
            &lights,
            &one_face([position as u16, EMPTY, EMPTY, EMPTY]),
            Case::default(),
        );
        assert_eq!(
            through_position,
            alone,
            "owner position {position} must read channel {position} (group {}, channel {})",
            position / 2,
            position % 2
        );
    }

    // Swapping two owners' positions together with their mask planes leaves
    // the image unchanged, within and across groups.
    let mask = [0.9, 0.6, 0.3, 0.15];
    let base = render(
        &ctx,
        &lights,
        &all,
        Case {
            mask,
            ..Case::default()
        },
    );
    for (a, b) in [(0usize, 1usize), (1, 2), (0, 3)] {
        let mut owners = [0u16, 1, 2, 3];
        owners.swap(a, b);
        let mut swapped_mask = mask;
        swapped_mask.swap(a, b);
        let swapped = render(
            &ctx,
            &lights,
            &one_face(owners),
            Case {
                mask: swapped_mask,
                ..Case::default()
            },
        );
        for column in 0..WIDTH as usize {
            assert_close(swapped[column], base[column], "owner swap");
        }
    }
}

#[test]
fn empty_owner_ends_the_entry_and_out_of_range_owner_adds_nothing() {
    let Some(ctx) = gpu_or_skip("empty_owner_ends_the_entry_and_out_of_range_owner_adds_nothing")
    else {
        return;
    };
    let lights = [Light::point(RED), Light::point(GREEN)];
    let terminated = render(
        &ctx,
        &lights,
        &one_face([0, EMPTY, 1, EMPTY]),
        Case::default(),
    );
    assert!(
        terminated.iter().all(|px| px[0] > 0.0 && px[1] == 0.0),
        "an owner after an empty channel is never read: {terminated:?}"
    );
    let out_of_range = render(
        &ctx,
        &lights,
        &one_face([99, EMPTY, EMPTY, EMPTY]),
        Case::default(),
    );
    assert!(
        out_of_range.iter().all(|px| *px == [0.0; 3]),
        "{out_of_range:?}"
    );
}

#[test]
fn sdf_specular_comes_only_from_the_fragment_selection() {
    let Some(ctx) = gpu_or_skip("sdf_specular_comes_only_from_the_fragment_selection") else {
        return;
    };
    let mut sdf_light = Light::point(BLUE);
    sdf_light.sdf = true;
    let lights = [Light::point(RED), sdf_light];
    let face = one_face(NO_OWNERS);

    // An SDF light the fragment did not select adds nothing, even in range.
    let unselected = render(&ctx, &lights, &face, Case::default());
    assert!(
        unselected.iter().all(|px| *px == [0.0; 3]),
        "{unselected:?}"
    );

    let selected = |factor: f32, force_lit: bool| {
        render(
            &ctx,
            &lights,
            &face,
            Case {
                sdf_indices: [1, u32::MAX, u32::MAX, u32::MAX],
                sdf_count: 1,
                sdf_factor: [factor, 1.0, 1.0, 1.0],
                sdf_force_lit: force_lit,
                ..Case::default()
            },
        )
    };
    let full = selected(1.0, false);
    let half = selected(0.5, false);
    let forced = selected(0.0, true);
    for column in 0..WIDTH as usize {
        assert!(
            full[column][2] > 0.0 && full[column][0] == 0.0,
            "{:?}",
            full[column]
        );
        assert_close(
            half[column],
            scaled(full[column], 0.5),
            "SDF visibility slice",
        );
        assert_eq!(forced[column], full[column], "SDF force-lit");
    }
}

// Missing data — a block miss, a missing or refused mask, placeholder mode —
// leaves owners unreadable (fs_main's `static_spec_owners_readable`). That
// drops every owner's specular and leaves the SDF selection's exactly as it
// was.
#[test]
fn unreadable_owners_drop_static_specular_and_keep_sdf_specular_unchanged() {
    let Some(ctx) =
        gpu_or_skip("unreadable_owners_drop_static_specular_and_keep_sdf_specular_unchanged")
    else {
        return;
    };
    let mut sdf_light = Light::point(BLUE);
    sdf_light.sdf = true;
    let lights = [Light::point(RED), sdf_light];
    let face = one_face([0, EMPTY, EMPTY, EMPTY]);
    let with_sdf = |owners_readable: bool| Case {
        sdf_indices: [1, u32::MAX, u32::MAX, u32::MAX],
        sdf_count: 1,
        sdf_factor: [0.5, 1.0, 1.0, 1.0],
        owners_readable,
        ..Case::default()
    };
    let readable = render(&ctx, &lights, &face, with_sdf(true));
    let unreadable = render(&ctx, &lights, &face, with_sdf(false));
    let sdf_only = render(&ctx, &lights, &one_face(NO_OWNERS), with_sdf(true));
    for column in 0..WIDTH as usize {
        assert!(readable[column][0] > 0.0, "{:?}", readable[column]);
        assert_eq!(unreadable[column][0], 0.0, "no static non-SDF specular");
        assert_eq!(
            unreadable[column], sdf_only[column],
            "SDF specular is unchanged"
        );
        assert_eq!(unreadable[column][2], readable[column][2]);
    }
}

#[test]
fn directional_owner_highlight_follows_its_aim_not_its_origin() {
    let Some(ctx) = gpu_or_skip("directional_owner_highlight_follows_its_aim_not_its_origin")
    else {
        return;
    };
    let face = one_face([0, EMPTY, EMPTY, EMPTY]);
    let tilted = [0.6, -0.8, 0.0];
    let here = render(
        &ctx,
        &[Light::directional([0.0, 5.0, 0.0], tilted)],
        &face,
        Case::default(),
    );
    let elsewhere = render(
        &ctx,
        &[Light::directional([-40.0, -3.0, 17.0], tilted)],
        &face,
        Case::default(),
    );
    assert_eq!(
        here, elsewhere,
        "moving the origin must not move the highlight"
    );
    assert!(
        here[0][0] > 0.0,
        "a sun above the floor lights it: {:?}",
        here[0]
    );

    // Every column sees the same sun, so a directional highlight is uniform
    // across the floor, and it changes with the aim.
    assert_columns_close(&here, &vec![here[0]; here.len()], "uniform sun highlight");
    let overhead = render(
        &ctx,
        &[Light::directional([0.0, 5.0, 0.0], [0.0, -1.0, 0.0])],
        &face,
        Case::default(),
    );
    assert!(
        overhead[0][0] > here[0][0],
        "straight down beats tilted under a top-down view"
    );
    // A sun aimed up from below the floor lights nothing.
    let below = render(
        &ctx,
        &[Light::directional([0.0, 5.0, 0.0], [0.0, 1.0, 0.0])],
        &face,
        Case::default(),
    );
    assert!(below.iter().all(|px| *px == [0.0; 3]), "{below:?}");
    // A zero aim falls back to straight down. The full-bright cone sentinels
    // still make the cone factor smoothstep(-1, 1, dot(-L, aim)), which is 1
    // only while the aim the cone reads is the light direction's own, so this
    // pins the fallback aim feeding the cone as well as the light direction.
    let unaimed = render(
        &ctx,
        &[Light::directional([0.0, 5.0, 0.0], [0.0; 3])],
        &face,
        Case::default(),
    );
    assert_columns_close(&unaimed, &overhead, "a zero aim is the straight-down sun");
}

// A directional light tagged `_shadow_type sdf` has no baked direct to
// follow: its SDF visibility slice is traced toward its origin and its SDF
// diffuse shines from there. Its specular stays the positional highlight it
// always was — keyed on the origin, with the zero-aim cone a point light at
// that origin gets — while an owned directional follows its aim.
#[test]
fn sdf_directional_specular_stays_keyed_on_its_origin() {
    let Some(ctx) = gpu_or_skip("sdf_directional_specular_stays_keyed_on_its_origin") else {
        return;
    };
    let tilted = [0.6, -0.8, 0.0];
    let sdf_sun = |origin: [f32; 3]| Light {
        sdf: true,
        ..Light::directional(origin, tilted)
    };
    let selected = Case {
        sdf_indices: [0, u32::MAX, u32::MAX, u32::MAX],
        sdf_count: 1,
        ..Case::default()
    };
    let face = one_face(NO_OWNERS);
    let here = render(&ctx, &[sdf_sun([0.0, 2.0, 0.0])], &face, selected);
    let moved = render(&ctx, &[sdf_sun([1.5, 2.0, 0.0])], &face, selected);
    assert!(here[LEFT_OF_EDGE][0] > 0.0, "{here:?}");
    assert_ne!(here, moved, "moving the origin moves the highlight");
    assert!(
        here.iter().any(|px| *px != here[0]),
        "a positional highlight varies across the floor: {here:?}"
    );

    // The same light as a cone-less point at that origin, through the owner
    // half: the expression SDF specular evaluated before owners existed.
    let point = Light {
        position: [0.0, 2.0, 0.0],
        range: 0.0,
        ..Light::point(WHITE)
    };
    let as_point = render(
        &ctx,
        &[point],
        &one_face([0, EMPTY, EMPTY, EMPTY]),
        Case::default(),
    );
    for column in 0..WIDTH as usize {
        assert_close(
            here[column],
            as_point[column],
            "SDF sun as its origin point",
        );
    }

    // An owned directional with the same origin and aim follows the aim.
    let owned = render(
        &ctx,
        &[Light::directional([0.0, 2.0, 0.0], tilted)],
        &one_face([0, EMPTY, EMPTY, EMPTY]),
        Case::default(),
    );
    assert_columns_close(
        &owned,
        &vec![owned[0]; owned.len()],
        "owned directional is uniform",
    );
    assert_ne!(owned, here);
}

// --- Texel-centre highlight (pixel style) and the smooth style ---

const OWNER_0: [u16; 4] = [0, EMPTY, EMPTY, EMPTY];

/// A point light and an eye off the floor's low corner, so the highlight
/// falls off monotonically across the whole floor along x and z.
fn flank_light() -> Light {
    Light::point(WHITE).at([-1.0, 1.5, -1.0], 10.0)
}

/// A spot straight down over the floor's middle, its cone edge crossing
/// texels.
fn spot_light() -> Light {
    Light::spot([2.0, 2.0, 2.0], [0.0, -1.0, 0.0], 20.0, 30.0)
}

fn sun() -> Light {
    Light::directional([2.0, 5.0, 2.0], [0.6, -0.8, 0.0])
}

/// A point light low over the floor's middle whose range circle crosses
/// texels.
fn ranged_light() -> Light {
    Light::point(WHITE).at([2.0, 0.5, 2.0], 1.5)
}

fn pixel_index(x: u32, y: u32) -> usize {
    (y * TEXEL_TARGET + x) as usize
}

fn pixels() -> impl Iterator<Item = (u32, u32)> {
    (0..TEXEL_TARGET).flat_map(|y| (0..TEXEL_TARGET).map(move |x| (x, y)))
}

/// Every fragment, grouped by the texel coordinate its shading UV
/// reconstructs to under `march`.
fn texel_groups(march: March) -> HashMap<(i64, i64), Vec<usize>> {
    let mut groups: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
    for (x, y) in pixels() {
        groups
            .entry(reconstructed_texel_key(x, y, march))
            .or_default()
            .push(pixel_index(x, y));
    }
    groups
}

/// Every fragment reconstructing to one texel coordinate draws one value.
fn assert_constant_per_texel(out: &[[f32; 3]], march: March, what: &str) {
    let groups = texel_groups(march);
    assert!(
        groups.values().filter(|members| members.len() > 1).count() >= 16,
        "{what}: the target must hold many multi-fragment texels",
    );
    for (key, members) in groups {
        let first = out[members[0]];
        for &member in &members {
            assert_eq!(
                out[member], first,
                "{what}: texel {key:?} must draw one value, fragment {member}"
            );
        }
    }
}

/// A fragment well inside albedo texel (tx, ty), clear of every seam band.
fn interior_value(out: &[[f32; 3]], tx: u32, ty: u32) -> [f32; 3] {
    out[pixel_index(
        tx * PIXELS_PER_TEXEL + PIXELS_PER_TEXEL / 2,
        ty * PIXELS_PER_TEXEL + PIXELS_PER_TEXEL / 2,
    )]
}

/// Whether some fragment of one un-marched texel interior differs from
/// another.
fn varies_inside_some_texel(out: &[[f32; 3]]) -> bool {
    texel_groups(March::OFF)
        .into_iter()
        .any(|((kx, ky), members)| {
            key_is_centre(kx)
                && key_is_centre(ky)
                && members.iter().any(|&m| out[m] != out[members[0]])
        })
}

fn bits(value: [f32; 3]) -> [u32; 3] {
    value.map(f32::to_bits)
}

#[test]
fn pixel_specular_is_constant_inside_a_magnified_texel() {
    let Some(ctx) = gpu_or_skip("pixel_specular_is_constant_inside_a_magnified_texel") else {
        return;
    };
    for (name, light) in [
        ("point", flank_light()),
        ("spot", spot_light()),
        ("directional", sun()),
    ] {
        for march in [March::OFF, March::ON] {
            let out = render_texels(
                &ctx,
                &[light],
                OWNER_0,
                TexelCase {
                    march,
                    ..TexelCase::new(Style::Pixel)
                },
            );
            assert_constant_per_texel(&out, march, &format!("{name} owner, {march:?}"));
        }
    }

    // Neighbouring texels sit at different points, so their highlights
    // differ, along both texture axes.
    for (name, light) in [("point", flank_light()), ("directional", sun())] {
        let out = render_texels(&ctx, &[light], OWNER_0, TexelCase::new(Style::Pixel));
        for a in 0..LATTICE {
            for b in 0..LATTICE - 1 {
                assert_ne!(
                    interior_value(&out, b, a),
                    interior_value(&out, b + 1, a),
                    "{name}: texels ({b}, {a}) and ({}, {a})",
                    b + 1
                );
                assert_ne!(
                    interior_value(&out, a, b),
                    interior_value(&out, a, b + 1),
                    "{name}: texels ({a}, {b}) and ({a}, {})",
                    b + 1
                );
            }
        }
    }
}

#[test]
fn smooth_specular_varies_inside_a_magnified_texel() {
    let Some(ctx) = gpu_or_skip("smooth_specular_varies_inside_a_magnified_texel") else {
        return;
    };
    for (name, light) in [
        ("point", flank_light()),
        ("spot", spot_light()),
        ("directional", sun()),
    ] {
        let out = render_texels(&ctx, &[light], OWNER_0, TexelCase::new(Style::Smooth));
        assert!(
            varies_inside_some_texel(&out),
            "{name}: smooth evaluates per fragment, so a texel's fragments differ",
        );
    }
}

#[test]
fn pixel_seam_band_lies_between_neighbouring_texels() {
    let Some(ctx) = gpu_or_skip("pixel_seam_band_lies_between_neighbouring_texels") else {
        return;
    };
    let out = render_texels(
        &ctx,
        &[flank_light()],
        OWNER_0,
        TexelCase::new(Style::Pixel),
    );
    let mut checked = 0;
    for (x, y) in pixels() {
        let (kx, ky) = reconstructed_texel_key(x, y, March::OFF);
        let band_along_u = !key_is_centre(kx) && key_is_centre(ky);
        let band_along_v = key_is_centre(kx) && !key_is_centre(ky);
        // The texels either side of the seam the fragment's band straddles.
        let (tx, ty) = (x / PIXELS_PER_TEXEL, y / PIXELS_PER_TEXEL);
        let neighbours = if band_along_u && tx + 1 < LATTICE {
            [
                interior_value(&out, tx, ty),
                interior_value(&out, tx + 1, ty),
            ]
        } else if band_along_v && ty + 1 < LATTICE {
            [
                interior_value(&out, tx, ty),
                interior_value(&out, tx, ty + 1),
            ]
        } else {
            continue;
        };
        let value = out[pixel_index(x, y)][0];
        let (low, high) = (
            neighbours[0][0].min(neighbours[1][0]),
            neighbours[0][0].max(neighbours[1][0]),
        );
        assert!(
            low < value && value < high,
            "seam fragment ({x}, {y}) draws {value}, outside its neighbours' {low}..{high}",
        );
        checked += 1;
    }
    assert!(
        checked >= 2 * 7 * 7 * 7,
        "every interior seam crossing must be checked, got {checked}"
    );
}

#[test]
fn pixel_specular_holds_across_range_and_cone_boundaries() {
    let Some(ctx) = gpu_or_skip("pixel_specular_holds_across_range_and_cone_boundaries") else {
        return;
    };
    for (name, light) in [("range", ranged_light()), ("cone", spot_light())] {
        for march in [March::OFF, March::ON] {
            let case = TexelCase {
                march,
                ..TexelCase::new(Style::Pixel)
            };
            let pixel = render_texels(&ctx, &[light], OWNER_0, case);
            assert_constant_per_texel(&pixel, march, &format!("{name} boundary, {march:?}"));
        }
        // The boundary crosses texel interiors: per fragment it splits them,
        // and the texel's own point decides the whole texel both ways.
        let pixel = render_texels(&ctx, &[light], OWNER_0, TexelCase::new(Style::Pixel));
        let smooth = render_texels(&ctx, &[light], OWNER_0, TexelCase::new(Style::Smooth));
        let lit_past_the_boundary =
            (0..pixel.len()).any(|i| smooth[i][0] == 0.0 && pixel[i][0] > 0.0);
        let dark_inside_it = (0..pixel.len()).any(|i| smooth[i][0] > 0.0 && pixel[i][0] == 0.0);
        assert!(
            lit_past_the_boundary && dark_inside_it,
            "{name}: the boundary must cross texels whose centres fall on either side",
        );
    }
}

#[test]
fn pixel_point_anchors_on_the_fragment_uv_under_a_march() {
    let Some(ctx) = gpu_or_skip("pixel_point_anchors_on_the_fragment_uv_under_a_march") else {
        return;
    };
    let lights = [flank_light()];
    let flat = render_texels(&ctx, &lights, OWNER_0, TexelCase::new(Style::Pixel));
    let marched = render_texels(
        &ctx,
        &lights,
        OWNER_0,
        TexelCase {
            march: March::ON,
            ..TexelCase::new(Style::Pixel)
        },
    );
    assert_constant_per_texel(&marched, March::ON, "marched");
    // Fragments from different texels of the plane land on one texel.
    let gathered = texel_groups(March::ON).values().any(|members| {
        let own = |m: usize| {
            let (x, y) = (m as u32 % TEXEL_TARGET, m as u32 / TEXEL_TARGET);
            reconstructed_texel_key(x, y, March::OFF)
        };
        members.iter().any(|&m| own(m) != own(members[0]))
    });
    assert!(
        gathered,
        "the march must gather fragments from several own texels"
    );

    // The point is the texel the march landed on, placed on the plane from
    // the fragment's own UV and position: the un-marched fragment that
    // reconstructs to that texel draws the same value.
    let mut flat_by_texel = HashMap::new();
    for (x, y) in pixels() {
        flat_by_texel.insert(
            reconstructed_texel_key(x, y, March::OFF),
            flat[pixel_index(x, y)],
        );
    }
    let mut matched = 0;
    for (x, y) in pixels() {
        if let Some(expected) = flat_by_texel.get(&reconstructed_texel_key(x, y, March::ON)) {
            assert_eq!(
                marched[pixel_index(x, y)],
                *expected,
                "fragment ({x}, {y}) under the march"
            );
            matched += 1;
        }
    }
    assert!(
        matched > (TEXEL_TARGET * TEXEL_TARGET / 2) as usize,
        "most marched fragments land on a texel the plane also shows, got {matched}",
    );
    assert_ne!(flat, marched, "the march moves the highlight");
}

#[test]
fn pixel_specular_scales_by_each_fragments_own_visibility() {
    let Some(ctx) = gpu_or_skip("pixel_specular_scales_by_each_fragments_own_visibility") else {
        return;
    };
    let sdf_light = Light {
        sdf: true,
        ..flank_light()
    };
    let sdf_selected = Case {
        sdf_indices: [0, u32::MAX, u32::MAX, u32::MAX],
        sdf_count: 1,
        ..Case::default()
    };
    for (name, light, owners, shadowing) in [
        ("owner mask", flank_light(), OWNER_0, Case::default()),
        ("SDF slice", sdf_light, NO_OWNERS, sdf_selected),
    ] {
        let case = TexelCase {
            shadowing,
            ..TexelCase::new(Style::Pixel)
        };
        let unshadowed = render_texels(&ctx, &[light], owners, case);
        let shadowed = render_texels(
            &ctx,
            &[light],
            owners,
            TexelCase {
                per_fragment_visibility: true,
                ..case
            },
        );
        assert_constant_per_texel(&unshadowed, March::OFF, name);
        for (x, y) in pixels() {
            let i = pixel_index(x, y);
            let visibility = fragment_visibility(x, y);
            assert_eq!(
                shadowed[i],
                unshadowed[i].map(|c| c * visibility),
                "{name}: fragment ({x}, {y}) at visibility {visibility}"
            );
        }
        assert!(
            varies_inside_some_texel(&shadowed),
            "{name}: visibility stays per fragment"
        );
    }
}

#[test]
fn degenerate_jacobian_specular_matches_the_per_pixel_evaluation() {
    let Some(ctx) = gpu_or_skip("degenerate_jacobian_specular_matches_the_per_pixel_evaluation")
    else {
        return;
    };
    let lights = [flank_light(), spot_light(), sun()];
    let owners = [0, 1, 2, EMPTY];
    for geometry in [Geometry::EdgeOn, Geometry::CollapsedUv] {
        for march in [March::OFF, March::ON] {
            let case = |style| TexelCase {
                geometry,
                march,
                ..TexelCase::new(style)
            };
            let pixel = render_texels(&ctx, &lights, owners, case(Style::Pixel));
            let smooth = render_texels(&ctx, &lights, owners, case(Style::Smooth));
            for (i, (p, s)) in pixel.iter().zip(&smooth).enumerate() {
                assert!(
                    p.iter().all(|c| c.is_finite()),
                    "{geometry:?}: fragment {i} is {p:?}"
                );
                assert_eq!(
                    bits(*p),
                    bits(*s),
                    "{geometry:?}, {march:?}: fragment {i} must fall back to per pixel"
                );
            }
            assert!(
                pixel.iter().any(|p| p[0] > 0.0),
                "{geometry:?}: the lights must reach the floor"
            );
        }
    }
}

#[test]
fn smooth_specular_matches_the_frozen_pre_change_function() {
    let Some(ctx) = gpu_or_skip("smooth_specular_matches_the_frozen_pre_change_function") else {
        return;
    };
    let sdf_light = Light {
        sdf: true,
        ..flank_light()
    };
    let all_lights = [
        flank_light(),
        spot_light(),
        sun(),
        ranged_light(),
        sdf_light,
    ];
    let smooth = TexelCase::new(Style::Smooth);
    let sdf_selected = Case {
        sdf_indices: [4, u32::MAX, u32::MAX, u32::MAX],
        sdf_count: 1,
        sdf_factor: [0.5, 1.0, 1.0, 1.0],
        ..Case::default()
    };
    let cases = [
        ("owners", smooth),
        (
            "owner masks and SDF selection",
            TexelCase {
                shadowing: Case {
                    mask: [0.9, 0.6, 0.3, 0.15],
                    ..sdf_selected
                },
                ..smooth
            },
        ),
        (
            "per-fragment visibility",
            TexelCase {
                shadowing: sdf_selected,
                per_fragment_visibility: true,
                ..smooth
            },
        ),
        (
            "force-one",
            TexelCase {
                shadowing: Case {
                    mask: [0.0; 4],
                    force_one: true,
                    ..Case::default()
                },
                ..smooth
            },
        ),
        (
            "unreadable owners",
            TexelCase {
                shadowing: Case {
                    owners_readable: false,
                    sdf_force_lit: true,
                    ..sdf_selected
                },
                ..smooth
            },
        ),
        (
            "march",
            TexelCase {
                march: March::ON,
                ..smooth
            },
        ),
        (
            "edge-on",
            TexelCase {
                geometry: Geometry::EdgeOn,
                ..smooth
            },
        ),
        (
            "collapsed UV",
            TexelCase {
                geometry: Geometry::CollapsedUv,
                ..smooth
            },
        ),
        (
            "sharp exponent",
            TexelCase {
                exponent: 96.0,
                ..smooth
            },
        ),
    ];
    for (name, case) in cases {
        let current = render_texels(&ctx, &all_lights, [0, 1, 2, 3], case);
        let frozen = render_texels(
            &ctx,
            &all_lights,
            [0, 1, 2, 3],
            TexelCase {
                frozen: true,
                ..case
            },
        );
        for (i, (c, f)) in current.iter().zip(&frozen).enumerate() {
            assert_eq!(
                bits(*c),
                bits(*f),
                "{name}: fragment {i} is {c:?}, the 1c7fc9e3a function drew {f:?}"
            );
        }
        assert!(
            current.iter().any(|p| p[0] > 0.0),
            "{name}: the lights must reach the floor"
        );
    }
}

/// `src` with every `//` comment removed, then every whitespace character.
fn squashed_code(src: &str) -> String {
    src.lines()
        .map(|line| line.split("//").next().unwrap_or(""))
        .collect::<String>()
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect()
}

#[test]
fn forward_fs_main_builds_the_specular_point_through_the_shared_builder() {
    let fs = squashed_code(wgsl_function(FORWARD_WGSL, "fs_main"));
    assert!(
        fs.contains("letddx=dpdx(in.uv);letddy=dpdy(in.uv);")
            && fs.contains(
                "letddx_world=dpdx(in.world_position);letddy_world=dpdy(in.world_position);"
            ),
        "the builder's derivatives are fs_main's hoisted ones",
    );
    assert_eq!(fs.matches("specular_texel_point(").count(), 1);
    assert!(
        fs.contains(
            "letspec_point=specular_texel_point(specular_style_smooth(),\
             SpecularPoint(in.world_position,V),in.uv,shade_uv,in.world_position,\
             uniforms.camera_position,vec2<f32>(textureDimensions(base_texture,0)),\
             ddx,ddy,ddx_world,ddy_world,);"
        ),
        "fs_main feeds the builder its own UV, the marched UV, its plane position, \
         the albedo lattice and the hoisted derivatives",
    );
    assert!(
        fs.contains("world_static_specular(spec_point.position,N_shade,spec_point.view,"),
        "world static specular takes L and V from the built point",
    );
    // One shared definition: forward calls the material snippet's builder and
    // reconstruction, and never redeclares either.
    let composed = super::pipeline_layout::SHADER_SOURCE;
    for name in [
        "specular_texel_point",
        "post_retro_texel_uv",
        "struct SpecularPoint",
    ] {
        let declaration = if name.starts_with("struct") {
            format!("{name} {{")
        } else {
            format!("fn {name}(")
        };
        assert!(
            !FORWARD_WGSL.contains(&declaration),
            "forward must not redeclare {name}"
        );
        assert!(
            MATERIAL_SHADING_WGSL.contains(&declaration),
            "{name} lives in material_shading.wgsl"
        );
        assert_eq!(
            composed.matches(&declaration).count(),
            1,
            "{name} is composed once"
        );
    }
    assert!(
        squashed_code(wgsl_function(FORWARD_WGSL, "sample_post_retro"))
            .contains("post_retro_texel_uv(uv,dims,ddx,ddy)"),
        "forward's texture sample reconstructs through the shared function",
    );
}

#[test]
fn spec_shadowmask_flag_bits_match_the_cpu_constants() {
    use postretro_render_cpu::frame_uniforms::{
        SPEC_SHADOWMASK_FLAG_FORCE_ONE, SPEC_SHADOWMASK_FLAG_MASK_PRESENT,
        SPEC_SHADOWMASK_FLAG_STYLE_SMOOTH,
    };
    for (name, cpu) in [
        (
            "SPEC_SHADOWMASK_FLAG_FORCE_ONE",
            SPEC_SHADOWMASK_FLAG_FORCE_ONE,
        ),
        (
            "SPEC_SHADOWMASK_FLAG_MASK_PRESENT",
            SPEC_SHADOWMASK_FLAG_MASK_PRESENT,
        ),
        (
            "SPEC_SHADOWMASK_FLAG_STYLE_SMOOTH",
            SPEC_SHADOWMASK_FLAG_STYLE_SMOOTH,
        ),
    ] {
        let line = wgsl_const_line(FORWARD_WGSL, name);
        let value = line
            .split('=')
            .nth(1)
            .and_then(|rhs| rhs.trim().strip_suffix("u;"))
            .and_then(|digits| digits.parse::<u32>().ok())
            .unwrap_or_else(|| panic!("`{line}` must be a decimal u32 literal"));
        assert_eq!(value, cpu, "{name}");
    }
}
