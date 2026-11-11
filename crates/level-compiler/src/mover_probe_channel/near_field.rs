// The mover probe channel's near-field domain: the probes a mid pose gathers
// bounce and depth for, and the static patches its later bounces gather.
// See: context/lib/build_pipeline.md §Compiler pipeline step 12 (Mover probe channel, Poses)

use glam::{DVec3, Vec3};

use crate::lightmap_bake::RAY_EPSILON;
use crate::mover_light_gate::AnalysedMover;
use crate::partition::Aabb;
use crate::sh_bake::{ProbeGridLayout, vec3_from};
use crate::surface_radiance::PatchSet;

/// The mover's brushes swept along its path, widened by the gather's lift
/// plus one probe spacing. A probe inside gathers at every mid pose; one
/// outside stores no mid-pose bounce or depth and blends across. A static
/// patch whose centre lies inside gathers a mid pose's later bounces posed. Why the
/// spacing: a probe one lattice step off the sweep still reads the mover in
/// its rays, and probes already blur over one step.
#[derive(Debug, Clone)]
pub(crate) struct NearFieldDomain {
    boxes: Vec<Aabb>,
    probe_spacing: f32,
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
        Self {
            boxes,
            probe_spacing,
        }
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

    /// The probe spacing the boxes are widened by, for cache keys.
    pub(crate) fn probe_spacing(&self) -> f32 {
        self.probe_spacing
    }

    /// The static patches whose centre lies inside the domain, ascending:
    /// the ones a mid pose's later bounces gather posed.
    pub(crate) fn static_patches(&self, patches: &PatchSet) -> Vec<u32> {
        (0..patches.len() as u32)
            .filter(|&patch| self.contains(patches.patches()[patch as usize].centroid))
            .collect()
    }

    /// The valid probes of `layout` inside the domain, ascending by dense
    /// index, at the `f32` positions the gather traces from.
    pub(crate) fn valid_probes(&self, layout: &ProbeGridLayout) -> Vec<u32> {
        (0..layout.total_probes())
            .filter(|&i| {
                layout.validity[i] != 0 && self.contains(vec3_from(layout.probe_positions[i]))
            })
            .map(|i| i as u32)
            .collect()
    }
}
