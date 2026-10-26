//! Planar proof fixtures: two water planes, off-screen and standing objects, layer readback.
//! See: context/lib/rendering_pipeline.md §7.3a

use crate::render::Renderer;
use crate::render::fluid_reflection_frame_test_fixtures::{
    self as fixture, FRAME, SceneReadback, TargetReadback, View, WorldQuad,
};
use crate::render::fluid_surface_frame_test::look_draw;
use glam::{UVec2, Vec2, Vec3, Vec4};
use postretro_level_format::environment_volumes::EnvironmentVolumeRecord;
use postretro_render_cpu::fluid_surface::reflection_quality::FluidReflectionQuality;
use postretro_render_cpu::fluid_surface::reflection_style::FluidReflectionStyle;
use postretro_render_cpu::planar_reflection::{MirrorCamera, PlanarPreset, planar_preset};
use postretro_render_cpu::sh_compose::f16_bits_to_f32;
use postretro_visibility::VisibleCells;

/// The large plane at height 0 and the second plane, 0.3 m up beside it.
/// Neither covers the other on screen from [`fixture::base_view`]: a sight
/// line meeting the second plane at x ≥ 1 meets height 0 at x ≥ 1.18.
pub(super) const LARGE_X: [f32; 2] = [-6.0, 0.6];
pub(super) const SECOND_X: [f32; 2] = [1.0, 6.0];
pub(super) const SECOND_H: f32 = 0.3;

/// A magenta object above the large plane, over the top of the main view
/// (its lowest edge 16° above the eye, the frustum's top 13°), whose
/// reflection lands on the large plane near the bottom of the screen.
pub(super) fn ceiling_object() -> WorldQuad {
    fixture::upright([-2.0, -0.6], [4.2, 6.0], 7.5, fixture::MAGENTA_UV)
}

/// The ceiling object's centre, reflected in the large plane: where its
/// mirrored pixel lies.
pub(super) const CEILING_REFLECTED: Vec3 = Vec3::new(-1.3, -5.1, 7.5);

/// A magenta object standing in the second plane, on screen.
pub(super) fn standing_object() -> WorldQuad {
    fixture::upright([1.6, 2.8], [SECOND_H, 1.3], 5.0, fixture::MAGENTA_UV)
}

pub(super) fn large_plane() -> EnvironmentVolumeRecord {
    fixture::pool(0, 0.0, &LARGE_X)
}

pub(super) fn second_plane() -> EnvironmentVolumeRecord {
    fixture::pool(1, SECOND_H, &SECOND_X)
}

/// A renderer holding `quads` over both planes, each a probe-bound mirror
/// drawn this frame, at `quality` in `style`, under a full ambient floor;
/// `None` without an adapter.
pub(super) fn two_plane_renderer(
    quality: FluidReflectionQuality,
    style: FluidReflectionStyle,
    quads: &[WorldQuad],
) -> Option<Renderer> {
    let mut renderer = fixture::ssr_renderer()?;
    renderer.set_fluid_reflection_style(style);
    fixture::install_world_quads(&mut renderer, quads);
    // Lit by the ambient floor, every world face and mover shows its
    // material, so the magenta objects read magenta in the capture.
    renderer.set_ambient_floor(1.0);
    renderer.install_fluid_surfaces(
        &[large_plane(), second_plane()],
        Some(&fixture::green_probe(&[(0, 0), (1, 0)])),
    );
    draw_both(&mut renderer);
    renderer.set_reflection_quality(quality);
    Some(renderer)
}

/// The two-plane scene's world: the ceiling object, the standing object and
/// the back wall.
pub(super) fn two_plane_world() -> [WorldQuad; 3] {
    [ceiling_object(), standing_object(), fixture::back_wall()]
}

pub(super) fn draw_both(renderer: &mut Renderer) {
    renderer.set_fluid_surface_draws(&[
        look_draw(0, fixture::mirror()),
        look_draw(1, fixture::mirror()),
    ]);
}

/// The standing-quad scene: the base pool alone, an opaque magenta quad standing
/// in it left of centre and the black back wall, at very high with the
/// planar layer at full scale, the base pool's plane selected.
pub(super) fn standing_quad_renderer() -> Option<(Renderer, View)> {
    let mut renderer = fixture::ssr_renderer()?;
    let quad = fixture::upright(
        [-1.4, -0.2],
        [0.0, 1.5],
        4.0,
        fixture::standing_quad().base_uv,
    );
    fixture::install_world_quads(&mut renderer, &[quad, fixture::back_wall()]);
    renderer.install_fluid_surfaces(
        &[fixture::base_pool()],
        Some(&fixture::green_probe(&[(0, 0)])),
    );
    renderer.set_fluid_surface_draws(&[look_draw(0, fixture::mirror())]);
    renderer.set_reflection_quality(FluidReflectionQuality::VeryHigh);
    renderer.set_planar_preset_override(Some(full_scale()));
    let view = fixture::base_view();
    select(&mut renderer, view, 0.0);
    Some((renderer, view))
}

