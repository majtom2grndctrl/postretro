// Per-direction probe depth tiles on the GPU: the tile texture at shared SH
// binding 18, holding id 59's tiles and then the mover probe channel's live
// tiles (id 57), its grid-info words, and the load's reference table.
// See: context/lib/rendering_pipeline.md §4, §10

use postretro_level_format::probe_depth_tiles::{
    PROBE_DEPTH_TILE_BORDERED_EDGE, PROBE_DEPTH_TILE_TEXEL_BYTES, ProbeDepthTileTextureLayout,
    ProbeDepthTilesSection,
};
use postretro_level_format::sh_volume::OctahedralShProbe;
use postretro_render_cpu::sh_compose::u16_slice_to_bytes;
use postretro_render_cpu::sh_depth_tiles::{
    DepthTileGridWords, ProbeDepthTileReferences, depth_tile_texture_plan,
};
use postretro_render_cpu::sh_volume::BIND_SH_DEPTH_TILES;

use super::gpu_create::GpuCreate;
use super::residency::{ResidencyAllocationState, source_ids};
use super::sh_allocation::depth_tile_allocation;
use super::sh_residency::ShAllocationLedger;
use super::uploads::UploadQueue;

/// One load's depth tiles: owned by its SH volume resources, so a reference
/// rewritten on one load never reaches another. Without tiles (id 59 absent
/// or zero tiles and no live tiles, or no usable SH) the binding holds a 1×1
/// dummy, the table is empty and the grid words are zero, so every texel
/// packs as before id 59.
pub(super) struct ShDepthTiles {
    references: ProbeDepthTileReferences,
    grid_words: DepthTileGridWords,
    /// Kept for live-tile writes: the renderer copies each live tile's
    /// blended texels in, the same `Rg16Float` texels id 59 uploads.
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    layout: Option<ProbeDepthTileTextureLayout>,
    /// Id 59's tile count: the first live tile's index.
    first_live_tile: u32,
    /// A whole-loaded level's id-34 `(E[d], E[d²])` bits per lattice probe,
    /// kept only while it has tiles: the whole-load packer drops its moments,
    /// and a reference rewrite repacks one texel from these (clearing restores
    /// `E[d²]`). Empty for a streamed level, which repacks from its retained
    /// base metadata, and for a level without tiles, where no rewrite can
    /// change a texel.
    whole_load_moments: Vec<[u16; 2]>,
}

impl ShDepthTiles {
    /// Upload `section`'s tiles into a texture with room for `live_tiles`
    /// live tiles after them, or the dummy when there are neither. Only a
    /// real tile texture enters the ledger, sourced to id 59 for its tiles
    /// and id 57 for the live ones; the dummy prices at 0 tile bytes.
    /// `whole_load_probes` is a whole-loaded level's id-34 probes.
    pub(super) fn new(
        device: &wgpu::Device,
        queue: &dyn crate::render::queue_writes::QueueWrites,
        section: Option<&ProbeDepthTilesSection>,
        live_tiles: u32,
        whole_load_probes: Option<&[OctahedralShProbe]>,
        ledger: &mut ShAllocationLedger,
    ) -> Self {
        let references = ProbeDepthTileReferences::from_section(section, live_tiles);
        let layout = references.texture_layout();
        let first_live_tile = section.map_or(0, |section| section.tile_count() as u32);
        let whole_load_moments = match (layout, whole_load_probes) {
            (Some(_), Some(probes)) => probes
                .iter()
                .map(|probe| [probe.mean_distance, probe.mean_sq_distance])
                .collect(),
            _ => Vec::new(),
        };
        let plan = depth_tile_texture_plan(section, live_tiles);
        let allocation = depth_tile_allocation(plan.extent);
        if layout.is_some() {
            ledger.record_texture(
                allocation,
                &source_ids([
                    (first_live_tile > 0).then_some(59),
                    (live_tiles > 0).then_some(57),
                ]),
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
            texture,
            view,
            layout,
            first_live_tile,
            whole_load_moments,
        }
    }

    /// The texture's layout; `None` for the dummy.
    pub(super) fn layout(&self) -> Option<ProbeDepthTileTextureLayout> {
        self.layout
    }

    /// Id 59's tile count: live tiles follow it.
    pub(super) fn first_live_tile(&self) -> u32 {
        self.first_live_tile
    }

    /// Stage `count` live tiles from `first`, consecutive on one tile row,
    /// on the frame's upload batch: `bytes` holds their bordered texels
    /// row-major across the run. Staged copies precede every command of the
    /// frame's first submission, so the frame samples them.
    pub(super) fn write_live_tiles(
        &self,
        queue: &UploadQueue,
        first: u32,
        count: u32,
        bytes: &[u8],
    ) {
        let layout = self
            .layout
            .expect("live tiles exist only in a real tile texture");
        debug_assert!(first >= self.first_live_tile);
        debug_assert!(first + count <= self.references.tile_count());
        debug_assert_eq!(
            first >> layout.tiles_per_row_shift,
            (first + count - 1) >> layout.tiles_per_row_shift,
            "a run stays on one tile row"
        );
        let [x, y] = layout.tile_origin(first);
        let width = count * PROBE_DEPTH_TILE_BORDERED_EDGE;
        queue.write_texture(
            &self.texture,
            wgpu::Origin3d { x, y, z: 0 },
            bytes,
            width * PROBE_DEPTH_TILE_TEXEL_BYTES as u32,
            PROBE_DEPTH_TILE_BORDERED_EDGE,
            wgpu::Extent3d {
                width,
                height: PROBE_DEPTH_TILE_BORDERED_EDGE,
                depth_or_array_layers: 1,
            },
        );
    }

    /// The tile texture, for readback tests.
    #[cfg(test)]
    pub(super) fn texture(&self) -> &wgpu::Texture {
        &self.texture
    }

    /// The table every moment-texel writer packs G from.
    pub(super) fn references(&self) -> &ProbeDepthTileReferences {
        &self.references
    }

    /// The runtime rewrite's one way into the table (`sh_depth_tile_rewrite`).
    pub(super) fn references_mut(&mut self) -> &mut ProbeDepthTileReferences {
        &mut self.references
    }

    /// Whole-load probe `probe`'s moment texel as the packer would write it
    /// now, B/A from its indirection `word`; `None` when no moments were kept
    /// for it (a streamed level, or one without tiles).
    pub(super) fn whole_load_texel(&self, probe: u32, word: u32) -> Option<[u16; 4]> {
        let &[mean, mean_sq] = self.whole_load_moments.get(probe as usize)?;
        Some([
            mean,
            self.references.second_channel(probe, mean_sq),
            word as u16,
            (word >> 16) as u16,
        ])
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
pub(in crate::render) mod tests;
