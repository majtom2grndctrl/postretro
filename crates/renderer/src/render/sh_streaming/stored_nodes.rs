//! Canonical dense-probe and stored-node address derivation: the derivation
//! lives in render-cpu with the residency plan; the slot rewrite stays here.

#[cfg(test)]
use postretro_level_format::sh_reconstruct::{Level, stored_node_prefix_sum};
#[cfg(test)]
pub(super) use postretro_render_cpu::sh_residency::derive_dense_node_layout;
pub(super) use postretro_render_cpu::sh_residency::{StoredNode, StoredNodeLayout};
#[cfg(test)]
pub(super) use postretro_render_cpu::sh_residency::{affinity_index, dense_index};

use super::ShResidencyDrainError;

pub(super) fn rewrite_slot(word: u32, slot: u32) -> Result<u32, ShResidencyDrainError> {
    const SLOT_SHIFT: u32 = 5;
    const FLAGS_MASK: u32 = (1 << SLOT_SHIFT) - 1;
    if slot > (u32::MAX >> SLOT_SHIFT) {
        return Err(ShResidencyDrainError::SlotOverflow);
    }
    Ok((word & FLAGS_MASK) | (slot << SLOT_SHIFT))
}

#[cfg(test)]
pub(super) mod equivalence_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slot_rewrite_retains_all_non_address_indirection_bits() {
        let word = 0b11_0011_0101_u32;
        let rewritten = rewrite_slot(word, 7).unwrap();
        assert_eq!(rewritten & 0x1f, word & 0x1f);
        assert_eq!(rewritten >> 5, 7);
    }
}
