// Exact point gravity/push resolution over a load-time acceleration index.
// See: context/lib/entity_model.md §7 (Environment Volumes) and §8.

use glam::Vec3;

use crate::bvh::TreeEntry;
use crate::leaf::{ContentsMask, ConvexLeaf};
use crate::point_grid::{PointGrid, valid_bounds};
use crate::volumes::EnvironmentVolume;

#[derive(Debug, Clone, Copy, PartialEq)]
struct AccelerationBrush {
    volume: usize,
    brush: usize,
    rank: usize,
    contents: ContentsMask,
    gravity: Option<f32>,
    push: Option<Vec3>,
    force: f32,
    bounds_prove_containment: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct AccelerationIndex {
    grid: PointGrid,
    brushes: Vec<AccelerationBrush>,
}

impl AccelerationIndex {
    pub(crate) const fn empty() -> Self {
        Self {
            grid: PointGrid::empty(),
            brushes: Vec::new(),
        }
    }

    pub(crate) fn build(volumes: &[EnvironmentVolume], ranks: &[usize]) -> Self {
        let mut entries = Vec::new();
        let mut brushes = Vec::new();
        for (volume_index, volume) in volumes.iter().enumerate() {
            let contents = volume.contents();
            if !contents.intersects(ContentsMask::GRAVITY | ContentsMask::PUSH) {
                continue;
            }
            for (brush_index, brush) in volume.brushes.iter().enumerate() {
                let (min, max, bounds_prove_containment) = acceleration_bounds(brush);
                let payload = u32::try_from(brushes.len())
                    .expect("environment acceleration brush count exceeds u32 indexing");
                entries.push(TreeEntry { min, max, payload });
                brushes.push(AccelerationBrush {
                    volume: volume_index,
                    brush: brush_index,
                    rank: ranks[volume_index],
                    contents,
                    gravity: volume.gravity,
                    push: volume.push,
                    force: volume.force,
                    bounds_prove_containment,
                });
            }
        }
        Self {
            grid: PointGrid::build(entries),
            brushes,
        }
    }

    /// Returns `(gravity, nominal push, the push winner's force — 1 with no push)`.
    pub(crate) fn resolve(
        &self,
        volumes: &[EnvironmentVolume],
        point: Vec3,
        level_gravity: f32,
    ) -> (f32, Vec3, f32) {
        let (mut gravity, mut push, mut force) = (level_gravity, Vec3::ZERO, 1.0);
        let (mut gravity_rank, mut push_rank) = (usize::MAX, usize::MAX);
        self.grid.visit_point(point, |index| {
            let brush = &self.brushes[*index as usize];
            let gravity_better = brush.gravity.is_some() && brush.rank < gravity_rank;
            let push_better = brush.push.is_some() && brush.rank < push_rank;
            if !(gravity_better || push_better) || !contains(volumes, brush, point) {
                return;
            }
            if gravity_better {
                gravity = brush.gravity.expect("gravity candidate carries gravity");
                gravity_rank = brush.rank;
            }
            if push_better {
                push = brush.push.expect("push candidate carries push");
                force = brush.force;
                push_rank = brush.rank;
            }
        });
        (gravity, push, force)
    }

    pub(crate) fn visit_containing_point(
        &self,
        volumes: &[EnvironmentVolume],
        point: Vec3,
        mask: ContentsMask,
        mut visit: impl FnMut(usize),
    ) {
        self.grid.visit_point(point, |index| {
            let brush = &self.brushes[*index as usize];
            if brush.contents.intersects(mask) && contains(volumes, brush, point) {
                visit(brush.volume);
            }
        });
    }
}

fn contains(volumes: &[EnvironmentVolume], brush: &AccelerationBrush, point: Vec3) -> bool {
    brush.bounds_prove_containment
        || volumes[brush.volume].brushes[brush.brush].contains_point(point)
}

fn acceleration_bounds(brush: &ConvexLeaf) -> (Vec3, Vec3, bool) {
    let (min, max) = brush.padded_bounds();
    // Keep malformed supplied bounds verbatim: min/max on NaNs could otherwise
    // turn a previously rejected brush into a candidate.
    if valid_bounds(min, max)
        && let Some((plane_min, plane_max)) = axis_bounds(brush)
    {
        let tight_min = min.max(plane_min);
        let tight_max = max.min(plane_max);
        if valid_bounds(tight_min, tight_max) {
            return (tight_min, tight_max, true);
        }
    }
    (min, max, false)
}

/// Only exact signed unit axes admit replacing `dot(p,n)-d <= 0` with a bound
/// comparison for finite points. Oblique/scaled/nonfinite planes retain their
/// original arithmetic; missing sides and repeated parallel planes are valid.
fn axis_bounds(brush: &ConvexLeaf) -> Option<(Vec3, Vec3)> {
    let (mut min, mut max) = (Vec3::splat(f32::NEG_INFINITY), Vec3::splat(f32::INFINITY));
    for plane in &brush.planes {
        if !plane.distance.is_finite() {
            return None;
        }
        match plane.normal {
            Vec3 {
                x: 1.0,
                y: 0.0,
                z: 0.0,
            } => max.x = max.x.min(plane.distance),
            Vec3 {
                x: -1.0,
                y: 0.0,
                z: 0.0,
            } => min.x = min.x.max(-plane.distance),
            Vec3 {
                x: 0.0,
                y: 1.0,
                z: 0.0,
            } => max.y = max.y.min(plane.distance),
            Vec3 {
                x: 0.0,
                y: -1.0,
                z: 0.0,
            } => min.y = min.y.max(-plane.distance),
            Vec3 {
                x: 0.0,
                y: 0.0,
                z: 1.0,
            } => max.z = max.z.min(plane.distance),
            Vec3 {
                x: 0.0,
                y: 0.0,
                z: -1.0,
            } => min.z = min.z.max(-plane.distance),
            _ => return None,
        }
    }
    Some((min, max))
}

#[cfg(test)]
#[path = "acceleration_tests.rs"]
mod tests;
