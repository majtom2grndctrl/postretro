// Receiver footprint for static lightmap texels: cone-kernel sample layout, probe early-out, wall clip.
// See: context/lib/build_pipeline.md §Compiler pipeline (step 12)

use std::cell::RefCell;
use std::f32::consts::{FRAC_PI_4, TAU};
use std::sync::Arc;

use glam::Vec3;

use crate::chart_raster::chart_grid;
use crate::lightmap_bake::{
    Chart, RAY_EPSILON, SoftProbes, area_sample_target, hard_ray_target, probe_sample_direction,
    soft_visibility_with,
};
use crate::map_data::{LightType, MapLight};

/// Whether a bake floors texel visibility when nothing says otherwise. The CLI
/// only offers `--no-penumbra-floor`, so this must stay `true`.
pub(crate) const PENUMBRA_FLOOR_DEFAULT: bool = true;

/// Footprint samples traced before escalating: the texel centre plus an
/// eight-point ring. The centre catches a strip thinner than half a texel that
/// runs through the texel; its ray runs to the emitter's centre, so it is the
/// texel's own centre ray for soft lights too. The ring catches an edge
/// anywhere within the kernel's reach, so agreeing probes never snap a texel
/// the kernel would filter to a fraction. A four-point ring lets such a strip
/// slip between its points.
pub(crate) const FOOTPRINT_PROBES: usize = 9;

const RING_SAMPLES: usize = FOOTPRINT_PROBES - 1;

/// Ring radius, in texels, at every count. A texel's probes must split
/// whenever the kernel filters it to a fraction, which is within 0.52 texel
/// of an edge (the cone's 10% point). Eight points 45° apart put one within
/// 22.5° of any edge normal, so this radius reaches `0.6 · cos 22.5° ≈ 0.55`
/// texel across it. A strip covering `[a, b]` texels from the centre holds a
/// ring point wherever `cos θ ∈ [a / 0.6, b / 0.6]`, which spans more than
/// 45° of the ring for every strip 0.4 texel wide that the kernel filters to
/// a fraction; a ring farther out lets such strips slip between centre and
/// ring. Narrower strips, and discs of radius 0.25 to 0.31 texel, can still
/// fit between the centre and the ring. At nine samples, centre plus ring, it
/// also keeps the spread near the cone's (`E[r²] = 0.3` wants a ring at 0.58).
pub(crate) const RING_RADIUS_TEXELS: f32 = 0.6;

/// Ring probe `k` sits at ring position `k · 3 mod 8`, so consecutive probes
/// lie 135° apart. Probes whose sector holds no emitter sample claim one in
/// probe order ([`TexelFootprint::emitter_pairing`]); the spread keeps
/// neighbouring sectors from claiming one after the other.
const RING_POSITION_STRIDE: usize = 3;

/// Golden angle, the azimuth step between the strata off the ring.
const GOLDEN_ANGLE: f32 = 2.399_963_3;

/// Radius `r` in texels whose cone-kernel CDF `F(r) = 3r² − 2r³` equals `u`:
/// the inverse smoothstep. The cone `w(r) = 1 − r` on `r < 1` is the round
/// one-texel kernel, matching bilinear's one-texel reach on every axis.
pub(crate) fn cone_radius_quantile(u: f32) -> f32 {
    0.5 - ((1.0 - 2.0 * u).asin() / 3.0).sin()
}

/// Strata off the ring that pair against its correlation: the eight with the
/// largest radii, or as many as there are
/// ([`TexelFootprint::claim_counter_ring`]).
const COUNTER_RING_SAMPLES: usize = RING_SAMPLES;

/// How far a footprint sample's lead-in test runs past each end of the
/// stretch its trace skips: back from the sample point, where its clip
/// segment ends, and on past the point the trace starts from. A wall through
/// either point is then entered inside the test, whichever side of it
/// rounding puts the point. Far above `f32` rounding at map scale, far below
/// a texel.
const LEAD_IN_OVERLAP: f32 = 0.25 * RAY_EPSILON;

