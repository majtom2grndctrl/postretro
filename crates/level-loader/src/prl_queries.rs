// Spatial queries over loaded runtime cell data.
// See: context/lib/build_pipeline.md §PRL Compilation

use glam::Vec3;

use crate::prl::{CellId, CellLocatorChild, CouplingTuple, LevelWorld};
#[cfg(any(feature = "dev-tools", test))]
use crate::prl::{CellLocatorSide, CellLocatorTrace, CellLocatorTraceStep};

impl LevelWorld {
    /// Locate the runtime cell containing `position`.
    ///
    /// On-plane positions choose the front child, matching the temporary
    /// compiler-side BSP traversal.
    pub fn locate_cell(&self, position: Vec3) -> usize {
        let mut current = self.cell_locator_root;

        loop {
            match current {
                CellLocatorChild::Cell(cell_idx) => return cell_idx,
                CellLocatorChild::Node(node_idx) => {
                    let node = &self.cell_locator_nodes[node_idx];
                    let side = node.plane_normal.dot(position) - node.plane_distance;
                    current = if side >= 0.0 { node.front } else { node.back };
                }
            }
        }
    }

    /// Trace the same point-to-cell descent as [`Self::locate_cell`] for
    /// diagnostics. Keeps UI code from duplicating locator traversal.
    #[cfg(any(feature = "dev-tools", test))]
    pub fn trace_locate_cell(&self, position: Vec3) -> CellLocatorTrace {
        let mut current = self.cell_locator_root;
        let mut steps = Vec::new();

        loop {
            match current {
                CellLocatorChild::Cell(result_cell) => {
                    return CellLocatorTrace {
                        root: self.cell_locator_root,
                        steps,
                        result_cell,
                    };
                }
                CellLocatorChild::Node(node_index) => {
                    let node = &self.cell_locator_nodes[node_index];
                    let signed_distance = node.plane_normal.dot(position) - node.plane_distance;
                    let (selected_side, selected_child) = if signed_distance >= 0.0 {
                        (CellLocatorSide::Front, node.front)
                    } else {
                        (CellLocatorSide::Back, node.back)
                    };
                    steps.push(CellLocatorTraceStep {
                        node_index,
                        signed_distance,
                        selected_side,
                        selected_child,
                    });
                    current = selected_child;
                }
            }
        }
    }

    pub fn cell_count(&self) -> usize {
        self.cells.len()
    }

    /// Whether two cells share the conservative portal-reachability component.
    /// Missing baked data deliberately treats every pair as perceivable.
    pub fn perceivable(&self, a: CellId, b: CellId) -> bool {
        self.cell_visibility
            .as_ref()
            .map(|visibility| visibility.perceivable(a, b))
            .unwrap_or(true)
    }

    /// Baked consumer-neutral coupling axes for two cells.
    ///
    /// Graded axes are absent when the optional section or its pair record is
    /// absent.
    pub fn coupling(&self, a: CellId, b: CellId) -> CouplingTuple {
        let perceivable = self.perceivable(a, b);
        let pair = self
            .cell_visibility
            .as_ref()
            .and_then(|visibility| visibility.coupled_pair(a, b));
        CouplingTuple {
            perceivable,
            distance: pair.map(|pair| pair.distance),
            aperture: pair.map(|pair| pair.aperture),
        }
    }

    pub fn total_face_count(&self) -> u32 {
        #[cfg(feature = "load-prl")]
        {
            self.face_meta.len() as u32
        }

        #[cfg(not(feature = "load-prl"))]
        {
            self.cells.iter().map(|cell| cell.face_count).sum()
        }
    }

    pub fn cell_portal_count(&self, cell_idx: usize) -> usize {
        let Some((start, end)) = self.cell_portal_range(cell_idx) else {
            return 0;
        };
        self.cell_portal_refs
            .get(start..end)
            .map_or(0, <[u32]>::len)
    }

    pub fn cell_portal_index(&self, cell_idx: usize, offset: usize) -> Option<usize> {
        let (start, end) = self.cell_portal_range(cell_idx)?;
        let idx = start.checked_add(offset)?;
        if idx >= end {
            return None;
        }
        self.cell_portal_refs
            .get(idx)
            .map(|&portal| portal as usize)
    }

    pub fn cell_is_solid(&self, cell_idx: usize) -> bool {
        self.cells
            .get(cell_idx)
            .map(|cell| cell.is_solid)
            .unwrap_or(false)
    }

    pub fn cell_face_count(&self, cell_idx: usize) -> u32 {
        self.cells
            .get(cell_idx)
            .map(|cell| cell.face_count)
            .unwrap_or(0)
    }

    pub fn cell_bounds(&self, cell_idx: usize) -> Option<(Vec3, Vec3)> {
        self.cells
            .get(cell_idx)
            .map(|cell| (cell.bounds_min, cell.bounds_max))
    }

    pub fn spawn_position(&self) -> Vec3 {
        let mut mins = Vec3::splat(f32::MAX);
        let mut maxs = Vec3::splat(f32::MIN);
        for cell in &self.cells {
            if cell.is_solid || cell.face_count == 0 {
                continue;
            }
            mins = mins.min(cell.bounds_min);
            maxs = maxs.max(cell.bounds_max);
        }
        (mins + maxs) * 0.5
    }

    fn cell_portal_range(&self, cell_idx: usize) -> Option<(usize, usize)> {
        let cell = self.cells.get(cell_idx)?;
        let start = cell.portal_ref_start as usize;
        let count = cell.portal_ref_count as usize;
        let end = start.checked_add(count)?;
        Some((start, end))
    }
}
