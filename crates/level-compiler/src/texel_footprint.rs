// Receiver footprint for static lightmap texels: cone-kernel sample layout, probe early-out, wall clip.
// See: context/lib/build_pipeline.md §Compiler pipeline (step 12)

use std::cell::RefCell;
use std::f32::consts::{FRAC_PI_4, TAU};
use std::sync::Arc;

use glam::Vec3;

use crate::chart_raster::chart_grid;
use crate::lightmap_bake::{
    Chart, RAY_EPSILON, SoftProbes, area_sample_target, emitter_lattice_spin, hard_ray_target,
    probe_sample_direction, soft_visibility_with,
};
use crate::map_data::{LightType, MapLight};

/// Whether a bake floors texel visibility when nothing says otherwise.
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

/// Smallest ring radius, in texels. A texel's probes must split whenever the
/// kernel filters it to a fraction, which is within 0.52 texel of an edge (the
/// cone's 10% point). Eight points 45° apart put one within 22.5° of any edge
/// normal, so this radius reaches `0.6 · cos 22.5° ≈ 0.55` texel across it. At
/// nine samples, centre plus ring, it also keeps the spread near the cone's
/// (`E[r²] = 0.3` wants a ring at 0.58).
pub(crate) const RING_MIN_RADIUS_TEXELS: f32 = 0.6;

/// Ring probe `k` sits at ring position `k · 3 mod 8`, so consecutive probes
/// lie 135° apart. Probes whose sector holds no emitter sample claim one in
/// probe order ([`TexelFootprint::emitter_pairing`]); the spread keeps
/// neighbouring sectors from claiming one after the other.
const RING_POSITION_STRIDE: usize = 3;

/// Golden angle, the azimuth step between interior strata.
const GOLDEN_ANGLE: f32 = 2.399_963_3;

/// Radius `r` in texels whose cone-kernel CDF `F(r) = 3r² − 2r³` equals `u`:
/// the inverse smoothstep. The cone `w(r) = 1 − r` on `r < 1` is the round
/// one-texel kernel, matching bilinear's one-texel reach on every axis.
pub(crate) fn cone_radius_quantile(u: f32) -> f32 {
    0.5 - ((1.0 - 2.0 * u).asin() / 3.0).sin()
}

/// Interior strata that pair against the ring's correlation: the eight just
/// inside it, or as many as there are ([`TexelFootprint::claim_counter_ring`]).
const COUNTER_RING_SAMPLES: usize = RING_SAMPLES;

/// Smallest cosine between the light direction and the receiver normal that
/// emitter offsets are projected along the light at; below it they project
/// straight down, so a grazing ray cannot blow them up.
const MIN_PROJECTION_COSINE: f32 = 1.0e-3;

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
    /// probes, then the interior strata by ascending radius, so the counter
    /// ring comes last.
    offsets: Vec<[f32; 2]>,
    /// Each emitter sample's offset from the emitter's centre at seed 0, up
    /// to scale: the unit sphere direction for a point or spot light, the part
    /// of the jittered direction perpendicular to the axis for a sun. A
    /// texel's seed turns the whole lattice ([`emitter_lattice_spin`]).
    lattice: Vec<Vec3>,
    /// Interior strata in the counter ring.
    counter_len: usize,
    /// Stride dealing the unclaimed emitter samples to the interior strata
    /// inside the counter ring ([`decorrelating_stride`]).
    rest_stride: usize,
}

/// Which emitter sample each footprint sample of one soft-light texel traces.
#[derive(Debug, Clone, Copy)]
struct EmitterPairing {
    /// Lattice sample `e` lies at `(x_axis·e, y_axis·e)` in the footprint's
    /// unrotated texel frame, up to the emitter's scale
    /// ([`TexelFootprint::emitter_pairing`]).
    x_axis: Vec3,
    y_axis: Vec3,
    /// `[0]` is the sample nearest the centre, which the centre probe's ray
    /// to the emitter's centre replaces; `[k]` is ring probe `k`'s sample.
    probes: [u32; FOOTPRINT_PROBES],
    /// `counter[j]` pairs with offset `len − 1 − j`, once claimed.
    counter: [u32; COUNTER_RING_SAMPLES],
    /// The probes' samples, then the counter ring's: the first `claimed_len`,
    /// sorted ascending once the counter ring is claimed.
    claimed: [u32; FOOTPRINT_PROBES + COUNTER_RING_SAMPLES],
    claimed_len: usize,
}

impl EmitterPairing {
    fn frame_position(&self, offset: Vec3) -> [f32; 2] {
        [self.x_axis.dot(offset), self.y_axis.dot(offset)]
    }
}

