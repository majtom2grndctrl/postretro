// Predicted working-set peak of the fused lightmap stage.
// See: context/lib/build_pipeline.md §Compiler pipeline

use crate::lightmap_bake::PreparedAtlas;
use crate::lightmap_layer;

use super::LIGHTMAP_PARTITION_WINDOW;

/// Bytes per bake-layer texel of the warm per-layer accumulator
/// (`lightmap_layer::IncrementalLayerAccumulator`): its `CompositedAtlas`
/// plane (RGBA f32 irradiance, `Vec3` direction, `bool` coverage = 29, the
/// whole cold plane) plus `weighted_dir` and `fallback_normal` (`Vec3` each)
/// and `chart_index` (`u32`). One layer is live at a time.
const LAYER_PLANE_BYTES_PER_TEXEL: u64 = (4 * 4 + 12 + 1) + 12 + 12 + 4;
/// Shadowmask raw fill (`shadowmask_bake::allocate_shadowmask_raw_fill`):
/// four mask slots per texel of every bake layer, empty layer area included,
/// live for the whole layer loop.
const SHADOWMASK_FILL_BYTES_PER_TEXEL: u64 = 4;
/// One resident light partition: at most one 8-byte record per bake-layer
/// texel (charts never overlap), held twice at its peak — the per-chart
/// buffers beside the assembled records, or the records beside their
/// serialized cache copy.
const PARTITION_BYTES_PER_TEXEL: u64 = 2 * std::mem::size_of::<lightmap_layer::LayerTexel>() as u64;

/// The lightmap stage's predicted working-set peak, from the prepared layout
/// alone: the shadowmask fill, one layer plane, the light window's resident
/// partitions, and the encoded sections.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::pipeline) struct PredictedLightmapPeak {
    pub(in crate::pipeline) shadowmask_fill: u64,
    pub(in crate::pipeline) layer_plane: u64,
    /// Every partition the light window may hold at once.
    pub(in crate::pipeline) partitions: u64,
    /// Encoded ids 22 and 42, twice: the section and its cache copy.
    pub(in crate::pipeline) sections: u64,
}

impl PredictedLightmapPeak {
    pub(in crate::pipeline) fn total(&self) -> u64 {
        self.shadowmask_fill + self.layer_plane + self.partitions + self.sections
    }
}

pub(in crate::pipeline) fn predicted_peak(
    prepared: &PreparedAtlas,
    uncompressed_irradiance: bool,
    partition_window: usize,
) -> PredictedLightmapPeak {
    let layer_texels = u64::from(prepared.atlas_width) * u64::from(prepared.atlas_height);
    let scale = u64::from(prepared.layout.direction_texel_scale.max(1));
    let irradiance_bytes_per_texel = if uncompressed_irradiance { 8 } else { 1 };
    let encoded: u64 = prepared
        .layout
        .blocks
        .iter()
        .map(|block| {
            let (w, h) = (u64::from(block.width), u64::from(block.height));
            // Irradiance, direction (Rg8 at the reduced scale), and both BC5
            // shadowmask groups.
            w * h * irradiance_bytes_per_texel + (w / scale) * (h / scale) * 2 + 2 * w * h
        })
        .sum();
    PredictedLightmapPeak {
        shadowmask_fill: SHADOWMASK_FILL_BYTES_PER_TEXEL
            * layer_texels
            * u64::from(prepared.layer_count),
        layer_plane: LAYER_PLANE_BYTES_PER_TEXEL * layer_texels,
        partitions: PARTITION_BYTES_PER_TEXEL * layer_texels * partition_window as u64,
        sections: 2 * encoded,
    }
}

/// `--verbose`: the predicted peak, for comparison with a measured RSS.
pub(in crate::pipeline) fn log_predicted_peak(
    prepared: &PreparedAtlas,
    uncompressed_irradiance: bool,
) {
    const MIB: f64 = 1024.0 * 1024.0;
    let peak = predicted_peak(prepared, uncompressed_irradiance, LIGHTMAP_PARTITION_WINDOW);
    log::info!(
        "[Compiler] lightmap stage predicted peak {:.0} MiB: shadowmask fill {:.0} + layer plane {:.0} + {} partitions {:.0} + sections {:.0} ({} blocks on {} bake layers of {}²)",
        peak.total() as f64 / MIB,
        peak.shadowmask_fill as f64 / MIB,
        peak.layer_plane as f64 / MIB,
        LIGHTMAP_PARTITION_WINDOW,
        peak.partitions as f64 / MIB,
        peak.sections as f64 / MIB,
        prepared.layout.blocks.len(),
        prepared.layer_count,
        prepared.atlas_width,
    );
}
