// One partition's per-mover row counts: texel gate masks folded into gated,
// shared and differing texels and the cell blocks they touch; the size estimate.
// See: context/lib/build_pipeline.md §Compiler pipeline (step 12)

use glam::DVec3;

use super::pose::Terminus;
use super::texels::CandidateMover;
use crate::partition::Aabb;

/// Estimate bytes per masked texel: one mask byte.
const MASK_BYTES_PER_TEXEL: u64 = 1;
/// Estimate bytes per touched (row, cell block): one block-table entry.
const MASK_BYTES_PER_BLOCK: u64 = 8;

/// A state mask's estimated bytes over `texels` masked texels in `blocks`
/// cell blocks.
pub(crate) fn mask_estimate(texels: u64, blocks: u64) -> u64 {
    texels * MASK_BYTES_PER_TEXEL + blocks * MASK_BYTES_PER_BLOCK
}

/// One mover's gated texels in one partition, at each terminus by
/// [`Terminus::index`].
#[derive(Debug, Clone)]
pub(crate) struct MoverTexels {
    pub(crate) mover: usize,
    pub(crate) gated: [u64; 2],
    /// Texels gated at both termini.
    pub(crate) both: u64,
    pub(crate) light_inside: [bool; 2],
    /// World bounds of the gated texel centres, unlifted.
    pub(crate) centres: [Option<Aabb>; 2],
    /// World bounds of the centres gated at this terminus only.
    pub(crate) differing_centres: [Option<Aabb>; 2],
    /// Cell blocks holding a gated texel; sorted and distinct after
    /// [`Self::finish`].
    pub(crate) blocks: [Vec<u32>; 2],
    /// Cell blocks holding a texel gated at this terminus only.
    pub(crate) differing_blocks: [Vec<u32>; 2],
}

impl MoverTexels {
    pub(crate) fn new(candidate: &CandidateMover) -> Self {
        Self {
            mover: candidate.mover,
            gated: [0; 2],
            both: 0,
            light_inside: candidate.light_inside,
            centres: [None, None],
            differing_centres: [None, None],
            blocks: [Vec::new(), Vec::new()],
            differing_blocks: [Vec::new(), Vec::new()],
        }
    }

    /// Fold one texel of cell block `block` with gate `mask`, one
    /// [`Terminus::bit`] per terminus that gates it.
    pub(crate) fn add(&mut self, mask: u8, centre: DVec3, block: u32) {
        for terminus in Terminus::BOTH {
            if mask & terminus.bit() == 0 {
                continue;
            }
            let k = terminus.index();
            self.gated[k] += 1;
            self.centres[k]
                .get_or_insert_with(Aabb::empty)
                .expand_point(centre);
            push_block(&mut self.blocks[k], block);
            if mask != Terminus::BOTH_BITS {
                self.differing_centres[k]
                    .get_or_insert_with(Aabb::empty)
                    .expand_point(centre);
                push_block(&mut self.differing_blocks[k], block);
            }
        }
        if mask == Terminus::BOTH_BITS {
            self.both += 1;
        }
    }

    /// Reduce the block lists to distinct ids.
    pub(crate) fn finish(&mut self) {
        for list in self.blocks.iter_mut().chain(&mut self.differing_blocks) {
            list.sort_unstable();
            list.dedup();
        }
    }
}

