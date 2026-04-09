// BSP partition types: tree nodes, leaves, bounding volumes.
// See: context/lib/build_pipeline.md §PRL

use crate::map_data::Face;
use glam::Vec3;

/// Axis-aligned bounding box.
#[derive(Debug, Clone)]
pub struct Aabb {
    pub min: Vec3,
    pub max: Vec3,
}

impl Aabb {
    pub fn empty() -> Self {
        Self {
            min: Vec3::splat(f32::INFINITY),
            max: Vec3::splat(f32::NEG_INFINITY),
        }
    }

    pub fn expand_point(&mut self, p: Vec3) {
        self.min = self.min.min(p);
        self.max = self.max.max(p);
    }

    pub fn expand_aabb(&mut self, other: &Aabb) {
        self.min = self.min.min(other.min);
        self.max = self.max.max(other.max);
    }

    pub fn centroid(&self) -> Vec3 {
        (self.min + self.max) * 0.5
    }

    pub fn is_valid(&self) -> bool {
        self.min.x <= self.max.x
            && self.min.y <= self.max.y
            && self.min.z <= self.max.z
            && self.min.is_finite()
            && self.max.is_finite()
    }

    /// Test whether two AABBs overlap (share any interior volume).
    pub fn intersects(&self, other: &Aabb) -> bool {
        self.min.x <= other.max.x
            && self.max.x >= other.min.x
            && self.min.y <= other.max.y
            && self.max.y >= other.min.y
            && self.min.z <= other.max.z
            && self.max.z >= other.min.z
    }

    /// Build an AABB from a set of points.
    pub fn from_points(points: &[Vec3]) -> Self {
        let mut aabb = Self::empty();
        for &p in points {
            aabb.expand_point(p);
        }
        aabb
    }
}

/// Reference to a BSP tree child (node or leaf).
#[derive(Debug, Clone)]
pub enum BspChild {
    Node(usize),
    Leaf(usize),
}

/// Interior BSP node with a splitting plane.
#[derive(Debug, Clone)]
pub struct BspNode {
    pub plane_normal: Vec3,
    pub plane_distance: f32,
    pub front: BspChild,
    pub back: BspChild,
    /// Parent node index (None for root).
    pub parent: Option<usize>,
}

/// BSP leaf containing face references and a bounding volume.
#[derive(Debug, Clone)]
pub struct BspLeaf {
    pub face_indices: Vec<usize>,
    pub bounds: Aabb,
    /// True if this leaf represents solid space (inside a brush volume).
    /// Solid leaves block portal generation and are excluded from PVS.
    pub is_solid: bool,
}

/// Arena-based BSP tree.
#[derive(Debug)]
pub struct BspTree {
    pub nodes: Vec<BspNode>,
    pub leaves: Vec<BspLeaf>,
}

/// Complete partition result: BSP tree and post-split faces.
#[derive(Debug)]
pub struct PartitionResult {
    pub tree: BspTree,
    pub faces: Vec<Face>,
}
