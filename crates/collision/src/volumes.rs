// Environment volume records and the installed volume set with its point query.
// See: context/plans/in-progress/E24--water-and-environment-volumes/index.md

use glam::Vec3;

use crate::leaf::{ContentsMask, ConvexLeaf};

/// One canonical environment volume: a place that may set gravity, push and a
/// fluid. A field left `None` is not set by this volume, so overlapping volumes
/// combine per field. Map order is the volume's index in the list it is built
/// from.
#[derive(Debug, Clone, PartialEq)]
pub struct EnvironmentVolume {
    /// Source map entity index, for diagnostics.
    pub source_entity: u32,
    pub priority: i32,
    /// Signed vertical acceleration, m/s², like the level default.
    pub gravity: Option<f32>,
    /// Acceleration in engine axes, m/s².
    pub push: Option<Vec3>,
    /// Resolved fluid index.
    pub fluid: Option<u16>,
    pub brushes: Vec<ConvexLeaf>,
}

impl EnvironmentVolume {
    /// The fields this volume sets.
    pub fn contents(&self) -> ContentsMask {
        let mut mask = ContentsMask::EMPTY;
        if self.gravity.is_some() {
            mask |= ContentsMask::GRAVITY;
        }
        if self.push.is_some() {
            mask |= ContentsMask::PUSH;
        }
        if self.fluid.is_some() {
            mask |= ContentsMask::FLUID;
        }
        mask
    }
}

/// Per-volume data precomputed at build so queries and the overlap tie-break
/// never recompute it.
#[derive(Debug, Clone, Copy, PartialEq)]
struct VolumeRecord {
    /// Sum of the volume's brush volumes, m³ — the overlap rule's second key.
    summed_brush_volume: f32,
}

/// One brush of one volume, as the query sees it.
#[derive(Debug, Clone, Copy, PartialEq)]
struct LeafEntry {
    volume_index: u32,
    brush_index: u32,
    contents: ContentsMask,
    bounds_min: Vec3,
    bounds_max: Vec3,
}

/// The installed set of environment volumes for one level. Built once at
/// install; queries borrow it and never allocate.
#[derive(Debug, Clone, PartialEq)]
pub struct EnvironmentVolumes {
    volumes: Vec<EnvironmentVolume>,
    records: Vec<VolumeRecord>,
    leaves: Vec<LeafEntry>,
}

impl Default for EnvironmentVolumes {
    fn default() -> Self {
        Self::empty()
    }
}

/// Prefilter bounds are widened by this relative margin so rounding in a
/// supplied AABB can never reject a point the planes contain.
const BOUNDS_RELATIVE_PAD: f32 = 1.0e-5;

impl EnvironmentVolumes {
    /// No volumes: every query reports nothing, so resolution yields level defaults.
    pub const fn empty() -> Self {
        Self {
            volumes: Vec::new(),
            records: Vec::new(),
            leaves: Vec::new(),
        }
    }

    /// Build the query set. Precomputes each volume's contents and summed brush
    /// volume, and flattens every brush into a prefiltered leaf entry.
    pub fn build(volumes: Vec<EnvironmentVolume>) -> Self {
        let mut records = Vec::with_capacity(volumes.len());
        let mut leaves = Vec::new();
        for (volume_index, volume) in volumes.iter().enumerate() {
            let contents = volume.contents();
            let summed_brush_volume = volume.brushes.iter().map(ConvexLeaf::volume).sum();
            records.push(VolumeRecord {
                summed_brush_volume,
            });
            for (brush_index, brush) in volume.brushes.iter().enumerate() {
                let pad = BOUNDS_RELATIVE_PAD
                    * brush
                        .aabb_min
                        .abs()
                        .max(brush.aabb_max.abs())
                        .max_element()
                        .max(1.0);
                leaves.push(LeafEntry {
                    volume_index: volume_index as u32,
                    brush_index: brush_index as u32,
                    contents,
                    bounds_min: brush.aabb_min - Vec3::splat(pad),
                    bounds_max: brush.aabb_max + Vec3::splat(pad),
                });
            }
        }
        Self {
            volumes,
            records,
            leaves,
        }
    }

    pub fn len(&self) -> usize {
        self.volumes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.volumes.is_empty()
    }

    /// The volume at map-order `index`.
    pub fn volume(&self, index: usize) -> Option<&EnvironmentVolume> {
        self.volumes.get(index)
    }

    /// Sum of the brush volumes of the volume at map-order `index`, m³.
    pub fn summed_brush_volume(&self, index: usize) -> Option<f32> {
        self.records
            .get(index)
            .map(|record| record.summed_brush_volume)
    }

    /// Call `visit` with the map-order index of every volume whose contents
    /// intersect `mask` and one of whose brushes contains `point`. A volume with
    /// several brushes containing the point may be visited more than once;
    /// visit order is unspecified, so callers must reduce order-independently.
    pub fn visit_volumes_containing_point(
        &self,
        point: Vec3,
        mask: ContentsMask,
        mut visit: impl FnMut(usize),
    ) {
        for leaf in &self.leaves {
            if !leaf.contents.intersects(mask)
                || point.cmplt(leaf.bounds_min).any()
                || point.cmpgt(leaf.bounds_max).any()
            {
                continue;
            }
            let volume_index = leaf.volume_index as usize;
            let brush = &self.volumes[volume_index].brushes[leaf.brush_index as usize];
            if brush.contains_point(point) {
                visit(volume_index);
            }
        }
    }

    /// Whether volume `a` beats volume `b` under the overlap rule: higher
    /// priority, then smaller summed brush volume, then earlier map order. The
    /// map-order key decides exact ties only.
    pub(crate) fn outranks(&self, a: usize, b: usize) -> bool {
        let priority_a = self.volumes[a].priority;
        let priority_b = self.volumes[b].priority;
        if priority_a != priority_b {
            return priority_a > priority_b;
        }
        let size_a = self.records[a].summed_brush_volume;
        let size_b = self.records[b].summed_brush_volume;
        if size_a != size_b {
            return size_a < size_b;
        }
        a < b
    }
}
