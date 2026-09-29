//! Atlas layouts the dry run compares: the stored packing, and soft
//! cluster-ordered packing at a capped layer size driven through the bake's own
//! leaf-cohesive packer (`pack_layers_with_layer_limit`).

use glam::Vec3;

use super::{ChartRect, DryRunInput, FaceSlot};
use crate::chart_raster::ChartPlacement;
use crate::lightmap_bake::{
    Chart, LightmapBakeError, MAX_ATLAS_DIMENSION, MAX_ATLAS_LAYERS, pack_layers,
    pack_layers_with_layer_limit,
};

/// Runtime `max_texture_array_layers` floor the bake packs under: the bake's
/// own layer cap.
pub(crate) const RUNTIME_LAYER_FLOOR: u32 = MAX_ATLAS_LAYERS;

/// A cell that cannot pack alone into one capped layer. Leaf cohesion forbids
/// splitting it, so the model gives it a dedicated layer at its own size,
/// outside the capped array.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct OversizeCell {
    pub cell: u32,
    pub layer_dim: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Layout {
    pub name: String,
    /// Sorted distinct layers each cell's charts occupy.
    pub cell_layers: Vec<Vec<u32>>,
    /// Square edge of each layer, in irradiance texels.
    pub layer_dims: Vec<u32>,
    /// Id 22 + id 42 bytes of each layer.
    pub layer_bytes: Vec<u64>,
    /// Layers of the shared capped array (excludes oversize layers).
    pub regular_layer_count: u32,
    pub oversize_cells: Vec<OversizeCell>,
    /// Placement per input chart, parallel to `DryRunInput::charts`.
    pub placements: Vec<ChartPlacement>,
}

impl Layout {
    pub(crate) fn total_layer_texels(&self) -> u64 {
        self.layer_dims
            .iter()
            .map(|&d| u64::from(d) * u64::from(d))
            .sum()
    }

    pub(crate) fn total_bytes(&self) -> u64 {
        self.layer_bytes.iter().sum()
    }
}

pub(crate) fn stored_layout(input: &DryRunInput) -> Layout {
    let formats = &input.formats;
    let layer_count = formats.layer_count as usize;
    let mut cell_layers = vec![Vec::new(); input.cell_count()];
    for chart in &input.charts {
        cell_layers[chart.cell as usize].push(chart.layer);
    }
    for layers in &mut cell_layers {
        layers.sort_unstable();
        layers.dedup();
    }
    Layout {
        name: "stored".to_string(),
        cell_layers,
        layer_dims: vec![formats.irr_width; layer_count],
        layer_bytes: vec![formats.stored_layer_bytes(); layer_count],
        regular_layer_count: formats.layer_count,
        oversize_cells: Vec::new(),
        placements: input
            .charts
            .iter()
            .map(|chart| ChartPlacement {
                x: chart.x,
                y: chart.y,
                layer: chart.layer,
            })
            .collect(),
    }
}

