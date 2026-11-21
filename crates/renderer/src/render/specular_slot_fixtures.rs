// Material textures for the forward and mover specular readback harnesses:
// a specular slot in each width the bake writes (or the production
// placeholder) and an albedo, uploaded mip-chained through the production
// `upload_texture_data` and sampled through the material aniso sampler.
// See: context/lib/resource_management.md §4.1, §4.6, §4.7

use super::gpu_test_harness::GpuCtx;
use super::loaded_texture::upload_texture_data;

/// A specular slot's stored width.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SlotFormat {
    /// `_s` alone: R intensity.
    R8,
    /// `_s` + `_h`: R intensity, G inverted height.
    Rg8,
    /// `_p` and/or `_t`: R intensity, G inverted height, B smoothness, A tint.
    Rgba8,
    /// The 1×1 black `R8Unorm` an absent slot binds.
    Placeholder,
}

impl SlotFormat {
    fn wgpu(self) -> wgpu::TextureFormat {
        match self {
            SlotFormat::R8 | SlotFormat::Placeholder => wgpu::TextureFormat::R8Unorm,
            SlotFormat::Rg8 => wgpu::TextureFormat::Rg8Unorm,
            SlotFormat::Rgba8 => wgpu::TextureFormat::Rgba8Unorm,
        }
    }

    fn channels(self) -> usize {
        match self {
            SlotFormat::R8 | SlotFormat::Placeholder => 1,
            SlotFormat::Rg8 => 2,
            SlotFormat::Rgba8 => 4,
        }
    }
}

/// The mip chain `material_textures` uploads for a `size`² slot of `format`,
/// as `(side, bytes)` per level: what the runtime loader measures relief from.
pub(super) fn slot_mip_chain(
    size: u32,
    format: SlotFormat,
    slot: &dyn Fn(u32, u32) -> [u8; 4],
) -> Vec<(u32, Vec<u8>)> {
    mip_chain(size, format.channels(), slot)
}

/// One material's textures and the sampler its bind group would carry.
pub(super) struct MaterialTextures {
    pub base: wgpu::TextureView,
    pub specular: wgpu::TextureView,
    pub sampler: wgpu::Sampler,
}

/// The stored bytes of a `size`² specular slot whose texel (x, y) holds
/// `texel(x, y)` (RGBA; channels past the format's are dropped), with a box
/// mip chain down to 1×1.
fn mip_chain(
    size: u32,
    channels: usize,
    texel: &dyn Fn(u32, u32) -> [u8; 4],
) -> Vec<(u32, Vec<u8>)> {
    let mut levels = Vec::new();
    let mut current: Vec<[u8; 4]> = (0..size * size)
        .map(|i| texel(i % size, i / size))
        .collect();
    let mut side = size;
    loop {
        let bytes = current
            .iter()
            .flat_map(|texel| texel[..channels].to_vec())
            .collect();
        levels.push((side, bytes));
        if side == 1 {
            return levels;
        }
        let next = side / 2;
        current = (0..next * next)
            .map(|i| {
                let (x, y) = (i % next * 2, i / next * 2);
                std::array::from_fn(|c| {
                    let sum: u32 = [(x, y), (x + 1, y), (x, y + 1), (x + 1, y + 1)]
                        .iter()
                        .map(|&(sx, sy)| u32::from(current[(sy * side + sx) as usize][c]))
                        .sum();
                    // Round half up, as a box filter of equal texels returns
                    // their value exactly.
                    ((sum + 2) / 4) as u8
                })
            })
            .collect();
        side = next;
    }
}

fn upload(
    ctx: &GpuCtx,
    format: wgpu::TextureFormat,
    levels: &[(u32, Vec<u8>)],
    label: &str,
) -> (wgpu::Texture, wgpu::TextureView) {
    let levels: Vec<(u32, u32, &[u8])> = levels
        .iter()
        .map(|(side, bytes)| (*side, *side, bytes.as_slice()))
        .collect();
    upload_texture_data(&ctx.device, &ctx.queue, format, &levels, label)
}

/// A `size`² material: an sRGB albedo and a specular slot of `format`, each
/// mip-chained, with the production aniso sampler for that chain. The
/// placeholder slot ignores `slot` and binds the production 1×1 texel.
pub(super) fn material_textures(
    ctx: &GpuCtx,
    size: u32,
    albedo: &dyn Fn(u32, u32) -> [u8; 4],
    format: SlotFormat,
    slot: &dyn Fn(u32, u32) -> [u8; 4],
) -> MaterialTextures {
    let base_levels = mip_chain(size, 4, albedo);
    let (_, base) = upload(
        ctx,
        wgpu::TextureFormat::Rgba8UnormSrgb,
        &base_levels,
        "specular harness albedo",
    );
    let specular = if format == SlotFormat::Placeholder {
        super::loaded_texture::placeholder_loaded_texture(&ctx.device, &ctx.queue).specular_view
    } else {
        let levels = mip_chain(size, format.channels(), slot);
        upload(ctx, format.wgpu(), &levels, "specular harness slot").1
    };
    let sampler = super::create_mip_aniso_sampler(&ctx.device, base_levels.len() as u32);
    MaterialTextures {
        base,
        specular,
        sampler,
    }
}

/// A white albedo under the specular slot `slot` describes.
pub(super) fn white_material(
    ctx: &GpuCtx,
    size: u32,
    format: SlotFormat,
    slot: &dyn Fn(u32, u32) -> [u8; 4],
) -> MaterialTextures {
    material_textures(ctx, size, &|_, _| [255; 4], format, slot)
}
