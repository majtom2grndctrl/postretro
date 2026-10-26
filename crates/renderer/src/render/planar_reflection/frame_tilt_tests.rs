//! Adapter-backed planar pixel-tilt proofs: the reflected edge shifts as SSR's, and tilt never changes c.
//! See: context/lib/rendering_pipeline.md §7.3a

use super::frame_test_fixtures::*;
use crate::render::fluid_reflection_frame_test_fixtures::{
    self as fixture, FRAME, TargetReadback, View, WorldQuad, pixels,
};
use crate::render::fluid_surface_frame_test::look_draw;
use crate::render::{ClearColor, Renderer, ShSampleRegionSets};
use glam::{IVec2, Vec2, Vec3, Vec4};
use postretro_level_loader::ShDrainBatch;
use postretro_render_cpu::fluid_surface::motion::displayed_offset;
use postretro_render_cpu::fluid_surface::reflection_quality::FluidReflectionQuality;
use postretro_render_cpu::fluid_surface::reflection_style::{
    FLUID_REFLECTION_STYLE_PIXEL, FluidReflectionStyle,
};
use postretro_render_cpu::planar_reflection::PlanarPreset;
use postretro_render_cpu::planar_tilt::{
    PlanarMirrorView, planar_covered, planar_resolve_texel, planar_tilt_ray,
};
use postretro_visibility::{CameraCullVisibility, VisibilityPath, VisibleCells};

const ALL: VisibleCells = VisibleCells::DrawAll;
const WHITE: ClearColor = ClearColor {
    r: 1.0,
    g: 1.0,
    b: 1.0,
    a: 1.0,
};

/// The edge proof's capture extent: at the fixtures' 64² a one-texel tilt
/// moves a reflection about two mirror texels, too few to place a colour
/// edge with margin on both sides; at 256² it moves about seven.
const EDGE_FRAME: u32 = 256;
/// The back wall's colour edge: magenta to its left, black to its right.
/// Row 3's centre cell (0, 3) lands 0.13 m left of it untilted, and a
/// one-texel +x tilt carries it 0.12 m right.
const EDGE_X: f32 = 1.27;
const WALL_Z: f32 = fixture::POOL_Z[1];
const WALL_TOP: f32 = 4.0;
/// A wall landing this close to the colour edge (about 2.5 mirror texels at
/// [`EDGE_FRAME`]) could resolve to either side in either layer; the proof
/// reads none.
const EDGE_MARGIN: f32 = 0.09;

/// The asymmetric back wall the pool meets: magenta left of [`EDGE_X`],
/// black right of it, one flat depth, so its reflected colour edge crosses
/// no depth edge.
fn edge_world() -> [WorldQuad; 2] {
    [
        fixture::upright([-6.0, EDGE_X], [0.0, WALL_TOP], WALL_Z, fixture::MAGENTA_UV),
        fixture::upright([EDGE_X, 6.0], [0.0, WALL_TOP], WALL_Z, fixture::BLACK_UV),
    ]
}

/// The striped wall's top. Row 3's cells reflect it 0.13 m below its top
/// untilted, so a cell tilted up reaches the empty mirror above it.
const STRIPED_TOP: f32 = 2.7;

/// A back wall of 0.3 m vertical stripes, magenta and black, up to
/// [`STRIPED_TOP`]: most tilted lookups that move change colour, and some
/// land on the empty mirror above its top.
fn striped_world() -> Vec<WorldQuad> {
    (0..40)
        .map(|stripe| {
            let x = -6.0 + stripe as f32 * 0.3;
            let uv = if stripe % 2 == 0 {
                fixture::MAGENTA_UV
            } else {
                fixture::BLACK_UV
            };
            fixture::upright([x, x + 0.3], [0.0, STRIPED_TOP], WALL_Z, uv)
        })
        .collect()
}

