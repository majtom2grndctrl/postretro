// Static lightmap pool textures: irradiance, direction and shadowmask
// `texture_2d_array`s of `LIGHTMAP_POOL_LAYER_EDGE`² layers, and where a cell
// block's texels land in each (region writes, block copies).
// See: context/lib/rendering_pipeline.md §4 (Lightmap cell-block residency)

use postretro_level_format::lightmap::{
    DIRECTION_TEXEL_BYTES, IRRADIANCE_FORMAT_BC6H, IRRADIANCE_TEXEL_BYTES,
    LIGHTMAP_POOL_LAYER_EDGE, LightmapBlockPayload, LightmapHeader,
};
use postretro_level_format::shadowmask_atlas::SHADOWMASK_GROUP_COUNT;
use postretro_render_cpu::lightmap_pool::{BlockPlacement, PoolCopy};

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

/// How a level's pool stores its texels: the irradiance format and the
/// direction texel scale, both from the id-22 header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PoolFormat {
    pub(crate) irradiance: wgpu::TextureFormat,
    pub(crate) direction_scale: u32,
}

impl PoolFormat {
    pub(crate) fn from_header(header: &LightmapHeader) -> Self {
        Self {
            irradiance: match header.irradiance_format {
                IRRADIANCE_FORMAT_BC6H => wgpu::TextureFormat::Bc6hRgbUfloat,
                // `IRRADIANCE_FORMAT_RGBA16F`, the only other tag the header
                // accepts.
                _ => wgpu::TextureFormat::Rgba16Float,
            },
            direction_scale: header.direction_texel_scale.max(1),
        }
    }

    /// Texel edge of one addressable unit and its bytes: a BC block for
    /// BC6H, a texel for Rgba16Float.
    fn irradiance_unit(self) -> (u32, u32) {
        match self.irradiance {
            wgpu::TextureFormat::Bc6hRgbUfloat => (BC_BLOCK_EDGE, BC_BLOCK_BYTES),
            _ => (1, IRRADIANCE_TEXEL_BYTES as u32),
        }
    }
}

/// One pool texture of a set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PoolPlane {
    Irradiance,
    Direction,
    Shadowmask,
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

/// Irradiance: one edge² layer per pool layer, in the stored format. BC6H and
/// Rgba16Float both bind as `Float { filterable: true }` through the linear
/// sampler; the shader reads `.rgb` either way.
pub(crate) fn irradiance_descriptor(
    format: PoolFormat,
    layers: u32,
) -> wgpu::TextureDescriptor<'static> {
    pool_descriptor(
        "Lightmap Irradiance Pool",
        LIGHTMAP_POOL_LAYER_EDGE,
        LIGHTMAP_POOL_LAYER_EDGE,
        layers,
        format.irradiance,
    )
}

/// Direction: the same layers at `edge / scale`, so a block's direction
/// offset is its irradiance offset over the scale and one normalized UV
/// addresses both pools.
pub(crate) fn direction_descriptor(
    format: PoolFormat,
    layers: u32,
) -> wgpu::TextureDescriptor<'static> {
    let edge = LIGHTMAP_POOL_LAYER_EDGE / format.direction_scale;
    pool_descriptor(
        "Lightmap Direction Pool",
        edge,
        edge,
        layers,
        wgpu::TextureFormat::Rg8Unorm,
    )
}

/// Shadowmask: BC5 `.rg`, two mask groups side by side per layer. Group A of
/// a block at `(x, y)` lives at `(x, y)`, group B at `(edge + x, y)`.
pub(crate) fn shadowmask_descriptor(layers: u32) -> wgpu::TextureDescriptor<'static> {
    pool_descriptor(
        "Shadowmask Pool",
        LIGHTMAP_POOL_LAYER_EDGE * SHADOWMASK_GROUP_COUNT,
        LIGHTMAP_POOL_LAYER_EDGE,
        layers,
        wgpu::TextureFormat::Bc5RgUnorm,
    )
}

#[cfg(test)]
pub(crate) fn irradiance_pool_descriptor(
    plan: &StaticPoolPlan,
) -> wgpu::TextureDescriptor<'static> {
    irradiance_descriptor(PoolFormat::from_header(&plan.header), plan.pool.layer_count)
}

#[cfg(test)]
pub(crate) fn direction_pool_descriptor(plan: &StaticPoolPlan) -> wgpu::TextureDescriptor<'static> {
    direction_descriptor(PoolFormat::from_header(&plan.header), plan.pool.layer_count)
}

#[cfg(test)]
pub(crate) fn shadowmask_pool_descriptor(
    plan: &StaticPoolPlan,
) -> wgpu::TextureDescriptor<'static> {
    shadowmask_descriptor(plan.pool.layer_count)
}

/// Create one pool texture set of `layers` array layers. Unwritten texels
/// stay wgpu's zero initialization.
pub(crate) fn create_pool_textures(
    device: &wgpu::Device,
    format: PoolFormat,
    layers: u32,
    with_shadowmask: bool,
) -> StaticPoolTextures {
    StaticPoolTextures {
        irradiance: device.create_texture(&irradiance_descriptor(format, layers)),
        direction: device.create_texture(&direction_descriptor(format, layers)),
        shadowmask: with_shadowmask.then(|| device.create_texture(&shadowmask_descriptor(layers))),
    }
}

/// One region write of a block's stored bytes into a pool texture, in the
/// row layout `Queue::write_texture` takes.
pub(crate) struct RegionWrite<'a> {
    pub(crate) plane: PoolPlane,
    pub(crate) origin: wgpu::Origin3d,
    pub(crate) extent: wgpu::Extent3d,
    pub(crate) bytes_per_row: u32,
    pub(crate) rows: u32,
    pub(crate) data: &'a [u8],
}

