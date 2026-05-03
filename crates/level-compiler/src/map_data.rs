// Compiler data types shared across parse, partition, and pack stages.
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

/// Texture projection extracted from the .map file.
///
/// Stored in Quake-space coordinates because the projection math depends on
/// matching the original axis convention. UV computation in `geometry.rs`
/// converts engine-space vertices back to Quake space before applying these.
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
/// Produced by clipping each brush side against the BSP tree and routing
/// surviving fragments into empty leaves. Carries source-brush attribution for
/// the coplanar tiebreaker at leaf emission.
#[derive(Debug, Clone)]
pub struct Face {
    /// Vertex positions in winding order (engine space, Y-up, meters).
    pub vertices: Vec<DVec3>,
    /// Face plane normal (unit length, engine space). Points outward from the source brush.
    pub normal: DVec3,
    /// Face plane distance from origin (engine space).
    pub distance: f64,
    pub texture: String,
    pub tex_projection: TextureProjection,
    /// Index of the source brush in `MapData::brush_volumes`. Used by the
    /// coplanar dedup rule: when two brushes share the same oriented plane
    /// in the same leaf, the lower index wins.
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
    pub normal: DVec3,
    pub distance: f64,
}

/// A textured polygon on one of a brush's bounding planes; input to brush-side projection.
#[derive(Debug, Clone)]
pub struct BrushSide {
    /// Vertex positions in winding order (engine space, Y-up, meters).
    pub vertices: Vec<DVec3>,
    /// Outward-facing plane normal (unit length, engine space).
    pub normal: DVec3,
    /// Plane distance from origin (engine space).
    pub distance: f64,
    pub texture: String,
    pub tex_projection: TextureProjection,
}

#[derive(Debug, Clone)]
pub struct EntityInfo {
    pub classname: String,
    pub origin: Option<DVec3>,
}

/// One non-light, non-worldspawn map entity collected for the runtime
/// classname dispatch. `angles` is engine-convention Euler radians (pitch,
/// yaw, roll) — the format adapter (`format/quake_map.rs`) converts at the
/// translation boundary so downstream stages and the runtime see no
/// source-format axis convention.
///
/// `key_values` has the reserved authoring keys stripped (`classname`,
/// `origin`, `_tags`, `angle`, `angles`, `mangle`); their data is hoisted into
/// dedicated fields. `tags` is the pre-split `_tags` list.
#[derive(Debug, Clone)]
pub struct MapEntityRecord {
    pub classname: String,
    pub origin: DVec3,
    pub angles: [f32; 3],
    // Vec preserves authoring order and allows duplicate keys (Quake .map format permits them).
    // Converted to HashMap at the scripting boundary; last occurrence wins on duplicates.
    pub key_values: Vec<(String, String)>,
    pub tags: Vec<String>,
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

/// How intensity falls off with distance. Directional lights ignore this.
///
/// `falloff_range` is the zero-intensity distance (Linear) or the clamp
/// distance (InverseDistance / InverseSquared).
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
/// Each channel holds uniform samples over `period`; runtime linearly
/// interpolates between adjacent samples. `None` channels are constant.
///
/// Format-agnostic: Quake light styles, Doom sector effects, UDMF curves,
/// and hand-authored data all translate to this shape. Translators own their
/// format's preset vocabulary and expand presets into sample curves.
#[derive(Debug, Clone, PartialEq)]
pub struct LightAnimation {
    pub period: f32,
    /// 0-1 offset within the cycle — desync otherwise-identical presets.
    pub phase: f32,
    pub brightness: Option<Vec<f32>>,
    pub color: Option<Vec<[f32; 3]>>,
    /// Animated spot-light aim vectors, uniformly spaced over `period`.
    /// Each sample must be unit length — the authoring seam (either the
    /// `direction_curve` FGD key or the `set_light_animation` scripting
    /// primitive) normalizes at write time. The GPU evaluator does not
    /// re-normalize. `None` means the light keeps its static `cone_direction`.
    pub direction: Option<Vec<[f32; 3]>>,
    /// Initial on/off state at map load. Authored via `_start_inactive` FGD key
    /// (absent/0 → true, 1 → false). Scripts toggle the GPU mirror at runtime;
    /// only this initial value is baked.
    pub start_active: bool,
}

/// Format-agnostic light record. The SH baker and runtime direct path both
/// consume `Vec<MapLight>`; neither sees source-format vocabulary.
///
/// See `context/lib/build_pipeline.md` §Custom FGD and §PRL Compilation.
#[derive(Debug, Clone, PartialEq)]
pub struct MapLight {
    /// Position in engine space (Y-up), meters. Directional lights still
    /// carry a position for probe/debug purposes but it is not used for
    /// lighting math.
    pub origin: DVec3,
    pub light_type: LightType,

