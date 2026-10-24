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

/// Whether a bake floors texel visibility when nothing says otherwise.
pub(crate) const PENUMBRA_FLOOR_DEFAULT: bool = true;

/// Footprint samples traced before escalating: the texel centre plus an
/// eight-point ring. The centre catches a strip thinner than half a texel that
/// runs through the texel; the ring catches an edge anywhere within the
/// kernel's reach, so agreeing probes never snap a texel the kernel would
/// filter to a fraction. A four-point ring lets such a strip slip between its
/// points.
pub(crate) const FOOTPRINT_PROBES: usize = 9;

const RING_SAMPLES: usize = FOOTPRINT_PROBES - 1;

/// Smallest ring radius, in texels. A texel's probes must split whenever the
/// kernel filters it to a fraction, which is within 0.52 texel of an edge (the
/// cone's 10% point). Eight points 45° apart put one within 22.5° of any edge
/// normal, so this radius reaches `0.6 · cos 22.5° ≈ 0.55` texel across it. At
/// nine samples, centre plus ring, it also keeps the spread near the cone's
/// (`E[r²] = 0.3` wants a ring at 0.58).
pub(crate) const RING_MIN_RADIUS_TEXELS: f32 = 0.6;

/// Ring probe `k` sits at ring position `k · 3 mod 8`: the probes' emitter
/// samples descend the emitter's polar lattice with `k`, and this stride puts
/// consecutive ones 135° apart on the ring, so footprint angle and emitter
/// latitude do not move together.
const RING_POSITION_STRIDE: usize = 3;

/// Golden angle, the azimuth step between interior strata.
const GOLDEN_ANGLE: f32 = 2.399_963_3;

/// Radius `r` in texels whose cone-kernel CDF `F(r) = 3r² − 2r³` equals `u`:
/// the inverse smoothstep. The cone `w(r) = 1 − r` on `r < 1` is the round
/// one-texel kernel, matching bilinear's one-texel reach on every axis.
pub(crate) fn cone_radius_quantile(u: f32) -> f32 {
    0.5 - ((1.0 - 2.0 * u).asin() / 3.0).sin()
}

/// One light's footprint sample set at one sample count: an offset in texels
/// and an emitter-lattice index per sample, in trace order.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct TexelFootprint {
    full_samples: u32,
    /// Unrotated offsets in parent-grid texels: the centre, the eight ring
    /// probes, then the interior strata by ascending radius.
    offsets: Vec<[f32; 2]>,
    /// Emitter-lattice index paired with each offset. Hard lights ignore it.
    emitter: Vec<u32>,
}

impl TexelFootprint {
    /// `N = max(area_sample_count, 9)` radial strata at the cone quantiles
    /// `F⁻¹((j + 0.5) / N)`. Stratum 0 becomes the exact centre and the top
    /// eight the ring; the rest take golden-angle azimuths.
    fn build(light: &MapLight, area_sample_count: u32) -> Self {
        let full_samples = area_sample_count.max(FOOTPRINT_PROBES as u32);
        let count = full_samples as usize;
        let radius = |stratum: usize| cone_radius_quantile((stratum as f32 + 0.5) / count as f32);
        let polar = |r: f32, angle: f32| [r * angle.cos(), r * angle.sin()];

        let mut offsets = Vec::with_capacity(count);
        offsets.push([0.0, 0.0]);
        let ring_first = count - RING_SAMPLES;
        for k in 0..RING_SAMPLES {
            let position = (k * RING_POSITION_STRIDE) % RING_SAMPLES;
            let r = radius(ring_first + position).max(RING_MIN_RADIUS_TEXELS);
            offsets.push(polar(r, position as f32 * FRAC_PI_4));
        }
        for stratum in 1..ring_first {
            offsets.push(polar(radius(stratum), GOLDEN_ANGLE * stratum as f32));
        }

        // The probes take emitter samples spread over the whole emitter, so an
        // occluder edge anywhere across it splits them. The remaining emitter
        // samples, ascending in polar angle, are dealt to the interior strata,
        // ascending in radius, by a stride coprime to their count, so emitter
        // latitude does not track footprint radius.
        let probes = probe_emitter_indices(light, full_samples);
        let rest: Vec<u32> = (0..full_samples)
            .filter(|index| !probes.contains(index))
            .collect();
        let stride = decorrelating_stride(rest.len());
        let mut emitter = probes.to_vec();
        emitter.extend((0..rest.len()).map(|slot| rest[(slot * stride) % rest.len()]));

        Self {
            full_samples,
            offsets,
            emitter,
        }
    }

    /// Unrotated offsets in parent-grid texels, in trace order.
    #[cfg(test)]
    pub(crate) fn offsets(&self) -> &[[f32; 2]] {
        &self.offsets
    }

