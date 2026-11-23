//! Adapter-backed gates for where the glossy overlay reflects from: the
//! reflection style (pixel reflects once per albedo texel from its centre,
//! smooth per fragment, each from the first frame after a switch) and Surface
//! Depth (the overlay marches each face as forward does, reads forward's
//! texel and side normals, and stops marching on the frame the switch turns
//! it off).
//! See: context/lib/rendering_pipeline.md §7.3a (Glossy world faces), §7.3 (Surface Depth)

use std::collections::HashMap;

use glam::{UVec2, Vec2, Vec3};
use postretro_render_cpu::fluid_surface::reflection_style::FluidReflectionStyle;
use postretro_render_cpu::surface_depth::SurfaceDepthQuality;
use postretro_render_data::material::Material;

use super::Renderer;
use super::fluid_reflection_frame_test_fixtures::{View, centre};
use super::glossy_overlay_frame_test_fixtures::{GlossyFrame, half_step, renderer, with_variant};
use super::glossy_overlay_room_frame_test_fixtures::{
    Face, Room, RoomMaterial, RoomReference, face_pixels, ray_through, room_frame, room_view,
    with_room_variant,
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
    let production = room_frame(renderer, view, 0.0);
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

// 1 where the overlay's shading normal leans off its face by more than 45°
// (a carved side), else 0.
@fragment
fn fs_test_march_side(in: GlossyVertexOutput) -> @location(0) vec4<f32> {
    let d = glossy_overlay_derivatives(in);
    let depth = glossy_overlay_march(in, d);
    let surface = glossy_overlay_surface(in, d, depth, false);
    return vec4<f32>(select(0.0, 1.0, dot(surface.normal, surface.outward) < 0.7071), 0.0, 0.0,
        0.0);
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
// ceiling), the march moves texels against the unmarched read, and carved
// sides reach the overlay's shading normal. Off, it reads the fragment's own
// texel and no side, from the frame the switch turns it off; on again,
// marched from that frame.
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
                    "carved sides reach the overlay: {side_count}"
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
