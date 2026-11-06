// Canonical dense-probe and stored-node address derivation.
// See: context/lib/rendering_pipeline.md §4 (Cluster SH residency)

use postretro_level_format::sh_reconstruct::{Level, StoredBrickRange, stored_node_prefix_sum};
use postretro_level_loader::ShStreamBaseMetadata;

use super::ShResidencyDrainError;
use super::node_map::NodeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct StoredNode {
    pub brick_origin: [u32; 3],
    pub scale: u8,
    pub level: u8,
}

/// Global stored-node metadata is retained only to translate a chunk-local
/// rank to the node's live allocation. It is not a host copy of atlas payload
/// bytes.
#[derive(Debug, Clone, Copy)]
pub struct StoredNodeLayout {
    pub global_base_slot: u32,
    pub tile_count: u32,
}

pub struct DenseNodeLayout {
    pub nodes: Vec<Option<StoredNode>>,
    pub local_slots: Vec<Option<u32>>,
    pub layouts: NodeMap<StoredNodeLayout>,
}

/// Rebuild the v11 stored-node prefix from the always-resident id-34
/// projection. The stream wire's local rank is chunk-relative; installs use
/// this retained layout to recover the rank *within* the node before adding
/// the live pool base.
pub fn derive_dense_node_layout(
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

    let l0_ranks = l0_local_ranks(base.grid_dimensions, &validity);
    let grid = NodeGrid::new(base.grid_dimensions);
    let mut nodes = Vec::with_capacity(base.probes.len());
    let mut local_slots = Vec::with_capacity(base.probes.len());
    let mut layouts = NodeMap::for_grid(base.grid_dimensions, base.probes.len());
    // Probes arrive in dense order, so grid coordinates advance by carry
    // instead of dividing the index per probe.
    let mut xyz = [0u32; 3];
    // Consecutive probes mostly share a node. Its prefix range, already
    // checked and recorded, is reused for them.
    let mut last_node: Option<(StoredNode, StoredBrickRange)> = None;
    for (dense_usize, probe) in base.probes.iter().enumerate() {
        let dense = u32::try_from(dense_usize).map_err(|_| ShResidencyDrainError::SlotOverflow)?;
        let coordinates = xyz;
        xyz[0] += 1;
        if xyz[0] == base.grid_dimensions[0] {
            xyz[0] = 0;
            xyz[1] += 1;
            if xyz[1] == base.grid_dimensions[1] {
                xyz[1] = 0;
                xyz[2] += 1;
            }
        }
        let node = stored_node_at(
            &grid,
            dense,
            coordinates,
            probe.validity,
            probe.density_level,
            probe.node_scale,
        )?;
        let Some(node) = node else {
            nodes.push(None);
            local_slots.push(None);
            continue;
        };
        let range = match last_node {
            Some((cached, range)) if cached == node => range,
            _ => {
                let node_index = affinity_index(node.brick_origin, affinity_dims)?;
                let range = *prefix.bricks.get(node_index).ok_or(
                    ShResidencyDrainError::MalformedChunk {
                        cluster_id: 0,
                        reason: "id-34 stored-node origin is outside the prefix",
                    },
                )?;
                if range.stored_tile_count == 0 {
                    return Err(ShResidencyDrainError::MalformedChunk {
                        cluster_id: 0,
                        reason: "valid probe resolves to an empty stored node",
                    });
                }
                layouts.insert_or_update(
                    node,
                    StoredNodeLayout {
                        global_base_slot: range.base_slot,
                        tile_count: range.stored_tile_count,
                    },
                    |_| {},
                );
                last_node = Some((node, range));
                range
            }
        };
        let local_slot = match node.level {
            0 => u32::from(l0_ranks[dense_usize]),
            1 | 2 => 0,
            _ => unreachable!("stored_node_at validated level"),
        };
        if local_slot >= range.stored_tile_count {
            return Err(ShResidencyDrainError::MalformedChunk {
                cluster_id: 0,
                reason: "id-34 local stored-node rank exceeds its prefix range",
            });
        }
        nodes.push(Some(node));
        local_slots.push(Some(local_slot));
    }
    Ok(DenseNodeLayout {
        nodes,
        local_slots,
        layouts,
    })
}

/// Grid extent checks every probe repeats, done once. `total` carries the
/// overflow error the first valid probe would otherwise hit.
struct NodeGrid {
    dimensions: [u32; 3],
    total: Result<u32, ShResidencyDrainError>,
}

impl NodeGrid {
    fn new(dimensions: [u32; 3]) -> Self {
        let total = dimensions[0]
            .checked_mul(dimensions[1])
            .and_then(|xy| xy.checked_mul(dimensions[2]))
            .ok_or(ShResidencyDrainError::SlotOverflow);
        Self { dimensions, total }
    }
}

/// The stored node probe `dense` belongs to, or `None` for an invalid probe.
/// `xyz` is the probe's grid coordinate.
fn stored_node_at(
    grid: &NodeGrid,
    dense: u32,
    xyz: [u32; 3],
    validity: u8,
    level: u8,
    scale: u8,
) -> Result<Option<StoredNode>, ShResidencyDrainError> {
    if validity == 0 {
        return Ok(None);
    }
    if level > 2 || scale > 31 || grid.dimensions.contains(&0) {
        return Err(ShResidencyDrainError::MalformedChunk {
            cluster_id: 0,
            reason: "id-34 node metadata is invalid",
        });
    }
    let total = grid.total.clone()?;
    if dense >= total {
        return Err(ShResidencyDrainError::MalformedChunk {
            cluster_id: 0,
            reason: "dense index exceeds id-34 grid",
        });
    }
    Ok(Some(StoredNode {
        // Align the brick down to the node's `1 << scale` span. `scale <= 31`
        // was checked above, so the shift pair equals `brick / span * span`
        // without a per-probe divide.
        brick_origin: xyz.map(|coordinate| ((coordinate / 4) >> scale) << scale),
        scale,
        level,
    }))
}

pub fn dense_index(xyz: [u32; 3], dimensions: [u32; 3]) -> Result<u32, ShResidencyDrainError> {
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

pub fn affinity_index(
    origin: [u32; 3],
    dimensions: [u32; 3],
) -> Result<usize, ShResidencyDrainError> {
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

/// Rank of every valid probe among the valid probes before it in its 4x4x4
/// brick, walking the brick z-major then y then x. A level-0 node stores one
/// tile per valid probe in that order, so this is the probe's node-local slot.
/// Entries for invalid probes are unused.
fn l0_local_ranks(dimensions: [u32; 3], validity: &[bool]) -> Vec<u8> {
    let mut ranks = vec![0u8; validity.len()];
    let [width, height, depth] = dimensions.map(|axis| axis as usize);
    for brick_z in (0..depth).step_by(4) {
        for brick_y in (0..height).step_by(4) {
            for brick_x in (0..width).step_by(4) {
                let mut count = 0u8;
                for z in brick_z..(brick_z + 4).min(depth) {
                    for y in brick_y..(brick_y + 4).min(height) {
                        let row = (z * height + y) * width;
                        for x in brick_x..(brick_x + 4).min(width) {
                            if validity[row + x] {
                                ranks[row + x] = count;
                                count += 1;
                            }
                        }
                    }
                }
            }
        }
    }
    ranks
}
