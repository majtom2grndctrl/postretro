// Joint soft visibility: one sample set traced against the static world alone
// and with a posed mover added, for the mover probe channel's direct half.
// See: context/lib/build_pipeline.md §Compiler pipeline step 12 (Mover probe channel)

use std::sync::Arc;

use glam::{DVec3, Vec3};

use super::posed_solid::PosedSolid;
use crate::lightmap_bake::{
    RAY_EPSILON, SOFT_PROBE_SAMPLES, SoftProbes, area_sample_target, emitter_radius,
    hard_ray_target,
};
use crate::map_data::{LightType, MapLight};
use crate::mover_light_gate::{BOX_SLACK_M, Pose};
use crate::partition::Aabb;
use crate::texel_footprint::ProbeEarlyOut;

/// Angular slack on the sun cone test, far above the `f32` rounding of a
/// jittered sample direction.
const SUN_CONE_SLACK_RAD: f64 = 1.0e-3;

/// What a valid probe inside a posed brush is tested against. Nothing lives
/// inside a brush, so such a probe feeds only that mover's own faces, which
/// sample with backface rejection off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InsideBrushRule {
    /// Without the brushes that contain the probe; the mover's other
    /// brushes still occlude.
    SkipContainingBrush,
    /// Every brush, so each segment starts in solid and the probe reads
    /// dark.
    Dark,
}

/// The rule the channel bakes take (owner, Stop 2): dark, so a valid probe
/// inside a posed brush stores q = 0 for that pose. Skipping the containing
/// brush leaks exterior direct light onto the mover's inner faces, since a
/// probe inside a wall serves both of its faces.
pub(crate) const INSIDE_BRUSH_RULE: InsideBrushRule = InsideBrushRule::Dark;

/// One mover posed at one terminus as an occluder of probe-point shadow rays.
#[derive(Clone)]
pub(crate) struct PosedOccluder {
    solid: Arc<PosedSolid>,
    rule: InsideBrushRule,
}

impl std::fmt::Debug for PosedOccluder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PosedOccluder")
            .field("pose", self.pose())
            .field("rule", &self.rule)
            .finish_non_exhaustive()
    }
}

impl PosedOccluder {
    pub(crate) fn new(solid: Arc<PosedSolid>, rule: InsideBrushRule) -> Self {
        Self { solid, rule }
    }

    pub(crate) fn pose(&self) -> &Pose {
        self.solid.pose()
    }

    /// The solid the shadow segments meet, for the memo key.
    pub(crate) fn solid(&self) -> &PosedSolid {
        &self.solid
    }

    /// Whether any sample segment from `probe` toward `light`'s emitter can
    /// meet the pose: every such segment lies in the box around the probe
    /// and the emitter sphere. A sun's samples leave the probe inside a cone
    /// about its direction, so they can meet the pose only where that cone
    /// meets the sphere around its bounds.
    pub(crate) fn may_meet(&self, probe: Vec3, light: &MapLight) -> bool {
        match light.light_type {
            LightType::Directional => self.sun_cone_may_meet(probe, light),
            LightType::Point | LightType::Spot => {
                let reach = DVec3::splat(f64::from(emitter_radius(light).max(0.0)));
                let probe = probe.as_dvec3();
                let span = Aabb {
                    min: probe.min(light.origin - reach),
                    max: probe.max(light.origin + reach),
                };
                crate::mover_light_gate::boxes_may_touch(&span, self.solid.bounds())
            }
        }
    }

    /// The cone test for a sun: every sample direction lies within half the
    /// angular diameter of the direction toward it, so a segment from the
    /// lifted probe can touch the pose only if that cone, widened by the
    /// angle the pose's bounding sphere subtends, holds the sphere's centre.
    fn sun_cone_may_meet(&self, probe: Vec3, light: &MapLight) -> bool {
        let aim = DVec3::from(
            light
                .cone_direction
                .unwrap_or([0.0, -1.0, 0.0])
                .map(f64::from),
        );
        let to_sun = (-aim).normalize_or_zero();
        if to_sun == DVec3::ZERO {
            return true;
        }
        let bounds = self.solid.bounds();
        // The lifts move a segment's start, and τ reaches past the posed
        // faces; the slack covers both.
        let radius =
            (bounds.max - bounds.min).length() * 0.5 + BOX_SLACK_M + f64::from(RAY_EPSILON);
        let towards = bounds.centroid() - probe.as_dvec3();
        let distance = towards.length();
        if distance <= radius {
            return true;
        }
        let half_angle = f64::from(light.angular_diameter.max(0.0)).to_radians() * 0.5;
        let angle = (towards.dot(to_sun) / distance).clamp(-1.0, 1.0).acos();
        angle <= half_angle + (radius / distance).asin() + SUN_CONE_SLACK_RAD
    }

