// Indexed point queries over teleporter brush unions, independent of armed state.
// See: context/lib/entity_model.md §7 (Teleporter regions)

use glam::Vec3;

use crate::bvh::{AabbTree, TreeEntry};
use crate::leaf::ConvexLeaf;

#[derive(Debug, Clone, PartialEq)]
struct Region {
    brushes: Vec<ConvexLeaf>,
    tree: AabbTree<usize>,
    summed_volume: f32,
}

impl Region {
    fn contains(&self, point: Vec3) -> bool {
        let mut inside = false;
        self.tree.visit_point(point, |&brush| {
            if !inside {
                inside = self.brushes[brush].contains_point(point);
            }
        });
        inside
    }
}

/// Teleporter geometry installed in map entity order. Each region is the union
/// of its convex brushes; overlap ranking uses their literal summed volumes,
/// including the contribution of overlapping brushes, then map order. Map order
/// breaks only an exact tie of the computed sums: near-identical regions rank
/// by whatever floating-point rounding their sums carry.
///
/// A region tree narrows point queries to nearby regions. Each region has its
/// own brush tree, so exact containment examines only candidate brush bounds and
/// the region is visited once even when several of its brushes contain a point.
/// Queries are immutable and allocate nothing. Armed state and pawn latches stay
/// with the caller, which supplies eligibility to [`Self::winner`].
#[derive(Debug, Clone, PartialEq)]
pub struct TeleporterRegions {
    regions: Vec<Region>,
    tree: AabbTree<usize>,
}

impl Default for TeleporterRegions {
    fn default() -> Self {
        Self::empty()
    }
}

impl TeleporterRegions {
    /// An empty installed set; every query misses.
    pub const fn empty() -> Self {
        Self {
            regions: Vec::new(),
            tree: AabbTree::empty(),
        }
    }

    /// Build once at level install from bounded convex leaves in map entity
    /// order. Empty regions retain their index but contain no point. Brush
    /// volumes are computed here, never in movement queries, without the
    /// quantization used by environment-volume overlap ranking.
    pub fn build(regions: Vec<Vec<ConvexLeaf>>) -> Self {
        let mut installed = Vec::with_capacity(regions.len());
        let mut entries = Vec::with_capacity(regions.len());
        for (index, brushes) in regions.into_iter().enumerate() {
            let summed_volume = brushes.iter().map(ConvexLeaf::volume).sum();
            let mut brush_entries = Vec::with_capacity(brushes.len());
            let mut region_bounds: Option<(Vec3, Vec3)> = None;
            for (brush_index, brush) in brushes.iter().enumerate() {
                let (min, max) = brush.padded_bounds();
                region_bounds = Some(match region_bounds {
                    Some((previous_min, previous_max)) => {
                        (previous_min.min(min), previous_max.max(max))
                    }
                    None => (min, max),
                });
                brush_entries.push(TreeEntry {
                    min,
                    max,
                    payload: brush_index,
                });
            }
            if let Some((min, max)) = region_bounds {
                entries.push(TreeEntry {
                    min,
                    max,
                    payload: index,
                });
            }
            installed.push(Region {
                brushes,
                tree: AabbTree::build(brush_entries),
                summed_volume,
            });
        }
        Self {
            regions: installed,
            tree: AabbTree::build(entries),
        }
    }

    pub fn len(&self) -> usize {
        self.regions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.regions.is_empty()
    }

    /// Whether a body point is inside region `index`; a face counts as inside.
    /// Invalid indices and non-finite points miss. Capsule overlap is irrelevant
    /// to this point-only contract.
    pub fn contains(&self, index: usize, point: Vec3) -> bool {
        point.is_finite()
            && self
                .regions
                .get(index)
                .is_some_and(|region| region.contains(point))
    }

    /// Visit each containing region exactly once, in deterministic tree order.
    /// The order is not map order; callers needing an overlap winner should use
    /// [`Self::winner`]. A non-finite point visits nothing.
    pub fn visit_containing(&self, point: Vec3, mut visit: impl FnMut(usize)) {
        if !point.is_finite() {
            return;
        }
        self.tree.visit_point(point, |&index| {
            if self.regions[index].contains(point) {
                visit(index);
            }
        });
    }

    /// The eligible containing region with the smallest summed convex volume,
    /// then the earliest map index. Eligibility is checked once per containing
    /// region before it participates in ranking, so a latched small region does
    /// not suppress a larger unlatched one.
    pub fn winner(&self, point: Vec3, mut eligible: impl FnMut(usize) -> bool) -> Option<usize> {
        let mut winner = None;
        self.visit_containing(point, |index| {
            if !eligible(index) {
                return;
            }
            if winner.is_none_or(|current: usize| {
                self.regions[index]
                    .summed_volume
                    .total_cmp(&self.regions[current].summed_volume)
                    .then(index.cmp(&current))
                    .is_lt()
            }) {
                winner = Some(index);
            }
        });
        winner
    }
}

#[cfg(test)]
#[path = "teleporters_tests.rs"]
mod tests;