/// A renderer capturing at `frame`², or `None` without an adapter.
fn renderer_at(frame: u32) -> Option<Renderer> {
    match Renderer::new_offscreen(frame, frame) {
        Ok(renderer) => Some(renderer),
        Err(error) if error.to_string().contains("requires a GPU adapter") => {
            eprintln!("[PlanarTilt] skipped: no adapter ({error:#})");
            None
        }
        Err(error) => panic!("adapter present but renderer initialization failed: {error:#}"),
    }
}

/// The base pool before `world` in pixel style at `quality`, the planar
/// layer at full scale with D and R off, its plane selected, captured at
/// `frame`²; `None` without an adapter. The green probe's box holds the whole
/// pool, so no cell lies on its rim and SSR's cell point is the unpulled
/// centre.
fn tilt_renderer(
    frame: u32,
    quality: FluidReflectionQuality,
    world: &[WorldQuad],
) -> Option<(Renderer, View)> {
    let mut renderer = renderer_at(frame)?;
    renderer.set_fluid_reflection_style(FluidReflectionStyle::Pixel);
    fixture::install_world_quads(&mut renderer, world);
    renderer.set_ambient_floor(1.0);
    renderer.install_fluid_surfaces(
        &[fixture::base_pool()],
        Some(&fixture::green_probe(&[(0, 0)])),
    );
    renderer.set_fluid_surface_draws(&[look_draw(0, fixture::mirror())]);
    renderer.set_reflection_quality(quality);
    renderer.set_planar_preset_override(Some(PlanarPreset {
        d: None,
        r: None,
        ..full_scale()
    }));
    let view = fixture::base_view();
    select(&mut renderer, view, 0.0);
    Some((renderer, view))
}

/// `point`'s screen position in `frame` pixels.
fn screen_at(view: View, point: Vec3, frame: u32) -> Vec2 {
    let clip = view.view_proj * point.extend(1.0);
    let ndc = clip.truncate() / clip.w;
    Vec2::new((ndc.x + 1.0) * 0.5, (1.0 - ndc.y) * 0.5) * frame as f32
}

/// One frame through `view` with the animation clock at `time` seconds,
/// inside a validation error scope.
fn capture_at(renderer: &mut Renderer, view: View, time: f32) {
    let scope = renderer
        .device
        .push_error_scope(wgpu::ErrorFilter::Validation);
    renderer
        .capture_frame_indirect(
            CameraCullVisibility {
                cells: &ALL,
                path: VisibilityPath::EmptyWorldFallback,
            },
            &[],
            &[],
            &[],
            ShSampleRegionSets {
                visible_cells: &ALL,
                fog_cells: &[],
                movers: &[],
            },
            None,
            view.view_proj,
            view.eye,
            &[],
            &[],
            time,
            WHITE,
            true,
            ShDrainBatch::default(),
        )
        .expect("capture")
        .frame
        .expect("a frame");
    let error = pollster::block_on(scope.pop());
    assert!(
        error.is_none(),
        "the frame raised a validation error: {error:?}"
    );
}

/// The mirror depth as the last planar frame left it, row-major.
fn read_mirror_depth(renderer: &Renderer) -> Vec<f32> {
    let texture = renderer
        .planar_mirror_depth_texture()
        .expect("mirror targets");
    let (width, height) = (texture.width(), texture.height());
    let row = width * 4;
    let padded =
        row.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let buffer = renderer.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Planar tilt proof depth readback"),
        size: u64::from(padded * height),
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut encoder = renderer
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Planar tilt proof depth readback"),
        });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::DepthOnly,
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
        .submit_unbatched([encoder.finish()], "planar tilt proof depth readback");
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
    let mut depth = Vec::with_capacity((width * height) as usize);
    for y in 0..height {
        let start = (y * padded) as usize;
        depth.extend(
            data[start..start + row as usize]
                .as_chunks::<4>()
                .0
                .iter()
                .map(|bytes| f32::from_le_bytes(*bytes)),
        );
    }
    drop(data);
    buffer.unmap();
    depth
}

