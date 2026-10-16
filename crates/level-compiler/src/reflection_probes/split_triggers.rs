// Reflection-probe split triggers: which bound water a capture point fails, by sight and by hidden mirror hits.
// See: context/lib/resource_management.md §5.1 · rendering_pipeline.md §7.3a

use glam::{DVec2, Vec3};

use super::capture_visibility::{
    AT_CAPTURE_POINT_M, DistanceTile, MIN_SEEN_FRACTION, face_samples, water_point,
};
use super::hit::SceneTracer;
use super::placement::{CAPTURE_LIFT_M, UpFacingFace};

/// Elevations above the horizon of the mirror-ray fan every water sample
/// traces, degrees. A mirror ray's elevation is the view ray's depression,
/// so these stand for eyes looking across the water from grazing to steep:
/// 10° is an eye 1.6 m up seeing water 9 m away, 60° one looking nearly
/// down. One fan for every probe, so a verdict never depends on the probe.
pub(crate) const FAN_ELEVATIONS_DEG: [f32; 3] = [10.0, 30.0, 60.0];
/// Azimuths per elevation, evenly spaced and turned half a step off the
/// axes, so no ray runs exactly along an axis-aligned wall or its edge.
pub(crate) const FAN_AZIMUTHS: u32 = 8;
/// A mirror ray's true hit passes while the distance tile, within this many
/// capture texels of angle of the hit's direction from the capture point,
/// stores a surface at or beyond the hit. Two texels are the capture-shadow
/// fade's width: a hit that close to a silhouette reads as the surface
/// beside it, which the runtime search samples anyway.
pub(crate) const HIDDEN_HIT_BUDGET_TEXELS: f32 = 2.0;
/// How far behind a stored surface a hit may lie, as `h = |v|₁·w − 1`, and
/// still count as on it: the fluid shader's capture-shadow depth margin κ,
/// which absorbs the filtered distance reading a corner a little near.
pub(crate) const HIDDEN_HIT_MARGIN: f32 = 0.05;
/// Taps around a hit's direction: rings at these fractions of the budget,
/// each of [`HIDDEN_HIT_RING_TAPS`] directions, after the centre tap.
const HIDDEN_HIT_RINGS: [f32; 2] = [0.5, 1.0];
const HIDDEN_HIT_RING_TAPS: u32 = 8;
/// A face fails the parallax trigger when more than this area-weighted share
/// of its fan rays hit geometry the capture point does not see.
pub(crate) const HIDDEN_RAY_SHARE_LIMIT: f64 = 0.25;

/// Every constant a split verdict reads, for the stage key.
pub(crate) fn split_key_bytes() -> Vec<u8> {
    let mut out = Vec::new();
    for value in FAN_ELEVATIONS_DEG
        .into_iter()
        .chain([HIDDEN_HIT_BUDGET_TEXELS, HIDDEN_HIT_MARGIN])
        .chain(HIDDEN_HIT_RINGS)
    {
        out.extend_from_slice(&value.to_le_bytes());
    }
    for value in [FAN_AZIMUTHS, HIDDEN_HIT_RING_TAPS] {
        out.extend_from_slice(&value.to_le_bytes());
    }
    out.extend_from_slice(&HIDDEN_RAY_SHARE_LIMIT.to_le_bytes());
    out
}

/// The fan's unit directions, elevation-major.
pub(crate) fn mirror_fan() -> Vec<Vec3> {
    let mut fan = Vec::with_capacity(FAN_ELEVATIONS_DEG.len() * FAN_AZIMUTHS as usize);
    for elevation in FAN_ELEVATIONS_DEG {
        let (sin_e, cos_e) = elevation.to_radians().sin_cos();
        for k in 0..FAN_AZIMUTHS {
            let azimuth = std::f32::consts::TAU * (k as f32 + 0.5) / FAN_AZIMUTHS as f32;
            let (sin_a, cos_a) = azimuth.sin_cos();
            fan.push(Vec3::new(cos_e * cos_a, sin_e, cos_e * sin_a));
        }
    }
    fan
}

/// One water sample of a bound face: what a later face cut reads.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct SampleVerdict {
    /// Horizontal `(x, z)`.
    pub point: DVec2,
    pub area: f64,
    /// The capture point sees it (the shader draws a probe term there).
    pub seen: bool,
    /// Fan rays from it whose true hit the capture point does not see.
    pub hidden_rays: u32,
}

/// One bound face against its probe's capture.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct FaceVerdict {
    pub key: (u32, u32),
    pub samples: Vec<SampleVerdict>,
    pub fan_rays: u32,
}

impl FaceVerdict {
    pub(crate) fn area(&self) -> f64 {
        self.samples.iter().map(|s| s.area).sum()
    }

