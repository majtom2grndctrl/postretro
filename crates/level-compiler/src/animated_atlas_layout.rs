// Compact animated-lightmap atlas layout: the identity layout and the
// cell-order paged repack, chosen so the atlas is never larger than today's.
// See: context/lib/build_pipeline.md §PRL section IDs (AnimatedLightWeightMaps)

use postretro_level_format::animated_light_weight_maps::AnimatedLightWeightMapsSection;
use postretro_level_format::animated_lightmap_atlas::{
    animated_atlas_byte_estimate, animated_page_size_lower_bound,
};

use crate::lightmap_bake::MaxRects;

/// Where one block lands: page, then top-left texel on that page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BlockPlacement {
    pub page: u32,
    pub x: u32,
    pub y: u32,
}

/// Move every block to `placements[block]` on `page_size` pages. Chunks keep
/// their offset inside their block, so this rewrites coordinates only: the
/// texel partition, weights and light lists are untouched.
pub(crate) fn apply_block_placements(
    section: &mut AnimatedLightWeightMapsSection,
    placements: &[BlockPlacement],
    page_size: u32,
) {
    assert_eq!(
        placements.len(),
        section.blocks.len(),
        "one placement per animated block"
    );
    for chunk in &mut section.chunk_rects {
        let block = &section.blocks[chunk.block as usize];
        let placement = placements[chunk.block as usize];
        let dx = chunk
            .compact_x
            .checked_sub(block.compact_x)
            .expect("chunk lies inside its block");
        let dy = chunk
            .compact_y
            .checked_sub(block.compact_y)
            .expect("chunk lies inside its block");
        chunk.compact_x = placement.x + dx;
        chunk.compact_y = placement.y + dy;
    }
    for (block, placement) in section.blocks.iter_mut().zip(placements) {
        block.compact_layer = placement.page;
        block.compact_x = placement.x;
        block.compact_y = placement.y;
    }
    section.page_size = page_size;
    section.compact_layers = placements
        .iter()
        .map(|placement| placement.page + 1)
        .max()
        .unwrap_or(0);
}

/// The identity layout: every block at its static position, one page per
/// static layer that holds a block, in ascending static-layer order, at the
/// static layer size. It reproduces the full-layer atlas exactly, so it is the
/// size ceiling for any packed layout and the parity reference.
pub(crate) fn identity_placements(section: &AnimatedLightWeightMapsSection) -> Vec<BlockPlacement> {
    let mut layers: Vec<u32> = section
        .blocks
        .iter()
        .map(|block| block.static_layer)
        .collect();
    layers.sort_unstable();
    layers.dedup();
    section
        .blocks
        .iter()
        .map(|block| BlockPlacement {
            page: layers
                .binary_search(&block.static_layer)
                .expect("block layer is in the layer list") as u32,
            x: block.static_x,
            y: block.static_y,
        })
        .collect()
}

pub(crate) fn apply_identity_layout(
    section: &mut AnimatedLightWeightMapsSection,
    static_layer_size: u32,
) {
    let placements = identity_placements(section);
    apply_block_placements(section, &placements, static_layer_size);
}

/// Pack block `(width, height)` extents in the given (cell) order onto
/// `page_size` pages. MaxRects fills the current page; a block that does not
/// fit opens the next page, so earlier pages are never revisited and no page
/// is empty. `None` when a block is larger than a page.
pub(crate) fn pack_blocks_in_order(
    extents: &[(u32, u32)],
    page_size: u32,
) -> Option<Vec<BlockPlacement>> {
    let mut placements = Vec::with_capacity(extents.len());
    let mut page = 0_u32;
    let mut packer = MaxRects::new(page_size, page_size);
    let mut page_used = false;
    for &(width, height) in extents {
        if width > page_size || height > page_size {
            return None;
        }
        let (x, y) = match packer.insert(width, height) {
            Some(origin) => origin,
            None => {
                debug_assert!(page_used, "an empty page fits any block no larger than it");
                page += 1;
                packer = MaxRects::new(page_size, page_size);
                packer
                    .insert(width, height)
                    .expect("an empty page fits a block no larger than the page")
            }
        };
        page_used = true;
        placements.push(BlockPlacement { page, x, y });
    }
    Some(placements)
}

