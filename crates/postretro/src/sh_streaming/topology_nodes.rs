//! Planner dense nodes: how a probe maps to its node, and the canonical owner
//! (lowest cluster id) of each node.
//!
//! A node's key is bounded grid coordinates: its origin brick and scale. The
//! planner asks for the owner of every valid probe's node, millions of times
//! on a large level, so the table is a flat array indexed by the origin
//! brick's affinity index. A key the array cannot hold exactly (an origin
//! outside the grid, or a second scale at an origin another scale already
//! holds) goes to an ordered overflow map, so the table answers every query
//! as a `BTreeMap<DenseNode, u32>` would, for any input.
//!
//! Nothing iterates this table, so its order is unobservable.

use std::collections::BTreeMap;

use super::controller::ShResidencyControllerError;

/// A node of aligned probe bricks: its origin brick and scale.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct DenseNode {
    pub(super) origin: [u32; 3],
    pub(super) scale: u8,
}

struct GridShape {
    width: u32,
    height: u32,
    total: usize,
}

/// The id-34 grid extent checks every probe repeats, done once. A grid whose
/// extent fails a check rejects at the first valid probe that needs a node,
/// with that check's message, as a per-probe check would.
pub(super) struct NodeGrid {
    shape: Result<GridShape, &'static str>,
}

impl NodeGrid {
    pub(super) fn new(dimensions: [u32; 3]) -> Self {
        Self {
            shape: Self::shape(dimensions),
        }
    }

    fn shape(dimensions: [u32; 3]) -> Result<GridShape, &'static str> {
        let width = usize::try_from(dimensions[0]).map_err(|_| "grid width exceeds usize")?;
        let height = usize::try_from(dimensions[1]).map_err(|_| "grid height exceeds usize")?;
        let xy = width
            .checked_mul(height)
            .ok_or("grid xy dimensions overflow")?;
        if xy == 0 {
            return Err("dense index is outside id-34 grid");
        }
        let depth = usize::try_from(dimensions[2]).map_err(|_| "grid depth exceeds usize")?;
        let total = xy.checked_mul(depth).ok_or("grid dimensions overflow")?;
        Ok(GridShape {
            width: dimensions[0],
            height: dimensions[1],
            total,
        })
    }

    /// The node probe `dense_index`, at grid coordinate `coords` (from a
    /// [`CoordWalk`]), belongs to.
    pub(super) fn node(
        &self,
        dense_index: usize,
        coords: [u32; 3],
        scale: u8,
    ) -> Result<DenseNode, ShResidencyControllerError> {
        let invalid = |message: &str| ShResidencyControllerError::InvalidTopology(message.into());
        let shape = self.shape.as_ref().map_err(|message| invalid(message))?;
        if dense_index >= shape.total {
            return Err(invalid("dense index is outside id-34 grid"));
        }
        if scale >= 32 {
            return Err(invalid("id-34 node scale overflows"));
        }
        // Align the brick down to the node's `1 << scale` span; for
        // `scale < 32` the shift pair equals `brick / span * span`.
        Ok(DenseNode {
            origin: coords.map(|coordinate| ((coordinate / 4) >> scale) << scale),
            scale,
        })
    }
}

/// Grid coordinates of consecutive dense indices, advanced by carry instead
/// of dividing each index. Coordinates of an index outside the grid are
/// meaningless, and `NodeGrid::node` rejects such an index first.
pub(super) struct CoordWalk {
    coords: [u32; 3],
}

impl CoordWalk {
    pub(super) fn start(grid: &NodeGrid, dense_index: u32) -> Self {
        let coords = match &grid.shape {
            Ok(shape) => {
                let index = dense_index as usize;
                let (width, height) = (shape.width as usize, shape.height as usize);
                [
                    (index % width) as u32,
                    (index / width % height) as u32,
                    (index / (width * height)) as u32,
                ]
            }
            Err(_) => [0; 3],
        };
        Self { coords }
    }

    pub(super) fn coords(&self) -> [u32; 3] {
        self.coords
    }

    pub(super) fn advance(&mut self, grid: &NodeGrid) {
        let Ok(shape) = &grid.shape else {
            return;
        };
        self.coords[0] += 1;
        if self.coords[0] == shape.width {
            self.coords[0] = 0;
            self.coords[1] += 1;
            if self.coords[1] == shape.height {
                self.coords[1] = 0;
                self.coords[2] += 1;
            }
        }
    }
}

/// Owner bookkeeping the topology build needs. `BTreeMap` is the reference
/// implementation the tests hold the array table to.
pub(super) trait NodeOwners {
    /// Record `cluster_id` as an owner of `node`, keeping the lowest.
    fn lower_owner(&mut self, node: DenseNode, cluster_id: u32);
    fn owner(&self, node: &DenseNode) -> Option<u32>;
}

#[cfg(test)]
impl NodeOwners for BTreeMap<DenseNode, u32> {
    fn lower_owner(&mut self, node: DenseNode, cluster_id: u32) {
        self.entry(node)
            .and_modify(|owner| *owner = (*owner).min(cluster_id))
            .or_insert(cluster_id);
    }

    fn owner(&self, node: &DenseNode) -> Option<u32> {
        self.get(node).copied()
    }
}

pub(super) struct DenseNodeOwners {
    /// Affinity-brick extent of `primary`; zero when the grid claims more
    /// bricks than there are probes (a malformed manifest).
    affinity_dims: [usize; 3],
    /// `(scale, owner)` of the node whose origin brick is the array index.
    primary: Vec<Option<(u8, u32)>>,
    overflow: BTreeMap<DenseNode, u32>,
}

impl DenseNodeOwners {
    pub(super) fn for_grid(grid_dimensions: [u32; 3], probe_count: usize) -> Self {
        let affinity_dims = grid_dimensions.map(|axis| axis.div_ceil(4) as usize);
        let brick_count = affinity_dims
            .iter()
            .try_fold(1usize, |count, &axis| count.checked_mul(axis));
        match brick_count {
            Some(count) if count <= probe_count.max(1) => Self {
                affinity_dims,
                primary: vec![None; count],
                overflow: BTreeMap::new(),
            },
            _ => Self {
                affinity_dims: [0; 3],
                primary: Vec::new(),
                overflow: BTreeMap::new(),
            },
        }
    }

    fn slot(&self, node: &DenseNode) -> Option<usize> {
        let [width, height, depth] = self.affinity_dims;
        let [x, y, z] = node.origin.map(|axis| axis as usize);
        if x >= width || y >= height || z >= depth {
            return None;
        }
        Some(x + width * (y + height * z))
    }
}

impl NodeOwners for DenseNodeOwners {
    fn lower_owner(&mut self, node: DenseNode, cluster_id: u32) {
        if let Some(slot) = self.slot(&node) {
            let cell = &mut self.primary[slot];
            match cell {
                Some((scale, owner)) if *scale == node.scale => {
                    *owner = (*owner).min(cluster_id);
                    return;
                }
                None => {
                    *cell = Some((node.scale, cluster_id));
                    return;
                }
                // Another scale holds this origin; this node lives in the
                // overflow map.
                Some(_) => {}
            }
        }
        self.overflow
            .entry(node)
            .and_modify(|owner| *owner = (*owner).min(cluster_id))
            .or_insert(cluster_id);
    }

    fn owner(&self, node: &DenseNode) -> Option<u32> {
        if let Some(slot) = self.slot(node)
            && let Some((scale, owner)) = self.primary[slot]
            && scale == node.scale
        {
            return Some(owner);
        }
        self.overflow.get(node).copied()
    }
}
