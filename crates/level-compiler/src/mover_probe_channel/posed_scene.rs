// One mover posed at one terminus for the SH base's rays: its solid (the
// static tracer's own hit test over its sides, its brushes' half-spaces for
// origins inside them) and patches on its sides.
// See: context/lib/build_pipeline.md §Compiler pipeline step 12 (Mover probe channel)

use std::sync::Arc;

use glam::{DVec3, Vec3};

use super::InsideBrushRule;
use super::posed_solid::{PosedHit, PosedSolid};
use crate::geometry::GeometryResult;
use crate::lightmap_bake::{RAY_EPSILON, emitter_radius, hard_ray_target};
use crate::map_data::{LightType, MapLight};
use crate::mover_light_gate::{AnalysedMover, BOX_SLACK_M, Terminus, segment_may_touch_box};
use crate::partition::Aabb;
use crate::surface_radiance::{PatchCountExceeded, PatchSet};

/// Whether posed sides become patches that reflect their albedo, or read
/// black: the owner's lean (patches) and its A/B alternative.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PosedSurfaces {
    Patches,
    Black,
}

/// The posed mover for the SH base: its solid, which every posed ray and
/// shadow segment meets ([`PosedSolid`]), and a patch set over its sides.
pub(crate) struct PosedScene {
    pub(crate) mover_id: u32,
    pub(crate) terminus: Terminus,
    pub(crate) label: String,
    pub(crate) rule: InsideBrushRule,
    pub(crate) surfaces: PosedSurfaces,
    solid: Arc<PosedSolid>,
    pub(crate) patches: PatchSet,
}

impl PosedScene {
    /// `analysed` posed at `terminus` as `solid`, with patches over its
    /// sides.
    pub(crate) fn new(
        solid: Arc<PosedSolid>,
        analysed: &AnalysedMover,
        terminus: Terminus,
        rule: InsideBrushRule,
        surfaces: PosedSurfaces,
    ) -> Result<Self, PatchCountExceeded> {
        let patches = PatchSet::try_build(solid.geometry())?;
        Ok(Self {
            mover_id: analysed.mover_id,
            terminus,
            label: analysed.label.clone(),
            rule,
            surfaces,
            solid,
            patches,
        })
    }

    /// The posed sides as geometry, which the patches cover.
    pub(crate) fn geometry(&self) -> &GeometryResult {
        self.solid.geometry()
    }

    /// Distance from `point` to the pose's bounds, zero inside them.
    pub(crate) fn distance_to_bounds(&self, point: Vec3) -> f64 {
        self.solid.distance_to_bounds(point)
    }

    /// Whether the shadow segment touches a posed brush: the posed half of
    /// a patch's shadow ray, met as the static tracer meets a static brush
    /// ([`PosedSolid::segment_blocked`]). A patch buried in a posed brush
    /// starts in solid and goes dark.
    pub(crate) fn segment_blocked(&self, from: Vec3, to: Vec3) -> bool {
        self.solid.segment_blocked(from, to, |_| false)
    }

    /// The posed brushes `point` lies in or on, by index.
    pub(crate) fn containing(&self, point: Vec3) -> Vec<usize> {
        self.solid.containing(point)
    }

    /// Whether any soft-visibility sample segment from `point` (lifted
    /// along its normal by at most [`RAY_EPSILON`]) toward `light` can meet
    /// the pose. Every target lies within a ball about one centre: the
    /// emitter sphere of a point or spot, the cone's far cap of a sun; so
    /// every segment lies within that radius of the centre segment.
    pub(crate) fn may_shadow(&self, point: Vec3, light: &MapLight) -> bool {
        let radius = f64::from(emitter_radius(light).max(0.0));
        let (centre, reach) = match light.light_type {
            LightType::Point | LightType::Spot => (light.origin, radius),
            LightType::Directional => {
                let centre = hard_ray_target(point, light).as_dvec3();
                let length = centre.distance(point.as_dvec3());
                // A chord of the cone's cap is at most its arc.
                (centre, length * (radius.to_radians() * 0.5) * 1.01 + 1.0e-3)
            }
        };
        let bounds = self.solid.bounds();
        let grow = DVec3::splat(reach + f64::from(RAY_EPSILON));
        let grown = Aabb {
            min: bounds.min - grow,
            max: bounds.max + grow,
        };
        segment_may_touch_box(point.as_dvec3(), centre, &grown)
    }

    /// Whether `light` can shade anything whose shadow segments meet the
    /// pose: a point or spot whose range ball (or emitter sphere) misses
    /// the pose's bounds cannot, since every segment it shades along lies
    /// inside that ball.
    pub(crate) fn light_may_reach(&self, light: &MapLight) -> bool {
        match light.light_type {
            LightType::Directional => true,
            LightType::Point | LightType::Spot => {
                let reach = f64::from(light.falloff_range.max(emitter_radius(light)).max(0.0))
                    + f64::from(RAY_EPSILON)
                    + BOX_SLACK_M;
                let origin = light.origin;
                let bounds = self.solid.bounds();
                !origin.is_finite()
                    || origin.distance(origin.clamp(bounds.min, bounds.max)) <= reach
            }
        }
    }

    /// Whether the unbounded ray from `origin` along `dir` can come near
    /// the pose's bounds: a gather ray that cannot never meets the pose.
    pub(crate) fn may_meet_ray(&self, origin: Vec3, dir: Vec3) -> bool {
        self.solid.may_meet_ray(origin, dir)
    }

    /// The pose's nearest hit along `dir` from `origin`, nearer than
    /// `nearest` ([`PosedSolid::nearest_hit`]).
    pub(crate) fn trace(&self, origin: Vec3, dir: Vec3, nearest: f32) -> Option<PosedHit> {
        self.solid.nearest_hit(origin, dir, nearest)
    }

    /// Fold what the pose bakes from: its solid (the inside test, the
    /// posed trace and the shadow segments) and each side's texture index
    /// (the patches' albedo lookup).
    pub(crate) fn fold_key(&self, hasher: &mut blake3::Hasher) {
        self.solid.fold_key(hasher);
        let faces = &self.solid.geometry().geometry.faces;
        hasher.update(&(faces.len() as u64).to_le_bytes());
        for face in faces {
            hasher.update(&face.texture_index.to_le_bytes());
        }
    }

    /// Heap bytes the scene holds: its solid and patch set.
    pub(crate) fn heap_bytes(&self) -> u64 {
        self.solid.heap_bytes() + self.patches.heap_bytes()
    }
}