/// One light's footprint sample set at one sample count: an offset in texels
/// per sample, in trace order, and the emitter lattice they pair with. Which
/// emitter sample each offset traces is chosen per texel
/// ([`TexelFootprint::emitter_pairing`]).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct TexelFootprint {
    full_samples: u32,
    /// Unrotated offsets in parent-grid texels: the centre, the eight ring
    /// probes, then the strata off the ring by ascending radius, so the
    /// counter ring comes last.
    offsets: Vec<[f32; 2]>,
    /// Each emitter sample's offset from the emitter's centre at seed 0, up
    /// to scale: the unit sphere direction for a point or spot light, the part
    /// of the jittered direction perpendicular to the axis for a sun. A
    /// texel's seed turns the whole lattice
    /// ([`emitter_lattice_spin`](crate::lightmap_bake::emitter_lattice_spin)).
    lattice: Vec<Vec3>,
    /// Strata in the counter ring.
    counter_len: usize,
    /// Stride dealing the unclaimed emitter samples to the strata off the ring
    /// and outside the counter ring ([`decorrelating_stride`]).
    rest_stride: usize,
}

impl TexelFootprint {
    /// `N = max(area_sample_count, 9)` radial strata at the cone quantiles
    /// `F⁻¹((j + 0.5) / N)`. Stratum 0 becomes the exact centre and the eight
    /// [`ring_strata`] the ring at [`RING_RADIUS_TEXELS`]; the rest keep their
    /// radii and take golden-angle azimuths.
    fn build(light: &MapLight, area_sample_count: u32) -> Self {
        let full_samples = area_sample_count.max(FOOTPRINT_PROBES as u32);
        let count = full_samples as usize;
        let radius = |stratum: usize| cone_radius_quantile((stratum as f32 + 0.5) / count as f32);
        let polar = |r: f32, angle: f32| [r * angle.cos(), r * angle.sin()];

        let mut offsets = Vec::with_capacity(count);
        offsets.push([0.0, 0.0]);
        for k in 0..RING_SAMPLES {
            let position = (k * RING_POSITION_STRIDE) % RING_SAMPLES;
            offsets.push(polar(RING_RADIUS_TEXELS, position as f32 * FRAC_PI_4));
        }
        let ring = ring_strata(count);
        for stratum in (1..count).filter(|stratum| !ring.contains(stratum)) {
            offsets.push(polar(radius(stratum), GOLDEN_ANGLE * stratum as f32));
        }

        let lattice = (0..full_samples)
            .map(|index| {
                let direction = probe_sample_direction(light, index, full_samples);
                match light.light_type {
                    LightType::Point | LightType::Spot => direction,
                    LightType::Directional => {
                        let axis = sun_axis(light);
                        direction - axis * axis.dot(direction)
                    }
                }
            })
            .collect();
        let off_ring = count - FOOTPRINT_PROBES;
        let counter_len = COUNTER_RING_SAMPLES.min(off_ring);
        Self {
            full_samples,
            offsets,
            lattice,
            counter_len,
            rest_stride: decorrelating_stride(off_ring - counter_len),
        }
    }

    /// Unrotated offsets in parent-grid texels, in trace order.
    #[cfg(test)]
    pub(crate) fn offsets(&self) -> &[[f32; 2]] {
        &self.offsets
    }
}

/// The eight consecutive strata of `count` whose quantile band
/// `[j / count, (j + 8) / count]` is centred nearest the cone's CDF at the
/// ring radius, `F(0.6) = 0.648`, clamped so the centre keeps stratum 0. The
/// ring takes the mass the cone holds about its radius, so the radial
/// distribution departs from the cone's by at most about half the band.
fn ring_strata(count: usize) -> std::ops::Range<usize> {
    let r = f64::from(RING_RADIUS_TEXELS);
    let quantile = 3.0 * r * r - 2.0 * r * r * r;
    let first = (quantile * count as f64 - (RING_SAMPLES / 2) as f64).round();
    let first = (first.max(1.0) as usize).min(count - RING_SAMPLES);
    first..first + RING_SAMPLES
}

/// Unit direction toward a sun, the axis of its emitter cone.
fn sun_axis(light: &MapLight) -> Vec3 {
    let aim = Vec3::from(light.cone_direction.unwrap_or([0.0, -1.0, 0.0]));
    (-aim).normalize_or_zero()
}

/// The integer nearest `count / φ` that is coprime to `count`: a full-period
/// stride over `count` slots that spreads neighbours apart.
fn decorrelating_stride(count: usize) -> usize {
    if count <= 1 {
        return 1;
    }
    let mut stride = ((count * 618_034 + 500_000) / 1_000_000).max(1);
    while gcd(stride, count) != 1 {
        stride += 1;
    }
    stride
}

