// All-resident static lightmap pool: irradiance, direction and shadowmask
// `texture_2d_array`s of `LIGHTMAP_POOL_LAYER_EDGE`² layers, each cell block
// written as a region in its stored format.
// See: context/lib/rendering_pipeline.md §4 (Lightmap cell-block residency)

use postretro_level_format::lightmap::{
    DIRECTION_TEXEL_BYTES, IRRADIANCE_FORMAT_BC6H, IRRADIANCE_TEXEL_BYTES,
    LIGHTMAP_POOL_LAYER_EDGE, LightmapBlockPayload,
};
use postretro_level_format::shadowmask_atlas::SHADOWMASK_GROUP_COUNT;

use super::plan::StaticPoolPlan;

/// BC texel block edge and bytes per BC6H / BC5 block.
const BC_BLOCK_EDGE: u32 = 4;
const BC_BLOCK_BYTES: u32 = 16;

/// Growth and repack copy blocks between pool generations and layers, so the
/// pool is a copy source as well as a sampled upload target.
const POOL_USAGE: wgpu::TextureUsages = wgpu::TextureUsages::TEXTURE_BINDING
    .union(wgpu::TextureUsages::COPY_DST)
    .union(wgpu::TextureUsages::COPY_SRC);

pub(crate) struct StaticPoolTextures {
    pub(crate) irradiance: wgpu::Texture,
    pub(crate) direction: wgpu::Texture,
    /// `None` when the plan keeps the all-visible shadowmask placeholder.
    pub(crate) shadowmask: Option<wgpu::Texture>,
}

fn pool_descriptor(
    label: &'static str,
    width: u32,
    height: u32,
    layers: u32,
    format: wgpu::TextureFormat,
) -> wgpu::TextureDescriptor<'static> {
    wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: layers,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: POOL_USAGE,
        view_formats: &[],
    }
}

pub(crate) fn irradiance_format(plan: &StaticPoolPlan) -> wgpu::TextureFormat {
    match plan.header.irradiance_format {
        IRRADIANCE_FORMAT_BC6H => wgpu::TextureFormat::Bc6hRgbUfloat,
        // `IRRADIANCE_FORMAT_RGBA16F`, the only other tag the header accepts.
        _ => wgpu::TextureFormat::Rgba16Float,
    }
}

/// Irradiance: one edge² layer per pool layer, in the stored format. BC6H and
/// Rgba16Float both bind as `Float { filterable: true }` through the linear
/// sampler; the shader reads `.rgb` either way.
pub(crate) fn irradiance_pool_descriptor(
    plan: &StaticPoolPlan,
) -> wgpu::TextureDescriptor<'static> {
    pool_descriptor(
        "Lightmap Irradiance Pool",
        LIGHTMAP_POOL_LAYER_EDGE,
        LIGHTMAP_POOL_LAYER_EDGE,
        plan.pool.layer_count,
        irradiance_format(plan),
    )
}

/// Direction: the same layers at `edge / scale`, so a block's direction
/// offset is its irradiance offset over the scale and one normalized UV
/// addresses both pools.
pub(crate) fn direction_pool_descriptor(plan: &StaticPoolPlan) -> wgpu::TextureDescriptor<'static> {
    let edge = LIGHTMAP_POOL_LAYER_EDGE / plan.header.direction_texel_scale.max(1);
    pool_descriptor(
        "Lightmap Direction Pool",
        edge,
        edge,
        plan.pool.layer_count,
        wgpu::TextureFormat::Rg8Unorm,
    )
}

/// Shadowmask: BC5 `.rg`, two mask groups side by side per layer. Group A of
/// a block at `(x, y)` lives at `(x, y)`, group B at `(edge + x, y)`.
pub(crate) fn shadowmask_pool_descriptor(
    plan: &StaticPoolPlan,
) -> wgpu::TextureDescriptor<'static> {
    pool_descriptor(
        "Shadowmask Pool",
        LIGHTMAP_POOL_LAYER_EDGE * SHADOWMASK_GROUP_COUNT,
        LIGHTMAP_POOL_LAYER_EDGE,
        plan.pool.layer_count,
        wgpu::TextureFormat::Bc5RgUnorm,
    )
}

/// Row layout of a region write: bytes per row and rows, for texel-addressed
/// (`block_edge` 1) or BC (`block_edge` 4) formats.
fn region_layout(width: u32, height: u32, block_edge: u32, unit_bytes: u32) -> (u32, u32) {
    (
        width.div_ceil(block_edge) * unit_bytes,
        height.div_ceil(block_edge),
    )
}

#[allow(clippy::too_many_arguments)]
fn write_region(
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    (layer, x, y): (u32, u32, u32),
    (width, height): (u32, u32),
    block_edge: u32,
    unit_bytes: u32,
    bytes: &[u8],
) {
    let (bytes_per_row, rows) = region_layout(width, height, block_edge, unit_bytes);
    debug_assert_eq!(bytes.len() as u64, u64::from(bytes_per_row * rows));
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d { x, y, z: layer },
            aspect: wgpu::TextureAspect::All,
        },
        bytes,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(bytes_per_row),
            rows_per_image: Some(rows),
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
}

/// Create the pool textures and write every block at its placement, in its
/// stored format with no decode. Unwritten texels stay wgpu's zero
/// initialization. Runs once per level install; the payloads drop after.
pub(crate) fn upload_static_pool(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    plan: &StaticPoolPlan,
    payloads: &[LightmapBlockPayload],
) -> StaticPoolTextures {
    let irradiance = device.create_texture(&irradiance_pool_descriptor(plan));
    let direction = device.create_texture(&direction_pool_descriptor(plan));
    let shadowmask = plan
        .with_shadowmask
        .then(|| device.create_texture(&shadowmask_pool_descriptor(plan)));
    let (irr_edge, irr_unit) = match irradiance_format(plan) {
        wgpu::TextureFormat::Bc6hRgbUfloat => (BC_BLOCK_EDGE, BC_BLOCK_BYTES),
        _ => (1, IRRADIANCE_TEXEL_BYTES as u32),
    };
    let scale = plan.header.direction_texel_scale.max(1);
    for ((&(width, height), placement), payload) in
        plan.extents.iter().zip(&plan.pool.placements).zip(payloads)
    {
        let origin = (placement.layer, placement.x, placement.y);
        write_region(
            queue,
            &irradiance,
            origin,
            (width, height),
            irr_edge,
            irr_unit,
            &payload.irradiance,
        );
        write_region(
            queue,
            &direction,
            (placement.layer, placement.x / scale, placement.y / scale),
            (width / scale, height / scale),
            1,
            DIRECTION_TEXEL_BYTES as u32,
            &payload.direction,
        );
        if let (Some(texture), Some([group_a, group_b])) = (&shadowmask, &payload.shadowmask) {
            for (group_x, group) in [
                (placement.x, group_a),
                (LIGHTMAP_POOL_LAYER_EDGE + placement.x, group_b),
            ] {
                write_region(
                    queue,
                    texture,
                    (placement.layer, group_x, placement.y),
                    (width, height),
                    BC_BLOCK_EDGE,
                    BC_BLOCK_BYTES,
                    group,
                );
            }
        }
    }
    StaticPoolTextures {
        irradiance,
        direction,
        shadowmask,
    }
}
