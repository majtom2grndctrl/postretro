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
    /// Animated spot-light aim vectors, uniformly spaced over `period`.
    /// Each sample must be unit length — the authoring seam (either the
    /// `direction_curve` FGD key or the `set_light_animation` scripting
    /// primitive) normalizes at write time. The GPU evaluator does not
    /// re-normalize. `None` means the light keeps its static `cone_direction`.
    pub direction: Option<Vec<[f32; 3]>>,
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

    /// Optional author-supplied tag (FGD `_tag`). Carried through the PRL
    /// `LightTags` section so the runtime can register each light with the
    /// scripting entity registry's tag column, enabling
    /// `world.query({ component: "light", tag: "<tag>" })`.
    pub tag: Option<String>,
}

// ---------------------------------------------------------------------------
// Keyframe resampling
//
// Authored `*_curve` FGD values carry timestamped keyframes. The canonical
// `LightAnimation` stores uniform samples along `period_ms`. `resample_keyframes`
// converts the former to the latter via Catmull-Rom over authored timestamps,
// so the wire format and GPU evaluator stay unchanged.
//
// See: context/lib/build_pipeline.md

/// Sample rate (per second of `period_ms`) used when resampling authored
/// keyframes into uniform `LightAnimation` samples.
pub const KEYFRAME_RESAMPLE_RATE_HZ: u32 = 32;

/// Maximum number of samples in the resampled uniform curve regardless of
/// period. Bounds descriptor buffer growth as authored maps scale.
pub const KEYFRAME_RESAMPLE_MAX_SAMPLES: usize = 256;

/// Values that participate in Catmull-Rom resampling. Implementors supply a
/// scalar linear interpolation; Catmull-Rom composes four such lerps.
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

/// Resample timestamped keyframes into a uniformly-spaced sample buffer
/// covering `[0, period_ms]`.
///
/// `keyframes` must be non-empty and monotonically increasing in timestamp
/// (the caller enforces this during parsing). `period_ms` must be positive.
/// Returns `round(period_ms / 1000.0 * samples_per_second)` samples, capped at
/// `KEYFRAME_RESAMPLE_MAX_SAMPLES`. The sample at index `i` corresponds to
/// time `i * period_ms / sample_count`.
///
/// The interior of the curve uses Catmull-Rom between consecutive keyframes;
/// endpoints are reflected (first/last keyframe duplicated) so the spline is
/// defined at the boundaries.
///
/// Times outside the authored keyframe range clamp to the nearest keyframe.
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

/// Evaluate the Catmull-Rom spline through `keyframes` at absolute time `t_ms`.
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

#[cfg(test)]
mod keyframe_resample_tests {
    use super::*;

    /// Sample the resampled curve at the nearest uniform-sample index for
    /// `t_ms` and return that sample. Used to check round-trip accuracy.
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