    /// Linear brightness multiplier, 0–1+ (InverseSquared close-range can
    /// legitimately exceed 1.0). Format-specific scales (e.g. Quake's 0–300)
    /// are normalized at the translator boundary; no further scaling downstream.
    pub intensity: f32,
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

    pub animation: Option<LightAnimation>,

    /// All FGD-authored lights cast shadows by default. The flag lets transient
    /// gameplay lights opt out programmatically.
    pub cast_shadows: bool,

    /// When true, participates only in the SH irradiance bake; excluded from
    /// the runtime direct-lighting path (AlphaLights and LightInfluence PRL
    /// sections). Defaults to false.
    pub bake_only: bool,

    /// When true, the light is treated as dynamic — evaluated at runtime via
    /// the direct lighting path with an optional shadow-map pool slot — and
    /// contributes nothing to the offline lightmap / SH bake. When false
    /// (the default), the light is static: it bakes into the lightmap and SH
    /// irradiance volume and emits no runtime shadow. Authored via the
    /// `_dynamic` FGD property on `light`, `light_spot`, and `light_sun`.
    /// See `context/lib/build_pipeline.md §Custom FGD`.
    pub is_dynamic: bool,

    /// Author-supplied script tags (FGD `_tags`, space-delimited). Carried
    /// through the PRL `LightTags` section so the runtime can register each
    /// light with the scripting entity registry. An entity matches
    /// `world.query({ component: "light", tag: "t" })` when any of its tags
    /// equals `"t"`. Empty means untagged.
    pub tags: Vec<String>,
}

// ---------------------------------------------------------------------------
// Keyframe resampling
//
// Authored `*_curve` FGD values carry timestamped keyframes. The canonical
// `LightAnimation` stores uniform samples along `period_ms`. `resample_keyframes`
// converts the former to the latter via Catmull-Rom over authored timestamps,
// so the wire format and GPU evaluator stay unchanged.

pub const KEYFRAME_RESAMPLE_RATE_HZ: u32 = 32;

/// Maximum number of samples in the resampled uniform curve regardless of
/// period. Bounds descriptor buffer growth as authored maps scale.
pub const KEYFRAME_RESAMPLE_MAX_SAMPLES: usize = 256;

/// Catmull-Rom resampling composes four scalar lerps; implementors supply the lerp.
pub trait Lerp: Sized + Clone {
    fn lerp(a: &Self, b: &Self, t: f32) -> Self;
}

impl Lerp for f32 {
    fn lerp(a: &f32, b: &f32, t: f32) -> f32 {
        a + (b - a) * t
    }
}

impl Lerp for [f32; 3] {
    fn lerp(a: &[f32; 3], b: &[f32; 3], t: f32) -> [f32; 3] {
        [
            a[0] + (b[0] - a[0]) * t,
            a[1] + (b[1] - a[1]) * t,
            a[2] + (b[2] - a[2]) * t,
        ]
    }
}

/// Catmull-Rom (uniform, tension 0.5) evaluated from four control values at
/// parameter `t ∈ [0, 1]` between `p1` and `p2`.
fn catmull_rom<T: Lerp>(p0: &T, p1: &T, p2: &T, p3: &T, t: f32) -> T {
    // Standard uniform Catmull-Rom basis, expanded into four lerps so the
    // implementation reuses the trait rather than requiring arithmetic ops.
    //   P(t) = 0.5 * ( (2 p1)
    //                + (-p0 + p2) t
    //                + (2 p0 - 5 p1 + 4 p2 - p3) t^2
    //                + (-p0 + 3 p1 - 3 p2 + p3) t^3 )
    // Factored form (Barry–Goldman pyramid) using only lerps.
    // `lerp(p0, p1, t + 1.0)` matches the standard basis coefficient for p0
    // in uniform Catmull-Rom: at t=0 the weight is 1·p1, consistent with the
    // polynomial above. See de Boor / Barry–Goldman §3 for derivation.
    let a1 = T::lerp(p0, p1, t + 1.0);
    let a2 = T::lerp(p1, p2, t);
    let a3 = T::lerp(p2, p3, t - 1.0);
    let b1 = T::lerp(&a1, &a2, (t + 1.0) * 0.5);
    let b2 = T::lerp(&a2, &a3, t * 0.5);
    T::lerp(&b1, &b2, t)
}

/// Resample timestamped keyframes into a uniformly-spaced sample buffer.
///
/// `keyframes` must be non-empty with monotonically increasing timestamps.
/// Output is capped at `KEYFRAME_RESAMPLE_MAX_SAMPLES`. Endpoints are reflected
/// (first/last keyframe duplicated) so Catmull-Rom is defined at the boundaries.
/// Times outside the authored range clamp to the nearest keyframe.
pub fn resample_keyframes<T: Lerp>(
    keyframes: &[(f32, T)],
    period_ms: f32,
    samples_per_second: u32,
) -> Vec<T> {
    debug_assert!(!keyframes.is_empty(), "keyframes must not be empty");
    debug_assert!(period_ms > 0.0, "period_ms must be positive");

    let raw_count = (period_ms / 1000.0 * samples_per_second as f32).round() as usize;
    let sample_count = raw_count.clamp(1, KEYFRAME_RESAMPLE_MAX_SAMPLES);

    let mut out = Vec::with_capacity(sample_count);
    for i in 0..sample_count {
        let t_ms = i as f32 * period_ms / sample_count as f32;
        out.push(sample_catmull_rom_at(keyframes, t_ms));
    }
    out
}

fn sample_catmull_rom_at<T: Lerp>(keyframes: &[(f32, T)], t_ms: f32) -> T {
    // Single-keyframe curve is constant.
    if keyframes.len() == 1 {
        return keyframes[0].1.clone();
    }

    // Clamp outside the authored range.
    if t_ms <= keyframes[0].0 {
        return keyframes[0].1.clone();
    }
    if t_ms >= keyframes[keyframes.len() - 1].0 {
        return keyframes[keyframes.len() - 1].1.clone();
    }

    // Find the segment [i, i+1] containing t_ms.
    let mut i = 0;
    while i + 1 < keyframes.len() && keyframes[i + 1].0 < t_ms {
        i += 1;
    }

    let (t1, p1) = (&keyframes[i].0, &keyframes[i].1);
    let (t2, p2) = (&keyframes[i + 1].0, &keyframes[i + 1].1);

    let segment_len = (t2 - t1).max(1e-6);
    let t = ((t_ms - t1) / segment_len).clamp(0.0, 1.0);

    // Reflect boundaries: when there's no neighbor on one side, duplicate the
    // endpoint. This produces a tangent of zero at the boundary, matching the
    // typical "held endpoint" convention.
    let p0 = if i == 0 { p1 } else { &keyframes[i - 1].1 };
    let p3 = if i + 2 >= keyframes.len() {
        p2
    } else {
        &keyframes[i + 2].1
    };

    catmull_rom(p0, p1, p2, p3, t)
}

/// Parsed and classified .map data for downstream compiler stages.
#[derive(Debug)]
pub struct MapData {
    /// Convex brush volumes from worldspawn brushes. BSP partition, face
    /// extraction, and portal stages all consume this representation.
    pub brush_volumes: Vec<BrushVolume>,
    /// Brush count per non-worldspawn entity. Diagnostic only; entity brushes
    /// do not flow into worldspawn BSP construction.
    pub entity_brushes: Vec<(String, usize)>,
    pub entities: Vec<EntityInfo>,
    pub lights: Vec<MapLight>,
    /// Optional path to the entry-point `.ts` script, taken verbatim from the
    /// `script` worldspawn KVP. Resolved relative to the `.map` file's
    /// directory by the compile pipeline. `None` when the worldspawn entity
    /// has no `script` property. See `context/lib/scripting.md`.
    pub script: Option<String>,
    /// Optional path to a data-script source file (`.ts`/`.js`/`.luau`), taken
    /// verbatim from the `data_script` worldspawn KVP. Resolved relative to the
    /// `.map` file's directory by the compile pipeline; the compiled output is
    /// embedded as the PRL `DataScript` section. `None` when the worldspawn
    /// entity has no `data_script` property. See `context/lib/scripting.md`
    /// §Data context.
    pub data_script: Option<String>,
    /// Non-light, non-worldspawn map entities collected for the runtime
    /// classname dispatch. Brush entities (e.g. `fog_volume`) are excluded
    /// — they are resolved separately during BSP construction. See the
    /// MapEntity PRL section in `context/lib/build_pipeline.md`.
    pub map_entities: Vec<MapEntityRecord>,
    /// Per-region volumetric fog volumes resolved from `fog_volume` brush
    /// entities and `fog_lamp` / `fog_tube` point entities. AABBs are in engine
    /// space (Y-up, meters); colour is linear 0–1. See
    /// `context/lib/build_pipeline.md`.
    pub fog_volumes: Vec<MapFogVolume>,
    /// Worldspawn `fog_pixel_scale` (1=full-res, 8=coarsest); clamped to 1..=8.
    /// Default 4 when the worldspawn entity does not author the key.
    pub fog_pixel_scale: u32,
}

/// One fog volume entity, resolved to an AABB (and optionally a convex plane
/// set) in engine space. Carries linear-RGB colour and the per-volume
/// density/falloff parameters authored on the entity. See
/// `parse::parse_map_file`.
#[derive(Debug, Clone, PartialEq)]
pub struct MapFogVolume {
    /// AABB minimum corner (engine space, meters). Conservative bound used by
    /// per-leaf mask computation and runtime culling.
    pub min: [f32; 3],
    /// AABB maximum corner (engine space, meters).
    pub max: [f32; 3],
    /// Linear RGB 0–1.
    pub color: [f32; 3],
    pub density: f32,
    pub falloff: f32,
    pub scatter: f32,
    pub height_gradient: f32,
    pub radial_falloff: f32,
    /// Convex bounding planes (engine space). A point `p` is inside the volume
    /// iff `dot(p, n) <= d` for every `(nx, ny, nz, d)` plane. Empty means the
    /// AABB is the only bound (semantic-entity / box case).
    pub planes: Vec<[f32; 4]>,
    /// Author-supplied script tags (FGD `_tags`, pre-split on whitespace).
    pub tags: Vec<String>,
}

#[cfg(test)]
mod keyframe_resample_tests {
    use super::*;

