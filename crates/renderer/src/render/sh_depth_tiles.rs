// Per-direction probe depth tiles (PRL id 59) on the GPU: the tile texture at
// shared SH binding 18, its grid-info words, and the load's reference table.
// See: context/lib/rendering_pipeline.md §4, §10

use postretro_level_format::probe_depth_tiles::ProbeDepthTilesSection;
use postretro_render_cpu::sh_compose::u16_slice_to_bytes;
use postretro_render_cpu::sh_depth_tiles::{
    DepthTileGridWords, ProbeDepthTileReferences, depth_tile_texture_plan,
};
use postretro_render_cpu::sh_volume::BIND_SH_DEPTH_TILES;

use super::gpu_create::GpuCreate;
use super::residency::{ResidencyAllocationState, source_ids};
use super::sh_allocation::depth_tile_allocation;
use super::sh_residency::ShAllocationLedger;

/// One load's depth tiles: owned by its SH volume resources, so a reference
/// rewritten on one load never reaches another. Without tiles (id 59 absent,
/// zero tiles, or no usable SH) the binding holds a 1×1 dummy, the table is
/// empty and the grid words are zero, so every texel packs as before id 59.
pub(super) struct ShDepthTiles {
    references: ProbeDepthTileReferences,
    grid_words: DepthTileGridWords,
    view: wgpu::TextureView,
}

impl ShDepthTiles {
    /// Upload `section`'s tiles, or the dummy when `section` is `None`. Only
    /// a real tile texture enters the ledger; the dummy prices at 0 tile bytes.
    pub(super) fn new(
        device: &wgpu::Device,
        queue: &dyn crate::render::queue_writes::QueueWrites,
        section: Option<&ProbeDepthTilesSection>,
        ledger: &mut ShAllocationLedger,
    ) -> Self {
        let references = ProbeDepthTileReferences::from_section(section);
        let layout = references.texture_layout();
        let plan = depth_tile_texture_plan(section);
        let allocation = depth_tile_allocation(plan.extent);
        if layout.is_some() {
            ledger.record_texture(
                allocation,
                &source_ids([Some(59)]),
                false,
                ResidencyAllocationState::Data,
            );
        }
        let texture = device.new_texture(&allocation.descriptor(Some(if layout.is_some() {
            "SH Depth Tiles"
        } else {
            "SH Depth Tiles Dummy"
        })));
        let flat: Vec<u16> = plan.texels.iter().flatten().copied().collect();
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &u16_slice_to_bytes(&flat),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4 * plan.extent[0]),
                rows_per_image: Some(plan.extent[1]),
            },
            allocation.extent,
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some("SH Depth Tiles View"),
            ..Default::default()
        });
        Self {
            references,
            grid_words: DepthTileGridWords::for_layout(layout),
            view,
        }
    }

    /// The table every moment-texel writer packs G from.
    pub(super) fn references(&self) -> &ProbeDepthTileReferences {
        &self.references
    }

    pub(super) fn grid_words(&self) -> DepthTileGridWords {
        self.grid_words
    }

    pub(super) fn bind_group_entry(&self) -> wgpu::BindGroupEntry<'_> {
        wgpu::BindGroupEntry {
            binding: BIND_SH_DEPTH_TILES,
            resource: wgpu::BindingResource::TextureView(&self.view),
        }
    }
}

/// Binding 18 in the shared SH layout, with the depth moments' visibility:
/// every SH sampler weighs depth through the same decode in any stage.
pub(super) fn depth_tiles_layout_entry() -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding: BIND_SH_DEPTH_TILES,
        visibility: wgpu::ShaderStages::VERTEX
            | wgpu::ShaderStages::FRAGMENT
            | wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

#[cfg(test)]
#[path = "sh_depth_tiles_harness.rs"]
mod harness;

#[cfg(test)]
#[path = "sh_depth_tiles_tests.rs"]
mod tests;
