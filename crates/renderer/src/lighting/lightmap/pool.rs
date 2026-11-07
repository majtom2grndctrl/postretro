// Lightmap and mask pool textures: irradiance and direction in the lightmap
// pool, BC5 planes in the mask pool, each a `texture_2d_array` of
// `LIGHTMAP_POOL_LAYER_EDGE`² layers, and where a cell block's texels land in
// each (region writes, block copies).
// See: context/lib/rendering_pipeline.md §4 (Lightmap cell-block residency)

use crate::render::gpu_create::GpuCreate;
use postretro_level_format::lightmap::{
    DIRECTION_TEXEL_BYTES, IRRADIANCE_FORMAT_BC6H, IRRADIANCE_TEXEL_BYTES,
    LIGHTMAP_POOL_LAYER_EDGE, LightmapBlockPayload, LightmapHeader,
};
use postretro_render_cpu::lightmap_pool::{BlockPlacement, PoolCopy, PoolKind};

use postretro_render_cpu::lightmap_pool::StaticPoolPlan;

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
    /// `None` when the level has no mask pool, or a mask pool with no plane to
    /// place: the placeholder binds in its place.
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

impl PoolPlane {
    /// The pool whose generation holds this texture.
    pub(crate) fn pool(self) -> PoolKind {
        match self {
            Self::Irradiance | Self::Direction => PoolKind::Lightmap,
            Self::Shadowmask => PoolKind::Mask,
        }
    }
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

/// The mask pool: BC5 `.rg`, each present plane its own placement.
pub(crate) fn shadowmask_descriptor(layers: u32) -> wgpu::TextureDescriptor<'static> {
    pool_descriptor(
        "Shadowmask Pool",
        LIGHTMAP_POOL_LAYER_EDGE,
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
    shadowmask_descriptor(plan.masks.as_ref().map_or(0, |masks| masks.layer_count))
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

/// The region writes that put one block's pair in the pools: irradiance at the
/// block's extent and Rg8 direction at `extent / scale` at `placement`, and
/// each stored BC5 plane at its own place in the mask pool. A plane with no
/// place writes nothing. The caller has matched every blob's length to the
/// block's extent.
pub(crate) fn block_region_writes<'a>(
    format: PoolFormat,
    placement: BlockPlacement,
    planes: [Option<BlockPlacement>; 2],
    (width, height): (u32, u32),
    payload: &'a LightmapBlockPayload,
) -> impl Iterator<Item = RegionWrite<'a>> {
    let scale = format.direction_scale;
    let lightmap = [
        region(
            PoolPlane::Irradiance,
            (placement.layer, placement.x, placement.y),
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
    let masks = payload.shadowmask.iter().flat_map(move |groups| {
        planes
            .into_iter()
            .zip(groups)
            .filter_map(move |(plane, group)| {
                plane.map(|at| {
                    region(
                        PoolPlane::Shadowmask,
                        (at.layer, at.x, at.y),
                        (width, height),
                        (BC_BLOCK_EDGE, BC_BLOCK_BYTES),
                        group,
                    )
                })
            })
    });
    lightmap.into_iter().chain(masks)
}

/// One same-texture region copy between two array layers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RegionCopy {
    pub(crate) plane: PoolPlane,
    pub(crate) src: wgpu::Origin3d,
    pub(crate) dst: wgpu::Origin3d,
    pub(crate) extent: wgpu::Extent3d,
}

/// The per-texture copies of one planned move in `pool`. Plan coordinates are
/// that pool's texels: a lightmap move also copies direction at the scale, and
/// a mask move copies its one plane.
pub(crate) fn block_copy_regions(
    pool: PoolKind,
    format: PoolFormat,
    copy: PoolCopy,
) -> impl Iterator<Item = RegionCopy> {
    let origin = |p: BlockPlacement, divide: u32| wgpu::Origin3d {
        x: p.x / divide,
        y: p.y / divide,
        z: p.layer,
    };
    let region = |plane, divide: u32| RegionCopy {
        plane,
        src: origin(copy.src, divide),
        dst: origin(copy.dst, divide),
        extent: wgpu::Extent3d {
            width: copy.width / divide,
            height: copy.height / divide,
            depth_or_array_layers: 1,
        },
    };
    let regions = match pool {
        PoolKind::Lightmap => [
            Some(region(PoolPlane::Irradiance, 1)),
            Some(region(PoolPlane::Direction, format.direction_scale)),
        ],
        PoolKind::Mask => [Some(region(PoolPlane::Shadowmask, 1)), None],
    };
    regions.into_iter().flatten()
}

/// Create the pool textures and write every block at its placements, in its
/// stored format with no decode. Unwritten texels stay wgpu's zero
/// initialization. Runs once per level install; the payloads drop after.
pub(crate) fn upload_static_pool(
    device: &wgpu::Device,
    queue: &dyn crate::render::queue_writes::QueueWrites,
    plan: &StaticPoolPlan,
    payloads: &[LightmapBlockPayload],
) -> StaticPoolTextures {
    let format = PoolFormat::from_header(&plan.header);
    let layers = plan.pool.layer_count;
    let textures = StaticPoolTextures {
        irradiance: device.new_texture(&irradiance_descriptor(format, layers)),
        direction: device.new_texture(&direction_descriptor(format, layers)),
        shadowmask: plan
            .masks
            .as_ref()
            .filter(|masks| masks.layer_count > 0)
            .map(|masks| device.new_texture(&shadowmask_descriptor(masks.layer_count))),
    };
    for (block, ((&extent, &placement), payload)) in plan
        .extents
        .iter()
        .zip(&plan.pool.placements)
        .zip(payloads)
        .enumerate()
    {
        let planes = plan
            .masks
            .as_ref()
            .map_or([None; 2], |masks| masks.planes[block]);
        for write in block_region_writes(format, placement, planes, extent, payload) {
            let texture = match write.plane {
                PoolPlane::Irradiance => &textures.irradiance,
                PoolPlane::Direction => &textures.direction,
                PoolPlane::Shadowmask => textures
                    .shadowmask
                    .as_ref()
                    .expect("a plane with a place has a mask pool texture"),
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
