// Adapter-backed guard on pixel style's floor layer at a grazing view: from
// a camera a third of a metre above a coarse-texel glossy floor, the floor
// layer's target takes as much confidence in pixel style, where each albedo
// texel marches once from its centre, as in smooth, and the overlay's
// per-texel read adds as much of it over the probe term.
// See: context/lib/rendering_pipeline.md §7.3a (Glossy world faces, Floors)

use glam::{UVec2, Vec3};
use postretro_render_cpu::fluid_surface::reflection_quality::FluidReflectionQuality;
use postretro_render_cpu::fluid_surface::reflection_style::FluidReflectionStyle;

use super::Renderer;
use super::fluid_reflection_frame_test_fixtures::{FRAME, View, view_from};
use super::glossy_floor_frame_test_fixtures::read_floor_layer;
use super::glossy_overlay_frame_test_fixtures::{renderer, with_probe_term_alone};
use super::glossy_overlay_room_frame_test_fixtures::{Room, RoomMaterial, room_frame};

/// The share of smooth style's confidence and addition pixel style keeps at
/// the least. Each texel taking one ray moves hits between texels, never
/// drops them wholesale.
const KEPT: f32 = 0.8;

/// A probe colour that turns with direction.
fn gradient(direction: Vec3, _mip: u32) -> Vec3 {
    let d = direction.normalize();
    Vec3::new(0.5 + 0.375 * d.x, 0.5 + 0.375 * d.y, 0.5 + 0.375 * d.z)
}

/// The grazing view: 0.36 m above the floor at the room's near end, looking
/// along it 3° down, so the north wall's foot stands in the floor's
/// reflection.
fn grazing_view() -> View {
    let eye = Vec3::new(0.0, 0.3556, 1.2);
    let pitch = (-3.0_f32).to_radians();
    view_from(eye, eye + Vec3::new(0.0, pitch.sin(), pitch.cos()))
}

/// The floor layer's summed confidence, and its summed addition over the
/// probe term alone, at one style.
fn floor_layer_at(renderer: &mut Renderer, style: FluidReflectionStyle) -> (f32, f32) {
    let view = grazing_view();
    renderer.set_fluid_reflection_style(style);
    let layered = room_frame(renderer, view, 0.0);
    let confidence = read_floor_layer(renderer)
        .expect("the floor tier runs the floor layer")
        .texels
        .iter()
        .map(|texel| texel.w)
        .sum();
    let alone = with_probe_term_alone(renderer, |renderer| room_frame(renderer, view, 0.0));
    let added = (0..FRAME * FRAME)
        .map(|index| {
            let pixel = UVec2::new(index % FRAME, index / FRAME);
            (layered.colour(pixel) - alone.colour(pixel))
                .abs()
                .element_sum()
        })
        .sum();
    (confidence, added)
}

// Floor texels of 0.75 m by 1 m and of 19 cm by 25 cm, at High and Medium:
// pixel style keeps at least `KEPT` of smooth style's floor-layer confidence
// and of what the floor layer adds over the probe term.
#[test]
fn pixel_style_keeps_the_floor_layer_at_a_grazing_view() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    let room = Room::new([gradient; 3]);
    for size in [8, 32] {
        room.install(
            &mut renderer,
            &RoomMaterial {
                size,
                ..RoomMaterial::WHITE_MIRROR
            },
        );
        for quality in [FluidReflectionQuality::High, FluidReflectionQuality::Medium] {
            renderer.set_reflection_quality(quality);
            let (smooth_c, smooth_added) =
                floor_layer_at(&mut renderer, FluidReflectionStyle::Smooth);
            let (pixel_c, pixel_added) = floor_layer_at(&mut renderer, FluidReflectionStyle::Pixel);
            assert!(
                smooth_c > 20.0 && smooth_added > 100.0,
                "{size}² {quality:?}: smooth's floor layer reflects the wall ({smooth_c}, {smooth_added})"
            );
            assert!(
                pixel_c >= KEPT * smooth_c && pixel_added >= KEPT * smooth_added,
                "{size}² {quality:?}: pixel keeps confidence {pixel_c} of {smooth_c} \
                 and addition {pixel_added} of {smooth_added}"
            );
        }
    }
}
