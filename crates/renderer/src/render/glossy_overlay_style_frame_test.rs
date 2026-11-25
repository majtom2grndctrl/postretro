//! Adapter-backed gates for where the glossy overlay reflects from: the
//! reflection style (pixel reflects once per albedo texel from its centre,
//! smooth per fragment, each from the first frame after a switch) and Surface
//! Depth (the overlay marches each face as forward does, reads forward's
//! texel, mirrors a carved side about the flat face normal, and stops
//! marching on the frame the switch turns it off).
//! See: context/lib/rendering_pipeline.md §7.3a (Glossy world faces), §7.3 (Surface Depth)

use std::collections::HashMap;

use glam::{UVec2, Vec2, Vec3};
use postretro_render_cpu::fluid_surface::reflection_style::FluidReflectionStyle;
use postretro_render_cpu::surface_depth::SurfaceDepthQuality;
use postretro_render_data::material::Material;

use super::Renderer;
use super::fluid_reflection_frame_test_fixtures::{View, centre};
use super::glossy_floor_frame_test_fixtures::read_floor_layer;
use super::glossy_overlay::GlossyOverlayForm;
use super::glossy_overlay_frame_test_fixtures::{
    GlossyFrame, half_step, renderer, with_probe_term_alone, with_variant,
};
use super::glossy_overlay_room_frame_test_fixtures::{
    Face, Room, RoomMaterial, RoomReference, face_pixels, ray_through, room_frame, room_view,
    set_room_material, with_room_variant,
};

const MARGIN: f32 = 0.08;
/// The style gate's texture: 8 texels across every face.
const TEXELS: u32 = 8;

fn bound(value: f32) -> f32 {
    1.0 / 255.0 + half_step(value)
}

/// A probe colour that turns with every direction component, so two points
/// of one face never read one colour.
fn gradient(direction: Vec3, _mip: u32) -> Vec3 {
    let d = direction.normalize();
    Vec3::new(0.5 + 0.375 * d.x, 0.5 + 0.375 * d.y, 0.5 + 0.375 * d.z)
}

/// An 8² white mirror: every texel reflects at W = 1.
const MIRROR_8: RoomMaterial = RoomMaterial {
    size: TEXELS,
    ..RoomMaterial::WHITE_MIRROR
};

/// Production and the zero-adding variant at one pose.
fn frames(renderer: &mut Renderer, view: View) -> (GlossyFrame, GlossyFrame) {
    // The probe term alone: the floor layer's style proof is
    // `glossy_floor_layer_frame_test`'s.
    let production = with_probe_term_alone(renderer, |renderer| room_frame(renderer, view, 0.0));
    let forward = with_variant(renderer, "fs_test_zero", |renderer| {
        room_frame(renderer, view, 0.0)
    });
    (production, forward)
}

/// The albedo texel under the pixel at `point` on `face`, and whether the
/// pixel lies clear of `sample_post_retro`'s seam band, where the texel UV
/// ramps toward the next texel's: further from every texel edge than one
/// pixel's UV footprint (the band is half that, the slack covers coarse
/// derivatives).
fn texel_of(view: View, face: Face, pixel: UVec2, point: Vec3) -> (UVec2, bool) {
    let uv = face.uv(point);
    let step = |offset: Vec2| {
        let (origin, dir) = ray_through(view, centre(pixel) + offset);
        let t = (point - origin).dot(face.normal()) / dir.dot(face.normal());
        face.uv(origin + dir * t)
    };
    let footprint = ((step(Vec2::X) - uv).abs() + (step(Vec2::Y) - uv).abs()) * TEXELS as f32;
    let at = uv * TEXELS as f32;
    let fraction = at - at.floor();
    let clear = fraction.cmpgt(footprint).all() && (1.0 - fraction).cmpgt(footprint).all();
    (at.floor().as_uvec2(), clear)
}

/// Per face and texel, the overlay's addition at each pixel well inside it.
fn additions_by_texel(
    view: View,
    production: &GlossyFrame,
    forward: &GlossyFrame,
) -> HashMap<(usize, UVec2), Vec<Vec3>> {
    let mut texels: HashMap<(usize, UVec2), Vec<Vec3>> = HashMap::new();
    for face in Face::ALL {
        for (pixel, point) in face_pixels(view, face, MARGIN) {
            let (texel, inside) = texel_of(view, face, pixel, point);
            if inside {
                texels
                    .entry((face.index(), texel))
                    .or_default()
                    .push(production.colour(pixel) - forward.colour(pixel));
            }
        }
    }
    texels
}

