// Static lightmap GPU resources: the all-resident or streamed cell-block pool
// (or the neutral placeholders), the group-4 bind group, and the group-6
// vertex block table.
// See: context/lib/rendering_pipeline.md §4 (Lightmap cell-block residency)

mod bindings;
mod plan;
mod pool;
#[cfg(test)]
mod pool_sample_test;
mod stream;
#[cfg(test)]
pub(crate) mod test_fixtures;
#[cfg(test)]
mod tests;
mod upload;

use postretro_level_format::SectionId;
use postretro_level_format::lightmap::LightmapBlockIndex;
use postretro_level_format::shadowmask_atlas::ShadowmaskBlockIndex;
use postretro_level_loader::{LightmapDrainBatch, LightmapDrainOutcome};
use postretro_render_cpu::lightmap_pool::{block_table_bytes, placeholder_block_table};
use wgpu::util::DeviceExt;

use crate::render::residency::{ResidencyAllocation, ResidencyAllocationState, texture_row};
use crate::render::{LIGHTMAP_SHADOWMASK, LIGHTMAP_STATIC_DIRECTION, LIGHTMAP_STATIC_IRRADIANCE};

use bindings::{ANIMATED_BLOCK_TABLE_BYTES, BIND_BLOCK_TABLE, Group4Bindings};
pub(crate) use bindings::{
    animated_block_table_bytes, bind_group_layout_entries, block_table_bind_group_layout_entries,
    filtering_sampler_descriptor,
};
#[cfg(test)]
pub(crate) use plan::StaticPoolPlan;
pub(crate) use plan::{StaticPool, StreamingPoolPlan, plan_static_pool, plan_streaming_pool};
pub(crate) use pool::upload_static_pool;
use stream::{DrainEffects, LightmapStreamState};
pub use stream::{LightmapResidencyDrainError, LightmapStreamCounters};
pub(crate) use upload::upload_placeholder_shadowmask;
pub use upload::{atlas_format_filterable, bc6h_irradiance_filterable};
use upload::{upload_placeholder_direction, upload_placeholder_irradiance};