    /// Whether the sample segment `from → to`, traced for a receiver at
    /// `probe`, meets the pose: as the static tracer meets the same brushes
    /// placed statically ([`PosedSolid::segment_blocked`]). Under the skip
    /// rule the brushes holding `probe` are left out.
    pub(crate) fn blocks(&self, probe: Vec3, from: Vec3, to: Vec3) -> bool {
        match self.rule {
            InsideBrushRule::Dark => self.solid.segment_blocked(from, to, |_| false),
            InsideBrushRule::SkipContainingBrush => {
                let containing = self.solid.containing(probe);
                self.solid
                    .segment_blocked(from, to, |brush| containing.contains(&brush))
            }
        }
    }
}

/// One sample set's clear counts under both traces. A posed-clear sample is
/// always static-clear: the posed trace is the static one with brushes added.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct JointVisibility {
    pub(crate) samples: u32,
    pub(crate) static_clear: u32,
    pub(crate) posed_clear: u32,
}

impl JointVisibility {
    /// q: the share of the static-clear samples that stay clear with the
    /// mover posed. The denominator is the joint set's static count, so
    /// static blocking never enters q. With no static-clear sample the
    /// light's baked term is already zero and q is 1.
    pub(crate) fn surviving_fraction(self) -> f32 {
        if self.static_clear == 0 {
            1.0
        } else {
            self.posed_clear as f32 / self.static_clear as f32
        }
    }

    /// q as the section stores it: one byte, q = byte / 255.
    pub(crate) fn surviving_byte(self) -> u8 {
        (self.surviving_fraction() * 255.0).round() as u8
    }

    /// Whether the posed mover blocks a sample the static world leaves
    /// clear. A light whose samples never meet it stores no entry.
    pub(crate) fn meets_mover(self) -> bool {
        self.posed_clear != self.static_clear
    }
}

/// [`crate::lightmap_bake::soft_visibility_early_out`] traced twice over one
/// sample set: `static_clear(from, to)` alone, and with `posed_blocks(from,
/// to)` added. Same origin lift, lattice, seed and escalation count. The
/// probe set ends the trace only when both traces end there under
/// `early_out`; otherwise both escalate to the full set together. Deciding
/// separately would let the static trace stop at four clear probes while the
/// posed trace escalates and meets static blocking the static count skipped.
// The soft-visibility inputs plus the second trace.
#[allow(clippy::too_many_arguments)]
pub(crate) fn joint_soft_visibility(
    surface_point: Vec3,
    surface_normal: Vec3,
    light: &MapLight,
    seed: u64,
    full_samples: u32,
    early_out: ProbeEarlyOut,
    static_clear: impl Fn(Vec3, Vec3) -> bool,
    posed_blocks: impl Fn(Vec3, Vec3) -> bool,
) -> JointVisibility {
    let origin = surface_point + surface_normal * RAY_EPSILON;
    let mut tally = JointVisibility::default();
    let sample = |tally: &mut JointVisibility, target: Vec3| {
        tally.samples += 1;
        if static_clear(origin, target) {
            tally.static_clear += 1;
            if !posed_blocks(origin, target) {
                tally.posed_clear += 1;
            }
        }
    };

    if emitter_radius(light) <= 0.0 {
        sample(&mut tally, hard_ray_target(surface_point, light));
        return tally;
    }

    let probes = SoftProbes::new(light, full_samples);
    let full_samples = probes.full_samples();
    for &i in probes.indices() {
        sample(
            &mut tally,
            area_sample_target(surface_point, light, seed, i, full_samples),
        );
    }
    if early_out.ends(tally.static_clear, SOFT_PROBE_SAMPLES)
        && early_out.ends(tally.posed_clear, SOFT_PROBE_SAMPLES)
    {
        return tally;
    }
    for i in (0..full_samples).filter(|i| !probes.indices().contains(i)) {
        sample(
            &mut tally,
            area_sample_target(surface_point, light, seed, i, full_samples),
        );
    }
    tally
}