fn gcd(mut a: usize, mut b: usize) -> usize {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

/// What a footprint's emitter lattice depends on besides the count. Point
/// and spot lights share the light-independent sphere lattice; a
/// directional light's cone lattice depends on its angle and aim.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EmitterKey {
    Sphere,
    Cone {
        angular_diameter: u32,
        direction: [u32; 3],
    },
}

fn emitter_key(light: &MapLight) -> EmitterKey {
    match light.light_type {
        LightType::Point | LightType::Spot => EmitterKey::Sphere,
        LightType::Directional => EmitterKey::Cone {
            angular_diameter: light.angular_diameter.to_bits(),
            direction: light
                .cone_direction
                .unwrap_or([0.0, -1.0, 0.0])
                .map(f32::to_bits),
        },
    }
}

thread_local! {
    /// The last footprint this thread built, keyed by count and emitter
    /// lattice. Its own memo, so the four-probe soft-visibility snap other
    /// stages share never changes.
    static FOOTPRINT_MEMO: RefCell<Option<(u32, EmitterKey, Arc<TexelFootprint>)>> =
        const { RefCell::new(None) };
}

fn footprint_for(light: &MapLight, area_sample_count: u32) -> Arc<TexelFootprint> {
    let full_samples = area_sample_count.max(FOOTPRINT_PROBES as u32);
    let key = emitter_key(light);
    FOOTPRINT_MEMO.with(|memo| {
        if let Some((count, memo_key, footprint)) = memo.borrow().as_ref()
            && *count == full_samples
            && *memo_key == key
        {
            return Arc::clone(footprint);
        }
        let footprint = Arc::new(TexelFootprint::build(light, area_sample_count));
        *memo.borrow_mut() = Some((full_samples, key, Arc::clone(&footprint)));
        footprint
    })
}

/// One parent-grid texel step along each chart axis, in world units. Offsets
/// go through the parent grid's pitch, so a sub-chart window reproduces its
/// parent's sample points bit for bit, and a scaled face keeps the same
/// footprint in texels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct TexelFrame {
    pitch_u: Vec3,
    pitch_v: Vec3,
}

impl TexelFrame {
    pub(crate) fn for_chart(chart: &Chart) -> Self {
        let grid = chart_grid(chart);
        Self {
            pitch_u: chart.u_axis * (grid.uv_extent[0] / grid.interior[0] as f32),
            pitch_v: chart.v_axis * (grid.uv_extent[1] / grid.interior[1] as f32),
        }
    }

    /// A frame with the given world-space texel steps.
    #[cfg(test)]
    pub(crate) fn new(pitch_u: Vec3, pitch_v: Vec3) -> Self {
        Self { pitch_u, pitch_v }
    }

    /// World distance no footprint sample reaches from its texel centre: the
    /// longer texel step. Offsets lie strictly within one texel and the chart
    /// axes are orthonormal, so every rotated offset is shorter than this.
    pub(crate) fn radius(&self) -> f32 {
        self.pitch_u.length().max(self.pitch_v.length())
    }

    /// World offset of texel-unit offset `[x, y]` rotated by `(sin, cos)`.
    fn offset(&self, [x, y]: [f32; 2], (sin, cos): (f32, f32)) -> Vec3 {
        self.pitch_u * (x * cos - y * sin) + self.pitch_v * (x * sin + y * cos)
    }
}

/// One texel as the visibility sampler sees it.
#[derive(Debug, Clone, Copy)]
pub(crate) struct TexelSite {
    pub(crate) world_p: Vec3,
    pub(crate) normal: Vec3,
    /// `chart_raster::chart_texel_seed`.
    pub(crate) seed: u64,
    pub(crate) frame: TexelFrame,
    /// Whether any triangle could block this texel's clip segments or
    /// lead-ins ([`ClipReach::may_clip`]). When none can, every one is clear
    /// and the footprint traces none.
    pub(crate) may_clip: bool,
}

/// How one `(light, chart)` walk samples a texel's visibility; built once per
/// walk, not per texel.
#[derive(Debug, Clone)]
pub(crate) enum TexelSampler {
    /// Floor disabled: centre-point soft visibility.
    Centre(SoftProbes),
    /// The receiver footprint.
    Footprint(Arc<TexelFootprint>),
}