/// A layer texel `(c·X, c)` reads the magenta wall: confident, red and blue
/// clearly over green. The black wall, the clear and a miss read otherwise.
fn magenta_texel(texel: Vec4) -> bool {
    texel.w > 0.25 && texel.x > 0.05 && texel.z > 0.05 && texel.y < 0.5 * texel.x.min(texel.z)
}

/// The mirror view the selected plane's planar pass draws through `view`,
/// at `frame`².
fn mirror_view(view: View, frame: u32) -> PlanarMirrorView {
    PlanarMirrorView::new(mirror_camera(view, 0.0).raster_vp, (frame, frame))
}

/// Where a base-pool cell's pixel-style ray at `time` meets the back wall's
/// plane, by the CPU reference: the same ray SSR marches from an unpulled
/// cell, which every cell of this pool is.
fn wall_landing(view: View, cell: IVec2, time: f32, offset_scale: i32) -> Vec3 {
    let centre = fixture::cell_centre(cell, 0.0);
    let tilt = planar_tilt_ray(
        cell.as_vec2() + 0.5,
        centre,
        Vec3::Y,
        fixture::pool_axes(),
        view.eye,
        time,
        0.0,
        offset_scale,
    );
    centre + tilt.tilted_ray * ((WALL_Z - centre.z) / tilt.tilted_ray.z)
}

/// A landing either layer resolves the same way: a texel clear of the colour
/// edge, above the wall's base and below SSR's screen-edge fade (the wall's
/// top lies off the main screen).
fn clear_landing(landing: Vec3) -> bool {
    (landing.x - EDGE_X).abs() >= EDGE_MARGIN && (0.2..=2.8).contains(&landing.y)
}

/// The cells of row `row` around the edge whose landings the proof reads.
fn window(row: i32) -> impl Iterator<Item = IVec2> {
    (-2..=1).map(move |column| IVec2::new(column, row))
}

/// Magenta texels along the screen row through row `row`'s cell centres,
/// across its window: the reflected edge's column, counted in texels.
fn edge_texels(view: View, layer: &TargetReadback, row: i32) -> usize {
    let z = row as f32 + 0.5;
    // World +x runs to the screen's left: the view looks down +z.
    let (one, y) = screen_at(view, Vec3::new(-2.0, 0.0, z), EDGE_FRAME).into();
    let (other, _) = screen_at(view, Vec3::new(2.0, 0.0, z), EDGE_FRAME).into();
    let y = y.floor() as i32;
    (one.min(other).ceil() as i32..one.max(other).floor() as i32)
        .filter(|&column| magenta_texel(layer.at(IVec2::new(column, y))))
        .count()
}

/// Both layers' frames at `time`: the planar layer with the plane selected,
/// then the SSR target with planar forced off (no plane selected, so SSR
/// covers the plane at the tier), at the same pose and inputs.
fn frame_pair(renderer: &mut Renderer, view: View, time: f32) -> (TargetReadback, TargetReadback) {
    select(renderer, view, 0.0);
    capture_at(renderer, view, time);
    let planar = read_planar_layer(renderer);
    deselect(renderer);
    capture_at(renderer, view, time);
    let ssr = fixture::read_reflection_target(renderer);
    (planar, ssr)
}