    /// Emitter-lattice index per sample, in trace order.
    #[cfg(test)]
    pub(crate) fn emitter(&self) -> &[u32] {
        &self.emitter
    }
}

/// Emitter-lattice index of each probe. The lattice's latitude follows its
/// index, so probe `k` searches the `k`-th of nine equal index bands and takes
/// the sample nearest the `k`-th direction of the nine-point lattice: one per
/// band spreads the probes pole to pole (or axis to rim) at every count, and
/// matching the small lattice within the band spreads their azimuths. A
/// nearest-direction snap over the whole lattice does not: nine ideal
/// directions sit close enough to the full lattice's first nine samples that
/// a narrow cone snaps to exactly those, all within its inner half. Bands
/// are disjoint, so the indices are distinct; at nine samples each band is
/// one index.
fn probe_emitter_indices(light: &MapLight, full_samples: u32) -> [u32; FOOTPRINT_PROBES] {
    let bands = FOOTPRINT_PROBES as u32;
    let band_start = |k: u32| (k * full_samples).div_ceil(bands);
    std::array::from_fn(|k| {
        let k = k as u32;
        let ideal = probe_sample_direction(light, k, bands);
        let mut best = band_start(k);
        let mut best_dot = f32::NEG_INFINITY;
        // Ties keep the lowest index, so the choice is deterministic.
        for index in band_start(k)..band_start(k + 1) {
            let dot = probe_sample_direction(light, index, full_samples).dot(ideal);
            if dot > best_dot {
                best_dot = dot;
                best = index;
            }
        }
        best
    })
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

/// What a footprint's emitter pairing depends on besides the count. Point
/// and spot lights snap on the light-independent sphere lattice; a
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
    /// Whether any triangle could block this texel's clip segments
    /// ([`ClipReach::may_clip`]). When none can, every segment is clear and
    /// the footprint traces none.
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

    /// The texel's raw visibility in `[0, 1]`. `trace(from, to)` and
    /// `reach(from, to)` both return true for a clear segment; `reach` tests
    /// the footprint's wall clip and should keep its own occluder cache.
    pub(crate) fn visibility(
        &self,
        light: &MapLight,
        site: &TexelSite,
        trace: impl Fn(Vec3, Vec3) -> bool,
        reach: impl Fn(Vec3, Vec3) -> bool,
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
                footprint_visibility(light, site, footprint, trace, reach).visibility
            }
        }
    }
}

/// One texel's footprint result, with how it was reached.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct FootprintOutcome {
    pub(crate) visibility: f32,
    /// A probe's origin lay behind a wall from the centre and was dropped.
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
/// The nine probes are traced first. When none is clipped and all agree, the
/// texel is exactly 0 or 1. A clipped probe breaks the ring, so the texel then
/// traces its full set even if its survivors agree.
pub(crate) fn footprint_visibility(
    light: &MapLight,
    site: &TexelSite,
    footprint: &TexelFootprint,
    trace: impl Fn(Vec3, Vec3) -> bool,
    reach: impl Fn(Vec3, Vec3) -> bool,
) -> FootprintOutcome {
    let lift = site.normal * RAY_EPSILON;
    let centre_origin = site.world_p + lift;
    let hard = match light.light_type {
        LightType::Point | LightType::Spot => light.light_size <= 0.0,
        LightType::Directional => light.angular_diameter <= 0.0,
    };
    let rotation = footprint_rotation(site.seed);
    // `None` when the sample is clipped, else whether its ray is clear.
    let sample = |k: usize| -> Option<bool> {
        let point = if k == 0 {
            site.world_p
        } else {
            let point = site.world_p + site.frame.offset(footprint.offsets[k], rotation);
            if site.may_clip && !reach(centre_origin, point + lift) {
                return None;
            }
            point
        };
        let target = if hard {
            hard_ray_target(point, light)
        } else {
            area_sample_target(
                point,
                light,
                site.seed,
                footprint.emitter[k],
                footprint.full_samples,
            )
        };
        Some(trace(point + lift, target))
    };

    let mut clear = 0u32;
    let mut surviving = 0u32;
    let mut clipped_probe = false;
    for k in 0..FOOTPRINT_PROBES {
        match sample(k) {
            Some(is_clear) => {
                surviving += 1;
                clear += u32::from(is_clear);
            }
            None => clipped_probe = true,
        }
    }
    // The centre is never clipped, so `surviving >= 1`.
    if !clipped_probe && (clear == 0 || clear == surviving) {
        return FootprintOutcome {
            visibility: clear as f32 / surviving as f32,
            clipped_probe,
            early_out: true,
        };
    }
    for k in FOOTPRINT_PROBES..footprint.offsets.len() {
        if let Some(is_clear) = sample(k) {
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
pub(crate) use clip_reach::ClipReach;

#[cfg(test)]
mod test_fixtures;
#[cfg(test)]
mod tests;
