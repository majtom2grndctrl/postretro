//! Atlas layouts the dry run compares: the stored cell blocks (each block one
//! region), and soft cluster-ordered whole-layer packing at a capped layer size
//! driven through the bake's leaf-cohesive chart packer
//! (`pack_layers_with_layer_limit`).

use glam::Vec3;

use super::cell_blocks::POOL_LAYER_EDGE;
use super::{ChartRect, DryRunInput, FaceSlot};
use crate::chart_raster::ChartPlacement;
use crate::lightmap_bake::{
    Chart, LightmapBakeError, MAX_ATLAS_DIMENSION, MAX_ATLAS_LAYERS, pack_cell_sub_blocks,
    pack_layers, pack_layers_with_layer_limit,
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
    /// `(width, height)` of each layer, in irradiance texels. Simulated
    /// layers are square; a stored block is its own rectangle.
    pub layer_dims: Vec<(u32, u32)>,
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
            .map(|&(w, h)| u64::from(w) * u64::from(h))
            .sum()
    }

    pub(crate) fn total_bytes(&self) -> u64 {
        self.layer_bytes.iter().sum()
    }
}

/// The stored layout: every id-22 block is its own region.
pub(crate) fn stored_layout(input: &DryRunInput) -> Layout {
    let formats = &input.formats;
    let mut cell_layers = vec![Vec::new(); input.cell_count()];
    for chart in &input.charts {
        cell_layers[chart.cell as usize].push(chart.layer);
    }
    for layers in &mut cell_layers {
        layers.sort_unstable();
        layers.dedup();
    }
    Layout {
        name: "stored blocks".to_string(),
        cell_layers,
        layer_dims: formats.blocks.iter().map(|b| (b.width, b.height)).collect(),
        layer_bytes: (0..formats.blocks.len())
            .map(|block| formats.stored_block_bytes(block))
            .collect(),
        regular_layer_count: formats.blocks.len() as u32,
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
    let mut layer_dims = vec![(pack.atlas_width, pack.atlas_width); regular_layer_count as usize];

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
        layer_dims.push((alone.atlas_width, alone.atlas_width));
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
            .map(|&(w, h)| formats.layer_bytes_at(w, h))
            .collect(),
        layer_dims,
        regular_layer_count,
        oversize_cells,
        placements,
    }
}

/// Repack every cell's faces in stored order, 1×1 placeholders included, with
/// the bake's own cell-block packer (`pack_cell_sub_blocks`) at the stored
/// alignment and the pool layer edge, and count recovered charts that land
/// where the PRL says they are. A cell's repacked blocks match its stored
/// blocks one for one, in block order. Confirms the recovery and the packer
/// reuse describe the same layout.
pub(crate) fn stored_repack_matches(input: &DryRunInput) -> RepackCheck {
    let formats = &input.formats;
    let align = formats.block_alignment();
    // Faces of each cell, in face order.
    let mut cell_faces: Vec<Vec<&FaceSlot>> = vec![Vec::new(); input.cell_count()];
    for slot in &input.faces {
        let cell = match *slot {
            FaceSlot::Chart(index) => input.charts[index].cell,
            FaceSlot::Placeholder { cell } => cell,
        };
        cell_faces[cell as usize].push(slot);
    }
    // Stored blocks of each cell, in block order.
    let mut blocks_of_cell: Vec<Vec<usize>> = vec![Vec::new(); input.cell_count()];
    for (block, stored) in formats.blocks.iter().enumerate() {
        if let Some(blocks) = blocks_of_cell.get_mut(stored.cell as usize) {
            blocks.push(block);
        }
    }

    let mut matched = 0;
    let mut dims_match = true;
    let mut blocks_packed = 0;
    for (faces, stored_blocks) in cell_faces.iter().zip(&blocks_of_cell) {
        let sizes: Vec<(u32, u32)> = faces
            .iter()
            .map(|slot| match **slot {
                FaceSlot::Chart(index) => (input.charts[index].width, input.charts[index].height),
                FaceSlot::Placeholder { .. } => (1, 1),
            })
            .collect();
        let packed = pack_cell_sub_blocks(&sizes, align, POOL_LAYER_EDGE);
        blocks_packed += packed.len();
        dims_match &= packed.len() == stored_blocks.len();
        for (sub, &block) in packed.iter().zip(stored_blocks) {
            let stored = &formats.blocks[block];
            dims_match &= (sub.block.width, sub.block.height) == (stored.width, stored.height);
            matched += sub
                .members
                .iter()
                .zip(&sub.block.placements)
                .filter(|&(&member, &(x, y))| match *faces[member] {
                    FaceSlot::Chart(index) => {
                        let chart = &input.charts[index];
                        chart.layer as usize == block && (chart.x, chart.y) == (x, y)
                    }
                    FaceSlot::Placeholder { .. } => false,
                })
                .count();
        }
    }
    RepackCheck {
        matched,
        total: input.charts.len(),
        dims_match: dims_match && blocks_packed == formats.blocks.len(),
        error: None,
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
