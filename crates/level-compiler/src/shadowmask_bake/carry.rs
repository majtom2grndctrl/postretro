//! Cut faces retain only their cell blocks until the last sub-face layer closes.
use crate::chart_raster::ChartPlacement;
use crate::lightmap_bake::{BlockLayout, CutParent, PreparedAtlas};
use std::collections::BTreeMap;

pub(super) fn block_hold_until(
    placements: &[ChartPlacement],
    layout: &BlockLayout,
    cuts: &[CutParent],
) -> Vec<u32> {
    let mut until: Vec<_> = layout.blocks.iter().map(|block| block.layer).collect();
    for cut in cuts {
        let last = cut
            .faces
            .clone()
            .map(|face| placements[face].layer)
            .max()
            .unwrap_or(0);
        for face in cut.faces.clone() {
            let block = layout.chart_blocks[face] as usize;
            until[block] = until[block].max(last);
        }
    }
    until
}

/// Largest raw carry after any layer close. Blocks shared by cuts are counted
/// once, until the latest of those cuts finishes. A same-layer cut adds zero.
pub(crate) fn maximum_cut_carry_bytes(prepared: &PreparedAtlas) -> u64 {
    let until = block_hold_until(
        &prepared.placements,
        &prepared.layout,
        &prepared.cut_parents,
    );
    let mut events: BTreeMap<u32, (u64, u64)> = BTreeMap::new();
    for (block, end) in prepared.layout.blocks.iter().zip(until) {
        if end == block.layer {
            continue;
        }
        let bytes = u64::from(block.width) * u64::from(block.height) * 4;
        events.entry(block.layer).or_default().0 += bytes;
        events.entry(end).or_default().1 += bytes;
    }
    let (mut live, mut peak) = (0u64, 0u64);
    for (_, (opened, closed)) in events {
        // A cut finishing on this layer releases its earlier blocks before
        // the layer copies any blocks belonging to cuts still open.
        live -= closed;
        live += opened;
        peak = peak.max(live);
    }
    peak
}

/// Encoding one layer uses two raw-layer images and half a raw layer of BC5.
/// Blocks retained in the carry encode separately, never exceeding that size.
pub(crate) fn encode_scratch_bytes(width: u32, height: u32) -> u64 {
    u64::from(width) * u64::from(height) * 10
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lightmap_bake::CellBlock;

    fn layout() -> PreparedAtlas {
        PreparedAtlas {
            charts: Vec::new(),
            placements: vec![
                ChartPlacement {
                    x: 0,
                    y: 0,
                    layer: 0,
                },
                ChartPlacement {
                    x: 0,
                    y: 0,
                    layer: 2,
                },
                ChartPlacement {
                    x: 4,
                    y: 0,
                    layer: 0,
                },
                ChartPlacement {
                    x: 0,
                    y: 0,
                    layer: 3,
                },
            ],
            atlas_width: 16,
            atlas_height: 16,
            layer_count: 4,
            layout: BlockLayout {
                direction_texel_scale: 2,
                blocks: (0..4)
                    .map(|layer| CellBlock {
                        cell_id: layer,
                        width: 8,
                        height: 8,
                        layer,
                        x: 0,
                        y: 0,
                    })
                    .collect(),
                chart_blocks: vec![0, 2, 0, 3],
            },
            cut_parents: Vec::new(),
        }
    }

    #[test]
    fn carry_counts_shared_blocks_once_until_the_last_cut_finishes() {
        let mut prepared = layout();
        assert_eq!(maximum_cut_carry_bytes(&prepared), 0);
        prepared.layer_count = 400;
        assert_eq!(
            maximum_cut_carry_bytes(&prepared),
            0,
            "bake-layer count adds no carry"
        );
        prepared.cut_parents = vec![
            CutParent {
                parent: 0,
                faces: 0..2,
                own: Vec::new(),
            },
            CutParent {
                parent: 1,
                faces: 2..4,
                own: Vec::new(),
            },
        ];
        assert_eq!(
            maximum_cut_carry_bytes(&prepared),
            8 * 8 * 4,
            "the shared first-layer block remains until layer 3"
        );
        prepared.placements[1].layer = 0;
        prepared.placements[3].layer = 0;
        prepared.layout.chart_blocks = vec![0; 4];
        assert_eq!(
            maximum_cut_carry_bytes(&prepared),
            0,
            "same-layer cuts retain no blocks"
        );
        assert_eq!(encode_scratch_bytes(16, 16), 16 * 16 * 10);
    }
}