/// Very high's preset at full scale.
pub(super) fn full_scale() -> PlanarPreset {
    PlanarPreset {
        scale: 1.0,
        ..planar_preset(FluidReflectionQuality::VeryHigh).expect("a planar tier")
    }
}

/// The fixtures' projection: [`fixture::view_from`]'s.
fn projection() -> glam::Mat4 {
    glam::camera::rh::proj::directx::perspective(60.0_f32.to_radians(), 1.0, 0.1, 100.0)
}

/// The mirror camera of the plane at height `h` through `view`.
pub(super) fn mirror_camera(view: View, h: f32) -> MirrorCamera {
    let projection = projection();
    let main_view = projection.inverse() * view.view_proj;
    MirrorCamera::new(view.eye, main_view, projection, h, None).expect("the eye is above the plane")
}

/// Select the plane at height `h` for the next frames.
pub(super) fn select(renderer: &mut Renderer, view: View, h: f32) {
    renderer.set_planar_reflection_view(Some(mirror_camera(view, h)), &VisibleCells::DrawAll);
}

/// Select no plane for the next frames.
pub(super) fn deselect(renderer: &mut Renderer) {
    renderer.set_planar_reflection_view(None, &VisibleCells::DrawAll);
}

/// The planar layer as the last frame left it: premultiplied `(c·X, c)`.
pub(super) fn read_planar_layer(renderer: &Renderer) -> TargetReadback {
    let texture = renderer
        .full()
        .fluid_surface
        .planar_layer_view()
        .expect("a planar layer")
        .texture()
        .clone();
    let (width, height) = (texture.width(), texture.height());
    let row = width * 8;
    let padded =
        row.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let buffer = renderer.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Planar proof readback"),
        size: u64::from(padded * height),
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut encoder = renderer
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Planar proof readback"),
        });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    renderer
        .queue
        .submit_unbatched([encoder.finish()], "planar proof readback");
    let slice = buffer.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |result| {
        tx.send(result).ok();
    });
    renderer
        .device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("poll");
    rx.recv().expect("map callback").expect("map");
    let data = slice.get_mapped_range().expect("mapped");
    let mut texels = Vec::with_capacity((width * height) as usize);
    for y in 0..height {
        let start = (y * padded) as usize;
        for texel in data[start..start + row as usize].as_chunks::<8>().0 {
            let channel =
                |c: usize| f16_bits_to_f32(u16::from_le_bytes([texel[2 * c], texel[2 * c + 1]]));
            texels.push(Vec4::new(channel(0), channel(1), channel(2), channel(3)));
        }
    }
    drop(data);
    buffer.unmap();
    TargetReadback {
        extent: UVec2::new(width, height),
        texels,
    }
}

/// A capture pixel reads magenta: red and blue both clearly over green, as
/// the placeholder checkerboard's magenta square reflects, never the green
/// probe, the black wall or the white clear.
pub(super) fn magenta(rgb: [u8; 3]) -> bool {
    rgb[0] >= rgb[1].saturating_add(16) && rgb[2] >= rgb[1].saturating_add(16)
}

/// The water at `pixel`'s centre on the plane at height `h` over `x` (and
/// the pool's z), inset by `margin`, when the opaque scene does not hide it.
pub(super) fn visible_on(
    view: View,
    scene: &SceneReadback,
    pixel: UVec2,
    h: f32,
    x: [f32; 2],
    margin: f32,
) -> Option<Vec3> {
    let point = on_extent(view, pixel, h, x, margin)?;
    let (_, depth) = fixture::project(view, point);
    (depth < scene.depth_at(pixel) - 1.0e-4).then_some(point)
}

/// The point at `pixel`'s centre on the plane at height `h`, when it lies
/// over `x` and the pool's z inset by `margin` (outset when negative).
pub(super) fn on_extent(
    view: View,
    pixel: UVec2,
    h: f32,
    x: [f32; 2],
    margin: f32,
) -> Option<Vec3> {
    let point = fixture::on_plane(view, fixture::centre(pixel), h)?;
    let [z0, z1] = fixture::POOL_Z;
    (point.x > x[0] + margin
        && point.x < x[1] - margin
        && point.z > z0 + margin
        && point.z < z1 - margin)
        .then_some(point)
}

/// The capture pixel `point` projects into, or `None` off screen.
pub(super) fn pixel_of(view: View, point: Vec3) -> Option<UVec2> {
    let (at, _) = fixture::project(view, point);
    (at.cmpge(Vec2::ZERO).all() && at.cmplt(Vec2::splat(FRAME as f32)).all()).then(|| at.as_uvec2())
}

/// The pixel mirrored across the screen's vertical centre line.
pub(super) fn mirrored(pixel: UVec2) -> UVec2 {
    UVec2::new(FRAME - 1 - pixel.x, pixel.y)
}
