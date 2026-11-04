// SPIKE HACK (per-direction-depth): sidecar depth tiles, never merged.
// See: context/research/per-direction-depth-spike/README.md
//
// `POSTRETRO_PDD_TILES=<file>` uploads a tile atlas for `sh_corner_depth_visibility`.
// Unset leaves a 1x1 dummy and T = 0, which is today's isotropic path.

use postretro_render_cpu::sh_volume::{PDD_ATLAS_EDGE, f32_to_f16_bits, pdd_publish};

use crate::render::gpu_create::GpuCreate;

const TEXEL_BYTES: usize = 4;

fn new_texture(device: &wgpu::Device, width: u32, height: u32, layers: u32) -> wgpu::Texture {
    device.new_texture(&wgpu::TextureDescriptor {
        label: Some("PDD Tiles"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: layers,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rg16Float,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    })
}

fn write_layer(
    queue: &dyn crate::render::queue_writes::QueueWrites,
    texture: &wgpu::Texture,
    layer: u32,
    width: u32,
    height: u32,
    bytes: &[u8],
) {
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d {
                x: 0,
                y: 0,
                z: layer,
            },
            aspect: wgpu::TextureAspect::All,
        },
        bytes,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(width * TEXEL_BYTES as u32),
            rows_per_image: Some(height),
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
}

/// Octahedral border: an edge texel mirrors across the fold, so bilinear taps
/// at a tile's rim read the texels that are their neighbours on the sphere.
fn source_texel(x: i32, y: i32, t: i32) -> (i32, i32) {
    let (cx, cy) = (x.clamp(0, t - 1), y.clamp(0, t - 1));
    let x_out = x != cx;
    let y_out = y != cy;
    match (x_out, y_out) {
        (false, false) => (cx, cy),
        (true, false) => (cx, t - 1 - cy),
        (false, true) => (t - 1 - cx, cy),
        (true, true) => (t - 1 - cx, t - 1 - cy),
    }
}

pub(crate) fn create_view(
    device: &wgpu::Device,
    queue: &dyn crate::render::queue_writes::QueueWrites,
) -> wgpu::TextureView {
    let synthetic = std::env::var("POSTRETRO_PDD_Q2_T").ok();
    let texture = match (std::env::var("POSTRETRO_PDD_TILES"), synthetic) {
        (Ok(path), _) => load(device, queue, &path),
        (_, Some(t)) => synthesize(device, queue, t.parse().unwrap()),
        _ => {
            let texture = new_texture(device, 1, 1, 1);
            write_layer(queue, &texture, 0, 1, 1, &[0u8; TEXEL_BYTES]);
            texture
        }
    };
    texture.create_view(&wgpu::TextureViewDescriptor {
        label: Some("PDD Tiles View"),
        dimension: Some(wgpu::TextureViewDimension::D2Array),
        ..Default::default()
    })
}

/// Q2 cost mode: one tile per stored SH slot, sized past the hallway's pool. Values
/// are far enough that every test passes, with noise so no texel compresses away.
fn synthesize(
    device: &wgpu::Device,
    queue: &dyn crate::render::queue_writes::QueueWrites,
    t: u32,
) -> wgpu::Texture {
    const SLOTS: u32 = 1_000_000;
    let stride = t + 2;
    let per_row = PDD_ATLAS_EDGE / stride;
    let per_layer = per_row * per_row;
    let layers = SLOTS.div_ceil(per_layer);
    let edge = per_row * stride;
    let texture = new_texture(device, edge, edge, layers);
    let table: Vec<[u8; TEXEL_BYTES]> = (0..1024)
        .map(|i| {
            let mean = 200.0 + (i as f32) * 0.01;
            let lo = f32_to_f16_bits(mean).to_le_bytes();
            let hi = f32_to_f16_bits(mean * mean).to_le_bytes();
            [lo[0], lo[1], hi[0], hi[1]]
        })
        .collect();
    let mut state = 0x9E37_79B9u32;
    let mut image = vec![0u8; edge as usize * edge as usize * TEXEL_BYTES];
    for layer in 0..layers {
        for texel in image.chunks_exact_mut(TEXEL_BYTES) {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            texel.copy_from_slice(&table[(state & 1023) as usize]);
        }
        write_layer(queue, &texture, layer, edge, edge, &image);
    }
    pdd_publish(t, None);
    log::info!(
        "[pdd] Q2 synthetic tiles: T {t}, {layers} layer(s) of {edge}, {} MiB",
        edge as u64 * edge as u64 * TEXEL_BYTES as u64 * layers as u64 >> 20
    );
    texture
}

