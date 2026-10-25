// Soft-light emitter pairing: which emitter sample each footprint sample of one texel traces.
// See: context/lib/build_pipeline.md §Compiler pipeline (step 12)

use glam::Vec3;

use super::{
    COUNTER_RING_SAMPLES, FOOTPRINT_PROBES, RING_POSITION_STRIDE, RING_SAMPLES, TexelFootprint,
    TexelSite, sun_axis,
};
use crate::lightmap_bake::{RAY_EPSILON, emitter_lattice_spin};
use crate::map_data::{LightType, MapLight};

// Every function the walk reaches here is `#[inline]`: the parent module
// calls in per texel and per sample, and the dev profile, with LTO off,
// inlines nothing across modules without it (about 5% of a soft-light
// lightmap bake).

/// Smallest cosine between the light direction and the receiver normal that
/// emitter offsets are projected along the light at; below it they project
/// straight down, so a grazing ray cannot blow them up.
const MIN_PROJECTION_COSINE: f32 = 1.0e-3;

/// Which emitter sample each footprint sample of one soft-light texel traces.
#[derive(Debug, Clone, Copy)]
pub(super) struct EmitterPairing {
    /// Lattice sample `e` lies at `(x_axis·e, y_axis·e)` in the footprint's
    /// unrotated texel frame, up to the emitter's scale
    /// ([`TexelFootprint::emitter_pairing`]).
    pub(super) x_axis: Vec3,
    pub(super) y_axis: Vec3,
    /// `[0]` is the sample nearest the centre, which the centre probe's ray
    /// to the emitter's centre replaces; `[k]` is ring probe `k`'s sample.
    pub(super) probes: [u32; FOOTPRINT_PROBES],
    /// `counter[j]` pairs with offset `len − 1 − j`, once claimed.
    pub(super) counter: [u32; COUNTER_RING_SAMPLES],
    /// The probes' samples, then the counter ring's: the first `claimed_len`,
    /// sorted ascending once the counter ring is claimed.
    pub(super) claimed: [u32; FOOTPRINT_PROBES + COUNTER_RING_SAMPLES],
    pub(super) claimed_len: usize,
}

impl EmitterPairing {
    #[inline]
    pub(super) fn frame_position(&self, offset: Vec3) -> [f32; 2] {
        [self.x_axis.dot(offset), self.y_axis.dot(offset)]
    }
}

/// Unit direction of each ring position, `position · 45°`, in the
/// footprint's unrotated texel frame.
pub(super) const RING_DIRECTIONS: [[f32; 2]; RING_SAMPLES] = {
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
#[inline]
pub(super) fn ring_sector([x, y]: [f32; 2]) -> (usize, f32) {
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
#[inline]
fn reach_key(reach: f32, index: u32) -> u64 {
    (u64::from(reach.to_bits()) << 32) | u64::from(u32::MAX - index)
}

/// The lattice index a [`reach_key`] holds; `u32::MAX` for an empty slot.
#[inline]
fn reach_key_index(key: u64) -> u32 {
    u32::MAX - key as u32
}

impl TexelFootprint {
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
    #[inline]
    pub(super) fn emitter_pairing(
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
    #[inline]
    pub(super) fn pairing_frame(
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
    #[inline]
    pub(super) fn claim_probes(&self, pairing: &mut EmitterPairing) {
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

    /// Claims the counter ring's emitter samples, before the strata off the ring
    /// are traced.
    ///
    /// Aligning the ring's footprint and emitter offsets correlates them, so
    /// those eight samples land farther out than independent pairs would and
    /// the full set alone would read a wider penumbra: an occluder standing
    /// well clear of the receiver gains over half a texel of ramp. The
    /// strata off the ring with the largest radii, largest first, each take
    /// the unclaimed sample reaching farthest against their own offset, which
    /// cancels most of that correlation, so an escalated texel keeps nearly
    /// the spread of its footprint and emitter taken independently.
    #[inline]
    pub(super) fn claim_counter_ring(&self, pairing: &mut EmitterPairing) {
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
    #[inline]
    pub(super) fn farthest_unclaimed(&self, pairing: &EmitterPairing, [x, y]: [f32; 2]) -> u32 {
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
    /// are dealt to the remaining strata, ascending in radius, by a stride
    /// coprime to their count, so emitter latitude does not track footprint
    /// radius.
    #[inline]
    pub(super) fn emitter_index(&self, pairing: &EmitterPairing, k: usize) -> u32 {
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

    /// Emitter sample per footprint sample, in trace order, for one texel of
    /// a soft light; the centre's is the sample its ray to the emitter's
    /// centre replaces.
    #[cfg(test)]
    pub(crate) fn emitter_indices(&self, light: &MapLight, site: &TexelSite) -> Vec<u32> {
        let mut pairing = self.emitter_pairing(light, site, super::footprint_rotation(site.seed));
        self.claim_counter_ring(&mut pairing);
        (0..self.offsets.len())
            .map(|k| self.emitter_index(&pairing, k))
            .collect()
    }
}
