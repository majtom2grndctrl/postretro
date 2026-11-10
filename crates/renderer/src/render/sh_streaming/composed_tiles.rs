//! Test-only: where each probe's composed tiles sit in the streamed pools.
//! See: context/lib/rendering_pipeline.md §4 (Cluster SH residency)

use super::ShResidencyState;
use super::gpu::AtlasShape;
use crate::render::composed_tiles::{ComposedAtlas, ComposedAtlases};
use postretro_render_cpu::sh_volume::STREAMED_SH_PHYSICAL_TILE_STRIDE;

impl ShResidencyState {
    /// The dense pools' composed atlases and the compose words that place a
    /// resident probe's tile in them; a non-resident probe's word is zero.
    pub(in crate::render) fn composed_atlases(&self) -> ComposedAtlases<'_> {
        let gpu = self
            .gpu
            .as_ref()
            .expect("a streamed level builds its pools at install");
        fn atlas(texture: &wgpu::Texture, shape: AtlasShape) -> ComposedAtlas<'_> {
            ComposedAtlas {
                texture,
                tiles_per_row: shape.tiles_per_row,
                tiles_per_layer: shape.tiles_per_layer,
                stride: STREAMED_SH_PHYSICAL_TILE_STRIDE,
            }
        }
        ComposedAtlases {
            words: &self.compose_words,
            indirect: atlas(&gpu.total, gpu.shape),
            direct: gpu
                .direct_atlas
                .as_ref()
                .map(|pool| atlas(&pool.texture, gpu.shape)),
            handoff: gpu
                .direct_handoff
                .as_ref()
                .map(|pool| atlas(&pool.texture, gpu.shape)),
        }
    }
}
