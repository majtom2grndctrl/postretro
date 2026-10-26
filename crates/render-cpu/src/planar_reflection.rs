// Planar water reflection, GPU-free: the mirror camera, candidate planes, selection and presets.
// See: context/lib/rendering_pipeline.md §7.3a

mod candidates;
mod mirror_camera;
mod selection;

use glam::{Mat4, Vec3, Vec4};

use crate::fluid_surface::{
    motion::{FLUID_RING_AMPLITUDE, FLUID_SHIMMER_AMPLITUDE},
    reflection::FLUID_REFLECTION_RAY_MIN_Y,
    reflection_quality::{FluidReflectionQuality, reflection_target_extent},
    reflection_style::FLUID_REFLECTION_PIXEL_TILT,
};

pub use candidates::{PlanarCandidate, PlanarCandidates, PlanarFace};
pub use mirror_camera::MirrorCamera;
pub use selection::{
    SelectedPlane, challenger_switches, collect_mirror_seed_cells, mark_seeded_planes, select_plane,
};

/// Faces, and fluid triangles, within this many metres of one height share a plane.
pub const PLANE_HEIGHT_TOLERANCE: f32 = 0.01;
/// The up-facing threshold selection groups faces by: a face, or a fluid
/// triangle, joins a plane only when its unit normal's y exceeds this. The
/// renderer's planar run split must use the same value, or a selected
/// plane's draws and its candidate faces disagree.
pub const PLANE_UP_NORMAL_MIN: f32 = 0.9999;
/// The oblique near plane sits this far beneath the water, so the waterline
/// keeps its own geometry in the mirror.
pub const PLANAR_CLIP_BIAS: f32 = 0.02;
/// A challenger must score above this multiple of the held plane to switch.
pub const PLANE_SELECTION_HYSTERESIS: f32 = 1.25;
/// The area floor, in m²: a plane whose total reflecting, probe-bound,
/// up-facing area (summed over its faces) falls below it never becomes a
/// candidate, so a puddle near the eye never outscores the lake. An engine
/// constant, never a map key, option or override (`rendering_pipeline.md`
/// §7.3a Selection). Placed from the baked fixtures' planes: the largest
/// puddle-scale plane is e24's floating block top, 7.9 m² (2.4 × 3.3 m),
/// and the smallest pool-scale one is reflection-probes' pool room, 23.8 m²
/// (4.9 × 4.9 m). 14 m², near their geometric mean, clears each by about
/// 1.7×.
pub const PLANAR_MIN_PLANE_AREA: f32 = 14.0;

/// Largest per-axis whole-texel displayed offset a pixel-style cell shows:
/// a shimmer plus a ring, each pinned at one texel per axis.
pub const MIRROR_CROP_MAX_OFFSET_TEXELS: i32 = FLUID_SHIMMER_AMPLITUDE + FLUID_RING_AMPLITUDE;

/// The mirror crop's pad: the largest angle between a pixel-style cell's
/// reflected ray and the flat water's ray through the same point, which is
/// the mirror eye's ray through it. Derivation:
/// - The tilt is `unit(u)·ox + unit(v)·oy` times `FLUID_REFLECTION_PIXEL_TILT`,
///   in the plane, so its tangent is at most `(|ox| + |oy|) · 0.05 = 4 · 0.05
///   = 0.2` (skewed texture axes reach the sum; orthogonal ones 2√2 · 0.05),
///   and the normal turns `atan(0.2) ≤ 0.2` rad.
/// - Mirroring about two normals `θ` apart differs by a rotation of `2θ`
///   (two half-turns compose to one), so the ray turns at most 0.4 rad.
/// - The horizon clamp (`FLUID_REFLECTION_RAY_MIN_Y`) moves a grazing flat
///   ray up to `asin(MIN_Y) ≤ π/2 · MIN_Y`, and moves the tilted ray no
///   further from the clamped flat one, so it adds at most `π · MIN_Y`.
///
/// ≈ 0.4031 rad (23.1°). Seen from the mirror eye, a sample at any depth
/// along such a ray lies within this angle of the eye's ray through its
/// water point, so it holds at every depth. Smooth style tilts nothing but
/// takes the same pad: the walk never depends on style.
pub const MIRROR_CROP_PAD_RADIANS: f32 =
    2.0 * (2 * MIRROR_CROP_MAX_OFFSET_TEXELS) as f32 * FLUID_REFLECTION_PIXEL_TILT
        + std::f32::consts::PI * FLUID_REFLECTION_RAY_MIN_Y;