impl TexelSampler {
    pub(crate) fn new(light: &MapLight, area_sample_count: u32, penumbra_floor: bool) -> Self {
        if penumbra_floor {
            Self::Footprint(footprint_for(light, area_sample_count))
        } else {
            Self::Centre(SoftProbes::new(light, area_sample_count))
        }
    }

    /// The texel's raw visibility in `[0, 1]`. `trace(from, to)`,
    /// `reach(from, to)` and `lead_in(from, to)` all return true for a clear
    /// segment. `trace` and `reach` skip their segment's first `RAY_EPSILON`,
    /// as [`segment_clear`](crate::lightmap_bake::segment_clear) does, which
    /// the footprint relies on. `reach` tests the footprint's wall clip and
    /// should keep its own occluder cache; `lead_in` is [`lead_in_clear`]
    /// against the chart's [`ClipBand`].
    pub(crate) fn visibility(
        &self,
        light: &MapLight,
        site: &TexelSite,
        trace: impl Fn(Vec3, Vec3) -> bool,
        reach: impl Fn(Vec3, Vec3) -> bool,
        lead_in: impl Fn(Vec3, Vec3) -> bool,
    ) -> f32 {
        match self {
            Self::Centre(probes) => soft_visibility_with(
                site.world_p,
                site.normal,
                light,
                site.seed,
                || *probes,
                trace,
            ),
            Self::Footprint(footprint) => {
                footprint_visibility(light, site, footprint, trace, reach, lead_in).visibility
            }
        }
    }
}

/// One texel's footprint result, with how it was reached.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct FootprintOutcome {
    pub(crate) visibility: f32,
    /// A probe was dropped: its origin lay behind a wall from the centre, or
    /// its shadow ray's lead-in met one.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) clipped_probe: bool,
    /// The probes agreed and the rest of the set went untraced.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) early_out: bool,
}

/// The per-texel seed's high half: the emitter lattice reads its low half, so
/// the footprint and emitter rotations vary independently.
fn footprint_rotation(seed: u64) -> (f32, f32) {
    let turn = (seed >> 32) as u32 as f32 / u32::MAX as f32;
    (turn * TAU).sin_cos()
}

/// Visibility filtered over the texel's footprint: the fraction of footprint
/// samples whose shadow ray is clear, each sample pairing a point of the
/// footprint with one point of the emitter.
///
/// Each sample's ray leaves from its own footprint point; N·L and falloff
/// stay at the centre (the caller's). A hard emitter traces its single target
/// from every point. A sample whose origin the centre cannot reach (`reach`
/// blocked: inside or behind a wall at an inside corner, or past a wall
/// thinner than the footprint) is dropped and the fraction renormalizes over
/// the survivors. Clip results depend on the texel alone.
///
/// Every trace starts `RAY_EPSILON` along its segment, clear of the receiver,
/// and tests nothing before that. A texel centre and a footprint point alike
/// can lie within that distance of a wall, wherever a face's edge, or a solid
/// standing on it, cuts the texel grid off its axes. So the clip segment,
/// from the lifted centre to the lifted point, is handed to `reach` starting
/// `RAY_EPSILON` behind the centre: the skip then covers exactly the stretch
/// behind the centre, and every wall the segment crosses clips, however near
/// the centre. Only a wall within `f32` rounding of the centre, where its side
/// is ambiguous anyway, could go either way.
///
/// A ray from a footprint point heading into a wall that near would start
/// past the wall's face, inside a brush whose other sides a compile culls,
/// and escape to a light the wall hides. So a clear ray from a footprint point
/// counts only if its lead-in, the skipped stretch overlapped by
/// [`LEAD_IN_OVERLAP`] at each end, enters no wall (`lead_in`); otherwise the
/// sample is dropped, as a clipped one is. A blocked ray takes no light, so it
/// needs no lead-in. The centre's ray stays the texel's own, traced as the
/// floor-off bake traces it.
///
/// The nine probes are traced first. When none is clipped and all agree, the
/// texel is exactly 0 or 1. A clipped probe breaks the ring, so the texel then
/// traces its full set even if its survivors agree. A soft emitter's samples
/// pair with footprint points per texel, so the probes stay a centre and a
/// ring in every occluder plane ([`TexelFootprint::emitter_pairing`]).
pub(crate) fn footprint_visibility(
    light: &MapLight,
    site: &TexelSite,
    footprint: &TexelFootprint,
    trace: impl Fn(Vec3, Vec3) -> bool,
    reach: impl Fn(Vec3, Vec3) -> bool,
    lead_in: impl Fn(Vec3, Vec3) -> bool,
) -> FootprintOutcome {
    footprint_visibility_with(light, site, footprint, trace, reach, lead_in, true)
}