    fn sample_at(samples: &[f32], period_ms: f32, t_ms: f32) -> f32 {
        let idx = ((t_ms / period_ms) * samples.len() as f32).round() as usize;
        samples[idx.min(samples.len() - 1)]
    }

    #[test]
    fn scalar_resample_count_matches_32hz_rate() {
        let keyframes = vec![(0.0_f32, 0.0_f32), (1000.0, 1.0)];
        let out = resample_keyframes(&keyframes, 1000.0, 32);
        assert_eq!(out.len(), 32);
    }

    #[test]
    fn resample_capped_at_256_samples() {
        // 10-second period at 32 Hz would want 320 samples; cap at 256.
        let keyframes = vec![(0.0_f32, 0.0_f32), (10_000.0, 1.0)];
        let out = resample_keyframes(&keyframes, 10_000.0, 32);
        assert_eq!(out.len(), KEYFRAME_RESAMPLE_MAX_SAMPLES);
    }

    #[test]
    fn scalar_resample_matches_authored_keyframes_within_one_percent() {
        // Authored keyframes at 0/500/1000 ms over a 1 s period. Sampling at
        // 32 Hz gives 32 uniform samples covering [0, 1000) (the last sample
        // is at t = 31 * 1000/32 ≈ 968.75 ms; t=1000 ms wraps to t=0 at
        // runtime). Check keyframes that fall within the sampled interior.
        let keyframes = vec![(0.0_f32, 0.1_f32), (500.0, 1.0), (1000.0, 0.3)];
        let period_ms = 1000.0;
        let out = resample_keyframes(&keyframes, period_ms, 32);

        // Interior keyframes land exactly on a sample index or between two;
        // Catmull-Rom passes through control points, so the nearest sample
        // must be within 1% of the authored value.
        for (t_ms, expected) in [(0.0_f32, 0.1_f32), (500.0, 1.0)] {
            let got = sample_at(&out, period_ms, t_ms);
            let tolerance = 0.01_f32;
            assert!(
                (got - expected).abs() < tolerance,
                "at t={t_ms} ms: got {got}, expected {expected} (tol {tolerance})"
            );
        }
    }

    #[test]
    fn vec3_resample_interpolates_between_endpoints() {
        let keyframes = vec![
            (0.0_f32, [0.0_f32, 0.0, 0.0]),
            (1000.0, [1.0_f32, 1.0, 1.0]),
        ];
        let out = resample_keyframes(&keyframes, 1000.0, 32);
        // First sample is the first keyframe (t=0 clamps to keyframe[0]).
        assert_eq!(out[0], [0.0, 0.0, 0.0]);
        // Midpoint should be roughly halfway (Catmull-Rom with reflected
        // endpoints on a linear ramp reproduces the linear midpoint).
        let mid = out[out.len() / 2];
        for c in 0..3 {
            assert!(
                (mid[c] - 0.5).abs() < 0.1,
                "midpoint channel {c} = {}, expected ~0.5",
                mid[c]
            );
        }
    }

    #[test]
    fn single_keyframe_produces_constant_curve() {
        let keyframes = vec![(0.0_f32, 0.42_f32)];
        let out = resample_keyframes(&keyframes, 1000.0, 32);
        for v in out {
            assert!((v - 0.42).abs() < 1e-6);
        }
    }
}
