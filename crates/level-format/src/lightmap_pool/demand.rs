// What a level's blocks ask of the two pools, counted with the runtime's own
// placement: the compiler's bound and the runtime's all-resident count read
// this one source, so they cannot differ.
// See: context/lib/rendering_pipeline.md §4 (Lightmap cell-block residency)

use super::per_pool::{PerPool, PoolKind, mask_item, mask_item_plane};
use super::{AllResidentPool, BlockPlacement, place_all_resident};
use crate::lightmap::LightmapBlockIndex;
use crate::shadowmask_atlas::ShadowmaskBlockIndex;

/// Which of a block's two planes it stores. An absent plane reads zero and
/// demands nothing of the mask pool.
pub type PlanePresence = [bool; 2];

/// Each block's extent and which planes it stores. An extent is one lightmap
/// placement and one mask placement per present plane, at the block's
/// alignment: a plane's extent is its block's, so the two pools round alike.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PoolDemand {
    alignment: u32,
    extents: Vec<(u32, u32)>,
    /// `None` for a level with no mask pool at all: no id 42, or a mask pool
    /// the device cannot hold. Distinct from a level whose every plane is
    /// absent, which has masks that read zero.
    planes: Option<Vec<PlanePresence>>,
}

/// Every plane of a level placed at once: the all-resident mask pool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AllResidentMasks {
    /// Where each block's planes live, indexed by block id; `None` for an
    /// absent plane.
    pub planes: Vec<[Option<BlockPlacement>; 2]>,
    pub layer_count: u32,
}

/// Reused across cells, so packing one set allocates only inside the placement.
#[derive(Debug, Default)]
pub struct PackScratch {
    lightmap: Vec<(u32, u32)>,
    mask: Vec<(u32, u32)>,
}

impl PoolDemand {
    /// `planes` holds one entry per block, or is `None` for no mask pool.
    pub fn new(
        alignment: u32,
        extents: Vec<(u32, u32)>,
        planes: Option<Vec<PlanePresence>>,
    ) -> Self {
        if let Some(planes) = &planes {
            assert_eq!(planes.len(), extents.len(), "one presence per block");
        }
        Self {
            alignment: alignment.max(1),
            extents,
            planes,
        }
    }

    /// A level with no mask pool.
    pub fn lightmap_only(alignment: u32, extents: Vec<(u32, u32)>) -> Self {
        Self::new(alignment, extents, None)
    }

    /// The demand a loaded level's block indices describe. The caller has
    /// paired id 42 with id 22 block for block.
    pub fn of_indices(
        lightmap: &LightmapBlockIndex,
        shadowmask: Option<&ShadowmaskBlockIndex>,
    ) -> Self {
        Self::new(
            lightmap.header.block_alignment(),
            lightmap
                .records
                .iter()
                .map(|r| (u32::from(r.width), u32::from(r.height)))
                .collect(),
            shadowmask.map(|index| {
                index
                    .records
                    .iter()
                    .map(|r| [r.group_a.len != 0, r.group_b.len != 0])
                    .collect()
            }),
        )
    }

    pub fn alignment(&self) -> u32 {
        self.alignment
    }

    pub fn block_count(&self) -> usize {
        self.extents.len()
    }

    /// `(width, height)` per block id.
    pub fn extents(&self) -> &[(u32, u32)] {
        &self.extents
    }

    pub fn has_mask_pool(&self) -> bool {
        self.planes.is_some()
    }

    /// The same blocks with no mask pool: every plane stays out of it.
    pub fn without_masks(mut self) -> Self {
        self.planes = None;
        self
    }

    pub fn planes(&self, block: u32) -> PlanePresence {
        self.planes
            .as_ref()
            .map_or([false; 2], |planes| planes[block as usize])
    }

    /// Every mask plane the level stores.
    pub fn plane_count(&self) -> u32 {
        (0..self.extents.len() as u32)
            .map(|block| self.planes(block).iter().filter(|&&stored| stored).count() as u32)
            .sum()
    }

    /// Every item of `kind`, ascending: block ids, or each present plane.
    pub fn items(&self, kind: PoolKind) -> Vec<u32> {
        let blocks = 0..self.extents.len() as u32;
        match kind {
            PoolKind::Lightmap => blocks.collect(),
            PoolKind::Mask => blocks
                .flat_map(|block| {
                    let planes = self.planes(block);
                    (0..2)
                        .filter(move |&plane| planes[plane])
                        .map(move |plane| mask_item(block, plane))
                })
                .collect(),
        }
    }