/// [`footprint_visibility`] with the probe early-out optional, so tests can
/// trace every sample for a reference.
fn footprint_visibility_with(
    light: &MapLight,
    site: &TexelSite,
    footprint: &TexelFootprint,
    trace: impl Fn(Vec3, Vec3) -> bool,
    reach: impl Fn(Vec3, Vec3) -> bool,
    lead_in: impl Fn(Vec3, Vec3) -> bool,
    early_out: bool,
) -> FootprintOutcome {
    let lift = site.normal * RAY_EPSILON;
    let centre_origin = site.world_p + lift;
    let hard = match light.light_type {
        LightType::Point | LightType::Spot => light.light_size <= 0.0,
        LightType::Directional => light.angular_diameter <= 0.0,
    };
    let rotation = footprint_rotation(site.seed);
    // A hard emitter traces one target from every point, so it pairs nothing.
    let mut pairing = (!hard).then(|| footprint.emitter_pairing(light, site, rotation));
    // `None` when the sample is clipped, else whether its ray is clear.
    let sample = |k: usize, pairing: Option<&EmitterPairing>| -> Option<bool> {
        let point = if k == 0 {
            site.world_p
        } else {
            let offset = site.frame.offset(footprint.offsets[k], rotation);
            let point = site.world_p + offset;
            // Started `RAY_EPSILON` behind the centre, `reach` skips exactly
            // the stretch behind it and tests the clip segment from the
            // centre on.
            if site.may_clip
                && !reach(
                    centre_origin - offset.normalize_or_zero() * RAY_EPSILON,
                    point + lift,
                )
            {
                return None;
            }
            point
        };
        // The centre probe's ray runs to the emitter's centre, soft or hard.
        let target = match pairing {
            Some(pairing) if k > 0 => area_sample_target(
                point,
                light,
                site.seed,
                footprint.emitter_index(pairing, k),
                footprint.full_samples,
            ),
            _ => hard_ray_target(point, light),
        };
        let origin = point + lift;
        let clear = trace(origin, target);
        if clear && k > 0 && site.may_clip {
            let along = (target - origin).normalize_or_zero();
            let lead_in_clear = lead_in(
                origin - along * LEAD_IN_OVERLAP,
                origin + along * (RAY_EPSILON + LEAD_IN_OVERLAP),
            );
            if !lead_in_clear {
                return None;
            }
        }
        Some(clear)
    };

    let mut clear = 0u32;
    let mut surviving = 0u32;
    let mut clipped_probe = false;
    for k in 0..FOOTPRINT_PROBES {
        match sample(k, pairing.as_ref()) {
            Some(is_clear) => {
                surviving += 1;
                clear += u32::from(is_clear);
            }
            None => clipped_probe = true,
        }
    }
    // The centre is never clipped, so `surviving >= 1`.
    if early_out && !clipped_probe && (clear == 0 || clear == surviving) {
        return FootprintOutcome {
            visibility: clear as f32 / surviving as f32,
            clipped_probe,
            early_out: true,
        };
    }
    if let Some(pairing) = pairing.as_mut() {
        footprint.claim_counter_ring(pairing);
    }
    for k in FOOTPRINT_PROBES..footprint.offsets.len() {
        if let Some(is_clear) = sample(k, pairing.as_ref()) {
            surviving += 1;
            clear += u32::from(is_clear);
        }
    }
    FootprintOutcome {
        visibility: clear as f32 / surviving as f32,
        clipped_probe,
        early_out: false,
    }
}

mod clip_reach;
mod pairing;
pub(crate) use clip_reach::{ClipBand, ClipReach, lead_in_clear};
use pairing::EmitterPairing;

#[cfg(test)]
mod test_fixtures;
#[cfg(test)]
mod tests;
