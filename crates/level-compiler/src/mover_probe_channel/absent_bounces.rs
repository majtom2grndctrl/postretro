// The mover-absent later bounces: id 34's own later bounces, one front side
// per static patch each, held apart for the posed bakes. A pose keeps those
// past its posed ones; a mid pose's near-field later bounces read them
// wherever a gather ray leaves its domain.
// See: context/lib/build_pipeline.md §Compiler pipeline step 12 (Mover probe channel)

use super::bake::{MoverBounceJob, sh_base_bounce_count};

/// Id 34's later bounces in bounce order, unscaled, from bounce two on: one
/// front side per static patch each. Shorter than the SH base's count where
/// a bounce went black and id 34 stopped: every bounce past it is black.
#[derive(Debug, Default, Clone, PartialEq)]
pub(crate) struct AbsentLaterBounces {
    pub(crate) layers: Vec<Vec<[f32; 3]>>,
}

impl AbsentLaterBounces {
    /// Later bounce `index` (0 is bounce two), or `None` past the last one
    /// id 34 gathered: black.
    pub(crate) fn layer(&self, index: u32) -> Option<&[[f32; 3]]> {
        self.layers.get(index as usize).map(Vec::as_slice)
    }

    /// The later bounces from index `from` on, in bounce order: what a pose
    /// that re-bakes `from` later bounces posed keeps of id 34's. The pose
    /// adds them one by one, as id 34 does: pre-summing them would
    /// reassociate the float sum.
    pub(crate) fn layers_from(&self, from: u32) -> &[Vec<[f32; 3]>] {
        self.layers.get(from as usize..).unwrap_or(&[])
    }

    /// Host bytes the layers hold.
    #[cfg(test)]
    pub(crate) fn heap_bytes(&self) -> u64 {
        self.layers
            .iter()
            .map(|layer| (layer.capacity() * size_of::<[f32; 3]>()) as u64)
            .sum()
    }

    /// Whether any of `jobs` reads id 34's later bounces at `fill`: a job
    /// re-baking bounce whose `light_bounces` stops short of id 34's count
    /// keeps those past its posed ones, and one with a mid pose reads them
    /// past its near-field domain. At fill 0 there are none.
    pub(crate) fn needed_by<'j>(
        jobs: impl IntoIterator<Item = &'j MoverBounceJob>,
        fill: f32,
    ) -> bool {
        fill != 0.0
            && jobs.into_iter().any(|job| {
                job.bounces > 0 && (job.has_mid_poses() || job.bounces < sh_base_bounce_count(fill))
            })
    }
}
