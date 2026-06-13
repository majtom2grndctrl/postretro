// Shared headless GPU harness for the UI pass's offscreen golden tests.
//
// The UI golden tests (`multi_batch_test`, `splash_golden_test`,
// `multi_layer_text_golden_test`) all need the same three things: a `pollster`
// headless `wgpu::Device`/`Queue` that self-skips when no adapter is present, an
// offscreen-texture readback that copies to a mappable buffer (256-byte row
// alignment), maps, and de-pads to a tight RGBA8 grid, and a `Readback` accessor
// over that grid. These were duplicated across the test modules (with two
// divergent `Readback` shapes); per testing_guide §4 ("Multiple test modules
// need the same builders" → extract into a `#[cfg(test)]` sibling) they live
// here once, and every UI golden migrates onto this single copy.
//
// No GPU context in CI is the norm (testing_guide §3): `try_init_gpu` returns
// `None` so each test self-skips rather than failing for adapter absence.
//
// See: context/lib/testing_guide.md §3, §4

/// A headless `wgpu` device + queue for offscreen rendering. No surface, no
/// window — the golden tests render into a `COPY_SRC` texture and read it back.
pub(crate) struct GpuCtx {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
}

/// Build a headless device, or `None` when no adapter is available (the
/// headless-CI case). Every UI golden self-skips on `None` so adapter absence
/// can never be the thing that fails CI.
pub(crate) fn try_init_gpu() -> Option<GpuCtx> {
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
        label: Some("UI golden test Device"),
        required_features: wgpu::Features::empty(),
        required_limits: wgpu::Limits::default(),
        ..Default::default()
    }))
    .ok()?;
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
    mut encoder: wgpu::CommandEncoder,
) -> Readback {
    let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let unpadded = width * 4;
    let padded = unpadded.div_ceil(align) * align;

    let buffer = ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("UI golden readback"),
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

    Readback {
        width,
        height,
        pixels: tight,
    }
}