/// Row layout of a region write: bytes per row and rows, for texel-addressed
/// (`block_edge` 1) or BC (`block_edge` 4) formats.
fn region_layout(width: u32, height: u32, block_edge: u32, unit_bytes: u32) -> (u32, u32) {
    (
        width.div_ceil(block_edge) * unit_bytes,
        height.div_ceil(block_edge),
    )
}

fn region<'a>(
    plane: PoolPlane,
    (layer, x, y): (u32, u32, u32),
    (width, height): (u32, u32),
    (block_edge, unit_bytes): (u32, u32),
    data: &'a [u8],
) -> RegionWrite<'a> {
    let (bytes_per_row, rows) = region_layout(width, height, block_edge, unit_bytes);
    RegionWrite {
        plane,
        origin: wgpu::Origin3d { x, y, z: layer },
        extent: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        bytes_per_row,
        rows,
        data,
    }
}

/// The region writes that put one block's pair at `placement`: irradiance at
/// the block's extent, Rg8 direction at `extent / scale`, and — when the pool
/// keeps a shadowmask — BC5 group A at `x` and group B at `edge + x`. The
/// caller has matched every blob's length to the block's extent.
pub(crate) fn block_region_writes<'a>(
    format: PoolFormat,
    placement: BlockPlacement,
    (width, height): (u32, u32),
    payload: &'a LightmapBlockPayload,
    with_shadowmask: bool,
) -> impl Iterator<Item = RegionWrite<'a>> {
    let scale = format.direction_scale;
    let at = (placement.layer, placement.x, placement.y);
    let lightmap = [
        region(
            PoolPlane::Irradiance,
            at,
            (width, height),
            format.irradiance_unit(),
            &payload.irradiance,
        ),
        region(
            PoolPlane::Direction,
            (placement.layer, placement.x / scale, placement.y / scale),
            (width / scale, height / scale),
            (1, DIRECTION_TEXEL_BYTES as u32),
            &payload.direction,
        ),
    ];
    let groups = payload
        .shadowmask
        .as_ref()
        .filter(|_| with_shadowmask)
        .into_iter()
        .flat_map(move |[group_a, group_b]| {
            [
                (placement.x, group_a),
                (LIGHTMAP_POOL_LAYER_EDGE + placement.x, group_b),
            ]
            .map(|(group_x, group)| {
                region(
                    PoolPlane::Shadowmask,
                    (placement.layer, group_x, placement.y),
                    (width, height),
                    (BC_BLOCK_EDGE, BC_BLOCK_BYTES),
                    group,
                )
            })
        });
    lightmap.into_iter().chain(groups)
}

/// One same-texture region copy between two array layers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RegionCopy {
    pub(crate) plane: PoolPlane,
    pub(crate) src: wgpu::Origin3d,
    pub(crate) dst: wgpu::Origin3d,
    pub(crate) extent: wgpu::Extent3d,
}

/// The per-texture copies of one planned block move. Plan coordinates are
/// irradiance texels: the direction copy divides them by the scale, and the
/// shadowmask copies both group halves.
pub(crate) fn block_copy_regions(
    format: PoolFormat,
    copy: PoolCopy,
    with_shadowmask: bool,
) -> impl Iterator<Item = RegionCopy> {
    let scale = format.direction_scale;
    let origin = |p: BlockPlacement, dx: u32, divide: u32| wgpu::Origin3d {
        x: dx + p.x / divide,
        y: p.y / divide,
        z: p.layer,
    };
    let extent = |divide: u32| wgpu::Extent3d {
        width: copy.width / divide,
        height: copy.height / divide,
        depth_or_array_layers: 1,
    };
    let plane = |plane, dx, divide| RegionCopy {
        plane,
        src: origin(copy.src, dx, divide),
        dst: origin(copy.dst, dx, divide),
        extent: extent(divide),
    };
    let lightmap = [
        plane(PoolPlane::Irradiance, 0, 1),
        plane(PoolPlane::Direction, 0, scale),
    ];
    let groups = with_shadowmask
        .then(|| {
            [
                plane(PoolPlane::Shadowmask, 0, 1),
                plane(PoolPlane::Shadowmask, LIGHTMAP_POOL_LAYER_EDGE, 1),
            ]
        })
        .into_iter()
        .flatten();
    lightmap.into_iter().chain(groups)
}

/// Create the pool textures and write every block at its placement, in its
/// stored format with no decode. Runs once per level install; the payloads
/// drop after.
pub(crate) fn upload_static_pool(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    plan: &StaticPoolPlan,
    payloads: &[LightmapBlockPayload],
) -> StaticPoolTextures {
    let format = PoolFormat::from_header(&plan.header);
    let textures =
        create_pool_textures(device, format, plan.pool.layer_count, plan.with_shadowmask);
    for ((&extent, &placement), payload) in
        plan.extents.iter().zip(&plan.pool.placements).zip(payloads)
    {
        for write in block_region_writes(format, placement, extent, payload, plan.with_shadowmask) {
            let texture = match write.plane {
                PoolPlane::Irradiance => &textures.irradiance,
                PoolPlane::Direction => &textures.direction,
                PoolPlane::Shadowmask => textures
                    .shadowmask
                    .as_ref()
                    .expect("shadowmask writes only when the pool keeps one"),
            };
            debug_assert_eq!(
                write.data.len() as u64,
                u64::from(write.bytes_per_row * write.rows)
            );
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture,
                    mip_level: 0,
                    origin: write.origin,
                    aspect: wgpu::TextureAspect::All,
                },
                write.data,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(write.bytes_per_row),
                    rows_per_image: Some(write.rows),
                },
                write.extent,
            );
        }
    }
    textures
}
