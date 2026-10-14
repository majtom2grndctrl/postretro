// Predicted working-set peak of the fused lightmap stage.
// See: context/lib/build_pipeline.md §Compiler pipeline

use crate::lightmap_bake::PreparedAtlas;
use crate::lightmap_layer;

use super::LIGHTMAP_PARTITION_WINDOW;
use super::metadata::{MetadataCounts, predicted_metadata};

/// Bytes per bake-layer texel of the warm per-layer accumulator
/// (`lightmap_layer::IncrementalLayerAccumulator`): its `CompositedAtlas`
/// plane (RGBA f32 irradiance, `Vec3` direction, `bool` coverage = 29, the
/// whole cold plane) plus `weighted_dir` and `fallback_normal` (`Vec3` each)
/// and `chart_index` (`u32`). One layer is live at a time.
const LAYER_PLANE_BYTES_PER_TEXEL: u64 = (4 * 4 + 12 + 1) + 12 + 12 + 4;
/// Shadowmask raw fill (`shadowmask_bake::allocate_shadowmask_raw_fill`):
/// four mask channels of one bake layer, plus blocks held by open cuts.
const SHADOWMASK_FILL_BYTES_PER_TEXEL: u64 = 4;
/// Shadowmask owner fill's texel → face map (`u32`) for the one bake layer
/// whose partitions are arriving.
const SHADOWMASK_FACE_MAP_BYTES_PER_TEXEL: u64 = 4;
/// One resident light partition: at most one 8-byte record per bake-layer
/// texel (charts never overlap), held twice at its peak — the per-chart
/// buffers beside the assembled records, or the records beside their
/// serialized cache copy.
const PARTITION_BYTES_PER_TEXEL: u64 = 2 * std::mem::size_of::<lightmap_layer::LayerTexel>() as u64;

/// The lightmap stage's predicted working-set peak, from the prepared layout
/// and allocation-free input counts: texel payloads plus key, work, cache and
/// ownership metadata. Cache mode never changes admission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::pipeline) struct PredictedLightmapPeak {
    pub(in crate::pipeline) shadowmask_fill: u64,
    pub(in crate::pipeline) shadowmask_encode_scratch: u64,
    pub(in crate::pipeline) shadowmask_face_map: u64,
    pub(in crate::pipeline) layer_plane: u64,
    /// Every partition the light window may hold at once.
    pub(in crate::pipeline) partitions: u64,
    /// Encoded ids 22 and 42, once each; persistence streams their blocks.
    pub(in crate::pipeline) sections: u64,
    pub(in crate::pipeline) metadata: u64,
    pub(in crate::pipeline) cache_metadata: u64,
    pub(in crate::pipeline) ownership_metadata: u64,
}

impl PredictedLightmapPeak {
    pub(in crate::pipeline) fn total(&self) -> u64 {
        [
            self.shadowmask_fill,
            self.shadowmask_encode_scratch,
            self.shadowmask_face_map,
            self.layer_plane,
            self.partitions,
            self.sections,
            self.metadata,
            self.cache_metadata,
            self.ownership_metadata,
        ]
        .into_iter()
        .fold(0, u64::saturating_add)
    }
}

pub(in crate::pipeline) fn predicted_peak(
    prepared: &PreparedAtlas,
    counts: MetadataCounts,
    uncompressed_irradiance: bool,
    partition_window: usize,
) -> PredictedLightmapPeak {
    if counts.static_lights == 0
        || prepared.placements.is_empty()
        || prepared.layout.blocks.is_empty()
    {
        return PredictedLightmapPeak {
            shadowmask_fill: 0,
            shadowmask_encode_scratch: 0,
            shadowmask_face_map: 0,
            layer_plane: 0,
            partitions: 0,
            sections: 0,
            metadata: 0,
            cache_metadata: 0,
            ownership_metadata: 0,
        };
    }
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
    let (metadata, cache_metadata, ownership_metadata) =
        predicted_metadata(prepared, counts, partition_window);
    PredictedLightmapPeak {
        metadata,
        cache_metadata,
        ownership_metadata,
        shadowmask_fill: SHADOWMASK_FILL_BYTES_PER_TEXEL * layer_texels
            + crate::shadowmask_bake::maximum_cut_carry_bytes(prepared),
        shadowmask_encode_scratch: crate::shadowmask_bake::encode_scratch_bytes(
            prepared.atlas_width,
            prepared.atlas_height,
        ),
        shadowmask_face_map: SHADOWMASK_FACE_MAP_BYTES_PER_TEXEL * layer_texels,
        layer_plane: LAYER_PLANE_BYTES_PER_TEXEL * layer_texels,
        partitions: PARTITION_BYTES_PER_TEXEL * layer_texels * partition_window as u64,
        sections: encoded,
    }
}

/// `--verbose`: the predicted peak, for comparison with a measured RSS.
pub(in crate::pipeline) fn log_predicted_peak(
    prepared: &PreparedAtlas,
    counts: MetadataCounts,
    uncompressed_irradiance: bool,
) {
    const MIB: f64 = 1024.0 * 1024.0;
    let peak = predicted_peak(
        prepared,
        counts,
        uncompressed_irradiance,
        LIGHTMAP_PARTITION_WINDOW,
    );
    log::info!(
        "[Compiler] lightmap stage predicted peak {:.0} MiB: shadowmask fill {:.0} + encode scratch {:.0} + face map {:.0} + layer plane {:.0} + {} partitions {:.0} + sections {:.0} + metadata {:.0} + cache metadata {:.0} + ownership metadata {:.0} ({} blocks on {} bake layers of {}²)",
        peak.total() as f64 / MIB,
        peak.shadowmask_fill as f64 / MIB,
        peak.shadowmask_encode_scratch as f64 / MIB,
        peak.shadowmask_face_map as f64 / MIB,
        peak.layer_plane as f64 / MIB,
        LIGHTMAP_PARTITION_WINDOW,
        peak.partitions as f64 / MIB,
        peak.sections as f64 / MIB,
        peak.metadata as f64 / MIB,
        peak.cache_metadata as f64 / MIB,
        peak.ownership_metadata as f64 / MIB,
        prepared.layout.blocks.len(),
        prepared.layer_count,
        prepared.atlas_width,
    );
}

/// Runs before either memo probe or the cut-face estimate, on both paths.
pub(super) fn gate_working_set(
    prepared: &PreparedAtlas,
    counts: MetadataCounts,
    uncompressed_irradiance: bool,
    partition_window: usize,
    budget: u64,
) -> anyhow::Result<()> {
    let predicted =
        predicted_peak(prepared, counts, uncompressed_irradiance, partition_window).total();
    anyhow::ensure!(
        predicted <= budget,
        "Lightmap working-set gate refused before baking: predicted {predicted} bytes exceeds budget {budget} bytes on {} packed bake layers (--working-set-max-size)",
        prepared.layer_count
    );
    log::info!(
        "[Compiler] Lightmap working-set gate: predicted {predicted} bytes, budget {budget} bytes, {} packed bake layers, admitted (--working-set-max-size)",
        prepared.layer_count
    );
    Ok(())
}