fn spread(values: &[Vec3]) -> f32 {
    let (lo, hi) = values.iter().fold(
        (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)),
        |(lo, hi), v| (lo.min(*v), hi.max(*v)),
    );
    (hi - lo).max_element()
}

// Acceptance row 35 (pin P9): in pixel style the overlay's term is one value
// per albedo texel, the reference's at the texel's centre point; in smooth it
// varies across the texel, the reference's at each fragment. Each holds on
// the first frame after a switch, both ways, on the floor, wall and ceiling.
#[test]
fn pixel_style_reflects_once_per_texel_and_smooth_per_fragment_from_the_switch_frame() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    let room = Room::new([gradient, gradient, gradient]);
    room.install(&mut renderer, &MIRROR_8);
    let view = room_view();
    let reference = RoomReference::new(&room, view, &[], 0.0, MIRROR_8);
    for style in [
        FluidReflectionStyle::Smooth,
        FluidReflectionStyle::Pixel,
        FluidReflectionStyle::Smooth,
    ] {
        renderer.set_fluid_reflection_style(style);
        let (production, forward) = frames(&mut renderer, view);
        let pixel = style == FluidReflectionStyle::Pixel;
        for face in Face::ALL {
            let mut checked = 0;
            for (screen, point) in face_pixels(view, face, MARGIN) {
                let (texel, inside) = texel_of(view, face, screen, point);
                if !inside {
                    continue;
                }
                let from = if pixel {
                    face.at_uv((texel.as_vec2() + 0.5) / TEXELS as f32)
                } else {
                    point
                };
                let want = forward.colour(screen)
                    + reference
                        .addition(face, from, forward.static_diffuse(screen))
                        .added;
                let got = production.colour(screen);
                for c in 0..3 {
                    assert!(
                        (got[c] - want[c]).abs() <= bound(want[c]),
                        "{style:?}: {face:?} at {screen}: channel {c} is {} against {}",
                        got[c],
                        want[c]
                    );
                }
                checked += 1;
            }
            assert!(checked > 100, "{style:?}: {face:?} checked {checked}");
        }
        let texels = additions_by_texel(view, &production, &forward);
        let several: Vec<_> = texels.values().filter(|values| values.len() >= 4).collect();
        assert!(several.len() > 20, "{} texels span 4 pixels", several.len());
        let varying = several
            .iter()
            .filter(|values| spread(values) > 0.01)
            .count();
        if pixel {
            for values in &several {
                let slack = 2.0 * bound(values[0].max_element());
                assert!(
                    spread(values) <= slack,
                    "a pixel texel varies by {}",
                    spread(values)
                );
            }
        } else {
            assert!(
                varying * 2 > several.len(),
                "smooth varies in {varying} of {} texels",
                several.len()
            );
        }
    }
}

/// The overlay variants Surface Depth's gate swaps in.
const MARCH_VARIANTS: &str = r#"
// The albedo of the texel the overlay's march picks, added over forward.
@fragment
fn fs_test_march_albedo(in: GlossyVertexOutput) -> @location(0) vec4<f32> {
    let d = glossy_overlay_derivatives(in);
    let depth = glossy_overlay_march(in, d);
    return vec4<f32>(sample_post_retro(base_texture, aniso_sampler, depth.uv, d.ddx, d.ddy).rgb,
        0.0);
}

// 1 where the overlay's march stops on a carved side, else 0.
@fragment
fn fs_test_march_side(in: GlossyVertexOutput) -> @location(0) vec4<f32> {
    let d = glossy_overlay_derivatives(in);
    let depth = glossy_overlay_march(in, d);
    return vec4<f32>(select(0.0, 1.0, !depth.hit_top), 0.0, 0.0, 0.0);
}

// On a carved side, in each style: red 1; green 1 where the overlay's mirror
// is exactly the view mirrored about the flat face normal; blue 1 where that
// mirror, before the hemisphere floor, lies in front of the face. 0 off a side.
fn test_side_mirror(in: GlossyVertexOutput, pixel: bool) -> vec4<f32> {
    let d = glossy_overlay_derivatives(in);
    let depth = glossy_overlay_march(in, d);
    let surface = glossy_overlay_surface(in, d, depth, pixel, true);
    let side = !depth.hit_top;
    let mirror = fluid_reflection_mirror(surface.view, surface.normal);
    let flat_mirror = fluid_reflection_mirror(surface.view, surface.outward);
    return vec4<f32>(select(0.0, 1.0, side),
        select(0.0, 1.0, side && all(mirror == flat_mirror)),
        select(0.0, 1.0, side && dot(mirror, surface.outward) > 0.0), 0.0);
}