    pub(crate) fn seen_area(&self) -> f64 {
        self.samples.iter().filter(|s| s.seen).map(|s| s.area).sum()
    }

    /// Area-weighted share of fan rays that hit hidden geometry.
    pub(crate) fn hidden_share(&self) -> f64 {
        let area = self.area();
        if area <= 0.0 || self.fan_rays == 0 {
            return 0.0;
        }
        let rays = f64::from(self.fan_rays);
        self.samples
            .iter()
            .map(|s| s.area * f64::from(s.hidden_rays) / rays)
            .sum::<f64>()
            / area
    }

    /// The fraction of the face the capture point serves: water it sees,
    /// weighted by the share of that water's fan rays whose hits it sees too.
    pub(crate) fn served_fraction(&self) -> f64 {
        let area = self.area();
        if area <= 0.0 {
            return 0.0;
        }
        let rays = f64::from(self.fan_rays.max(1));
        self.samples
            .iter()
            .filter(|s| s.seen)
            .map(|s| s.area * (1.0 - f64::from(s.hidden_rays) / rays))
            .sum::<f64>()
            / area
    }

    pub(crate) fn seen_fraction(&self) -> f64 {
        let area = self.area();
        if area > 0.0 {
            self.seen_area() / area
        } else {
            1.0
        }
    }
}

/// Which of a probe's bound faces fail, by the two triggers, face by face
/// (the face is the split's unit). Sight fails a face its capture point
/// sees under [`MIN_SEEN_FRACTION`] of: that water draws no probe term. A
/// probe seeing under half of its whole bound area always has such a face.
/// Parallax fails a face whose hidden-ray share passes
/// [`HIDDEN_RAY_SHARE_LIMIT`].
pub(crate) fn failing_faces(verdicts: &[FaceVerdict]) -> Vec<bool> {
    verdicts
        .iter()
        .map(|v| v.seen_fraction() < MIN_SEEN_FRACTION || v.hidden_share() > HIDDEN_RAY_SHARE_LIMIT)
        .collect()
}

/// Judge one bound face against the capture at `capture`: every sample's
/// sight, by the shader's capture-shadow test, and its fan of mirror rays
/// traced from 2 mm above the water against the static world.
pub(crate) fn judge_face(
    tracer: &SceneTracer<'_>,
    tile: &DistanceTile,
    capture: Vec3,
    face: &UpFacingFace,
    fan: &[Vec3],
    max_distance: f32,
) -> FaceVerdict {
    let mut samples = Vec::new();
    let lift = Vec3::Y * CAPTURE_LIFT_M as f32;
    face_samples(&face.outline, |point, area| {
        let water = water_point(face, point);
        let origin = water + lift;
        let hidden_rays = fan
            .iter()
            .filter(|&&direction| {
                tracer
                    .closest_hit(origin, direction, max_distance)
                    .is_some_and(|hit| {
                        hit_is_hidden(tile, capture, origin + direction * hit.distance)
                    })
            })
            .count() as u32;
        samples.push(SampleVerdict {
            point,
            area,
            seen: tile.sees(capture, water),
            hidden_rays,
        });
    });
    FaceVerdict {
        key: (face.volume, face.face),
        samples,
        fan_rays: fan.len() as u32,
    }
}

/// Whether a mirror ray's true hit lies behind every surface the capture
/// stored near its direction: no tap within [`HIDDEN_HIT_BUDGET_TEXELS`] of
/// `hit − capture` stores a surface at or beyond it (`|v|₁·w ≤ 1 + κ`). A
/// tap that stored a miss (0) passes it, so open sky never hides a hit.
pub(crate) fn hit_is_hidden(tile: &DistanceTile, capture: Vec3, hit: Vec3) -> bool {
    let offset = hit - capture;
    let l1 = offset.abs().element_sum();
    if l1 < AT_CAPTURE_POINT_M {
        return false;
    }
    let passes = |direction: Vec3| l1 * tile.sample(direction) <= 1.0 + HIDDEN_HIT_MARGIN;
    let toward = offset / offset.length();
    if passes(toward) {
        return false;
    }
    let helper = if toward.y.abs() < 0.9 {
        Vec3::Y
    } else {
        Vec3::X
    };
    let across = toward.cross(helper).normalize();
    let up = toward.cross(across);
    let budget = HIDDEN_HIT_BUDGET_TEXELS * tile.texel_angle();
    !HIDDEN_HIT_RINGS.iter().any(|&ring| {
        let (sin_r, cos_r) = (budget * ring).sin_cos();
        (0..HIDDEN_HIT_RING_TAPS).any(|k| {
            let turn = std::f32::consts::TAU * k as f32 / HIDDEN_HIT_RING_TAPS as f32;
            let (sin_t, cos_t) = turn.sin_cos();
            passes(toward * cos_r + (across * cos_t + up * sin_t) * sin_r)
        })
    })
}