/// Pack every chart in (cluster, cell, face) order with layers capped at
/// `cap²`. Cells too large for one capped layer are reported as oversize.
pub(crate) fn cluster_ordered_layout(input: &DryRunInput, cap: u32) -> Layout {
    let mut order: Vec<usize> = (0..input.charts.len()).collect();
    order.sort_by_key(|&i| {
        let cell = input.charts[i].cell;
        (input.cells[cell as usize].cluster, cell, i)
    });

    // Cheap exclusions first; the retry loop below catches the MaxRects
    // fragmentation cases an area bound cannot see.
    let mut cell_area = vec![0u64; input.cell_count()];
    let mut oversize = vec![false; input.cell_count()];
    for chart in &input.charts {
        cell_area[chart.cell as usize] += chart.area();
        if chart.width > cap || chart.height > cap {
            oversize[chart.cell as usize] = true;
        }
    }
    let cap_area = u64::from(cap) * u64::from(cap);
    for (cell, &area) in cell_area.iter().enumerate() {
        if area > cap_area {
            oversize[cell] = true;
        }
    }

    let pack = loop {
        let packed: Vec<usize> = order
            .iter()
            .copied()
            .filter(|&i| !oversize[input.charts[i].cell as usize])
            .collect();
        let charts: Vec<Chart> = packed
            .iter()
            .map(|&i| chart_for(&input.charts[i]))
            .collect();
        match pack_layers_with_layer_limit(&charts, cap, u32::MAX, 0.0) {
            Ok(pack) => break (packed, pack),
            Err(LightmapBakeError::LeafTooLarge { leaf_index, .. }) => {
                oversize[leaf_index as usize] = true;
            }
            Err(LightmapBakeError::ChartTooLarge { face_index, .. }) => {
                oversize[input.charts[packed[face_index]].cell as usize] = true;
            }
            Err(error) => panic!("capped packing failed: {error}"),
        }
    };
    let (packed, pack) = pack;

    let formats = &input.formats;
    let mut placements = vec![
        ChartPlacement {
            x: 0,
            y: 0,
            layer: u32::MAX,
        };
        input.charts.len()
    ];
    let regular_layer_count = if packed.is_empty() {
        0
    } else {
        pack.layer_count
    };
    for (slot, &chart_index) in packed.iter().enumerate() {
        placements[chart_index] = pack.placements[slot];
    }
    let mut layer_dims = vec![pack.atlas_width; regular_layer_count as usize];

    let mut oversize_cells = Vec::new();
    for cell in (0..input.cell_count()).filter(|&c| oversize[c]) {
        let members: Vec<usize> = order
            .iter()
            .copied()
            .filter(|&i| input.charts[i].cell as usize == cell)
            .collect();
        let charts: Vec<Chart> = members
            .iter()
            .map(|&i| chart_for(&input.charts[i]))
            .collect();
        let alone = pack_layers(&charts, MAX_ATLAS_DIMENSION, 0.0)
            .expect("a stored cell packs alone within the bake's maximum layer");
        let layer = layer_dims.len() as u32;
        layer_dims.push(alone.atlas_width);
        for (slot, &chart_index) in members.iter().enumerate() {
            placements[chart_index] = ChartPlacement {
                layer,
                ..alone.placements[slot]
            };
        }
        oversize_cells.push(OversizeCell {
            cell: cell as u32,
            layer_dim: alone.atlas_width,
        });
    }

    let mut cell_layers = vec![Vec::new(); input.cell_count()];
    for (chart, placement) in input.charts.iter().zip(&placements) {
        cell_layers[chart.cell as usize].push(placement.layer);
    }
    for layers in &mut cell_layers {
        layers.sort_unstable();
        layers.dedup();
    }
    Layout {
        name: format!(
            "cluster-ordered cap {cap} ({}²)",
            if regular_layer_count == 0 {
                0
            } else {
                pack.atlas_width
            }
        ),
        cell_layers,
        layer_bytes: layer_dims
            .iter()
            .map(|&dim| formats.layer_bytes_at(dim, dim))
            .collect(),
        layer_dims,
        regular_layer_count,
        oversize_cells,
        placements,
    }
}

/// Repack every face in stored order, 1×1 placeholders included, with the
/// bake's own packer and count recovered charts that land where the PRL
/// says they are. Confirms the recovery and the packer reuse describe the
/// same layout.
pub(crate) fn stored_repack_matches(input: &DryRunInput) -> RepackCheck {
    let charts: Vec<Chart> = input
        .faces
        .iter()
        .map(|slot| match *slot {
            FaceSlot::Chart(index) => chart_for(&input.charts[index]),
            FaceSlot::Placeholder { cell } => placeholder_chart(cell),
        })
        .collect();
    let total = input.charts.len();
    match pack_layers(&charts, MAX_ATLAS_DIMENSION, 0.0) {
        Ok(pack) => RepackCheck {
            matched: input
                .faces
                .iter()
                .zip(&pack.placements)
                .filter(|(slot, placement)| match **slot {
                    FaceSlot::Chart(index) => {
                        let chart = &input.charts[index];
                        chart.x == placement.x
                            && chart.y == placement.y
                            && chart.layer == placement.layer
                    }
                    FaceSlot::Placeholder { .. } => false,
                })
                .count(),
            total,
            dims_match: pack.atlas_width == input.formats.irr_width
                && pack.layer_count == input.formats.layer_count,
            error: None,
        },
        Err(error) => RepackCheck {
            matched: 0,
            total,
            dims_match: false,
            error: Some(error.to_string()),
        },
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RepackCheck {
    pub matched: usize,
    pub total: usize,
    pub dims_match: bool,
    /// The packer refused the stored-order input outright.
    pub error: Option<String>,
}

impl RepackCheck {
    pub(crate) fn reproduces_stored(&self) -> bool {
        self.error.is_none() && self.dims_match && self.matched == self.total
    }
}

/// The 1×1 chart the bake plans for a degenerate face.
fn placeholder_chart(cell: u32) -> Chart {
    chart_for(&ChartRect {
        cell,
        layer: 0,
        x: 0,
        y: 0,
        width: 1,
        height: 1,
    })
}

/// The packer reads only size and leaf; the projection fields are inert.
fn chart_for(rect: &ChartRect) -> Chart {
    Chart {
        origin: Vec3::ZERO,
        u_axis: Vec3::X,
        v_axis: Vec3::Y,
        uv_min: [0.0, 0.0],
        uv_extent: [0.0, 0.0],
        normal: Vec3::Y,
        width_texels: rect.width,
        height_texels: rect.height,
        leaf_index: rect.cell,
    }
}