@fragment
fn fs_test_side_mirror_smooth(in: GlossyVertexOutput) -> @location(0) vec4<f32> {
    return test_side_mirror(in, false);
}

@fragment
fn fs_test_side_mirror_pixel(in: GlossyVertexOutput) -> @location(0) vec4<f32> {
    return test_side_mirror(in, true);
}
"#;

/// A 16² stone pattern for Surface Depth: raised blocks of two hues, sunk
/// grout between them every fourth texel.
const CARVED: RoomMaterial = RoomMaterial {
    size: 16,
    albedo: |x, y| {
        if x % 4 == 0 || y % 4 == 0 {
            [40, 200, 40, 255]
        } else if (x / 4 + y / 4) % 2 == 0 {
            [230, 60, 30, 255]
        } else {
            [30, 80, 230, 255]
        }
    },
    slot: |x, y| {
        let height = if x % 4 == 0 || y % 4 == 0 { 0 } else { 255 };
        [255, height, 255, 255]
    },
    glow: None,
    material: Material::Concrete,
};

/// The unit hue of a colour.
fn hue(colour: Vec3) -> Vec3 {
    colour / colour.element_sum().max(1.0e-6)
}

/// What the overlay's variant drew at each pixel of each face: the variant's
/// frame minus forward's.
fn variant_frame(renderer: &mut Renderer, view: View, entry: &str) -> (GlossyFrame, GlossyFrame) {
    let drawn = with_room_variant(renderer, MARCH_VARIANTS, entry, |renderer| {
        room_frame(renderer, view, 0.0)
    });
    let forward = with_variant(renderer, "fs_test_zero", |renderer| {
        room_frame(renderer, view, 0.0)
    });
    (drawn, forward)
}

/// Per face: the pixels whose overlay albedo matches forward's D_static in
/// hue, and those checked: every pixel forward lights (a grout floor deep in
/// its carve can take no static light, and black has no hue); and the
/// overlay's albedo at every pixel.
fn albedo_matches(
    view: View,
    drawn: &GlossyFrame,
    forward: &GlossyFrame,
) -> (Vec<(usize, usize)>, HashMap<UVec2, Vec3>) {
    let mut albedos = HashMap::new();
    let counts = Face::ALL
        .iter()
        .map(|&face| {
            let pixels = face_pixels(view, face, MARGIN);
            let mut lit = 0;
            let matched = pixels
                .iter()
                .filter(|(pixel, _)| {
                    let albedo = drawn.colour(*pixel) - forward.colour(*pixel);
                    albedos.insert(*pixel, albedo);
                    let static_diffuse = forward.static_diffuse(*pixel);
                    if static_diffuse.max_element() < 1.0e-3 {
                        return false;
                    }
                    lit += 1;
                    (hue(albedo) - hue(static_diffuse)).abs().max_element() < 0.02
                })
                .count();
            assert!(
                lit * 10 >= pixels.len() * 8,
                "{face:?}: forward lights {lit} of {}",
                pixels.len()
            );
            (matched, lit)
        })
        .collect();
    (counts, albedos)
}

// Acceptance row 67 (pin P11): with Surface Depth on, the overlay marches
// every face as forward does: the texel it reads is forward's (its albedo
// matches forward's D_static in hue at every pixel of the floor, wall and
// ceiling), the march moves texels against the unmarched read, and the march
// stops on carved sides. Off, it reads the fragment's own texel and no side,
// from the frame the switch turns it off; on again, marched from that frame.
#[test]
fn surface_depth_marches_the_overlay_as_forward_and_a_toggle_takes_the_same_frame() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    let room = Room::new([gradient, gradient, gradient]);
    room.install(&mut renderer, &CARVED);
    renderer.set_fluid_reflection_style(FluidReflectionStyle::Smooth);
    let view = room_view();
    let mut marched = HashMap::new();
    for quality in [
        SurfaceDepthQuality::On,
        SurfaceDepthQuality::Off,
        SurfaceDepthQuality::On,
    ] {
        renderer.set_surface_depth_quality(quality);
        let (drawn, forward) = variant_frame(&mut renderer, view, "fs_test_march_albedo");
        let (counts, albedos) = albedo_matches(view, &drawn, &forward);
        for (face, (matched, checked)) in Face::ALL.iter().zip(&counts) {
            eprintln!("[row 67] {quality:?}: {face:?} {matched} of {checked} match forward");
            assert!(
                matched == checked && *checked > 150,
                "{quality:?}: {face:?}: the overlay's texel is forward's at {matched} of {checked}"
            );
        }
        let (sides, sides_forward) = variant_frame(&mut renderer, view, "fs_test_march_side");
        let side_count = Face::ALL
            .iter()
            .flat_map(|&face| face_pixels(view, face, MARGIN))
            .filter(|(pixel, _)| (sides.colour(*pixel) - sides_forward.colour(*pixel)).x > 0.5)
            .count();
        match quality {
            SurfaceDepthQuality::On => {
                assert!(
                    side_count > 20,
                    "the overlay's march stops on carved sides: {side_count}"
                );
                if marched.is_empty() {
                    marched = albedos;
                } else {
                    assert_eq!(marched, albedos, "on again marches as at first");
                }
            }
            SurfaceDepthQuality::Off => {
                assert_eq!(side_count, 0, "off, no side reaches the overlay");
                // Per face: the march moves texels against the unmarched read.
                for face in Face::ALL {
                    let moved = face_pixels(view, face, MARGIN)
                        .iter()
                        .filter(|(pixel, _)| {
                            (albedos[pixel] - marched[pixel]).abs().max_element() > 0.05
                        })
                        .count();
                    eprintln!("[row 67] {face:?}: the march moves {moved} pixels' texels");
                    assert!(
                        moved > 20,
                        "{face:?}: the march moves {moved} texels against Off"
                    );
                }
            }
        }
    }
}

