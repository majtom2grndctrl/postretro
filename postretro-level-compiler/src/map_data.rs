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

/// Light shape. Governs which fields of `MapLight` are meaningful.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LightType {
    /// Omnidirectional point light.
    Point,
    /// Spot light: uses `cone_angle_inner`, `cone_angle_outer`, `cone_direction`.
    Spot,
    /// Parallel directional light (e.g., sunlight). Ignores `falloff_range`;
    /// uses `cone_direction` as the aim vector.
    Directional,
}

/// How intensity falls off with distance. Applies to Point and Spot lights;
/// Directional lights ignore this.
///
/// `falloff_range` is the distance at which the light reaches zero (Linear)
/// or the clamp distance (InverseDistance / InverseSquared).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FalloffModel {
    /// `brightness = 1 - (distance / falloff_range)`, clamped at 0.
    Linear,
    /// `brightness = 1 / distance`, clamped at `falloff_range`.
    InverseDistance,
    /// `brightness = 1 / (distance^2)`, clamped at `falloff_range`.
    InverseSquared,
}

/// Curve-based animation over a repeating cycle.
///
/// Each channel is a `Vec` of samples distributed uniformly over the period.
/// Runtime linearly interpolates between adjacent samples at the current
/// cycle time. `None` channels hold constant for the cycle.
///
/// Format-agnostic — Quake light styles, Doom sector effects, UDMF curves,
/// or hand-authored data all translate into this shape. Translators own
/// their format's preset vocabulary and expand presets into sample curves.
#[derive(Debug, Clone, PartialEq)]
pub struct LightAnimation {
    /// Cycle duration in seconds.
    pub period: f32,
    /// 0-1 offset within the cycle (desync identical presets).
    pub phase: f32,
    /// Intensity multipliers, uniformly spaced over `period`.
    pub brightness: Option<Vec<f32>>,
    /// Linear RGB overrides, uniformly spaced over `period`.
    pub color: Option<Vec<[f32; 3]>>,
    /// Initial runtime on/off state. `true` (the default) = lit at map load.
    /// Authored via the `_start_inactive` FGD key: key absent or 0 → `true`,
    /// key = 1 → `false`. Scripts toggle the GPU mirror at runtime; only the
    /// initial value is baked.
    pub start_active: bool,
}

/// Format-agnostic light record. The SH baker and runtime direct path both
/// consume `Vec<MapLight>`; neither sees source-format vocabulary.
///
/// When `bake_only` is true the light contributes to the SH irradiance volume
/// bake only and is excluded from the runtime direct-lighting path (AlphaLights
/// and LightInfluence PRL sections).
///
/// See `context/plans/in-progress/lighting-foundation/1-fgd-canonical.md`
/// §Map light format for the full design rationale.
#[derive(Debug, Clone, PartialEq)]
pub struct MapLight {
    /// Position in engine space (Y-up), meters. Directional lights still
    /// carry a position for probe/debug purposes but it is not used for
    /// lighting math.
    pub origin: DVec3,
    pub light_type: LightType,

    /// Linear brightness multiplier applied to `color`. Range 0–1+ (close-
    /// range InverseSquared falloff legitimately exceeds 1.0). Format-
    /// specific authoring conventions (Quake's 0–300 radiosity-energy scale,
    /// etc.) are normalized at the translator boundary; downstream consumers
    /// (SH baker, direct light shader) treat this as a plain linear scalar
    /// with no further scaling.
    pub intensity: f32,
    /// Linear RGB, 0-1.
    pub color: [f32; 3],

    /// Falloff model for Point and Spot lights. Ignored for Directional.
    pub falloff_model: FalloffModel,
    /// Distance at which the light reaches zero (Linear) or the clamp
    /// distance (InverseDistance / InverseSquared). Meters. Must be `> 0`
    /// for Point and Spot lights; unused for Directional.
    pub falloff_range: f32,

    /// Inner cone half-angle in radians. `Some` only for Spot lights.
    pub cone_angle_inner: Option<f32>,
    /// Outer cone half-angle in radians. `Some` only for Spot lights.
    pub cone_angle_outer: Option<f32>,
    /// Normalized aim vector in engine space. `Some` for Spot and
    /// Directional lights; `None` for Point.
    pub cone_direction: Option<[f32; 3]>,

    /// Animation curves. `None` means constant light.
    pub animation: Option<LightAnimation>,

    /// All FGD-authored lights cast shadows by default. The flag exists so
    /// transient gameplay lights (Milestone 6+) can opt out programmatically.
    pub cast_shadows: bool,

    /// When true, the light participates only in the SH irradiance volume bake
    /// and is excluded from the runtime direct-lighting path (AlphaLights PRL
    /// section and LightInfluence PRL section). Defaults to false.
    pub bake_only: bool,

    /// When true, the light is treated as dynamic — evaluated at runtime via
    /// the direct lighting path with an optional shadow-map pool slot — and
    /// contributes nothing to the offline lightmap / SH bake. When false
    /// (the default), the light is static: it bakes into the lightmap and SH
    /// irradiance volume and emits no runtime shadow. Authored via the
    /// `_dynamic` FGD property on `light`, `light_spot`, and `light_sun`.
    /// See `context/plans/ready/lighting-dynamic-flag/index.md`.
    pub is_dynamic: bool,
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
    /// Format-agnostic lights translated from source-format entities. Feeds
    /// the SH baker (sub-plan 2) and the runtime direct-lighting path
    /// (sub-plan 3) via the AlphaLights PRL section.
    pub lights: Vec<MapLight>,
}