/// Static lightmap GPU resources for one level install.
///
/// Always allocated. A level with blocks binds the all-resident or streamed
/// pool; a level without them (no id 22, zero blocks, or a pool the device
/// rejected) binds the 1×1 neutral placeholders, so the shader path is
/// identical in every case and the bind group layouts stay independent of
/// map content.
///
/// The bind group layouts are built separately (`bind_group_layout`,
/// `block_table_bind_group_layout`) because the pipeline layout needs them
/// before any level exists.
pub struct LightmapResources {
    pub bind_group: wgpu::BindGroup,
    /// Group 6: the vertex-stage block table.
    pub block_table_bind_group: wgpu::BindGroup,
    /// Whether the shadowmask pool is bound (false = 2x1x1 fully-visible
    /// placeholder). Rejected or absent shadowmask data uses this all-visible
    /// fallback so static specular remains fully lit.
    pub shadowmask_present: bool,
    /// Static irradiance, static direction and shadowmask meter rows, read
    /// from the textures this set actually binds.
    pub residency: [ResidencyAllocation; 3],
    group4: Group4Bindings,
    /// The streamed pool, when the level streams its blocks.
    stream: Option<LightmapStreamState>,
    /// The all-visible placeholder a streamed pool without id 42 keeps bound
    /// across growth rebinds.
    streamed_shadowmask_placeholder: Option<wgpu::Texture>,
    /// Highest drain generation this set or an earlier level accepted.
    generation_high_water: u64,
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
    /// `pool` is the plan `plan_static_pool` or `plan_streaming_pool` made,
    /// or `StaticPool::Absent` before any level; the animated atlas was built
    /// against the same plan. The all-resident
    /// upload owns the GPU-only payloads and drops them once the pool holds
    /// their texels; the level keeps only the block indices. A streamed pool
    /// starts empty and rejects drain generations at or below
    /// `generation_floor`, the previous set's
    /// [`generation_high_water`](Self::generation_high_water).
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
        generation_floor: u64,
    ) -> Self {
        debug_assert_eq!(animated_block_table.len(), ANIMATED_BLOCK_TABLE_BYTES);
        let group4 = Group4Bindings {
            layout: bind_group_layout.clone(),
            // Nearest sampler for the octahedral direction texture (binding
            // 1): linear interpolation of octahedral-encoded unit vectors
            // does not commute with slerp, so direction must stay nearest.
            sampler: device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("Lightmap Sampler (Nearest)"),
                address_mode_u: wgpu::AddressMode::ClampToEdge,
                address_mode_v: wgpu::AddressMode::ClampToEdge,
                address_mode_w: wgpu::AddressMode::ClampToEdge,
                mag_filter: wgpu::FilterMode::Nearest,
                min_filter: wgpu::FilterMode::Nearest,
                mipmap_filter: wgpu::MipmapFilterMode::Nearest,
                ..Default::default()
            }),
            // Linear sampler for the irradiance pool, the animated atlas and
            // the BC5 shadowmask pool. Turns baked penumbra ramps into continuous
            // gradients under magnification. Always used — Rgba16Float
            // linear-filterability is a hard runtime requirement;
            // non-filterable adapters are rejected at init (see
            // `atlas_format_filterable`). See rendering_pipeline.md §4.
            filtering_sampler: device.create_sampler(&filtering_sampler_descriptor()),
            animated_atlas_view: animated_atlas_view.clone(),
            animated_direction_view: animated_direction_view.clone(),
            animated_block_table: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Animated LM Block Table"),
                contents: animated_block_table,
                usage: wgpu::BufferUsages::UNIFORM,
            }),
        };

        let mut stream = None;
        let mut streamed_shadowmask_placeholder = None;
        let (irradiance_tex, direction_tex, shadowmask_tex, table_buffer) = match pool {
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
                    table_buffer(device, &block_table_bytes(&plan.extents, &placements)),
                )
            }
            StaticPool::Streaming(plan) => {
                let state = LightmapStreamState::new(device, plan, generation_floor);
                let textures = state.textures();
                let shadowmask = match &textures.shadowmask {
                    Some(shadowmask) => shadowmask.clone(),
                    None => streamed_shadowmask_placeholder
                        .insert(upload_placeholder_shadowmask(device, queue))
                        .clone(),
                };
                let bound = (
                    textures.irradiance.clone(),
                    textures.direction.clone(),
                    shadowmask,
                    state.table().clone(),
                );
                stream = Some(state);
                bound
            }
            StaticPool::Absent | StaticPool::Empty | StaticPool::Rejected => (
                upload_placeholder_irradiance(device, queue),
                upload_placeholder_direction(device, queue),
                upload_placeholder_shadowmask(device, queue),
                table_buffer(device, &placeholder_block_table()),
            ),
        };
        // The pool now holds every all-resident texel; the payloads end here.
        drop(payloads);
        let shadowmask_present = match pool {
            StaticPool::Blocks(plan) => plan.with_shadowmask,
            StaticPool::Streaming(plan) => plan.with_shadowmask,
            _ => false,
        };

        let lightmap_state = match pool {
            StaticPool::Blocks(_) | StaticPool::Streaming(_) => ResidencyAllocationState::Data,
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

        let bind_group = group4.bind(device, &irradiance_tex, &direction_tex, &shadowmask_tex);
        let block_table_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Lightmap Block Table Bind Group"),
            layout: block_table_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: BIND_BLOCK_TABLE,
                resource: table_buffer.as_entire_binding(),
            }],
        });

        Self {
            bind_group,
            block_table_bind_group,
            shadowmask_present,
            residency,
            group4,
            generation_high_water: stream
                .as_ref()
                .map_or(generation_floor, LightmapStreamState::generation_high_water),
            stream,
            streamed_shadowmask_placeholder,
        }
    }

    /// The generation floor the next level install hands its streamed pool.
    pub(crate) fn generation_high_water(&self) -> u64 {
        self.stream
            .as_ref()
            .map_or(self.generation_high_water, |stream| {
                stream.generation_high_water()
            })
    }

    /// Counters of the streamed pool, `None` when the level does not stream.
    pub(crate) fn stream_counters(&self) -> Option<LightmapStreamCounters> {
        self.stream.as_ref().map(LightmapStreamState::counters)
    }

    /// Bytes of a grown-out streamed generation awaiting release.
    pub(crate) fn retiring_bytes(&self) -> u64 {
        self.stream
            .as_ref()
            .map_or(0, LightmapStreamState::retiring_bytes)
    }

    /// Execute one streamed drain. On growth, group 4 rebinds the new
    /// generation and the static meter rows follow it. Returns whether the
    /// byte meter changed.
    pub(crate) fn drain_streaming(
        &mut self,
        device: &wgpu::Device,
        queue: &crate::render::uploads::UploadQueue,
        batch: LightmapDrainBatch,
    ) -> Result<(LightmapDrainOutcome, bool), LightmapResidencyDrainError> {
        let stream = self
            .stream
            .as_mut()
            .ok_or(LightmapResidencyDrainError::NotStreaming)?;
        let (
            outcome,
            DrainEffects {
                pool_replaced,
                meter_changed,
            },
        ) = stream.drain(device, queue, batch)?;
        if pool_replaced {
            let textures = stream.textures();
            let [irradiance_row, direction_row, shadowmask_row] = &mut self.residency;
            refresh_row(irradiance_row, &textures.irradiance);
            refresh_row(direction_row, &textures.direction);
            if let Some(shadowmask) = &textures.shadowmask {
                refresh_row(shadowmask_row, shadowmask);
            }
            // Without id 42 the install's placeholder stays bound.
            let shadowmask = textures
                .shadowmask
                .as_ref()
                .or(self.streamed_shadowmask_placeholder.as_ref())
                .expect("a streamed pool without a shadowmask keeps its placeholder");
            self.bind_group = self.group4.bind(
                device,
                &textures.irradiance,
                &textures.direction,
                shadowmask,
            );
        }
        Ok((outcome, meter_changed))
    }

    #[cfg(test)]
    pub(crate) fn stream_state(&self) -> Option<&LightmapStreamState> {
        self.stream.as_ref()
    }
}

fn table_buffer(device: &wgpu::Device, table: &[u8]) -> wgpu::Buffer {
    // `COPY_DST` leaves room for per-entry residency writes; only a streamed
    // pool's drains write it after install.
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("Lightmap Block Table"),
        contents: table,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
    })
}

/// Re-read a meter row's bytes and shape from the texture now bound in its
/// place; its name, sources and state stand.
fn refresh_row(row: &mut ResidencyAllocation, texture: &wgpu::Texture) {
    let fresh = texture_row(row.name, texture, &[], row.state);
    row.bytes = fresh.bytes;
    row.shape = fresh.shape;
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
