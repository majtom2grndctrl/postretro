// Area-light texel footprint: the polygon lattice a texel samples, paired with its footprint
// points, combined by the ratio estimator.
// See: context/lib/build_pipeline.md §Compiler pipeline (step 12, Area lights)

use glam::{DVec3, Vec3};

use super::{
    FOOTPRINT_PROBES, FootprintOutcome, ProbeEarlyOut, TexelFootprint, TexelSite, footprint_point,
    footprint_rotation, trace_footprint_sample,
};
use crate::area_light::{AreaSample, area_lattice_point, weighted_visibility};
use crate::lightmap_bake::area_sampler;
use crate::map_data::MapLight;

/// One texel's polygon lattice: the targets its footprint traces and the
/// weight each carries.
pub(super) struct AreaLattice {
    /// Each lattice sample's target, lifted off its polygon, as traced.
    pub(super) targets: Vec<Vec3>,
    /// Each sample's cosθ at the texel centre, the ratio estimator's weight.
    pub(super) weights: Vec<f64>,
    /// The targets' mean, which the pairing frame projects along.
    anchor: Vec3,
}

impl AreaLattice {
    /// The `full_samples` lattice samples at `site`, from the solid-angle
    /// sampler over the polygons whose front half-space holds the texel
    /// centre; `None` when no polygon faces it, where the light has no term.
    /// The polygon lattice depends on the light and the receiver, so it is
    /// built per texel and never memoized (research.md O9).
    pub(super) fn at(light: &MapLight, site: &TexelSite, full_samples: u32) -> Option<Self> {
        let sampler = area_sampler(light, site.world_p, site.normal)?;
        let samples: Vec<AreaSample> = (0..full_samples)
            .map(|i| sampler.sample(area_lattice_point(i, full_samples, site.seed)))
            .collect();
        let sum: DVec3 = samples.iter().map(|sample| sample.target).sum();
        Some(Self {
            targets: samples
                .iter()
                .map(|sample| sample.target.as_vec3())
                .collect(),
            weights: samples.iter().map(|sample| sample.weight).collect(),
            anchor: (sum / f64::from(full_samples.max(1))).as_vec3(),
        })
    }

    /// Each sample's offset from the anchor: the lattice the pairing reads.
    fn offsets(&self) -> Vec<Vec3> {
        self.targets
            .iter()
            .map(|&target| target - self.anchor)
            .collect()
    }
}

/// [`super::footprint_visibility`] for an area light: the Heitz–Hill–McGuire
/// ratio estimator Σ w·V / Σ w over the footprint's surviving samples, each
/// footprint point paired with one lattice sample and weighted by that
/// sample's cosθ at the texel centre, where N·L and the window stay
/// (research.md §2).
///
/// Pairing follows the sphere rule with the polygon's own offsets: each
/// sample's offset from the lattice's mean, projected onto the receiver
/// along the direction to that mean. The centre probe traces the lifted
/// lattice sample nearest the centre, the one it stands in for, so it is the
/// texel's own centre ray and never aims at the light's origin, which may
/// sit in solid or on the glass (research.md O19). Every target lies on a
/// polygon whose front half-space holds the texel centre, lifted off it by
/// the tracer tolerance, inside the emitter sphere's slack.
///
/// Agreeing probes end the texel as for a sphere, all blocked storing exactly
/// 0 and all clear exactly 1, but agreement counts only the probes that carry
/// weight ([`ProbeEarlyOut::ends_weighted`]): the lattice is not clipped to
/// the receiver's horizon, so where the receiver's plane cuts a polygon every
/// probe can lie below it, blocked by the receiver itself, on a lit texel.
/// Probes with no weight between them trace the full set. A texel whose
/// surviving samples all carry no weight stores 0 (research.md O8). An area
/// light is never flagged embedded, so production walks take
/// [`ProbeEarlyOut::Agreed`].
pub(super) fn footprint_visibility(
    light: &MapLight,
    site: &TexelSite,
    footprint: &TexelFootprint,
    trace: impl Fn(Vec3, Vec3) -> bool,
    reach: impl Fn(Vec3, Vec3) -> bool,
    lead_in: impl Fn(Vec3, Vec3) -> bool,
    early_out: ProbeEarlyOut,
) -> FootprintOutcome {
    let Some(lattice) = AreaLattice::at(light, site, footprint.full_samples) else {
        return FootprintOutcome {
            visibility: 0.0,
            clipped_probe: false,
            early_out: true,
        };
    };
    let offsets = lattice.offsets();
    let rotation = footprint_rotation(site.seed);
    let mut pairing = footprint.area_pairing_frame(site, rotation, lattice.anchor);
    footprint.claim_probes_in(&mut pairing, &offsets);
    // `None` when the sample is clipped, else its weight and whether its ray
    // is clear.
    let sample = |k: usize, pairing: &super::EmitterPairing| -> Option<(f64, bool)> {
        let point = footprint_point(site, footprint, k, rotation, &reach)?;
        let index = footprint.emitter_index(pairing, k) as usize;
        let clear =
            trace_footprint_sample(site, k, point, lattice.targets[index], &trace, &lead_in)?;
        Some((lattice.weights[index], clear))
    };

    let mut traced: Vec<(f64, bool)> = Vec::with_capacity(footprint.offsets.len());
    let mut clipped_probe = false;
    for k in 0..FOOTPRINT_PROBES {
        match sample(k, &pairing) {
            Some(traced_sample) => traced.push(traced_sample),
            None => clipped_probe = true,
        }
    }
    if !clipped_probe && early_out.ends_weighted(&traced) {
        return FootprintOutcome {
            visibility: weighted_visibility(traced) as f32,
            clipped_probe,
            early_out: true,
        };
    }
    footprint.claim_counter_ring_in(&mut pairing, &offsets);
    traced.extend((FOOTPRINT_PROBES..footprint.offsets.len()).filter_map(|k| sample(k, &pairing)));
    FootprintOutcome {
        visibility: weighted_visibility(traced) as f32,
        clipped_probe,
        early_out: false,
    }
}
