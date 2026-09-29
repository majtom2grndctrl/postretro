//! Synthetic dry-run inputs shaped like compiler output.

use postretro_level_format::cell_visibility::{
    CELL_VISIBILITY_DISTANCE_FIXED_POINT_SCALE, CoupledPairRecord,
};
use postretro_level_format::lightmap::{
    DIRECTION_FORMAT_OCT_RG8, DIRECTION_FORMAT_OCT_RGBA8, IRRADIANCE_FORMAT_BC6H,
    IRRADIANCE_FORMAT_RGBA16F,
};

use super::{
    AtlasFormats, CellInfo, ChartRect, DryRunInput, FaceSlot, ReconstructionStats,
    ShadowmaskFormat, ShadowmaskState,
};

pub(super) const METER: u32 = CELL_VISIBILITY_DISTANCE_FIXED_POINT_SCALE;

/// Current bake encodings: BC6H irradiance, half-resolution Rg8 direction,
/// BC5 side-by-side shadowmask.
pub(super) fn bc6h_formats(dim: u32, layers: u32, shadowmask: bool) -> AtlasFormats {
    let blocks = u64::from(dim / 4) * u64::from(dim / 4) * u64::from(layers);
    AtlasFormats {
        layer_count: layers,
        irr_width: dim,
        irr_height: dim,
        irr_format: IRRADIANCE_FORMAT_BC6H,
        irr_payload_bytes: blocks * 16,
        dir_width: dim / 2,
        dir_height: dim / 2,
        dir_format: DIRECTION_FORMAT_OCT_RG8,
        dir_payload_bytes: u64::from(dim / 2) * u64::from(dim / 2) * 2 * u64::from(layers),
        shadowmask: if shadowmask {
            ShadowmaskState::Stored(ShadowmaskFormat {
                width: dim,
                height: dim,
                layer_count: layers,
                payload_bytes: blocks * 32,
            })
        } else {
            ShadowmaskState::Absent
        },
    }
}

/// Uncompressed debug encodings: Rgba16Float irradiance, legacy Rgba8
/// direction at full resolution, no shadowmask.
pub(super) fn raw_formats(dim: u32, layers: u32) -> AtlasFormats {
    let texels = u64::from(dim) * u64::from(dim) * u64::from(layers);
    AtlasFormats {
        layer_count: layers,
        irr_width: dim,
        irr_height: dim,
        irr_format: IRRADIANCE_FORMAT_RGBA16F,
        irr_payload_bytes: texels * 8,
        dir_width: dim,
        dir_height: dim,
        dir_format: DIRECTION_FORMAT_OCT_RGBA8,
        dir_payload_bytes: texels * 4,
        shadowmask: ShadowmaskState::Absent,
    }
}

pub(super) fn chart(cell: u32, layer: u32, x: u32, y: u32, width: u32, height: u32) -> ChartRect {
    ChartRect {
        cell,
        layer,
        x,
        y,
        width,
        height,
    }
}

pub(super) fn pair(cell_a: u32, cell_b: u32, distance: u32) -> CoupledPairRecord {
    CoupledPairRecord {
        cell_a: cell_a.min(cell_b),
        cell_b: cell_a.max(cell_b),
        distance,
        aperture: METER,
    }
}

/// One cell per `cell_clusters` entry, placed 10 m apart on X, all in
/// reachability component 0 unless the caller overrides `component_ids`.
pub(super) fn input(
    formats: AtlasFormats,
    cell_clusters: &[u32],
    charts: Vec<ChartRect>,
    pairs: Vec<CoupledPairRecord>,
) -> DryRunInput {
    let cluster_count = cell_clusters.iter().max().map_or(0, |&c| c + 1);
    DryRunInput {
        formats,
        reconstruction: ReconstructionStats {
            faces: charts.len(),
            charts: charts.len(),
            ..Default::default()
        },
        faces: (0..charts.len()).map(FaceSlot::Chart).collect(),
        charts,
        cells: cell_clusters
            .iter()
            .enumerate()
            .map(|(cell, &cluster)| CellInfo {
                center: [cell as f32 * 10.0, 0.0, 0.0],
                cluster,
                camera_candidate: true,
            })
            .collect(),
        cluster_count,
        pinned_clusters: Vec::new(),
        cell_visibility_present: true,
        component_ids: vec![0; cell_clusters.len()],
        coupled_pairs: pairs,
        portal_graph: None,
    }
}

/// Eight cells in four clusters on a 128² BC6H atlas, charts placed off the
/// 4-texel block grid so blocks straddle cells, with a chain of couplings.
pub(super) fn corridor_input() -> DryRunInput {
    let charts = vec![
        chart(0, 0, 0, 0, 30, 18),
        chart(0, 0, 30, 0, 9, 9),
        chart(1, 0, 39, 0, 25, 14),
        chart(2, 0, 0, 18, 50, 21),
        chart(3, 1, 3, 5, 40, 40),
        chart(4, 1, 43, 5, 17, 33),
        chart(5, 0, 64, 0, 61, 61),
        chart(6, 1, 0, 50, 70, 30),
        chart(7, 1, 70, 50, 13, 13),
    ];
    let pairs = vec![
        pair(0, 1, 5 * METER),
        pair(1, 2, 12 * METER),
        pair(2, 3, 20 * METER),
        pair(0, 3, 30 * METER),
        pair(3, 4, 45 * METER),
        pair(4, 5, 70 * METER),
        pair(0, 6, 100 * METER),
    ];
    input(
        bc6h_formats(128, 2, true),
        &[0, 0, 1, 1, 2, 2, 3, 3],
        charts,
        pairs,
    )
}