fn load(
    device: &wgpu::Device,
    queue: &dyn crate::render::queue_writes::QueueWrites,
    path: &str,
) -> wgpu::Texture {
    let file = std::fs::read(path).unwrap_or_else(|e| panic!("[pdd] {path}: {e}"));
    let word = |i: usize| u32::from_le_bytes(file[i * 4..i * 4 + 4].try_into().unwrap());
    assert_eq!(word(0), 0x3144_4450, "[pdd] bad magic in {path}");
    let (t, dims, rays) = (word(1), [word(2), word(3), word(4)], word(5));
    let probes = (dims[0] * dims[1] * dims[2]) as usize;
    let tile_bytes = (t * t) as usize * TEXEL_BYTES;
    assert_eq!(
        file.len(),
        24 + probes * tile_bytes,
        "[pdd] short file {path}"
    );

    let stride = t + 2;
    let per_row = PDD_ATLAS_EDGE / stride;
    let per_layer = (per_row * per_row) as usize;
    let layers = probes.div_ceil(per_layer).max(1) as u32;
    let edge = per_row * stride;
    let texture = new_texture(device, edge, edge, layers);
    let row_bytes = edge as usize * TEXEL_BYTES;
    let ti = t as i32;
    for layer in 0..layers as usize {
        let mut image = vec![0u8; row_bytes * edge as usize];
        let first = layer * per_layer;
        for probe in first..probes.min(first + per_layer) {
            let tile = &file[24 + probe * tile_bytes..][..tile_bytes];
            let local = (probe - first) as u32;
            let (ox, oy) = ((local % per_row) * stride, (local / per_row) * stride);
            for y in -1..=ti {
                for x in -1..=ti {
                    let (sx, sy) = source_texel(x, y, ti);
                    let src = (sy * ti + sx) as usize * TEXEL_BYTES;
                    let (dx, dy) = ((ox as i32 + 1 + x) as usize, (oy as i32 + 1 + y) as usize);
                    let dst = dy * row_bytes + dx * TEXEL_BYTES;
                    image[dst..dst + TEXEL_BYTES].copy_from_slice(&tile[src..src + TEXEL_BYTES]);
                }
            }
        }
        write_layer(queue, &texture, layer as u32, edge, edge, &image);
    }
    pdd_publish(t, Some(dims));
    log::info!("[pdd] {path}: T {t}, {rays} rays, grid {dims:?}, {layers} layer(s) of {edge}");
    texture
}

/// Q3 join: `POSTRETRO_PDD_Q3_BITS` is the compiler's byte per linear probe. Logs
/// flagged probes by the density level the loaded level stored them at.
pub(crate) fn report_q3(probes: &[postretro_level_format::sh_volume::OctahedralShProbe]) {
    let Ok(path) = std::env::var("POSTRETRO_PDD_Q3_BITS") else {
        return;
    };
    let bits = std::fs::read(&path).unwrap_or_else(|e| panic!("[pdd] {path}: {e}"));
    assert_eq!(bits.len(), probes.len(), "[pdd] Q3 bitmap grid mismatch");
    let mut rows = [[0u64; 5]; 4];
    let mut mismatched = 0u64;
    for (probe, b) in probes.iter().zip(&bits) {
        let traced = b & 0x80 != 0;
        if traced != (probe.validity != 0) {
            mismatched += 1;
        }
        if probe.validity == 0 {
            continue;
        }
        let row = &mut rows[(probe.density_level as usize).min(3)];
        row[0] += 1;
        for ti in 0..3 {
            row[1 + ti] += u64::from(b >> ti & 1);
        }
        row[4] += u64::from(probe.node_scale);
    }
    for (level, r) in rows.iter().enumerate() {
        log::info!(
            "[pdd q3] level {level}: {} valid, flagged T4 {} T6 {} T8 {}, node_scale sum {}",
            r[0], r[1], r[2], r[3], r[4]
        );
    }
    log::info!("[pdd q3] validity disagreements with the current map: {mismatched}");
}
