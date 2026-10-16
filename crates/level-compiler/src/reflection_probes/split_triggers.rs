// Reflection-probe split triggers: which bound water a capture point fails, sample by sample, by sight and by unserved mirror hits.
// See: context/lib/resource_management.md §5.1 · rendering_pipeline.md §7.3a

use glam::{DVec2, Vec3};

use super::capture_visibility::{
    AT_CAPTURE_POINT_M, CAPTURE_SHADOW_MARGIN, DistanceTile, face_samples, water_point,
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
/// Taps around a hit's direction: rings at these fractions of the budget,
/// each of [`HIDDEN_HIT_RING_TAPS`] directions, after the centre tap.
const HIDDEN_HIT_RINGS: [f32; 2] = [0.5, 1.0];
const HIDDEN_HIT_RING_TAPS: u32 = 8;
/// A hit the capture point sees is still unserved when one capture texel
/// there, seen from the eye the mirror ray stands for, spans more than this
/// many capture texels: the reflection would show the capture magnified
/// past a texel and a half. A pool's coping seen from its centre stays under
/// it; a dock's underside seen from out on the lake does not.
pub(crate) const COARSE_HIT_BUDGET_TEXELS: f32 = 1.5;
/// The eye a mirror ray stands for sits this high above the water, on the
/// view ray: the standing player's eye above its feet (capsule half height
/// plus radius plus eye offset, 0.8 + 0.2 + 0.5 m), as the reference player
/// descriptor declares it. The engine defines no eye height of its own; a
/// parity test pins this mirror to that descriptor.
pub(crate) const PLAYER_EYE_HEIGHT_M: f32 = 1.5;
/// A water sample fails parallax when more than this share of its fan rays
/// are unserved.
pub(crate) const UNSERVED_RAY_SHARE_LIMIT: f64 = 0.25;
/// A face fails when more than this share of its area fails.
pub(crate) const FAILING_AREA_SHARE: f64 = 0.5;

/// Every constant a split verdict reads, for the stage key.
pub(crate) fn split_key_bytes() -> Vec<u8> {
    let mut out = Vec::new();
    for value in FAN_ELEVATIONS_DEG
        .into_iter()
        .chain([
            HIDDEN_HIT_BUDGET_TEXELS,
            CAPTURE_SHADOW_MARGIN,
            COARSE_HIT_BUDGET_TEXELS,
            PLAYER_EYE_HEIGHT_M,
        ])
        .chain(HIDDEN_HIT_RINGS)
    {
        out.extend_from_slice(&value.to_le_bytes());
    }
    for value in [FAN_AZIMUTHS, HIDDEN_HIT_RING_TAPS] {
        out.extend_from_slice(&value.to_le_bytes());
    }
    for value in [UNSERVED_RAY_SHARE_LIMIT, FAILING_AREA_SHARE] {
        out.extend_from_slice(&value.to_le_bytes());
    }
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

/// One water sample of a bound face against its probe's capture. Its fail
/// bit is what the face verdict, and a later face cut, read.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct SampleVerdict {
    /// Horizontal `(x, z)`.
    pub point: DVec2,
    pub area: f64,
    /// The capture point sees it: the fluid shader draws a probe term there.
    pub seen: bool,
    /// Fan rays from it whose true hit the capture cannot serve.
    pub unserved_rays: u32,
    pub fan_rays: u32,
}

impl SampleVerdict {
    /// The sample fails: its capture point does not see it (sight), or more
    /// than [`UNSERVED_RAY_SHARE_LIMIT`] of its fan rays are unserved
    /// (parallax).
    pub(crate) fn fails(&self) -> bool {
        !self.seen
            || f64::from(self.unserved_rays) > UNSERVED_RAY_SHARE_LIMIT * f64::from(self.fan_rays)
    }

    fn unserved_share(&self) -> f64 {
        if self.fan_rays == 0 {
            0.0
        } else {
            f64::from(self.unserved_rays) / f64::from(self.fan_rays)
        }
    }
}

/// One bound face against its probe's capture.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct FaceVerdict {
    pub key: (u32, u32),
    pub samples: Vec<SampleVerdict>,
}

impl FaceVerdict {
    /// The summed area of the samples `keep` accepts. Folds from +0, where
    /// an empty float sum is −0, which a log prints as "-0.00".
    fn area_where(&self, keep: impl Fn(&SampleVerdict) -> bool) -> f64 {
        self.samples
            .iter()
            .filter(|s| keep(s))
            .fold(0.0, |sum, s| sum + s.area)
    }

    pub(crate) fn area(&self) -> f64 {
        self.area_where(|_| true)
    }

    /// The area of the samples that fail.
    pub(crate) fn failing_area(&self) -> f64 {
        self.area_where(SampleVerdict::fails)
    }

    /// The face fails when more than [`FAILING_AREA_SHARE`] of its area does.
    pub(crate) fn fails(&self) -> bool {
        self.failing_area() > FAILING_AREA_SHARE * self.area()
    }