// AC 10, GPU half. In pixel style, one GPU frame pair per clock at the same
// pose and inputs, planar on and planar forced off (SSR on the plane): the
// back wall's asymmetric reflected colour edge, on a row of cells clear of
// depth edges (the wall's base and top) and of the probe box's rim, shifts
// between an untilted clock (every window cell's offset zero) and a tilted
// one (a window cell's non-zero offset carries its ray across the edge) in
// the same direction and by the same texel count in both layers. The
// planar layer with its offset forced to zero holds the untilted edge.
#[test]
fn planar_gpu_edge_shifts_like_ssr() {
    // Ultra: at 256² the march's one-pixel stride needs ultra's step budget
    // to reach the wall with full confidence.
    let Some((mut renderer, view)) =
        tilt_renderer(EDGE_FRAME, FluidReflectionQuality::Ultra, &edge_world())
    else {
        return;
    };
    let steady = |cell: IVec2, time: f32| {
        displayed_offset(cell, time - 0.02, 0.0) == displayed_offset(cell, time + 0.02, 0.0)
    };
    let magenta = |landing: Vec3| landing.x < EDGE_X;
    let mut shifted_rows = 0;
    for row in 3..=5 {
        let clocks: Vec<f32> = (0..4000).map(|k| 0.13 + k as f32 * 0.05).collect();
        // Untilted: every window cell's offset is zero and its landing clear.
        let untilted = clocks.iter().copied().find(|&time| {
            window(row).all(|cell| {
                steady(cell, time)
                    && displayed_offset(cell, time, 0.0) == IVec2::ZERO
                    && clear_landing(wall_landing(view, cell, time, 1))
            })
        });
        // Tilted: every window landing clear, and some cell's offset moves
        // its landing across the edge.
        let tilted = clocks.iter().copied().find(|&time| {
            window(row)
                .all(|cell| steady(cell, time) && clear_landing(wall_landing(view, cell, time, 1)))
                && window(row).any(|cell| {
                    magenta(wall_landing(view, cell, time, 1))
                        != magenta(wall_landing(view, cell, time, 0))
                })
        });
        let (Some(untilted), Some(tilted)) = (untilted, tilted) else {
            eprintln!("[PlanarTilt] row {row}: no clear clock pair, not measured");
            continue;
        };
        let mut expected = 0i32;
        for cell in window(row) {
            let (before, after) = (
                magenta(wall_landing(view, cell, untilted, 1)),
                magenta(wall_landing(view, cell, tilted, 1)),
            );
            if before != after {
                assert_ne!(
                    displayed_offset(cell, tilted, 0.0),
                    IVec2::ZERO,
                    "row {row}: a crossing cell carries a non-zero offset"
                );
                expected += i32::from(after) - i32::from(before);
            }
        }
        let (planar_before, ssr_before) = frame_pair(&mut renderer, view, untilted);
        let (planar_after, ssr_after) = frame_pair(&mut renderer, view, tilted);
        select(&mut renderer, view, 0.0);
        renderer.set_planar_zero_offset(true);
        capture_at(&mut renderer, view, tilted);
        let planar_zero = read_planar_layer(&renderer);
        renderer.set_planar_zero_offset(false);

        let count = |layer: &TargetReadback| edge_texels(view, layer, row) as i32;
        let planar_shift = count(&planar_after) - count(&planar_before);
        let ssr_shift = count(&ssr_after) - count(&ssr_before);
        eprintln!(
            "[PlanarTilt] row {row}: clocks {untilted} -> {tilted}, cells {expected:+}; planar \
             {} -> {} ({planar_shift:+} texels), SSR {} -> {} ({ssr_shift:+} texels), planar \
             zero-offset {}",
            count(&planar_before),
            count(&planar_after),
            count(&ssr_before),
            count(&ssr_after),
            count(&planar_zero)
        );
        assert_eq!(
            count(&planar_before),
            count(&ssr_before),
            "row {row}: both layers hold one untilted edge"
        );
        assert_eq!(
            count(&planar_zero),
            count(&planar_before),
            "row {row}: the zero-offset planar frame is untilted"
        );
        assert_eq!(
            planar_shift, ssr_shift,
            "row {row}: the edge shifts by one texel count in both layers"
        );
        assert_ne!(planar_shift, 0, "row {row}: the tilt moves the edge");
        assert_eq!(
            planar_shift.signum(),
            expected.signum(),
            "row {row}: in the direction the tilted rays give"
        );
        shifted_rows += 1;
    }
    assert!(shifted_rows >= 1, "some row measures a shift");
    renderer.release_level_resources();
}