/// Unit direction of each ring position, `position · 45°`, in the
/// footprint's unrotated texel frame.
const RING_DIRECTIONS: [[f32; 2]; RING_SAMPLES] = {
    use std::f32::consts::FRAC_1_SQRT_2 as D;
    [
        [1.0, 0.0],
        [D, D],
        [0.0, 1.0],
        [-D, D],
        [-1.0, 0.0],
        [-D, -D],
        [0.0, -1.0],
        [D, -D],
    ]
};

/// The ring sector of a point in the footprint's unrotated texel frame, the
/// eighth of the plane within 22.5° of ring position `sector · 45°`, and how
/// far the point reaches along that direction: the ring direction it reaches
/// farthest along, among `±x`, `±(x + y)/√2`, `±y` and `±(y − x)/√2`. Ties
/// keep the first in that order, `+` before `−`.
///
/// A lattice sample's sector is as good as random, so the choice is written
/// as selects between magnitudes, which compile without branches to
/// mispredict, rather than a scan of the eight signed values.
fn ring_sector([x, y]: [f32; 2]) -> (usize, f32) {
    let diagonal = std::f32::consts::FRAC_1_SQRT_2;
    let along = [x, (x + y) * diagonal, y, (y - x) * diagonal];
    // Spelled out: a generic `along.map(f32::abs)` stayed an outlined call
    // in the dev profile, at about twice the cost of the whole sector pass.
    let reach = [
        along[0].abs(),
        along[1].abs(),
        along[2].abs(),
        along[3].abs(),
    ];
    let (low, low_reach) = if reach[1] > reach[0] {
        (1, reach[1])
    } else {
        (0, reach[0])
    };
    let (high, high_reach) = if reach[3] > reach[2] {
        (3, reach[3])
    } else {
        (2, reach[2])
    };
    let (position, reach) = if high_reach > low_reach {
        (high, high_reach)
    } else {
        (low, low_reach)
    };
    let opposite = usize::from(along[position] < 0.0) * (RING_SAMPLES / 2);
    (position + opposite, reach)
}

/// A lattice sample keyed for the sector pass: its reach's bits above its
/// inverted index, so keys compare as `(reach, −index)` do. A reach is never
/// negative, so its bits order like its value, and of two equal reaches the
/// lower index keys higher, as the first found does in a strict `>` scan.
/// Taking the larger of two keys is then a branch-free top-1 update. Key 0,
/// below every sample, marks an empty slot.
fn reach_key(reach: f32, index: u32) -> u64 {
    (u64::from(reach.to_bits()) << 32) | u64::from(u32::MAX - index)
}

