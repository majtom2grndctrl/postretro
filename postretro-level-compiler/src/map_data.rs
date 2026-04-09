// Compiler data types: Face, EntityInfo, MapData.
// See: context/lib/index.md

use glam::Vec3;

/// A convex face polygon extracted from a world brush.
#[derive(Debug, Clone)]
pub struct Face {
    /// Vertex positions in winding order.
    pub vertices: Vec<Vec3>,
    /// Face plane normal (unit length).
    pub normal: Vec3,
    /// Face plane distance from origin.
    pub distance: f32,
    /// Texture name from the .map file.
    pub texture: String,
}

/// A convex brush volume defined by its bounding half-planes.
///
/// A point is inside the brush when it is on the back side (negative half-space)
/// of every plane: `dot(point, normal) - distance <= 0` for all planes.
#[derive(Debug, Clone)]
pub struct BrushVolume {
    pub planes: Vec<BrushPlane>,
    /// Axis-aligned bounding box of the brush volume, computed from face vertices
    /// at parse time. Used for AABB pre-filtering in CSG face clipping.
    pub aabb: crate::partition::Aabb,
}

/// A single bounding half-plane of a brush volume.
#[derive(Debug, Clone)]
pub struct BrushPlane {
    /// Outward-facing normal.
    pub normal: Vec3,
    /// Plane distance from origin.
    pub distance: f32,
}

/// Minimal entity info extracted from the .map file.
#[derive(Debug, Clone)]
pub struct EntityInfo {
    pub classname: String,
    pub origin: Option<Vec3>,
}

/// Parsed and classified .map data for downstream compiler stages.
#[derive(Debug)]
pub struct MapData {
    /// Faces from worldspawn brushes, ready for spatial partitioning.
    pub world_faces: Vec<Face>,
    /// Convex brush volumes from worldspawn brushes, for solid/empty classification.
    pub brush_volumes: Vec<BrushVolume>,
    /// Brush count per non-worldspawn entity (stored, not processed in Phase 1).
    pub entity_brushes: Vec<(String, usize)>,
    /// Info for all entities (classnames, origins).
    pub entities: Vec<EntityInfo>,
}
