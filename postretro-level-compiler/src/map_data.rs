// Compiler data types: Face, EntityInfo, MapData.
// See: context/lib/index.md

use glam::DVec3;

impl Default for TextureProjection {
    fn default() -> Self {
        TextureProjection::Standard {
            u_offset: 0.0,
            v_offset: 0.0,
            angle: 0.0,
            scale_u: 1.0,
            scale_v: 1.0,
        }
    }
}

/// Texture projection data extracted from the .map file, stored in Quake space.
///
/// Two variants match the .map format (Standard vs Valve). UV computation in
/// `geometry.rs` handles both. Stored in Quake-space coordinates because the
/// projection math depends on matching the original axis convention.
#[derive(Debug, Clone)]
pub enum TextureProjection {
    /// Standard (idTech2) format: project onto closest axis-aligned plane,
    /// then apply rotation, scale, and offset.
    Standard {
        u_offset: f64,
        v_offset: f64,
        angle: f64,
        scale_u: f64,
        scale_v: f64,
    },
    /// Valve 220 format: explicit U/V projection axes with per-axis offset.
    Valve {
        u_axis: DVec3,
        u_offset: f64,
        v_axis: DVec3,
        v_offset: f64,
        scale_u: f64,
        scale_v: f64,
    },
}

/// A convex face polygon extracted from a world brush.
#[derive(Debug, Clone)]
pub struct Face {
    /// Vertex positions in winding order (engine space, Y-up, meters).
    pub vertices: Vec<DVec3>,
    /// Face plane normal (unit length, engine space).
    pub normal: DVec3,
    /// Face plane distance from origin (engine space).
    pub distance: f64,
    /// Texture name from the .map file.
    pub texture: String,
    /// Texture projection parameters from the .map file (Quake space).
    /// UV computation in `geometry.rs` converts engine-space vertices back to
    /// Quake space before applying these parameters.
    pub tex_projection: TextureProjection,
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
    pub normal: DVec3,
    /// Plane distance from origin.
    pub distance: f64,
}

/// Minimal entity info extracted from the .map file.
#[derive(Debug, Clone)]
pub struct EntityInfo {
    pub classname: String,
    pub origin: Option<DVec3>,
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