// A carved side mirrors the view about the flat face normal, in both styles
// and on the floor, wall and ceiling: its mirror is the flat face's exactly
// and lies in front of the face, so the hemisphere floor never flattens it
// onto the horizon. A side's own normal lies in the face's plane and would
// mirror the view behind the face. The floor layer marches from the same
// surface (`glossy_overlay_surface`), so both layers reflect a side alike.
#[test]
fn a_carved_side_mirrors_about_the_flat_face_normal_in_front_of_the_face() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    let room = Room::new([gradient, gradient, gradient]);
    room.install(&mut renderer, &CARVED);
    renderer.set_surface_depth_quality(SurfaceDepthQuality::On);
    let view = room_view();
    for entry in ["fs_test_side_mirror_smooth", "fs_test_side_mirror_pixel"] {
        let (drawn, forward) = variant_frame(&mut renderer, view, entry);
        for face in Face::ALL {
            let mut sides = 0;
            for (pixel, _) in face_pixels(view, face, MARGIN) {
                let added = drawn.colour(pixel) - forward.colour(pixel);
                if added.x <= 0.5 {
                    continue;
                }
                sides += 1;
                assert!(
                    added.y > 0.5,
                    "{entry}: {face:?} at {pixel}: a side mirrors about its own normal"
                );
                assert!(
                    added.z > 0.5,
                    "{entry}: {face:?} at {pixel}: a side's mirror points behind the face"
                );
            }
            eprintln!("[side mirror] {entry}: {face:?} {sides} side pixels");
            assert!(sides > 5, "{entry}: {face:?}: {sides} side pixels");
        }
    }
}

// Acceptance rows 35 (both layers) and 45 (GPU half). The room's floor
// reflects its walls through the floor layer; what the floor layer adds over
// the probe term alone, F·colour·c·(X − R), is one value per albedo texel in
// pixel style (the texel's weight, its point's probe term and the floor
// layer's texel at its centre's projection) and varies within a texel in
// smooth, on the first frame after a switch, both ways. The bound wall and
// ceiling take nothing from it at any style: their pixels are the probe
// term's bit for bit, and the floor layer is clear wherever they cover it.
#[test]
fn the_floor_layer_reaches_the_floor_alone_once_per_texel_in_pixel_style() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    let room = Room::new([gradient, gradient, gradient]);
    room.install(&mut renderer, &MIRROR_8);
    let view = room_view();
    let scene = UVec2::splat(super::fluid_reflection_frame_test_fixtures::FRAME);
    for style in [
        FluidReflectionStyle::Smooth,
        FluidReflectionStyle::Pixel,
        FluidReflectionStyle::Smooth,
    ] {
        renderer.set_fluid_reflection_style(style);
        let layered = room_frame(&mut renderer, view, 0.0);
        let floor = read_floor_layer(&renderer).expect("the room's tier runs the floor layer");
        let alone =
            with_probe_term_alone(&mut renderer, |renderer| room_frame(renderer, view, 0.0));
        for face in [Face::Wall, Face::Ceiling] {
            let covered = face_pixels(view, face, MARGIN);
            assert!(covered.len() > 100, "{face:?} in view");
            let on_face: std::collections::HashSet<UVec2> =
                covered.iter().map(|(pixel, _)| *pixel).collect();
            for &(pixel, _) in &covered {
                assert_eq!(
                    layered.colour(pixel),
                    alone.colour(pixel),
                    "{style:?}: {face:?} at {pixel} takes the floor layer"
                );
            }
            // Every floor-layer texel whose scene pixels the face covers.
            for y in 0..floor.extent.y {
                for x in 0..floor.extent.x {
                    let block = scene / floor.extent;
                    let first = UVec2::new(x, y) * block;
                    let all = (0..block.y).all(|dy| {
                        (0..block.x).all(|dx| on_face.contains(&(first + UVec2::new(dx, dy))))
                    });
                    if all {
                        assert_eq!(
                            floor.at(UVec2::new(x, y)),
                            glam::Vec4::ZERO,
                            "{style:?}: the floor layer reaches {face:?}"
                        );
                    }
                }
            }
        }
        let texels: HashMap<_, _> = additions_by_texel(view, &layered, &alone)
            .into_iter()
            .filter(|((face, _), values)| *face == Face::Floor.index() && values.len() >= 4)
            .collect();
        let taking = texels
            .values()
            .filter(|values| values.iter().any(|value| value.abs().max_element() > 0.01))
            .count();
        assert!(
            taking > 8,
            "{style:?}: {taking} floor texels take the floor layer"
        );
        let varying = texels
            .values()
            .filter(|values| spread(values) > 0.01)
            .count();
        if style == FluidReflectionStyle::Pixel {
            for values in texels.values() {
                let slack = 2.0 * bound(values[0].max_element());
                assert!(
                    spread(values) <= slack,
                    "a pixel texel's floor layer varies by {}",
                    spread(values)
                );
            }
        } else {
            assert!(
                varying * 4 > taking,
                "{style:?}: {varying} of {taking} texels vary"
            );
        }
    }
}

