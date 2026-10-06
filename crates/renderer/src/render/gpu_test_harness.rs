// Shared headless GPU harness for the renderer's offscreen readback tests.
//
// The UI goldens all need the same three things: a `pollster`
// headless `wgpu::Device`/`Queue` that self-skips when no adapter is present, an
// offscreen-texture readback that copies to a mappable buffer (256-byte row
// alignment), maps, and de-pads to a tight RGBA8 grid, and a `Readback` accessor
// over that grid. These were duplicated across the test modules (with two
// divergent `Readback` shapes); per testing_guide §4 ("Multiple test modules
// need the same builders" → extract into a `#[cfg(test)]` sibling) they live
// here once, at the `render/` root so any pass's tests can reach it.
//
// No GPU context in CI is the norm (testing_guide §3): `try_init_gpu` returns
// `None` so each test self-skips rather than failing for adapter absence.
//
// See: context/lib/testing_guide.md §3, §4

use super::uploads::UploadQueue;

/// A headless `wgpu` device + queue for offscreen rendering. No surface, no
/// window — the golden tests render into a `COPY_SRC` texture and read it back.
pub(crate) struct GpuCtx {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
}

/// Build a headless device, or `None` when no adapter is available (the
/// headless-CI case). Every caller self-skips on `None` so adapter absence
/// can never be the thing that fails CI.
pub(crate) fn try_init_gpu() -> Option<GpuCtx> {
    try_init_gpu_with_features(wgpu::Features::empty())
}

pub(crate) fn try_init_gpu_with_features(required_features: wgpu::Features) -> Option<GpuCtx> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::PRIMARY,
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let adapter = match pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: None,
        force_fallback_adapter: false,
        apply_limit_buckets: false,
    })) {
        Ok(adapter) => adapter,
        Err(error) => {
            eprintln!("GPU test skipped: no adapter available ({error})");
            return None;
        }
    };
    if !adapter.features().contains(required_features) {
        eprintln!("GPU test skipped: adapter lacks {required_features:?}");
        return None;
    }
    let identity = adapter.get_info();
    eprintln!("[UploadAdapter] {} {:?}", identity.name, identity.backend);
    let (device, queue) =
        match pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("GPU test harness Device"),
            required_features,
            required_limits: wgpu::Limits::default(),
            ..Default::default()
        })) {
            Ok(gpu) => gpu,
            Err(error) => {
                eprintln!("GPU test skipped: device unavailable ({error})");
                return None;
            }
        };
    Some(GpuCtx { device, queue })
}

/// A read-back RGBA8 pixel grid, de-padded to a tight `width*4` stride. Carries
/// `height` so callers can iterate full rows/columns (the multi-layer golden's
/// per-band ink scan needs it; the half-split golden only uses `width`).
pub(crate) struct Readback {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

impl Readback {
    /// The RGBA bytes at `(x, y)`.
    pub fn at(&self, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * self.width + x) * 4) as usize;
        [
            self.pixels[i],
            self.pixels[i + 1],
            self.pixels[i + 2],
            self.pixels[i + 3],
        ]
    }
}

/// Copy `texture` to a mappable buffer (respecting the 256-byte row alignment),
/// submit `encoder`, map, and de-pad into a tight `width*4` RGBA8 buffer, wrapped
/// in a `Readback`. Consumes the caller's `encoder` (the texture-to-buffer copy is
/// the last command it records) and blocks on the map via a `pollster`-style
/// channel + `device.poll`.
pub(crate) fn read_texture_rgba8(
    ctx: &GpuCtx,
    texture: &wgpu::Texture,
    width: u32,
    height: u32,
    encoder: wgpu::CommandEncoder,
) -> Readback {
    read_texture_rgba8_with_submit(ctx, texture, width, height, encoder, |command| {
        ctx.queue.submit([command]);
    })
}

/// Read pixels from work whose renderer-owned writes use the deferred batch.
/// Pixel assertions also prove that the upload copies executed before the draws.
pub(crate) fn read_texture_rgba8_staged(
    ctx: &GpuCtx,
    uploads: &UploadQueue,
    texture: &wgpu::Texture,
    width: u32,
    height: u32,
    encoder: wgpu::CommandEncoder,
) -> Readback {
    let before = uploads.counts();
    assert!(
        before.writes > 0,
        "golden must exercise staged renderer writes"
    );
    assert_eq!(before.direct_writes, 0);
    let pixels = read_texture_rgba8_with_submit(ctx, texture, width, height, encoder, |command| {
        uploads.submit([command]);
    });
    let after = uploads.counts();
    assert_eq!(after.submits, before.submits + 1);
    assert_eq!(after.batches, before.batches + 1);
    assert!(after.copies > before.copies);
    uploads.assert_empty("golden frame exit");
    pixels
}

fn read_texture_rgba8_with_submit(
    ctx: &GpuCtx,
    texture: &wgpu::Texture,
    width: u32,
    height: u32,
    mut encoder: wgpu::CommandEncoder,
    submit: impl FnOnce(wgpu::CommandBuffer),
) -> Readback {
    let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let unpadded = width * 4;
    let padded = unpadded.div_ceil(align) * align;

    let buffer = ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("GPU test harness readback"),
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
    submit(encoder.finish());

    let slice = buffer.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |r| {
        tx.send(r).ok();
    });
    ctx.device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("poll");
    rx.recv().expect("map channel").expect("map ok");

    let data = slice
        .get_mapped_range()
        .expect("buffer mapped for readback");
    let mut tight = Vec::with_capacity((unpadded * height) as usize);
    for row in 0..height {
        let start = (row * padded) as usize;
        tight.extend_from_slice(&data[start..start + unpadded as usize]);
    }
    drop(data);
    buffer.unmap();

    Readback {
        width,
        height,
        pixels: tight,
    }
}
