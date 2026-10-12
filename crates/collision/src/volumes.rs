// Environment volume records and the installed volume set with its tree-backed leaf queries.
// See: context/lib/entity_model.md §7 (Environment Volumes)

use glam::Vec3;

use crate::bvh::{AabbTree, TreeEntry};
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
    /// The overlap rule's second key: the sum of the volume's brush volumes,
    /// m³, quantized by [`size_key`].
    size_key: u32,
}

/// Mantissa bits the overlap rule's size key keeps. The summed volume of a
/// brush set depends on where it sits, since its planes round differently at
/// different coordinates, so two identical copies differ in their last bits.
/// Rounded to 8 significant bits (a relative step of 2^-8, about 0.4%), sizes
/// closer than that compare equal and map order decides, which keeps "a pool
/// inside a room wins" while identical copies almost always tie. Two sizes
/// straddling a rounding boundary still order by size, deterministically.
const SIZE_KEY_MANTISSA_BITS: u32 = 8;

/// A monotone, coarse key for a summed brush volume: the f32 rounded to
/// [`SIZE_KEY_MANTISSA_BITS`] significant bits, as its bit pattern (positive
/// floats order as their bits). Zero, negative and NaN sizes key 0; +inf keys
/// above every finite size.
fn size_key(summed: f32) -> u32 {
    if summed.is_nan() || summed <= 0.0 {
        return 0;
    }
    if summed.is_infinite() {
        return u32::MAX;
    }
    let dropped = f32::MANTISSA_DIGITS - 1 - SIZE_KEY_MANTISSA_BITS;
    let half = 1u32 << (dropped - 1);
    (summed.to_bits() + half) & !((1u32 << dropped) - 1)
}

/// Sum of a volume's brush volumes, m³.
fn summed_brush_volume(volume: &EnvironmentVolume) -> f32 {
    volume.brushes.iter().map(ConvexLeaf::volume).sum()
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
            records.push(VolumeRecord {
                size_key: size_key(summed_brush_volume(volume)),
            });
            for (brush_index, brush) in volume.brushes.iter().enumerate() {
                let (min, max) = brush.padded_bounds();
                entries.push(TreeEntry {
                    min,
                    max,
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

    /// Sum of the brush volumes of the volume at map-order `index`, m³: the
    /// value its overlap-rule size key quantizes.
    #[cfg(test)]
    pub(crate) fn summed_brush_volume(&self, index: usize) -> Option<f32> {
        self.volumes.get(index).map(summed_brush_volume)
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

    /// Whether any brush of a volume carrying fluid `fluid` contains `point` (on
    /// a face counts).
    pub(crate) fn fluid_contains_point(&self, fluid: u16, point: Vec3) -> bool {
        let mut inside = false;
        self.tree.visit_point(point, |leaf| {
            if !inside && self.carries_fluid(leaf, fluid) {
                inside = self.brush(leaf).contains_point(point);
            }
        });
        inside
    }

    /// Call `visit` with every brush of a volume carrying fluid `fluid` whose
    /// bounds come within `reach` of `point` on every axis
    /// ([`ConvexLeaf::bounds_within`]). Order unspecified.
    pub(crate) fn visit_fluid_brushes_near(
        &self,
        fluid: u16,
        point: Vec3,
        reach: f32,
        mut visit: impl FnMut(&ConvexLeaf),
    ) {
        let reach_box = Vec3::splat(reach);
        self.tree
            .visit_box(point - reach_box, point + reach_box, |leaf| {
                if !self.carries_fluid(leaf, fluid) {
                    return;
                }
                let brush = self.brush(leaf);
                if brush.bounds_within(point, reach) {
                    visit(brush);
                }
            });
    }

    /// Whether `point` is within `epsilon` of the union of every volume
    /// carrying fluid `fluid` ([`ConvexLeaf::is_near`] on some brush).
    pub(crate) fn fluid_is_near(&self, fluid: u16, point: Vec3, epsilon: f32) -> bool {
        let mut near = false;
        self.visit_fluid_brushes_near(fluid, point, epsilon, |brush| {
            near = near || brush.is_near(point, epsilon);
        });
        near
    }

    /// Conservative signed depth of `point` in the union of every volume
    /// carrying fluid `fluid`, over the brushes whose bounds lie within `reach`
    /// of it: the maximum of their [`ConvexLeaf::plane_depth`]. `None` when no
    /// such brush is that close.
    pub(crate) fn fluid_plane_depth(&self, fluid: u16, point: Vec3, reach: f32) -> Option<f32> {
        let mut depth: Option<f32> = None;
        self.visit_fluid_brushes_near(fluid, point, reach, |brush| {
            let brush_depth = brush.plane_depth(point);
            depth = Some(depth.map_or(brush_depth, |best| best.max(brush_depth)));
        });
        depth
    }

    /// Whether volume `a` beats volume `b` under the overlap rule: higher
    /// priority, then smaller summed brush volume compared by its quantized key
    /// ([`SIZE_KEY_MANTISSA_BITS`]), then earlier map order. Map order decides
    /// only when both keys tie, so the order is total.
    pub(crate) fn outranks(&self, a: usize, b: usize) -> bool {
        let priority_a = self.volumes[a].priority;
        let priority_b = self.volumes[b].priority;
        if priority_a != priority_b {
            return priority_a > priority_b;
        }
        let size_a = self.records[a].size_key;
        let size_b = self.records[b].size_key;
        if size_a != size_b {
            return size_a < size_b;
        }
        a < b
    }

    fn brush(&self, leaf: &LeafRef) -> &ConvexLeaf {
        &self.volumes[leaf.volume as usize].brushes[leaf.brush as usize]
    }

    fn carries_fluid(&self, leaf: &LeafRef, fluid: u16) -> bool {
        leaf.contents.intersects(ContentsMask::FLUID)
            && self.volumes[leaf.volume as usize].fluid == Some(fluid)
    }
}
