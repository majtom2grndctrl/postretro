// Static lightmap GPU resources: the all-resident cell-block pool (or the
// neutral placeholders), the group-4 bind group, and the group-6 vertex block
// table.
// See: context/lib/rendering_pipeline.md §4 (Lightmap cell-block residency)

mod bindings;
mod plan;
mod pool;
#[cfg(test)]
mod pool_sample_test;
#[cfg(test)]
pub(crate) mod test_fixtures;
#[cfg(test)]
mod tests;
mod upload;

use postretro_level_format::SectionId;
use postretro_level_format::lightmap::LightmapBlockIndex;
use postretro_level_format::shadowmask_atlas::ShadowmaskBlockIndex;
use postretro_render_cpu::lightmap_pool::{
    BLOCK_TABLE_ENTRY_BYTES, block_table_bytes, placeholder_block_table,
};
use wgpu::util::DeviceExt;

use crate::render::residency::{ResidencyAllocation, ResidencyAllocationState, texture_row};
use crate::render::{LIGHTMAP_SHADOWMASK, LIGHTMAP_STATIC_DIRECTION, LIGHTMAP_STATIC_IRRADIANCE};

use bindings::{
    ANIMATED_BLOCK_TABLE_BYTES, BIND_ANIMATED_ATLAS, BIND_ANIMATED_BLOCK_TABLE,
    BIND_ANIMATED_DIRECTION, BIND_BLOCK_TABLE, BIND_DIRECTION, BIND_FILTERING_SAMPLER,
    BIND_IRRADIANCE, BIND_SAMPLER, BIND_SHADOWMASK_ATLAS,
};
pub(crate) use bindings::{
    animated_block_table_bytes, bind_group_layout_entries, block_table_bind_group_layout_entries,
    filtering_sampler_descriptor,
};
#[cfg(test)]
pub(crate) use plan::StaticPoolPlan;
pub(crate) use plan::{StaticPool, plan_static_pool};
pub(crate) use pool::upload_static_pool;
pub(crate) use upload::upload_placeholder_shadowmask;
pub use upload::{atlas_format_filterable, bc6h_irradiance_filterable};
use upload::{upload_placeholder_direction, upload_placeholder_irradiance};

/// Static lightmap GPU resources for one level install.
///
/// Always allocated. A level with blocks binds the all-resident pool; a level
/// without them (no id 22, zero blocks, or a pool the device rejected) binds
/// the 1×1 neutral placeholders, so the shader path is identical in every
/// case and the bind group layouts stay independent of map content.
///
/// The bind group layouts are built separately (`bind_group_layout`,
/// `block_table_bind_group_layout`) because the pipeline layout needs them
/// before any level exists.
pub struct LightmapResources {
    pub bind_group: wgpu::BindGroup,
    /// Group 6: the vertex-stage block table.
    pub block_table_bind_group: wgpu::BindGroup,
    /// Whether the cell-block pool is bound (false = placeholder mode).
    #[allow(dead_code)]
    pub present: bool,
    /// Whether the shadowmask pool is bound (false = 2x1x1 fully-visible
    /// placeholder). Rejected or absent shadowmask data uses this all-visible
    /// fallback so static specular remains fully lit.
    pub shadowmask_present: bool,
    /// Pool layers bound (0 in placeholder mode).
    #[allow(dead_code)]
    pub pool_layers: u32,
    /// Block-table entries written since this resource set was built. Install
    /// writes every entry once; no frame writes any.
    block_table_entries_written: u64,
    /// Static irradiance, static direction and shadowmask meter rows, read
    /// from the textures this set actually binds.
    pub residency: [ResidencyAllocation; 3],
}

/// Build the lightmap bind group layout. Callable before resources exist so
/// the pipeline layout can be assembled up front.
pub fn bind_group_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("Lightmap Bind Group Layout"),
        entries: &bind_group_layout_entries(),
    })
}

