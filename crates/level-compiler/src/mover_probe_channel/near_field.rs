// The mover probe channel's near-field domain: the probes a mid pose gathers
// bounce one and depth for.
// See: context/lib/build_pipeline.md §Compiler pipeline step 12 (Mover probe channel, Poses)

use glam::{DVec3, Vec3};

use crate::lightmap_bake::RAY_EPSILON;
use crate::mover_light_gate::AnalysedMover;
use crate::partition::Aabb;

/// The mover's brushes swept along its path, widened by the gather's lift
/// plus one probe spacing. A probe inside gathers at every mid pose; one
/// outside stores no mid-pose bounce or depth and blends across. Why the
/// spacing: a probe one lattice step off the sweep still reads the mover in
/// its rays, and probes already blur over one step.
#[derive(Debug, Clone)]
pub(crate) struct NearFieldDomain {
    boxes: Vec<Aabb>,
}

impl NearFieldDomain {
    pub(crate) fn new(mover: &AnalysedMover, probe_spacing: f32) -> Self {
        let margin = DVec3::splat(f64::from(RAY_EPSILON) + f64::from(probe_spacing));
        let boxes = mover
            .brush_sweeps
            .iter()
            .map(|sweep| Aabb {
                min: sweep.min - margin,
                max: sweep.max + margin,
            })
            .collect();
        Self { boxes }
    }

    /// Inclusive: a probe on the boundary is inside.
    pub(crate) fn contains(&self, probe: Vec3) -> bool {
        let p = probe.as_dvec3();
        self.boxes
            .iter()
            .any(|b| p.cmpge(b.min).all() && p.cmple(b.max).all())
    }

    /// The widened boxes, in brush-then-segment order, for cache keys.
    pub(crate) fn boxes(&self) -> &[Aabb] {
        &self.boxes
    }
}
