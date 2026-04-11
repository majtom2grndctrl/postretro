// Compiler data types shared across parse, partition, and pack stages:
// BrushVolume, BrushSide, BrushPlane, Face, TextureProjection, EntityInfo, MapData.
// See: context/lib/build_pipeline.md §PRL Compilation

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

/// A convex world face polygon emitted by brush-side projection.
///
/// Faces are produced at the tail of the partition stage by clipping each
/// brush side against the BSP tree, then routing the surviving fragments
/// into empty leaves. The face stores its plane, vertices, and source-brush
/// attribution for the coplanar tiebreaker that runs at leaf emission.
#[derive(Debug, Clone)]
pub struct Face {
    /// Vertex positions in winding order (engine space, Y-up, meters).
    pub vertices: Vec<DVec3>,
    /// Face plane normal (unit length, engine space). Points outward from
    /// the source brush — same orientation as the brush side it came from.
    pub normal: DVec3,
    /// Face plane distance from origin (engine space).
    pub distance: f64,
    /// Texture name from the .map file.
    pub texture: String,
    /// Texture projection parameters from the .map file (Quake space).
    /// `geometry.rs` converts engine-space vertices back to Quake space
    /// before applying these parameters during UV bake.
    pub tex_projection: TextureProjection,
    /// Index of the source brush in `MapData::brush_volumes`. Used by the
    /// coplanar dedup rule in brush-side projection: when two brushes share
    /// the same oriented plane in the same leaf, the lower index wins.
    pub brush_index: usize,
}

/// A convex brush volume defined by its bounding half-planes.
///
/// A point is inside the brush when it lies on the back side of every plane
/// (`dot(point, normal) - distance <= 0` for all planes). Brush-volume BSP
/// construction partitions space using these planes; brush-side projection
/// reads the textured `sides` to emit world faces.
#[derive(Debug, Clone)]
pub struct BrushVolume {
    pub planes: Vec<BrushPlane>,
    /// Textured polygons bounding this brush, one per non-degenerate face
    /// the parser emitted. The `sides` and `planes` lists are not index-
    /// aligned: degenerate sides are skipped while their planes survive.
    pub sides: Vec<BrushSide>,
    /// Axis-aligned bounding box of the volume in engine space. Used for
    /// candidate-brush pruning during BSP descent and as one input to the
    /// world AABB derivation in `partition::brush_bsp`.
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

/// A textured polygon on one of a brush's bounding planes.
///
/// Brush sides are the input to brush-side projection: each side's polygon
/// is walked through the BSP tree, accumulated into a visible hull, then
/// distributed back into empty leaves as one or more world `Face`s.
#[derive(Debug, Clone)]
pub struct BrushSide {
    /// Vertex positions in winding order (engine space, Y-up, meters).
    pub vertices: Vec<DVec3>,
    /// Outward-facing plane normal (unit length, engine space).
    pub normal: DVec3,
    /// Plane distance from origin (engine space).
    pub distance: f64,
    /// Texture name from the .map file.
    pub texture: String,
    /// Texture projection parameters from the .map file (Quake space).
    pub tex_projection: TextureProjection,
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
    /// Convex brush volumes from worldspawn brushes. Each volume carries its
    /// bounding planes, AABB, and textured sides — the BSP partition, face
    /// extraction, and portal stages all consume this representation.
    pub brush_volumes: Vec<BrushVolume>,
    /// Brush count per non-worldspawn entity. Stored for diagnostics; entity
    /// brushes do not flow into worldspawn BSP construction.
    pub entity_brushes: Vec<(String, usize)>,
    /// Info for all entities (classnames, origins).
    pub entities: Vec<EntityInfo>,
}