// AC 11. At a fixed pose and clock with D and R off, c over the selected
// plane matches the same frame with the displayed offset forced to zero,
// texel for texel, except where the tilted lookup lands on an empty mirror
// texel (by the CPU reference over the frame's own mirror depth), and there
// the tilted frame reads c = 0. Most water texels are compared, and the tilt
// moves the colour many of them sample.
#[test]
fn planar_gpu_tilt_moves_samples_not_confidence() {
    let Some((mut renderer, view)) =
        tilt_renderer(FRAME, FluidReflectionQuality::VeryHigh, &striped_world())
    else {
        return;
    };
    // A clock at which some row-3 cell's tilted ray clears the wall's top
    // by a mirror texel, a millisecond off every visible cell's step
    // boundary, so the CPU and GPU clocks step alike.
    let steady = |cell: IVec2, time: f32| {
        displayed_offset(cell, time - 1.0e-3, 0.0) == displayed_offset(cell, time + 1.0e-3, 0.0)
    };
    let row = |time: f32| (-6..=2).map(move |column| (IVec2::new(column, 3), time));
    let time = (0..4000)
        .map(|k| 0.13 + k as f32 * 0.05)
        .find(|&time| {
            (0..8).all(|z| (-6..=2).all(|x| steady(IVec2::new(x, z), time)))
                && row(time)
                    .any(|(cell, time)| wall_landing(view, cell, time, 1).y > STRIPED_TOP + 0.15)
        })
        .expect("a clock tilts a row-3 cell over the wall's top");
    renderer.set_planar_zero_offset(true);
    capture_at(&mut renderer, view, time);
    let untilted = read_planar_layer(&renderer);
    let depth = read_mirror_depth(&renderer);
    renderer.set_planar_zero_offset(false);
    capture_at(&mut renderer, view, time);
    let tilted = read_planar_layer(&renderer);
    assert!(
        read_mirror_depth(&renderer) == depth,
        "tilt changes nothing the mirror draws"
    );
    let mirror = mirror_view(view, FRAME);
    let depth_at = |texel: IVec2| depth[(texel.y as u32 * FRAME + texel.x as u32) as usize];
    let lands_empty = |point: Vec3, offset_scale: i32| {
        planar_resolve_texel(
            FLUID_REFLECTION_STYLE_PIXEL,
            Vec2::new(point.x, point.z) * fixture::POOL_TEXELS_PER_METRE,
            point,
            Vec3::Y,
            fixture::pool_axes(),
            view.eye,
            time,
            0.0,
            offset_scale,
            &mirror,
            depth_at,
        )
        .is_some_and(|texel| !planar_covered(depth_at(texel)))
    };
    let (mut water, mut compared, mut excluded, mut moved, mut covered) = (0, 0, 0, 0, 0);
    for pixel in pixels() {
        let Some(point) = fixture::on_plane(view, fixture::centre(pixel), 0.0) else {
            continue;
        };
        if !fixture::on_pool(point, 0.05) || fixture::cell_margin(point) < 0.01 {
            continue;
        }
        water += 1;
        let at = pixel.as_ivec2();
        let (before, after) = (untilted.at(at), tilted.at(at));
        if lands_empty(point, 1) || lands_empty(point, 0) {
            excluded += 1;
            if lands_empty(point, 1) {
                assert_eq!(
                    after.w, 0.0,
                    "{pixel}: a lookup onto an empty texel reads c = 0"
                );
            }
            continue;
        }
        compared += 1;
        assert_eq!(
            after.w, before.w,
            "{pixel}: tilt changed c ({} untilted, {} tilted)",
            before.w, after.w
        );
        covered += usize::from(after.w > 0.0);
        moved += usize::from(after.truncate() != before.truncate());
    }
    eprintln!(
        "[PlanarTilt] c at clock {time}: {water} water texels, {compared} compared ({covered} covered), \
         {excluded} excluded onto empty texels, {moved} sampled a different colour"
    );
    assert!(
        compared * 10 >= water * 9 && covered * 2 > water,
        "{compared} of {water}, {covered} covered"
    );
    assert!(moved > 100, "the tilt moves samples: {moved}");
    assert!(excluded > 0, "some tilted lookup lands on the empty mirror");
    renderer.release_level_resources();
}
