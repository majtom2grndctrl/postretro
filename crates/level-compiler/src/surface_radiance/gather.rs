// Probe gather from the surface radiance cache: a hit reads its patch, no shadow ray.
// See: context/lib/build_pipeline.md §Compiler pipeline

use glam::Vec3;

use super::patches::PatchSet;
use super::sum::PatchRadiance;
use crate::sh_bake::{Hit, RaytracingCtx, SKY_COLOR, gather_hit};

/// How a gather hit reads the patches of its face.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PatchLookup {
    /// The patch whose cell holds the hit: the parity measurement's
    /// comparison against [`Self::Bilinear`], which production reads.
    #[cfg(test)]
    Nearest,
    /// The bilinear blend of the patches whose centres surround the hit.
    Bilinear,
}

/// The SH probe gather's radiance source over a summed patch cache.
pub(crate) struct PatchGather<'a> {
    patches: &'a PatchSet,
    radiance: &'a PatchRadiance,
    lookup: PatchLookup,
}

impl<'a> PatchGather<'a> {
    pub(crate) fn new(
        patches: &'a PatchSet,
        radiance: &'a PatchRadiance,
        lookup: PatchLookup,
    ) -> Self {
        Self {
            patches,
            radiance,
            lookup,
        }
    }

    /// One gather ray's outgoing radiance and hit distance. The hit is the
    /// per-hit estimator's ([`gather_hit`]), so a miss returns `SKY_COLOR`
    /// and `far_sentinel` and a hit its own distance: the depth moments do
    /// not depend on the radiance source.
    pub(crate) fn sample(
        &self,
        ctx: &RaytracingCtx<'_>,
        origin: Vec3,
        dir: Vec3,
        far_sentinel: f32,
    ) -> (Vec3, f32) {
        match gather_hit(ctx, origin, dir) {
            None => (Vec3::from(SKY_COLOR), far_sentinel),
            Some(hit) => (self.radiance_at(&hit, dir), hit.distance),
        }
    }

    /// The stored radiance of the patch (or blend) and side `hit` reads;
    /// zero on a face with no patch.
    pub(crate) fn radiance_at(&self, hit: &Hit, dir: Vec3) -> Vec3 {
        match self.lookup {
            #[cfg(test)]
            PatchLookup::Nearest => self
                .patches
                .lookup(hit.triangle, hit.point, dir)
                .map_or(Vec3::ZERO, |(patch, side)| self.radiance.get(patch, side)),
            PatchLookup::Bilinear => self
                .patches
                .lookup_bilinear(hit.triangle, hit.point, dir)
                .map_or(Vec3::ZERO, |(blend, side)| {
                    blend
                        .entries()
                        .iter()
                        .map(|&(patch, w)| self.radiance.get(patch, side) * w)
                        .sum()
                }),
        }
    }
}
