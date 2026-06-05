// Optional headless golden for the splash UI pass (Task 6b).
//
// Builds a wgpu device via `pollster` (the `curve_eval_test` /
// `sdf_light_select_test` precedent), constructs a `UiPass`, encodes the splash
// draw list (background fill + framed 9-slice panel + a shaped-text line) into an
// OFFSCREEN texture — never a real surface — reads the texture back, and asserts
// tolerance-scoped STRUCTURAL properties of the readback.
//
// This is NOT the hard gate. The CPU draw-list / layout assertion
// (`splash_layout_test`) pins the geometry; this test only confirms the pass
// composites end-to-end on a real device. It self-skips cleanly when no GPU
// adapter is present (return early — never panic/fail), so it can never be the
// thing that fails CI.
//
// Golden approach: STRUCTURAL readback, not a committed PNG. AA glyph coverage
// rasterizes subtly differently per backend/driver (the plan's
// "Golden-image portability" open question), so a committed reference image would
// be backend-fragile and over-engineered for Goal A. Instead we assert:
//   1. the background-fill quad drew (a corner pixel is no longer the black clear
//      and reads back ~ the documented sRGB(21,27,35) background, within a
//      generous tolerance), and
//   2. the framed panel drew over the background (the panel-center pixel differs
//      from the background corner by more than the tolerance).
// Text is encoded through the pass (so the full path runs on the device) but its
// pixels are NOT asserted — only that `encode` composites without error.
//
// See: context/plans/in-progress/M13--ui-render-pass-slice (Task 6b; "optional
// headless golden ... self-skips ... not the hard gate").

use super::layout;
use super::splash::{SplashDescriptor, build_splash_descriptor};
use super::{UiBatch, UiDrawList, UiPass};
use crate::render::splash::splash_bg_rgba;

/// Offscreen render-target size. The exact 1280x720 reference (device scale 1.0,
/// no letterbox) so the readback maps 1:1 to logical-reference coordinates and the
/// background fill covers the whole target. 1280*4 = 5120 bytes/row is already a
/// multiple of `COPY_BYTES_PER_ROW_ALIGNMENT` (256), but the readback still pads
/// generically so a future size change can't silently corrupt the decode.
const TARGET_W: u32 = 1280;
const TARGET_H: u32 = 720;

/// sRGB-encoded byte tolerance for the background-color match and the
/// panel-differs-from-background threshold. Generous (16/255) because the per-pixel
/// linear->sRGB encode rounds differently across backends/drivers, and the point is
/// structural presence — not an exact golden. The panel/background contrast is far
/// larger than this, so it discriminates cleanly while tolerating backend drift.
const COLOR_TOL: i32 = 16;

struct GpuCtx {
    device: wgpu::Device,
    queue: wgpu::Queue,
}

/// Build a headless device, or `None` when no adapter is available (the
/// headless-CI case). Mirrors `curve_eval_test::try_init_gpu`.
fn try_init_gpu() -> Option<GpuCtx> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::PRIMARY,
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::default(),
        compatible_surface: None,
        force_fallback_adapter: false,
    }))
    .ok()?;
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("splash_golden_test Device"),
        required_features: wgpu::Features::empty(),
        required_limits: wgpu::Limits::default(),
        ..Default::default()
    }))
    .ok()?;
    Some(GpuCtx { device, queue })
}

/// A read-back RGBA8 pixel grid (de-padded to a tight `width*4` stride).
struct Readback {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

impl Readback {
    /// The RGBA bytes at `(x, y)`.
    fn at(&self, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * self.width + x) * 4) as usize;
        [
            self.pixels[i],
            self.pixels[i + 1],
            self.pixels[i + 2],
            self.pixels[i + 3],
        ]
    }
}

