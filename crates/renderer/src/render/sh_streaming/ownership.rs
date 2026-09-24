//! Canonical dense-probe and stored-node address derivation.

use std::collections::BTreeMap;

use postretro_level_format::sh_reconstruct::{Level, stored_node_prefix_sum};
use postretro_level_loader::ShStreamBaseMetadata;

use super::ShResidencyDrainError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct StoredNode {
    pub(super) brick_origin: [u32; 3],
    pub(super) scale: u8,
    pub(super) level: u8,
}

/// Global stored-node metadata is retained only to translate a chunk-local
/// rank to the canonical owner's allocation. It is not a host copy of atlas
/// payload bytes.
#[derive(Debug, Clone, Copy)]
pub(super) struct StoredNodeLayout {
    pub(super) global_base_slot: u32,
    pub(super) tile_count: u32,
}

pub(super) struct DenseNodeLayout {
    pub(super) nodes: Vec<Option<StoredNode>>,
    pub(super) local_slots: Vec<Option<u32>>,
    pub(super) layouts: BTreeMap<StoredNode, StoredNodeLayout>,
}

/// Rebuild the v11 stored-node prefix from the always-resident id-34
/// projection. The stream wire's local rank is closure-relative; installs use
/// this retained layout to recover the rank *within* the node before adding
/// the canonical live pool base.
pub(super) fn derive_dense_node_layout(
    base: &ShStreamBaseMetadata,
) -> Result<DenseNodeLayout, ShResidencyDrainError> {
    let affinity_dims = base.grid_dimensions.map(|axis| axis.div_ceil(4));
    let brick_count = affinity_dims.into_iter().try_fold(1usize, |count, axis| {
        count
            .checked_mul(axis as usize)
            .ok_or(ShResidencyDrainError::SlotOverflow)
    })?;
    let mut levels = Vec::with_capacity(brick_count);
    let mut scales = Vec::with_capacity(brick_count);
    for brick_z in 0..affinity_dims[2] {
        for brick_y in 0..affinity_dims[1] {
            for brick_x in 0..affinity_dims[0] {
                let dense = dense_index(
                    [brick_x * 4, brick_y * 4, brick_z * 4],
                    base.grid_dimensions,
                )?;
                let probe = base.probes.get(dense as usize).ok_or(
                    ShResidencyDrainError::MalformedChunk {
                        cluster_id: 0,
                        reason: "id-34 projection has incomplete brick metadata",
                    },
                )?;
                levels.push(Level::from_u8(probe.density_level).ok_or(
                    ShResidencyDrainError::MalformedChunk {
                        cluster_id: 0,
                        reason: "id-34 projection has an invalid density level",
                    },
                )?);
                scales.push(probe.node_scale);
            }
        }
    }
    let validity: Vec<bool> = base
        .probes
        .iter()
        .map(|probe| probe.validity != 0)
        .collect();
    let prefix = stored_node_prefix_sum(base.grid_dimensions, &levels, &scales, &validity).ok_or(
        ShResidencyDrainError::MalformedChunk {
            cluster_id: 0,
            reason: "id-34 projection has an invalid stored-node prefix",
        },
    )?;

    let mut nodes = Vec::with_capacity(base.probes.len());
    let mut local_slots = Vec::with_capacity(base.probes.len());
    let mut layouts = BTreeMap::new();
    for (dense_usize, probe) in base.probes.iter().enumerate() {
        let dense = u32::try_from(dense_usize).map_err(|_| ShResidencyDrainError::SlotOverflow)?;
        let node = stored_node_for_dense(
            dense,
            base.grid_dimensions,
            probe.validity,
            probe.density_level,
            probe.node_scale,
        )?;
        let Some(node) = node else {
            nodes.push(None);
            local_slots.push(None);
            continue;
        };
        let node_index = affinity_index(node.brick_origin, affinity_dims)?;
        let range =
            *prefix
                .bricks
                .get(node_index)
                .ok_or(ShResidencyDrainError::MalformedChunk {
                    cluster_id: 0,
                    reason: "id-34 stored-node origin is outside the prefix",
                })?;
        if range.stored_tile_count == 0 {
            return Err(ShResidencyDrainError::MalformedChunk {
                cluster_id: 0,
                reason: "valid probe resolves to an empty stored node",
            });
        }
        let local_slot = match node.level {
            0 => l0_local_slot(dense, base.grid_dimensions, &validity)?,
            1 | 2 => 0,
            _ => unreachable!("stored_node_for_dense validated level"),
        };
        if local_slot >= range.stored_tile_count {
            return Err(ShResidencyDrainError::MalformedChunk {
                cluster_id: 0,
                reason: "id-34 local stored-node rank exceeds its prefix range",
            });
        }
        layouts.entry(node).or_insert(StoredNodeLayout {
            global_base_slot: range.base_slot,
            tile_count: range.stored_tile_count,
        });
        nodes.push(Some(node));
        local_slots.push(Some(local_slot));
    }
    Ok(DenseNodeLayout {
        nodes,
        local_slots,
        layouts,
    })
}