/// Build the group-6 block-table layout. Callable before resources exist.
pub fn block_table_bind_group_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("Lightmap Block Table Bind Group Layout"),
        entries: &block_table_bind_group_layout_entries(),
    })
}

impl LightmapResources {
    /// Builds one coherent group-4/group-6 resource set. Its independently
    /// constructed static, animated, and layout inputs are intentionally kept
    /// explicit at this renderer boundary rather than wrapped in a one-use
    /// parameter type.
    ///
    /// `pool` is the plan `plan_static_pool` made from `index`, `shadowmask`
    /// and the payloads; the animated atlas was built against the same plan.
    /// The upload owns the GPU-only payloads and drops them once the pool
    /// holds their texels; the level keeps only the block indices.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        index: Option<&LightmapBlockIndex>,
        shadowmask: Option<&ShadowmaskBlockIndex>,
        pool: &StaticPool,
        payloads: postretro_level_loader::GpuLightingPayloads,
        bind_group_layout: &wgpu::BindGroupLayout,
        block_table_bind_group_layout: &wgpu::BindGroupLayout,
        animated_atlas_view: &wgpu::TextureView,
        animated_direction_view: &wgpu::TextureView,
        animated_block_table: &[u8],
    ) -> Self {
        // Nearest sampler for the octahedral direction texture (binding 1):
        // linear interpolation of octahedral-encoded unit vectors does not
        // commute with slerp, so direction must stay nearest.
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Lightmap Sampler (Nearest)"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });

        // Linear sampler for the irradiance + animated atlases and the BC5
        // shadowmask. Turns baked penumbra ramps into continuous gradients
        // under magnification. Always used — Rgba16Float linear-filterability
        // is a hard runtime requirement; non-filterable adapters are rejected
        // at init (see `atlas_format_filterable`). See rendering_pipeline.md §4.
        let filtering_sampler = device.create_sampler(&filtering_sampler_descriptor());

        let (irradiance_tex, direction_tex, shadowmask_tex, table) = match pool {
            StaticPool::Blocks(plan) => {
                let textures = upload_static_pool(device, queue, plan, &payloads.blocks);
                let placements: Vec<_> = plan.pool.placements.iter().copied().map(Some).collect();
                log::info!(
                    "[Renderer] Lightmap pool: {} cell block(s) on {} layer(s) of {}², \
                     shadowmask {}",
                    plan.extents.len(),
                    plan.pool.layer_count,
                    postretro_level_format::lightmap::LIGHTMAP_POOL_LAYER_EDGE,
                    if textures.shadowmask.is_some() {
                        "resident"
                    } else {
                        "placeholder"
                    },
                );
                (
                    textures.irradiance,
                    textures.direction,
                    textures
                        .shadowmask
                        .unwrap_or_else(|| upload_placeholder_shadowmask(device, queue)),
                    block_table_bytes(&plan.extents, &placements),
                )
            }
            StaticPool::Absent | StaticPool::Empty | StaticPool::Rejected => (
                upload_placeholder_irradiance(device, queue),
                upload_placeholder_direction(device, queue),
                upload_placeholder_shadowmask(device, queue),
                placeholder_block_table(),
            ),
        };
        // The pool now holds every texel; the payloads end here.
        drop(payloads);
        let (present, pool_layers, shadowmask_present) = match pool {
            StaticPool::Blocks(plan) => (true, plan.pool.layer_count, plan.with_shadowmask),
            _ => (false, 0, false),
        };

        let lightmap_state = match pool {
            StaticPool::Blocks(_) => ResidencyAllocationState::Data,
            StaticPool::Rejected => ResidencyAllocationState::Fallback,
            StaticPool::Absent | StaticPool::Empty => ResidencyAllocationState::Dummy,
        };
        let shadowmask_state = match (shadowmask.is_some(), shadowmask_present, pool) {
            (_, true, _) => ResidencyAllocationState::Data,
            (true, false, StaticPool::Empty) | (false, _, _) => ResidencyAllocationState::Dummy,
            (true, false, _) => ResidencyAllocationState::Fallback,
        };
        let lightmap_sources = section_source(SectionId::Lightmap, index.is_some());
        let shadowmask_sources = section_source(SectionId::ShadowmaskAtlas, shadowmask.is_some());
        let residency = [
            texture_row(
                LIGHTMAP_STATIC_IRRADIANCE,
                &irradiance_tex,
                &lightmap_sources,
                lightmap_state,
            ),
            texture_row(
                LIGHTMAP_STATIC_DIRECTION,
                &direction_tex,
                &lightmap_sources,
                lightmap_state,
            ),
            texture_row(
                LIGHTMAP_SHADOWMASK,
                &shadowmask_tex,
                &shadowmask_sources,
                shadowmask_state,
            ),
        ];

        // Group-4 bindings 0/1/6 declare `D2Array`; pinning the view dimension
        // keeps a one-layer pool or placeholder aligned with the BGL.
        let array_view = |texture: &wgpu::Texture| {
            texture.create_view(&wgpu::TextureViewDescriptor {
                dimension: Some(wgpu::TextureViewDimension::D2Array),
                ..Default::default()
            })
        };
        let irr_view = array_view(&irradiance_tex);
        let dir_view = array_view(&direction_tex);
        let shadowmask_view = array_view(&shadowmask_tex);
        debug_assert_eq!(animated_block_table.len(), ANIMATED_BLOCK_TABLE_BYTES);
        let animated_block_table_buffer =
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Animated LM Block Table"),
                contents: animated_block_table,
                usage: wgpu::BufferUsages::UNIFORM,
            });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Lightmap Bind Group"),
            layout: bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: BIND_IRRADIANCE,
                    resource: wgpu::BindingResource::TextureView(&irr_view),
                },
                wgpu::BindGroupEntry {
                    binding: BIND_DIRECTION,
                    resource: wgpu::BindingResource::TextureView(&dir_view),
                },
                wgpu::BindGroupEntry {
                    binding: BIND_SAMPLER,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
                wgpu::BindGroupEntry {
                    binding: BIND_ANIMATED_ATLAS,
                    resource: wgpu::BindingResource::TextureView(animated_atlas_view),
                },
                wgpu::BindGroupEntry {
                    binding: BIND_FILTERING_SAMPLER,
                    resource: wgpu::BindingResource::Sampler(&filtering_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: BIND_ANIMATED_DIRECTION,
                    resource: wgpu::BindingResource::TextureView(animated_direction_view),
                },
                wgpu::BindGroupEntry {
                    binding: BIND_SHADOWMASK_ATLAS,
                    resource: wgpu::BindingResource::TextureView(&shadowmask_view),
                },
                wgpu::BindGroupEntry {
                    binding: BIND_ANIMATED_BLOCK_TABLE,
                    resource: animated_block_table_buffer.as_entire_binding(),
                },
            ],
        });

        // Written once here. `COPY_DST` leaves room for per-entry residency
        // writes; no frame writes the table.
        let block_table_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Lightmap Block Table"),
            contents: &table,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        });
        let block_table_entries_written = (table.len() / BLOCK_TABLE_ENTRY_BYTES) as u64;
        let block_table_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Lightmap Block Table Bind Group"),
            layout: block_table_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: BIND_BLOCK_TABLE,
                resource: block_table_buffer.as_entire_binding(),
            }],
        });

        Self {
            bind_group,
            block_table_bind_group,
            present,
            shadowmask_present,
            pool_layers,
            block_table_entries_written,
            residency,
        }
    }

    /// Block-table entries written since install (AC 15's write counter).
    #[allow(dead_code)]
    pub(crate) fn block_table_entries_written(&self) -> u64 {
        self.block_table_entries_written
    }
}

/// A row cites its section whenever the level supplied it, as the SH ledger's
/// rows do — including a Fallback row whose section was rejected.
fn section_source(section: SectionId, section_present: bool) -> Vec<u16> {
    if section_present {
        vec![section as u16]
    } else {
        Vec::new()
    }
}