/// One frame with the overlay's production fragment `entry` in every
/// pipeline's place.
fn entry_frame(renderer: &mut Renderer, view: View, entry: &str) -> GlossyFrame {
    with_room_variant(renderer, "", entry, |renderer| {
        room_frame(renderer, view, 0.0)
    })
}

// The overlay's forms change no frame. A material that never marches and
// binds the neutral normal placeholder draws through the plain pipelines,
// and its frame is the full (marching, normal-sampling) pipelines' and the
// march-free ones' bit for bit, in both styles. A carved material draws
// through the full pipeline (the march-free one draws it otherwise at On),
// and at Off the march-free and plain pipelines draw its frame bit for bit.
// At High the room's floor runs the floor layer, so each frame is drawn
// through the floor layer's forms, and they hold it too.
#[test]
fn every_overlay_form_draws_the_full_pipelines_frame_for_the_materials_it_takes() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    let room = Room::new([gradient, gradient, gradient]);
    room.install(&mut renderer, &MIRROR_8);
    assert_eq!(
        renderer.level().glossy_overlay_form(0),
        GlossyOverlayForm::Plain
    );
    let view = room_view();
    let forward = with_variant(&mut renderer, "fs_test_zero", |renderer| {
        room_frame(renderer, view, 0.0)
    });
    for (style, entries) in [
        (
            FluidReflectionStyle::Pixel,
            ["fs_pixel_floor", "fs_pixel_flat_floor"],
        ),
        (
            FluidReflectionStyle::Smooth,
            ["fs_smooth_floor", "fs_smooth_flat_floor"],
        ),
    ] {
        renderer.set_fluid_reflection_style(style);
        let production = room_frame(&mut renderer, view, 0.0);
        assert_ne!(
            production.scene.colour, forward.scene.colour,
            "{style:?}: the overlay adds its term"
        );
        for entry in entries {
            let full = entry_frame(&mut renderer, view, entry);
            assert!(
                production.scene.colour == full.scene.colour,
                "{style:?}: the plain frame is {entry}'s"
            );
        }
    }

    set_room_material(&mut renderer, &CARVED);
    assert_eq!(
        renderer.level().glossy_overlay_form(0),
        GlossyOverlayForm::March
    );
    renderer.set_fluid_reflection_style(FluidReflectionStyle::Smooth);
    for quality in [SurfaceDepthQuality::On, SurfaceDepthQuality::Off] {
        renderer.set_surface_depth_quality(quality);
        let production = room_frame(&mut renderer, view, 0.0);
        for entry in ["fs_smooth_flat_floor", "fs_smooth_plain_floor"] {
            let flat = entry_frame(&mut renderer, view, entry);
            assert_eq!(
                production.scene.colour == flat.scene.colour,
                quality == SurfaceDepthQuality::Off,
                "{quality:?}: the carved material marches through its own pipeline ({entry})"
            );
        }
    }
}
