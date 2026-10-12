// Environment volume records and the installed volume set with its tree-backed leaf queries.
// See: context/plans/in-progress/E24--water-and-environment-volumes/index.md

use glam::Vec3;

use crate::bvh::{AabbTree, TreeEntry};
use crate::leaf::{ContentsMask, ConvexLeaf};
use crate::line::{BrushInterval, clip_line};

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

/// The tree payload: one brush of one volume, with its volume's contents so a
/// masked query skips it without touching the volume.
#[derive(Debug, Clone, Copy, PartialEq)]
struct LeafRef {
    volume: u32,
    brush: u32,
    contents: ContentsMask,
}

/// The installed set of environment volumes for one level. Built once at
/// install, with every brush a leaf of a load-time tree; queries borrow it and
/// never allocate.
#[derive(Debug, Clone, PartialEq)]
pub struct EnvironmentVolumes {
    volumes: Vec<EnvironmentVolume>,
    records: Vec<VolumeRecord>,
    tree: AabbTree<LeafRef>,
}

impl Default for EnvironmentVolumes {
    fn default() -> Self {
        Self::empty()
    }
}

/// Leaf bounds are widened by this relative margin so rounding in a supplied
/// AABB can never reject a point or line the planes contain.
const BOUNDS_RELATIVE_PAD: f32 = 1.0e-5;

impl EnvironmentVolumes {
    /// No volumes: every query reports nothing, so resolution yields level defaults.
    pub const fn empty() -> Self {
        Self {
            volumes: Vec::new(),
            records: Vec::new(),
            tree: AabbTree::empty(),
        }
    }

    /// Build the query set: precompute each volume's summed brush volume and
    /// build the leaf tree over every brush's padded bounds. Deterministic in the
    /// volume list, so every peer builds the same set.
    pub fn build(volumes: Vec<EnvironmentVolume>) -> Self {
        let mut records = Vec::with_capacity(volumes.len());
        let mut entries = Vec::new();
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
                entries.push(TreeEntry {
                    min: brush.aabb_min - Vec3::splat(pad),
                    max: brush.aabb_max + Vec3::splat(pad),
                    payload: LeafRef {
                        volume: volume_index as u32,
                        brush: brush_index as u32,
                        contents,
                    },
                });
            }
        }
        Self {
            volumes,
            records,
            tree: AabbTree::build(entries),
        }
    }

    pub fn len(&self) -> usize {
        self.volumes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.volumes.is_empty()
    }

    /// Number of brush leaves in the query tree.
    pub fn leaf_count(&self) -> usize {
        self.tree.len()
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
    /// intersect `mask` and one of whose brushes contains `point` (a point on a
    /// face is inside). A volume with several brushes containing the point may be
    /// visited more than once; visit order is unspecified, so callers must reduce
    /// order-independently.
    pub fn visit_volumes_containing_point(
        &self,
        point: Vec3,
        mask: ContentsMask,
        mut visit: impl FnMut(usize),
    ) {
        self.tree.visit_point(point, |leaf| {
            if !leaf.contents.intersects(mask) {
                return;
            }
            if self.brush(leaf).contains_point(point) {
                visit(leaf.volume as usize);
            }
        });
    }

    /// Call `visit(volume, brush_index, brush)` for every brush of a volume whose
    /// contents intersect `mask` and whose padded bounds meet the segment
    /// `origin + t * dir`, `t` in `[t_min, t_max]`. A prefilter: the caller clips
    /// the brush itself. Order unspecified.
    pub(crate) fn visit_brushes_on_line(
        &self,
        origin: Vec3,
        dir: Vec3,
        t_min: f32,
        t_max: f32,
        mask: ContentsMask,
        mut visit: impl FnMut(usize, u32, &ConvexLeaf),
    ) {
        self.tree.visit_line(origin, dir, t_min, t_max, |leaf| {
            if leaf.contents.intersects(mask) {
                visit(leaf.volume as usize, leaf.brush, self.brush(leaf));
            }
        });
    }

    /// Feed `sink` the clipped interval of every brush of volume `volume` along
    /// `origin + t * dir`, `t` in `[t_min, t_max]`. Order unspecified; brushes the
    /// line misses yield nothing.
    pub(crate) fn volume_line_intervals(
        &self,
        volume: usize,
        origin: Vec3,
        dir: Vec3,
        t_min: f32,
        t_max: f32,
        sink: &mut dyn FnMut(BrushInterval),
    ) {
        self.tree.visit_line(origin, dir, t_min, t_max, |leaf| {
            if leaf.volume as usize != volume {
                return;
            }
            if let Some(interval) = clip_line(&self.brush(leaf).planes, origin, dir, t_min, t_max) {
                sink(BrushInterval {
                    brush: leaf.brush,
                    interval,
                });
            }
        });
    }

    /// Whether any brush of volume `volume` contains `point` (on a face counts).
    pub(crate) fn volume_contains_point(&self, volume: usize, point: Vec3) -> bool {
        let mut inside = false;
        self.tree.visit_point(point, |leaf| {
            if !inside && leaf.volume as usize == volume {
                inside = self.brush(leaf).contains_point(point);
            }
        });
        inside
    }

    /// Signed depth of `point` in the union of volume `index`'s brushes, metres:
    /// the maximum over its brushes of the brush's smallest normalized plane
    /// distance `-(dot(p, n̂) - d̂)`. Positive inside, zero on the boundary,
    /// negative outside. Both signs are conservative: the magnitude never
    /// exceeds the true distance to the union's boundary, and equals it for a
    /// point straight off one face of a lone brush (a point above a pool's flat
    /// top reads minus its height above it). Inside near a seam between two
    /// brushes of one volume it reads no deeper than the distance to the seam.
    /// `None` for an index out of range or a volume with no brushes.
    pub fn signed_depth(&self, index: usize, point: Vec3) -> Option<f32> {
        let volume = self.volumes.get(index)?;
        volume
            .brushes
            .iter()
            .map(|brush| brush_signed_depth(brush, point))
            .reduce(f32::max)
    }

    /// Whether volume `a` beats volume `b` under the overlap rule: higher
    /// priority, then smaller summed brush volume, then earlier map order. The
    /// map-order key decides exact ties only, so the order is total.
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

    fn brush(&self, leaf: &LeafRef) -> &ConvexLeaf {
        &self.volumes[leaf.volume as usize].brushes[leaf.brush as usize]
    }
}

/// A brush's smallest normalized inward plane distance at `point`. A plane with
/// a zero or non-finite normal bounds nothing and is skipped.
fn brush_signed_depth(brush: &ConvexLeaf, point: Vec3) -> f32 {
    brush
        .planes
        .iter()
        .filter_map(|plane| {
            let length = plane.normal.length();
            (length > 0.0 && length.is_finite()).then(|| -plane.side(point) / length)
        })
        .fold(f32::INFINITY, f32::min)
}
