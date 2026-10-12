// Shared builders for this crate's unit tests: axis-aligned box leaves and volume records.
// See: context/lib/testing_guide.md §4

use glam::Vec3;

use crate::leaf::{ConvexLeaf, LeafPlane};
use crate::volumes::EnvironmentVolume;

/// An axis-aligned box as a six-plane convex leaf.
pub(crate) fn axis_box(min: Vec3, max: Vec3) -> ConvexLeaf {
    let plane = |normal: Vec3, distance: f32| LeafPlane { normal, distance };
    ConvexLeaf {
        planes: vec![
            plane(Vec3::X, max.x),
            plane(Vec3::NEG_X, -min.x),
            plane(Vec3::Y, max.y),
            plane(Vec3::NEG_Y, -min.y),
            plane(Vec3::Z, max.z),
            plane(Vec3::NEG_Z, -min.z),
        ],
        aabb_min: min,
        aabb_max: max,
    }
}

/// A priority-0 volume over `brushes` that sets no field; callers fill the fields
/// under test with struct-update syntax.
pub(crate) fn volume(brushes: Vec<ConvexLeaf>) -> EnvironmentVolume {
    EnvironmentVolume {
        source_entity: 0,
        priority: 0,
        gravity: None,
        push: None,
        fluid: None,
        brushes,
    }
}