    /// The share of the face's area that passes. No area passes in full.
    pub(crate) fn passing_fraction(&self) -> f64 {
        let area = self.area();
        if area > 0.0 {
            1.0 - self.failing_area() / area
        } else {
            1.0
        }
    }

    /// The fraction of the face the capture point serves: water it sees,
    /// weighted by the share of that water's fan rays it serves. Breaks ties
    /// in [`Self::passing_fraction`] when every face fails.
    pub(crate) fn served_fraction(&self) -> f64 {
        let area = self.area();
        if area <= 0.0 {
            return 0.0;
        }
        self.samples
            .iter()
            .filter(|s| s.seen)
            .map(|s| s.area * (1.0 - s.unserved_share()))
            .sum::<f64>()
            / area
    }

    /// Diagnostic: the fraction of the face's area the capture point sees.
    pub(crate) fn seen_fraction(&self) -> f64 {
        let area = self.area();
        if area <= 0.0 {
            return 1.0;
        }
        self.area_where(|s| s.seen) / area
    }

    /// Diagnostic: the area-weighted share of fan rays the capture does not
    /// serve.
    pub(crate) fn unserved_share(&self) -> f64 {
        let area = self.area();
        if area <= 0.0 {
            return 0.0;
        }
        self.samples
            .iter()
            .map(|s| s.area * s.unserved_share())
            .sum::<f64>()
            / area
    }
}

/// Which of a probe's bound faces fail: each face's own area majority of
/// failing samples ([`FaceVerdict::fails`]), so the face verdict and a later
/// face cut read one fail bit.
pub(crate) fn failing_faces(verdicts: &[FaceVerdict]) -> Vec<bool> {
    verdicts.iter().map(FaceVerdict::fails).collect()
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
        let unserved_rays = fan
            .iter()
            .filter(|&&direction| {
                tracer
                    .closest_hit(origin, direction, max_distance)
                    .is_some_and(|hit| {
                        hit_is_unserved(tile, capture, origin, direction, hit.distance)
                    })
            })
            .count() as u32;
        samples.push(SampleVerdict {
            point,
            area,
            seen: tile.sees(capture, water),
            unserved_rays,
            fan_rays: fan.len() as u32,
        });
    });
    FaceVerdict {
        key: (face.volume, face.face),
        samples,
    }
}

/// Whether the capture at `capture` cannot serve the true hit of the mirror
/// ray from `origin` along unit `direction`, `distance` away: the hit is
/// hidden ([`hit_is_hidden`]) or coarse ([`hit_is_coarse`]). A direction
/// the capture stored as a miss serves every hit, so open sky never fails a
/// sample.
pub(crate) fn hit_is_unserved(
    tile: &DistanceTile,
    capture: Vec3,
    origin: Vec3,
    direction: Vec3,
    distance: f32,
) -> bool {
    let hit = origin + direction * distance;
    let offset = hit - capture;
    if offset.abs().element_sum() < AT_CAPTURE_POINT_M || tile.sample(offset) <= 0.0 {
        return false;
    }
    hit_is_coarse(capture, direction, distance, hit) || hit_is_hidden(tile, capture, hit)
}

/// Whether one capture texel at `hit`, seen from the eye the mirror ray
/// along unit `direction` stands for, spans more than
/// [`COARSE_HIT_BUDGET_TEXELS`] capture texels. That eye sits
/// [`PLAYER_EYE_HEIGHT_M`] above the water on the view ray (the mirror ray
/// reflected in the water plane), so the reflected path from it is
/// `|E − P| + |P − H|`, with `|E − P|` the eye height over the ray's sine
/// of elevation; a texel `|H − C|` from the capture point spans
/// `|H − C| / path` texels from there.
pub(crate) fn hit_is_coarse(capture: Vec3, direction: Vec3, distance: f32, hit: Vec3) -> bool {
    if direction.y <= 0.0 {
        return false;
    }
    let path = PLAYER_EYE_HEIGHT_M / direction.y + distance;
    hit.distance(capture) > COARSE_HIT_BUDGET_TEXELS * path
}

/// Whether a mirror ray's true hit lies behind every surface the capture
/// stored near its direction: no tap within [`HIDDEN_HIT_BUDGET_TEXELS`] of
/// `hit − capture` stores a surface at or beyond it (`h = |v|₁·w − 1 ≤ κ`,
/// the shader's capture-shadow margin, which absorbs the filtered distance
/// reading a corner a little near). A tap that stored a miss (0) passes it,
/// so open sky never hides a hit.
pub(crate) fn hit_is_hidden(tile: &DistanceTile, capture: Vec3, hit: Vec3) -> bool {
    let offset = hit - capture;
    let l1 = offset.abs().element_sum();
    if l1 < AT_CAPTURE_POINT_M {
        return false;
    }
    let passes = |direction: Vec3| l1 * tile.sample(direction) <= 1.0 + CAPTURE_SHADOW_MARGIN;
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
