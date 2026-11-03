// Gating each analysed light's reached SH probes at both termini of every
// mover it may meet.
// See: context/lib/build_pipeline.md §Compiler pipeline (step 12)

use std::ops::Range;

use bvh::bvh::Bvh;
use rayon::prelude::*;

use super::lights::GateLight;
use super::pose::{MoverPoses, Terminus};
use super::texels::{CandidateMover, light_candidates};
use crate::bake_control::BakeControl;
use crate::bvh_build::BvhPrimitive;
use crate::geometry::GeometryResult;
use crate::lightmap_bake::hard_ray_target;
use crate::map_data::{LightType, MapLight};
use crate::partition::Aabb;
use crate::ray_traversal::segment_clear;
use crate::sh_bake::{ProbeGridLayout, light_reaches_point, spot_cone_parameters, vec3_from};

/// The SH probe grid and the static world the SH bakes trace.
pub(crate) struct ProbeScene<'a> {
    pub(crate) layout: &'a ProbeGridLayout,
    pub(crate) bvh: &'a Bvh<f32, 3>,
    pub(crate) primitives: &'a [BvhPrimitive],
    pub(crate) geometry: &'a GeometryResult,
}

/// One mover's gated probes for one light, at each terminus by
/// [`Terminus::index`].
#[derive(Debug, Clone)]
pub(crate) struct MoverProbes {
    /// Index into [`MoverPoses::analysed`].
    pub(crate) mover: usize,
    pub(crate) gated: [u64; 2],
    /// Probes gated at both termini.
    pub(crate) both: u64,
    pub(crate) light_inside: [bool; 2],
    /// World bounds of the gated probe centres.
    pub(crate) centres: [Option<Aabb>; 2],
}

impl MoverProbes {
    fn new(candidate: &CandidateMover) -> Self {
        Self {
            mover: candidate.mover,
            gated: [0; 2],
            both: 0,
            light_inside: candidate.inside.map(|inside| inside == Some(true)),
            centres: [None, None],
        }
    }
}

/// One light's gated probes, per mover with a gated probe.
#[derive(Debug, Clone)]
pub(crate) struct LightProbes {
    pub(crate) source_index: usize,
    pub(crate) movers: Vec<MoverProbes>,
}

/// Gate every light in `lights` over the probe grid, one governed work item
/// per light. Output follows `lights`' order, whatever order workers finish
/// in, and holds only lights with a gated probe.
///
/// A valid probe counts at a terminus when the light reaches it (inside its
/// falloff range, inside a spot's outer cone, and on a centre segment clear
/// of static geometry) and that segment touches a posed brush, or the light
/// sits inside one. The static trace runs last, only for a probe some pose
/// gates.
pub(crate) fn gate_probes(
    poses: &MoverPoses,
    lights: &[GateLight<'_>],
    scene: &ProbeScene<'_>,
    control: &BakeControl,
) -> Vec<LightProbes> {
    if poses.analysed.is_empty() || scene.layout.is_empty() {
        return Vec::new();
    }
    lights
        .par_iter()
        .filter_map(|gate_light| {
            let _permit = control.governor().enter();
            let candidates = light_candidates(gate_light.light, poses);
            if candidates.is_empty() {
                return None;
            }
            let movers = gate_light_probes(poses, gate_light.light, &candidates, scene);
            (!movers.is_empty()).then_some(LightProbes {
                source_index: gate_light.source_index,
                movers,
            })
        })
        .collect()
}

fn gate_light_probes(
    poses: &MoverPoses,
    light: &MapLight,
    candidates: &[CandidateMover],
    scene: &ProbeScene<'_>,
) -> Vec<MoverProbes> {
    let layout = scene.layout;
    let mut tallies: Vec<MoverProbes> = candidates.iter().map(MoverProbes::new).collect();
    let mut masks = vec![0u8; candidates.len()];
    let cone = (light.light_type == LightType::Spot).then(|| {
        let (axis, _, cos_outer) = spot_cone_parameters(light);
        (axis, cos_outer)
    });
    let origin = vec3_from(light.origin);
    let [nx, ny, _] = layout.dims.map(|count| count as usize);
    let [xs, ys, zs] = influence_ranges(light, layout);
    for z in zs {
        for y in ys.clone() {
            for x in xs.clone() {
                let index = x + y * nx + z * nx * ny;
                if layout.validity[index] == 0 {
                    continue;
                }
                let probe = vec3_from(layout.probe_positions[index]);
                if !light_reaches_point(light, probe) {
                    continue;
                }
                // Past the outer cone the spot's attenuation is exactly zero.
                if let Some((axis, cos_outer)) = cone
                    && axis.dot((probe - origin).normalize_or_zero()) <= cos_outer
                {
                    continue;
                }
                let target = hard_ray_target(probe, light);
                let (start, end) = (probe.as_dvec3(), target.as_dvec3());
                let mut any = false;
                for (mask, candidate) in masks.iter_mut().zip(candidates) {
                    let analysed = &poses.analysed[candidate.mover];
                    *mask = 0;
                    for terminus in Terminus::BOTH {
                        let gated = match candidate.inside[terminus.index()] {
                            None => false,
                            Some(true) => true,
                            Some(false) => analysed.pose(terminus).segment_touches(start, end),
                        };
                        if gated {
                            *mask |= terminus.bit();
                        }
                    }
                    any |= *mask != 0;
                }
                if !any
                    || !segment_clear(scene.bvh, scene.primitives, scene.geometry, probe, target)
                {
                    continue;
                }
                for (tally, &mask) in tallies.iter_mut().zip(&masks) {
                    for terminus in Terminus::BOTH {
                        if mask & terminus.bit() != 0 {
                            let k = terminus.index();
                            tally.gated[k] += 1;
                            tally.centres[k]
                                .get_or_insert_with(Aabb::empty)
                                .expand_point(layout.probe_positions[index]);
                        }
                    }
                    if mask == Terminus::First.bit() | Terminus::Last.bit() {
                        tally.both += 1;
                    }
                }
            }
        }
    }
    tallies.retain(|tally| tally.gated.iter().any(|&count| count > 0));
    tallies
}

/// Grid index ranges, per axis, of the probes inside the box around the
/// light's falloff sphere; the whole grid for a sun.
fn influence_ranges(light: &MapLight, layout: &ProbeGridLayout) -> [Range<usize>; 3] {
    let whole = layout.dims.map(|count| 0..count as usize);
    match light.light_type {
        LightType::Directional => whole,
        LightType::Point | LightType::Spot => {
            let reach = f64::from(light.falloff_range.max(1.0e-4));
            std::array::from_fn(|axis| {
                let spacing = f64::from(layout.cell_size[axis]);
                let from = layout.world_min[axis];
                let low = ((light.origin[axis] - reach - from) / spacing).floor();
                let high = ((light.origin[axis] + reach - from) / spacing).ceil();
                let last = whole[axis].end as f64 - 1.0;
                if low.is_nan() || high.is_nan() || low > last || high < 0.0 {
                    return 0..0;
                }
                low.max(0.0) as usize..high.min(last) as usize + 1
            })
        }
    }
}