/// Render the splash quads into an offscreen sRGB texture and read it back. Uses
/// the same surface-format family the live pass uses (`Rgba8UnormSrgb`) so the
/// shader's linear-write / sRGB-encode behavior matches the real swapchain.
fn render_splash_offscreen(ctx: &GpuCtx) -> Readback {
    let format = wgpu::TextureFormat::Rgba8UnormSrgb;
    let mut pass = UiPass::new(&ctx.device, &ctx.queue, format);

    let target = ctx.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("splash_golden offscreen target"),
        size: wgpu::Extent3d {
            width: TARGET_W,
            height: TARGET_H,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&wgpu::TextureViewDescriptor::default());

    // Assemble the splash draw data exactly as `Renderer::record_splash_ui`:
    // oversized background fill first (outside the tree), then the descriptor
    // tree's panel quads + version text laid out through the pass's font system.
    let viewport = [TARGET_W, TARGET_H];
    // The real banner asset is 2028x582; the structural golden only needs the
    // panel-vs-background contrast, so any plausible logo aspect serves — pass the
    // real source aspect so the descriptor is shaped exactly as the engine builds it.
    let desc: SplashDescriptor = build_splash_descriptor(2028.0 / 582.0, "postretro v0.1.0");

    let bg = SplashDescriptor::background_element(splash_bg_rgba());
    let mut panel_list: UiDrawList = layout::project(&[bg], viewport);
    // The tree's panel quads (border + fill) concatenate into the white-texel
    // batch behind the logo/text. The logo's own texture binding is not needed
    // for the structural assertion (background-vs-panel contrast), so its image
    // batch is omitted — the golden does not depend on the committed PNG. The
    // version text is encoded through the pass so the full path runs on-device;
    // its pixels are not asserted.
    let draw = pass.layout_tree(desc.tree(), viewport);
    panel_list
        .instances
        .extend_from_slice(&draw.quads.instances);

    // Panels sample the pass's 1x1 white texel.
    let white_bg = pass.white_bind_group().clone();
    let batches = [UiBatch {
        list: &panel_list,
        bind_group: &white_bg,
    }];
    let texts = draw.texts;

    let mut encoder = ctx
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("splash_golden encoder"),
        });
    pass.encode(
        &ctx.device,
        &ctx.queue,
        &mut encoder,
        &view,
        viewport,
        wgpu::LoadOp::Clear(wgpu::Color::BLACK),
        &batches,
        &texts,
    );

    let readback = read_texture_rgba8(ctx, &target, TARGET_W, TARGET_H, encoder);
    Readback {
        width: TARGET_W,
        height: TARGET_H,
        pixels: readback,
    }
}

/// Copy `texture` to a mappable buffer (respecting the 256-byte row alignment),
/// submit `encoder`, map, and de-pad into a tight `width*4` RGBA8 buffer.
fn read_texture_rgba8(
    ctx: &GpuCtx,
    texture: &wgpu::Texture,
    width: u32,
    height: u32,
    mut encoder: wgpu::CommandEncoder,
) -> Vec<u8> {
    let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let unpadded = width * 4;
    let padded = unpadded.div_ceil(align) * align;

    let buffer = ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("splash_golden readback"),
        size: (padded * height) as u64,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
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
    ctx.queue.submit(std::iter::once(encoder.finish()));

    let slice = buffer.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |r| {
        tx.send(r).ok();
    });
    ctx.device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("poll");
    rx.recv().expect("map channel").expect("map ok");

    let data = slice.get_mapped_range();
    let mut tight = Vec::with_capacity((unpadded * height) as usize);
    for row in 0..height {
        let start = (row * padded) as usize;
        tight.extend_from_slice(&data[start..start + unpadded as usize]);
    }
    drop(data);
    buffer.unmap();
    tight
}

fn within(a: u8, b: u8, tol: i32) -> bool {
    (a as i32 - b as i32).abs() <= tol
}

/// The splash background fill quad composites over the black clear and reads back
/// as the documented sRGB(21,27,35) background. Structural: confirms the
/// first-quad background actually painted the whole target (corner sampled, away
/// from the centered panel).
#[test]
fn splash_background_fill_covers_target_with_bg_color() {
    let Some(ctx) = try_init_gpu() else {
        eprintln!("[splash_golden_test] skipping: no GPU adapter available");
        return;
    };

    let rb = render_splash_offscreen(&ctx);

    // A corner pixel — well outside the centered 740x360 panel — is pure
    // background fill. SPLASH_BG_COLOR is "linear-space sRGB(21,27,35)", so the
    // sRGB texture encodes it back to ~ (21,27,35).
    let corner = rb.at(2, 2);
    // Not the black clear — the background quad drew.
    assert!(
        corner[0] as i32 + corner[1] as i32 + corner[2] as i32 > 0,
        "background corner is still the black clear ({corner:?}) — bg fill did not draw",
    );
    // ~ sRGB(21,27,35), generous tolerance for cross-backend encode rounding.
    let expected = [21u8, 27, 35, 255];
    for c in 0..4 {
        assert!(
            within(corner[c], expected[c], COLOR_TOL),
            "background corner channel {c} = {} not within {COLOR_TOL} of {} (got {corner:?})",
            corner[c],
            expected[c],
        );
    }
}

/// The framed 9-slice panel composites over the background fill: the panel-center
/// pixel differs from the background corner by more than the color tolerance.
/// Structural: confirms the panel quads drew on top of the background (the
/// readback has real panel-vs-background contrast), independent of exact AA edges.
#[test]
fn splash_panel_region_differs_from_background() {
    let Some(ctx) = try_init_gpu() else {
        eprintln!("[splash_golden_test] skipping: no GPU adapter available");
        return;
    };

    let rb = render_splash_offscreen(&ctx);

    let corner = rb.at(2, 2);
    // Dead center of the target = center of the framed panel (the fill panel).
    let center = rb.at(rb.width / 2, rb.height / 2);

    let diff = (0..3)
        .map(|c| (center[c] as i32 - corner[c] as i32).abs())
        .max()
        .unwrap();
    assert!(
        diff > COLOR_TOL,
        "panel center {center:?} does not differ from background corner {corner:?} \
         (max channel diff {diff} <= tol {COLOR_TOL}) — panel quads did not composite",
    );
}