pub(super) fn stored_node_for_dense(
    dense: u32,
    dimensions: [u32; 3],
    validity: u8,
    level: u8,
    scale: u8,
) -> Result<Option<StoredNode>, ShResidencyDrainError> {
    if validity == 0 {
        return Ok(None);
    }
    if level > 2 || scale > 31 || dimensions.contains(&0) {
        return Err(ShResidencyDrainError::MalformedChunk {
            cluster_id: 0,
            reason: "id-34 node metadata is invalid",
        });
    }
    let xy = dimensions[0]
        .checked_mul(dimensions[1])
        .ok_or(ShResidencyDrainError::SlotOverflow)?;
    if dense
        >= xy
            .checked_mul(dimensions[2])
            .ok_or(ShResidencyDrainError::SlotOverflow)?
    {
        return Err(ShResidencyDrainError::MalformedChunk {
            cluster_id: 0,
            reason: "dense index exceeds id-34 grid",
        });
    }
    let xyz = [
        dense % dimensions[0],
        (dense / dimensions[0]) % dimensions[1],
        dense / xy,
    ];
    let brick = xyz.map(|coordinate| coordinate / 4);
    let span = 1u32
        .checked_shl(u32::from(scale))
        .ok_or(ShResidencyDrainError::SlotOverflow)?;
    Ok(Some(StoredNode {
        brick_origin: brick.map(|coordinate| coordinate / span * span),
        scale,
        level,
    }))
}

pub(super) fn rewrite_slot(word: u32, slot: u32) -> Result<u32, ShResidencyDrainError> {
    const SLOT_SHIFT: u32 = 5;
    const FLAGS_MASK: u32 = (1 << SLOT_SHIFT) - 1;
    if slot > (u32::MAX >> SLOT_SHIFT) {
        return Err(ShResidencyDrainError::SlotOverflow);
    }
    Ok((word & FLAGS_MASK) | (slot << SLOT_SHIFT))
}

fn dense_index(xyz: [u32; 3], dimensions: [u32; 3]) -> Result<u32, ShResidencyDrainError> {
    if xyz[0] >= dimensions[0] || xyz[1] >= dimensions[1] || xyz[2] >= dimensions[2] {
        return Err(ShResidencyDrainError::MalformedChunk {
            cluster_id: 0,
            reason: "id-34 affine origin exceeds grid",
        });
    }
    let xy = dimensions[0]
        .checked_mul(dimensions[1])
        .ok_or(ShResidencyDrainError::SlotOverflow)?;
    xyz[0]
        .checked_add(
            xyz[1]
                .checked_mul(dimensions[0])
                .ok_or(ShResidencyDrainError::SlotOverflow)?,
        )
        .and_then(|index| index.checked_add(xyz[2].checked_mul(xy)?))
        .ok_or(ShResidencyDrainError::SlotOverflow)
}

fn affinity_index(origin: [u32; 3], dimensions: [u32; 3]) -> Result<usize, ShResidencyDrainError> {
    let xy = dimensions[0]
        .checked_mul(dimensions[1])
        .ok_or(ShResidencyDrainError::SlotOverflow)?;
    let index = origin[0]
        .checked_add(
            origin[1]
                .checked_mul(dimensions[0])
                .ok_or(ShResidencyDrainError::SlotOverflow)?,
        )
        .and_then(|index| index.checked_add(origin[2].checked_mul(xy)?))
        .ok_or(ShResidencyDrainError::SlotOverflow)?;
    usize::try_from(index).map_err(|_| ShResidencyDrainError::SlotOverflow)
}

fn l0_local_slot(
    dense: u32,
    dimensions: [u32; 3],
    validity: &[bool],
) -> Result<u32, ShResidencyDrainError> {
    let xy = dimensions[0]
        .checked_mul(dimensions[1])
        .ok_or(ShResidencyDrainError::SlotOverflow)?;
    let xyz = [
        dense % dimensions[0],
        (dense / dimensions[0]) % dimensions[1],
        dense / xy,
    ];
    let brick = xyz.map(|axis| axis / 4);
    let mut count = 0u32;
    for local_z in 0..4 {
        for local_y in 0..4 {
            for local_x in 0..4 {
                let coord = [
                    brick[0] * 4 + local_x,
                    brick[1] * 4 + local_y,
                    brick[2] * 4 + local_z,
                ];
                if coord[0] >= dimensions[0]
                    || coord[1] >= dimensions[1]
                    || coord[2] >= dimensions[2]
                {
                    continue;
                }
                let index = dense_index(coord, dimensions)?;
                if index == dense {
                    return Ok(count);
                }
                if validity[index as usize] {
                    count = count
                        .checked_add(1)
                        .ok_or(ShResidencyDrainError::SlotOverflow)?;
                }
            }
        }
    }
    Err(ShResidencyDrainError::MalformedChunk {
        cluster_id: 0,
        reason: "valid L0 probe is absent from its brick",
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slot_rewrite_retains_all_non_address_indirection_bits() {
        let word = 0b1_10_01_1_0101u32;
        let rewritten = rewrite_slot(word, 7).unwrap();
        assert_eq!(rewritten & 0x1f, word & 0x1f);
        assert_eq!(rewritten >> 5, 7);
    }
}