/// One planar tier's engine-constant preset: the planar layer's per-axis
/// scale of the scene extent and the absolute-metre distance limits. `None`
/// leaves a distance unlimited. Never a player setting or map key.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlanarPreset {
    pub scale: f32,
    /// Mirror reach: distance from the mirror eye to the reflected surface.
    pub d: Option<f32>,
    pub band_d: f32,
    /// Receiver reach: distance from the main eye to the water fragment.
    pub r: Option<f32>,
    pub band_r: f32,
}

/// Both planar tiers start at half scale with D and R off; the owner retunes
/// them from a motion read.
const PLANAR_PRESET_START: PlanarPreset = PlanarPreset {
    scale: 0.5,
    d: None,
    band_d: 4.0,
    r: None,
    band_r: 4.0,
};

const PLANAR_PRESETS: [(FluidReflectionQuality, PlanarPreset); 2] = [
    (FluidReflectionQuality::VeryHigh, PLANAR_PRESET_START),
    (FluidReflectionQuality::Ultra, PLANAR_PRESET_START),
];

/// The planar preset of `quality`: `Some` only at very high and ultra, the
/// tiers that run the planar layer.
pub fn planar_preset(quality: FluidReflectionQuality) -> Option<PlanarPreset> {
    PLANAR_PRESETS
        .iter()
        .find(|(tier, _)| *tier == quality)
        .map(|(_, preset)| preset.sanitized())
}

impl PlanarPreset {
    /// A supported scale (¼, ½ or 1), positive finite limits and bands.
    pub fn sanitized(self) -> Self {
        let scale = if self.scale.is_finite() {
            [0.25_f32, 0.5, 1.0]
                .into_iter()
                .min_by(|a, b| (self.scale - a).abs().total_cmp(&(self.scale - b).abs()))
                .expect("three supported planar scales")
        } else {
            0.5
        };
        let limit = |value: Option<f32>| value.filter(|v| v.is_finite() && *v > 0.0);
        let band = |value: f32| {
            if value.is_finite() {
                value.max(0.001)
            } else {
                4.0
            }
        };
        Self {
            scale,
            d: limit(self.d),
            band_d: band(self.band_d),
            r: limit(self.r),
            band_r: band(self.band_r),
        }
    }

    /// The planar layer's extent for a `scene` extent: rounded up, at least 1×1.
    pub fn target_extent(self, scene: (u32, u32)) -> (u32, u32) {
        reflection_target_extent(scene, self.sanitized().scale)
    }
}

/// Absolute-metre fade. An unlimited distance keeps full coverage. `band`
/// is positive, as [`PlanarPreset::sanitized`] keeps it.
pub fn planar_fade(distance: f32, limit: Option<f32>, band: f32) -> f32 {
    limit.map_or(1.0, |limit| ((limit - distance) / band).clamp(0.0, 1.0))
}

/// A [0, 1]-depth view-projection's six inward planes (left, right, bottom,
/// top, near, far) as `dot(plane, vec4(p, 1)) >= 0`, unnormalized. Near is
/// the true near plane (row 2), the one the depth range clips at.
fn view_proj_planes(m: Mat4) -> [Vec4; 6] {
    let (r0, r1, r2, r3) = (m.row(0), m.row(1), m.row(2), m.row(3));
    [r3 + r0, r3 - r0, r3 + r1, r3 - r1, r2, r3 - r2]
}

/// Whether an AABB reaches inside every plane of `planes` (the p-vertex
/// test): conservative, so a box straddling a plane passes.
pub fn aabb_in_planes(min: Vec3, max: Vec3, planes: &[Vec4; 6]) -> bool {
    planes.iter().all(|plane| {
        let p = Vec3::new(
            if plane.x >= 0.0 { max.x } else { min.x },
            if plane.y >= 0.0 { max.y } else { min.y },
            if plane.z >= 0.0 { max.z } else { min.z },
        );
        plane.truncate().dot(p) + plane.w >= 0.0
    })
}

#[cfg(test)]
#[path = "planar_reflection_tests.rs"]
mod tests;