/// The lattice index a [`reach_key`] holds; `u32::MAX` for an empty slot.
fn reach_key_index(key: u64) -> u32 {
    u32::MAX - key as u32
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
        let interior = count - FOOTPRINT_PROBES;
        let counter_len = COUNTER_RING_SAMPLES.min(interior);
        Self {
            full_samples,
            offsets,
            lattice,
            counter_len,
            rest_stride: decorrelating_stride(interior - counter_len),
        }
    }

    /// The emitter samples one soft-light texel's probes trace.
    ///
    /// A sample's ray from footprint point `p + δ` to emitter point `L + ε`
    /// crosses an occluder a fraction `t` of the way to the light. Carried
    /// back to the receiver along the light, that crossing sits at
    /// `δ + t / (1 − t) · ε_r` from the texel centre, where `ε_r` is the
    /// emitter offset projected onto the receiver plane along the light. The
    /// ring splits every edge within the kernel's reach only where those
    /// points still form a centre and a ring, at every `t`, so each probe
    /// pairs with an emitter point that keeps its place: ring probe `k` with
    /// the sample reaching farthest within its own 45° sector of the
    /// footprint frame, and the centre with the emitter's centre itself, in
    /// place of the sample nearest it. The centre probe is then the texel's
    /// own centre ray, so a strip too thin for the ring is caught for soft
    /// lights as for hard ones. A sector with no sample left takes the
    /// unclaimed sample reaching farthest its way. Ties keep the lowest
    /// index; the samples are distinct and deterministic.
    fn emitter_pairing(
        &self,
        light: &MapLight,
        site: &TexelSite,
        rotation: (f32, f32),
    ) -> EmitterPairing {
        let mut pairing = self.pairing_frame(light, site, rotation);
        self.claim_probes(&mut pairing);
        pairing
    }

    /// One texel's pairing with nothing claimed yet: the two vectors that
    /// carry a lattice sample into the footprint's unrotated texel frame.
    fn pairing_frame(
        &self,
        light: &MapLight,
        site: &TexelSite,
        rotation: (f32, f32),
    ) -> EmitterPairing {
        let axis = match light.light_type {
            LightType::Point | LightType::Spot => (light.origin.as_vec3()
                - (site.world_p + site.normal * RAY_EPSILON))
                .normalize_or_zero(),
            LightType::Directional => sun_axis(light),
        };
        // A world offset `w` projects onto the receiver along `axis` as
        // `w − axis · (n·w) / (n·axis)`; its texel coordinates are dots with
        // the pitch axes over their squared lengths, unrotated by the
        // footprint's turn. All of that is linear in `w`, so it folds into two
        // vectors, carried into the unrotated lattice's frame instead of
        // turning every lattice sample by the texel's seed.
        let normal = site.normal;
        let facing = normal.dot(axis);
        let lean = if facing > MIN_PROJECTION_COSINE {
            axis / facing
        } else {
            normal
        };
        let texel_axis = |pitch: Vec3| {
            let along = pitch / pitch.length_squared();
            along - normal * lean.dot(along)
        };
        let (u, v) = (
            texel_axis(site.frame.pitch_u),
            texel_axis(site.frame.pitch_v),
        );
        let (sin, cos) = rotation;
        let unspin = emitter_lattice_spin(light, site.seed).inverse();
        EmitterPairing {
            x_axis: unspin * (u * cos + v * sin),
            y_axis: unspin * (v * cos - u * sin),
            probes: [0; FOOTPRINT_PROBES],
            counter: [0; COUNTER_RING_SAMPLES],
            claimed: [0; FOOTPRINT_PROBES + COUNTER_RING_SAMPLES],
            claimed_len: 0,
        }
    }

    /// Claims the probes' emitter samples ([`TexelFootprint::emitter_pairing`]).
    fn claim_probes(&self, pairing: &mut EmitterPairing) {
        // One pass finds the sample nearest the centre and each sector's two
        // farthest, the second standing in when the first is that nearest.
        // Both are kept as keys, so every update is a `min` or `max`: the
        // nearest's squared radius (never negative) above its index, and the
        // sectors' [`reach_key`]s.
        let mut nearest = u64::MAX;
        let mut first = [0u64; RING_SAMPLES];
        let mut second = [0u64; RING_SAMPLES];
        // Copied out of `pairing`, which the compiler would otherwise reload
        // on every sample.
        let (x_axis, y_axis) = (pairing.x_axis, pairing.y_axis);
        for (index, offset) in self.lattice.iter().enumerate() {
            let index = index as u32;
            let position = [x_axis.dot(*offset), y_axis.dot(*offset)];
            let radius_sq = position[0] * position[0] + position[1] * position[1];
            nearest = nearest.min((u64::from(radius_sq.to_bits()) << 32) | u64::from(index));
            let (sector, reach) = ring_sector(position);
            let key = reach_key(reach, index);
            let displaced = first[sector];
            first[sector] = displaced.max(key);
            second[sector] = second[sector].max(displaced.min(key));
        }
        let nearest = nearest as u32;
        pairing.probes[0] = nearest;
        pairing.claimed[0] = nearest;
        pairing.claimed_len = 1;
        // Bit `k`: ring probe `k`'s sector held no sample to claim.
        let mut unfilled = 0u16;
        for k in 1..FOOTPRINT_PROBES {
            let sector = (k - 1) * RING_POSITION_STRIDE % RING_SAMPLES;
            let index = if reach_key_index(first[sector]) == nearest {
                reach_key_index(second[sector])
            } else {
                reach_key_index(first[sector])
            };
            if index == u32::MAX {
                unfilled |= 1 << k;
            } else {
                pairing.probes[k] = index;
                pairing.claimed[pairing.claimed_len] = index;
                pairing.claimed_len += 1;
            }
        }
        for k in (1..FOOTPRINT_PROBES).filter(|k| unfilled & (1 << k) != 0) {
            let sector = (k - 1) * RING_POSITION_STRIDE % RING_SAMPLES;
            let index = self.farthest_unclaimed(pairing, RING_DIRECTIONS[sector]);
            pairing.probes[k] = index;
            pairing.claimed[pairing.claimed_len] = index;
            pairing.claimed_len += 1;
        }
    }

    /// Claims the counter ring's emitter samples, before the interior strata
    /// are traced.
    ///
    /// Aligning the ring's footprint and emitter offsets correlates them, so
    /// those eight samples land farther out than independent pairs would and
    /// the full set alone would read a wider penumbra: an occluder standing
    /// well clear of the receiver gains over half a texel of ramp. The
    /// interior strata nearest the ring, largest first, each take the
    /// unclaimed sample reaching farthest against their own offset, which
    /// cancels most of that correlation, so an escalated texel keeps nearly
    /// the spread of its footprint and emitter taken independently.
    fn claim_counter_ring(&self, pairing: &mut EmitterPairing) {
        let len = self.offsets.len();
        for j in 0..self.counter_len {
            let [x, y] = self.offsets[len - 1 - j];
            let length = x.hypot(y);
            let index = self.farthest_unclaimed(pairing, [-x / length, -y / length]);
            pairing.counter[j] = index;
            pairing.claimed[pairing.claimed_len] = index;
            pairing.claimed_len += 1;
        }
        pairing.claimed[..pairing.claimed_len].sort_unstable();
    }

    /// The unclaimed lattice sample reaching farthest along unit direction
    /// `[x, y]` of the footprint's unrotated texel frame. Ties keep the lowest
    /// index.
    fn farthest_unclaimed(&self, pairing: &EmitterPairing, [x, y]: [f32; 2]) -> u32 {
        let claimed = &pairing.claimed[..pairing.claimed_len];
        let mut best = f32::NEG_INFINITY;
        let mut best_index = 0;
        for (index, offset) in self.lattice.iter().enumerate() {
            let position = pairing.frame_position(*offset);
            let reach = position[0] * x + position[1] * y;
            if reach > best && !claimed.contains(&(index as u32)) {
                best = reach;
                best_index = index as u32;
            }
        }
        best_index
    }

    /// Emitter sample traced by footprint sample `k`, once the counter ring
    /// is claimed. The samples left unclaimed, ascending in lattice latitude,
    /// are dealt to the interior strata inside the counter ring, ascending in
    /// radius, by a stride coprime to their count, so emitter latitude does
    /// not track footprint radius.
    fn emitter_index(&self, pairing: &EmitterPairing, k: usize) -> u32 {
        let len = self.offsets.len();
        if k < FOOTPRINT_PROBES {
            return pairing.probes[k];
        }
        if k >= len - self.counter_len {
            return pairing.counter[len - 1 - k];
        }
        let rest = len - FOOTPRINT_PROBES - self.counter_len;
        // The slot's rank among the unclaimed indices, stepped past each
        // claimed index at or below it.
        let mut index = ((k - FOOTPRINT_PROBES) * self.rest_stride % rest) as u32;
        for &claimed in &pairing.claimed[..pairing.claimed_len] {
            if claimed > index {
                break;
            }
            index += 1;
        }
        index
    }

    /// Unrotated offsets in parent-grid texels, in trace order.
    #[cfg(test)]
    pub(crate) fn offsets(&self) -> &[[f32; 2]] {
        &self.offsets
    }

    /// Emitter sample per footprint sample, in trace order, for one texel of
    /// a soft light; the centre's is the sample its ray to the emitter's
    /// centre replaces.
    #[cfg(test)]
    pub(crate) fn emitter_indices(&self, light: &MapLight, site: &TexelSite) -> Vec<u32> {
        let mut pairing = self.emitter_pairing(light, site, footprint_rotation(site.seed));
        self.claim_counter_ring(&mut pairing);
        (0..self.offsets.len())
            .map(|k| self.emitter_index(&pairing, k))
            .collect()
    }
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
    /// segment. `reach` tests the footprint's wall clip and should keep its
    /// own occluder cache; `lead_in` is [`lead_in_clear`] against the chart's
    /// [`ClipBand`].
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
/// The trace starts each ray `RAY_EPSILON` along it, clear of the receiver,
/// and tests nothing before that. A footprint point, unlike a texel centre
/// half a texel in from its face's edge, can lie within that distance of a
/// wall. A ray from it heading into the wall would start past the wall's
/// face, inside a brush whose other sides a compile culls, and escape to a
/// light the wall hides. So a clear ray from a footprint point counts only if
/// its lead-in, the skipped stretch overlapped by [`LEAD_IN_OVERLAP`] at each
/// end, enters no wall (`lead_in`); otherwise the sample is dropped, as a
/// clipped one is. A blocked ray takes no light, so it needs no lead-in, and
/// the centre's ray stays the texel's own.
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
            let point = site.world_p + site.frame.offset(footprint.offsets[k], rotation);
            if site.may_clip && !reach(centre_origin, point + lift) {
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
pub(crate) use clip_reach::{ClipBand, ClipReach, lead_in_clear};

#[cfg(test)]
mod test_fixtures;
#[cfg(test)]
mod tests;