fn layout_bytes(page_size: u32, pages: u32) -> u64 {
    animated_atlas_byte_estimate(page_size, page_size, pages)
}

/// Which layout [`choose_compact_layout`] wrote, for the compile log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChosenLayout {
    Packed,
    Identity,
}

/// Relayout a section (in any layout) into the smallest compact atlas: every
/// power-of-two page size from the lower bound up to the static layer size is
/// packed in cell order, and the fewest-bytes result wins (ties keep the
/// smaller page). A packed layout larger than the identity layout is never
/// written; the identity layout ships instead.
pub(crate) fn choose_compact_layout(
    section: &mut AnimatedLightWeightMapsSection,
    static_layer_size: u32,
) -> ChosenLayout {
    if section.blocks.is_empty() {
        return ChosenLayout::Identity;
    }
    let identity = identity_placements(section);
    let identity_pages = identity.iter().map(|p| p.page + 1).max().unwrap_or(0);
    let identity_bytes = layout_bytes(static_layer_size, identity_pages);

    // Blocks are indexed in cell order (ascending face index; faces are
    // ordered by cell), which is the packing order.
    let extents: Vec<(u32, u32)> = section
        .blocks
        .iter()
        .map(|block| (block.width, block.height))
        .collect();
    let lower =
        animated_page_size_lower_bound(section.largest_block_side(), Some(static_layer_size))
            .next_power_of_two();

    let mut best: Option<(u64, u32, Vec<BlockPlacement>)> = None;
    let mut page_size = lower;
    while page_size <= static_layer_size {
        if let Some(placements) = pack_blocks_in_order(&extents, page_size) {
            let pages = placements.iter().map(|p| p.page + 1).max().unwrap_or(0);
            let bytes = layout_bytes(page_size, pages);
            if best
                .as_ref()
                .is_none_or(|(best_bytes, _, _)| bytes < *best_bytes)
            {
                best = Some((bytes, page_size, placements));
            }
        }
        page_size *= 2;
    }

    match best {
        Some((bytes, page_size, placements)) if bytes <= identity_bytes => {
            apply_block_placements(section, &placements, page_size);
            ChosenLayout::Packed
        }
        _ => {
            apply_block_placements(section, &identity, static_layer_size);
            ChosenLayout::Identity
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_level_format::animated_light_weight_maps::{
        AnimatedBlock, ChunkAtlasRect, TexelLight, TexelLightEntry,
    };
    use postretro_level_format::animated_lightmap_atlas::ANIMATED_PAGE_MIN_SIZE;

    /// A section in identity layout: one chunk per block covering the block's
    /// interior (the placement minus a 2-texel gutter), weights one per texel.
    fn identity_section(
        blocks: &[(u32, u32, u32, u32, u32)],
        static_size: u32,
    ) -> AnimatedLightWeightMapsSection {
        let mut section = AnimatedLightWeightMapsSection::empty();
        let mut texel_offset = 0;
        for (index, &(layer, x, y, width, height)) in blocks.iter().enumerate() {
            section.blocks.push(AnimatedBlock {
                static_layer: layer,
                static_x: x,
                static_y: y,
                compact_x: x,
                compact_y: y,
                compact_layer: 0,
                width,
                height,
            });
            let (chunk_w, chunk_h) = (
                width.saturating_sub(4).max(1),
                height.saturating_sub(4).max(1),
            );
            section.chunk_rects.push(ChunkAtlasRect {
                compact_x: x + 2.min(width - 1),
                compact_y: y + 2.min(height - 1),
                width: chunk_w,
                height: chunk_h,
                texel_offset,
                block: index as u32,
            });
            for _ in 0..chunk_w * chunk_h {
                section.offset_counts.push(TexelLightEntry {
                    offset: section.texel_lights.len() as u32,
                    count: 1,
                });
                section.texel_lights.push(TexelLight {
                    light_index: index as u32,
                    weight: 0.5,
                    direction_oct: [1, 2],
                });
            }
            texel_offset += chunk_w * chunk_h;
        }
        apply_identity_layout(&mut section, static_size);
        section
    }

    fn pages(section: &AnimatedLightWeightMapsSection) -> u32 {
        section.compact_layers
    }

    #[test]
    fn identity_layout_keeps_static_positions_one_page_per_occupied_layer() {
        let section = identity_section(
            &[(5, 10, 20, 30, 30), (2, 0, 0, 16, 16), (5, 100, 100, 8, 8)],
            2048,
        );
        assert_eq!(section.page_size, 2048);
        assert_eq!(
            section.compact_layers, 2,
            "today's slot count: layers 2 and 5"
        );
        let pages: Vec<u32> = section.blocks.iter().map(|b| b.compact_layer).collect();
        assert_eq!(pages, [1, 0, 1], "ascending static-layer order");
        for (index, block) in section.blocks.iter().enumerate() {
            assert_eq!(
                (block.compact_x, block.compact_y),
                (block.static_x, block.static_y)
            );
            assert_eq!(
                section.chunk_static_origin(index),
                Some((
                    block.static_layer,
                    section.chunk_rects[index].compact_x,
                    section.chunk_rects[index].compact_y
                )),
                "identity chunks compose at their static position"
            );
        }
        assert_eq!(section.consistency_error(), None);
    }

    #[test]
    fn repack_rewrites_positions_only_and_keeps_chunk_offsets_inside_blocks() {
        let mut section = identity_section(
            &[
                (0, 1500, 1500, 300, 200),
                (3, 40, 900, 120, 500),
                (3, 700, 20, 64, 64),
            ],
            2048,
        );
        let before = section.clone();
        let static_origins: Vec<_> = (0..before.chunk_rects.len())
            .map(|i| before.chunk_static_origin(i))
            .collect();

        assert_eq!(
            choose_compact_layout(&mut section, 2048),
            ChosenLayout::Packed
        );

        assert_eq!(section.offset_counts, before.offset_counts);
        assert_eq!(section.texel_lights, before.texel_lights);
        for (index, (chunk, old)) in section
            .chunk_rects
            .iter()
            .zip(&before.chunk_rects)
            .enumerate()
        {
            assert_eq!(
                (chunk.width, chunk.height, chunk.texel_offset, chunk.block),
                (old.width, old.height, old.texel_offset, old.block)
            );
            let block = section.blocks[chunk.block as usize];
            let old_block = before.blocks[old.block as usize];
            assert_eq!(
                (
                    chunk.compact_x - block.compact_x,
                    chunk.compact_y - block.compact_y
                ),
                (
                    old.compact_x - old_block.compact_x,
                    old.compact_y - old_block.compact_y
                ),
            );
            assert_eq!(section.chunk_static_origin(index), static_origins[index]);
        }
        for (block, old) in section.blocks.iter().zip(&before.blocks) {
            assert_eq!(
                (
                    block.static_layer,
                    block.static_x,
                    block.static_y,
                    block.width,
                    block.height
                ),
                (
                    old.static_layer,
                    old.static_x,
                    old.static_y,
                    old.width,
                    old.height
                ),
                "blocks keep their full static placement"
            );
        }
        assert_eq!(section.consistency_error(), None);
    }

    #[test]
    fn blocks_that_fit_one_page_allocate_one_page_across_many_static_layers() {
        let blocks: Vec<_> = (0..6).map(|layer| (layer, 64, 64, 200, 200)).collect();
        let mut section = identity_section(&blocks, 2048);
        assert_eq!(section.compact_layers, 6);
        choose_compact_layout(&mut section, 2048);
        assert_eq!(section.page_size, ANIMATED_PAGE_MIN_SIZE);
        assert_eq!(pages(&section), 1);
        assert_eq!(section.consistency_error(), None);
    }

    #[test]
    fn overflowing_blocks_spill_into_a_second_page_with_no_empty_page() {
        // Five 600² blocks: at 1024² one fits per page; at 2048² nine fit.
        let blocks: Vec<_> = (0..5).map(|layer| (layer, 0, 0, 600, 600)).collect();
        let mut section = identity_section(&blocks, 2048);
        choose_compact_layout(&mut section, 2048);
        // One 600² block per 1024² page (five pages) loses to one 2048² page.
        assert_eq!(section.page_size, 2048);
        assert_eq!(pages(&section), 1);

        let blocks: Vec<_> = (0..12).map(|layer| (layer, 0, 0, 600, 600)).collect();
        let mut section = identity_section(&blocks, 2048);
        choose_compact_layout(&mut section, 2048);
        assert_eq!(section.page_size, 2048);
        assert_eq!(pages(&section), 2, "nine per 2048² page, then a second");
        let on_second: Vec<_> = section
            .blocks
            .iter()
            .filter(|b| b.compact_layer == 1)
            .collect();
        assert_eq!(on_second.len(), 3);
        assert_eq!(section.consistency_error(), None);
    }

    #[test]
    fn page_count_is_the_fewest_the_packer_fills_in_cell_order() {
        let extents = [(700, 700), (300, 300), (700, 700), (300, 300)];
        let placements = pack_blocks_in_order(&extents, 1024).unwrap();
        let pages: Vec<u32> = placements.iter().map(|p| p.page).collect();
        // Cell order is kept: the second 700² block cannot share page 0 with
        // the first, so it opens page 1; the last small block follows it.
        assert_eq!(pages, [0, 0, 1, 1]);
    }

    #[test]
    fn a_page_sized_block_packs_alone_at_the_origin_and_the_next_starts_a_new_page() {
        let extents = [(100, 100), (1024, 1024), (10, 10)];
        let placements = pack_blocks_in_order(&extents, 1024).unwrap();
        assert_eq!(
            placements[1],
            BlockPlacement {
                page: 1,
                x: 0,
                y: 0
            }
        );
        assert_eq!(placements[2].page, 2);
        assert_eq!(placements[0].page, 0);
    }

    #[test]
    fn page_size_is_a_power_of_two_within_the_decided_bounds() {
        for &(largest, static_size) in &[(645, 2048), (1500, 2048), (78, 128), (10, 8192)] {
            let mut section = identity_section(&[(0, 0, 0, largest, largest)], static_size);
            choose_compact_layout(&mut section, static_size);
            let page = section.page_size;
            assert!(page.is_power_of_two(), "{page}");
            assert!(
                page >= largest && page <= static_size,
                "{page} for {largest}/{static_size}"
            );
            assert!(page >= static_size.min(ANIMATED_PAGE_MIN_SIZE), "{page}");
        }
    }

    #[test]
    fn packing_that_would_exceed_the_identity_layout_ships_identity() {
        // 128² static atlas: the page floor is the static size itself. Cell
        // order interleaves layers, so in-order packing needs three pages
        // where the identity layout needs two.
        let mut section = identity_section(
            &[(0, 0, 0, 128, 64), (1, 0, 0, 128, 100), (0, 0, 64, 128, 64)],
            128,
        );
        let identity = section.clone();
        assert_eq!(identity.compact_layers, 2);
        let packed = pack_blocks_in_order(&[(128, 64), (128, 100), (128, 64)], 128).unwrap();
        assert_eq!(packed.iter().map(|p| p.page).max(), Some(2));

        assert_eq!(
            choose_compact_layout(&mut section, 128),
            ChosenLayout::Identity
        );
        assert_eq!(section, identity);
    }

    #[test]
    fn a_packed_layout_is_never_larger_than_identity() {
        let blocks: Vec<_> = (0..4).map(|layer| (layer, 8, 8, 40, 40)).collect();
        let mut section = identity_section(&blocks, 128);
        let identity_bytes = layout_bytes(128, section.compact_layers);
        assert_eq!(
            choose_compact_layout(&mut section, 128),
            ChosenLayout::Packed
        );
        assert_eq!(section.compact_layers, 1);
        assert!(layout_bytes(section.page_size, section.compact_layers) <= identity_bytes);
    }
}