/// Append `block` unless it is the last entry: a chart's texels arrive
/// together, so the list stays near one entry per chart before the dedup.
fn push_block(list: &mut Vec<u32>, block: u32) {
    if list.last() != Some(&block) {
        list.push(block);
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use glam::DVec3;
    use proptest::prelude::*;

    use super::*;
    use crate::mover_light_gate::GateTally;
    use crate::mover_light_gate::pose::{AnalysedMover, MoverPoses, Pose};

    const LIGHT: usize = 3;

    fn one_mover() -> MoverPoses {
        let pose = |name: &str| Pose {
            waypoint: name.to_string(),
            brushes: Vec::new(),
            bounds: Aabb::empty(),
        };
        MoverPoses {
            analysed: vec![AnalysedMover {
                mover_id: 7,
                label: "kinematic_mover 'test'".to_string(),
                poses: [pose("a"), pose("b")],
                swept: Aabb::empty(),
            }],
            not_analysed: Vec::new(),
        }
    }

    fn candidate() -> CandidateMover {
        CandidateMover {
            mover: 0,
            inside: [Some(false); 2],
            light_inside: [false; 2],
        }
    }

    /// Blocks per bake layer in the generated texels: a block lives in one
    /// layer, as `CellBlock::layer` places it.
    const BLOCKS_PER_LAYER: u32 = 4;

    /// One texel: its bake layer, its block within that layer, and whether
    /// each terminus gates it.
    type Texel = (u32, u32, bool, bool);

    /// Fold `texels` as the walk does, one partition per bake layer, and
    /// return the tally.
    fn fold(texels: &[Texel]) -> GateTally {
        let poses = one_mover();
        let mut tally = GateTally::default();
        let layers: BTreeSet<u32> = texels.iter().map(|texel| texel.0).collect();
        for layer in layers {
            let mut counts = MoverTexels::new(&candidate());
            for &(_, block, first, last) in texels.iter().filter(|texel| texel.0 == layer) {
                let bit = |gated: bool, terminus: Terminus| if gated { terminus.bit() } else { 0 };
                let mask = bit(first, Terminus::First) | bit(last, Terminus::Last);
                if mask != 0 {
                    counts.add(mask, DVec3::ZERO, layer * BLOCKS_PER_LAYER + block);
                }
            }
            counts.finish();
            tally.add_partition(&poses, LIGHT, &[counts]);
        }
        tally
    }

    fn texel_strategy() -> impl Strategy<Value = Texel> {
        (0u32..3, 0..BLOCKS_PER_LAYER, any::<bool>(), any::<bool>())
    }

    proptest! {
        /// Shared plus each terminus's differing texels make the union of
        /// the two gated sets, and a differing-only mask never costs more
        /// than the every-gated one.
        #[test]
        fn shared_and_differing_partition_the_union_and_bound_the_estimates(
            texels in prop::collection::vec(texel_strategy(), 0..64),
        ) {
            let tally = fold(&texels);
            let gated_at = |first: bool| -> Vec<(usize, u32)> {
                texels
                    .iter()
                    .enumerate()
                    .filter(|(_, texel)| if first { texel.2 } else { texel.3 })
                    .map(|(at, texel)| (at, texel.0 * BLOCKS_PER_LAYER + texel.1))
                    .collect()
            };
            let (first, last) = (gated_at(true), gated_at(false));
            let first_ids: BTreeSet<usize> = first.iter().map(|texel| texel.0).collect();
            let last_ids: BTreeSet<usize> = last.iter().map(|texel| texel.0).collect();
            let union = first_ids.union(&last_ids).count() as u64;

            let rows: Vec<_> = tally.rows().collect();
            let row = |terminus: Terminus| {
                rows.iter()
                    .find(|(key, _)| key.terminus == terminus)
                    .map(|(_, row)| (*row).clone())
                    .unwrap_or_default()
            };
            let (at_first, at_last) = (row(Terminus::First), row(Terminus::Last));
            prop_assert_eq!(at_first.shared_texels, at_last.shared_texels);
            prop_assert_eq!(
                at_first.shared_texels + at_first.differing_texels() + at_last.differing_texels(),
                union
            );

            for (row, gated, other) in [
                (&at_first, &first, &last_ids),
                (&at_last, &last, &first_ids),
            ] {
                let blocks: BTreeSet<u32> = gated.iter().map(|texel| texel.1).collect();
                let differing: Vec<&(usize, u32)> =
                    gated.iter().filter(|texel| !other.contains(&texel.0)).collect();
                let differing_blocks: BTreeSet<u32> =
                    differing.iter().map(|texel| texel.1).collect();
                prop_assert_eq!(row.gated_texels, gated.len() as u64);
                prop_assert_eq!(row.differing_texels(), differing.len() as u64);
                prop_assert_eq!(row.touched_blocks, blocks.len() as u64);
                prop_assert_eq!(row.differing_blocks, differing_blocks.len() as u64);
                prop_assert!(row.differing_only_estimate() <= row.every_gated_estimate());
            }
        }
    }

    #[test]
    fn a_row_in_one_cell_block_estimates_its_gated_texels_plus_one_block_entry() {
        let mut counts = MoverTexels::new(&candidate());
        for _ in 0..37 {
            counts.add(Terminus::First.bit(), DVec3::ZERO, 5);
        }
        counts.finish();
        let mut tally = GateTally::default();
        tally.add_partition(&one_mover(), LIGHT, &[counts]);
        let (_, row) = tally.rows().next().expect("one row");
        assert_eq!(row.touched_blocks, 1);
        assert_eq!(row.every_gated_estimate(), 37 + 8);
    }
}