    /// The layers holding every block at once, in `edge`² layers. `None` when
    /// a block does not fit a layer.
    pub fn all_resident(&self, edge: u32) -> Option<PerPool<u32>> {
        let blocks: Vec<u32> = (0..self.extents.len() as u32).collect();
        self.layers_of(&blocks, edge, &mut PackScratch::default())
    }

    /// The layers holding `blocks`, each once.
    pub fn layers_of(
        &self,
        blocks: &[u32],
        edge: u32,
        scratch: &mut PackScratch,
    ) -> Option<PerPool<u32>> {
        scratch.lightmap.clear();
        scratch.mask.clear();
        for &block in blocks {
            let extent = self.extents[block as usize];
            scratch.lightmap.push(extent);
            for stored in self.planes(block) {
                if stored {
                    scratch.mask.push(extent);
                }
            }
        }
        Some(PerPool {
            lightmap: self.pack(&scratch.lightmap, edge)?.layer_count,
            mask: self.pack(&scratch.mask, edge)?.layer_count,
        })
    }

    /// The lightmap pool with every block placed.
    pub fn place_blocks(&self, edge: u32) -> Option<AllResidentPool> {
        self.pack(&self.extents, edge)
    }

    /// The mask pool with every plane placed, in the same packing order a
    /// streamed pool's from-scratch pack takes.
    pub fn place_planes(&self, edge: u32) -> Option<AllResidentMasks> {
        let items = self.items(PoolKind::Mask);
        let extents: Vec<(u32, u32)> = items
            .iter()
            .map(|&item| self.extents[PoolKind::Mask.block_of(item) as usize])
            .collect();
        let pool = self.pack(&extents, edge)?;
        let mut planes = vec![[None; 2]; self.extents.len()];
        for (&item, placement) in items.iter().zip(pool.placements) {
            let block = PoolKind::Mask.block_of(item) as usize;
            planes[block][mask_item_plane(item)] = Some(placement);
        }
        Some(AllResidentMasks {
            planes,
            layer_count: pool.layer_count,
        })
    }

    fn pack(&self, extents: &[(u32, u32)], edge: u32) -> Option<AllResidentPool> {
        place_all_resident(extents, self.alignment, edge)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn demand() -> PoolDemand {
        PoolDemand::new(
            4,
            vec![(32, 32), (32, 32), (32, 32)],
            Some(vec![[true, true], [false, true], [false, false]]),
        )
    }

    #[test]
    fn a_plane_is_one_placement_and_an_absent_plane_none() {
        let demand = demand();
        assert_eq!(demand.plane_count(), 3);
        assert_eq!(
            demand.items(PoolKind::Mask),
            vec![mask_item(0, 0), mask_item(0, 1), mask_item(1, 1)]
        );
        let masks = demand.place_planes(64).unwrap();
        assert_eq!(masks.layer_count, 1);
        assert!(masks.planes[0][0].is_some() && masks.planes[0][1].is_some());
        assert!(masks.planes[1][0].is_none() && masks.planes[1][1].is_some());
        assert_eq!(masks.planes[2], [None, None]);
    }

    // Two planes of one block sit at two places, and the counts agree with the
    // placements a level actually takes.
    #[test]
    fn layer_counts_equal_the_placements() {
        let demand = demand();
        let layers = demand.all_resident(32).unwrap();
        assert_eq!(
            layers.lightmap,
            demand.place_blocks(32).unwrap().layer_count
        );
        assert_eq!(layers.mask, demand.place_planes(32).unwrap().layer_count);
        assert_eq!(
            layers,
            PerPool {
                lightmap: 3,
                mask: 3
            }
        );
        let masks = demand.place_planes(32).unwrap();
        assert_ne!(masks.planes[0][0], masks.planes[0][1]);
    }

    #[test]
    fn a_level_with_no_mask_pool_differs_from_one_with_every_plane_absent() {
        let none = PoolDemand::lightmap_only(4, vec![(8, 8)]);
        let absent = PoolDemand::new(4, vec![(8, 8)], Some(vec![[false, false]]));
        assert!(!none.has_mask_pool() && absent.has_mask_pool());
        assert_eq!(none.all_resident(64).unwrap().mask, 0);
        assert_eq!(absent.all_resident(64).unwrap().mask, 0);
    }
}
