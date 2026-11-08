// Which walkable spans lie under each posed mover's solid: the region split input.
// See: context/lib/build_pipeline.md §Navigation bake

use std::collections::BTreeMap;

use glam::DVec3;

use super::WalkGrid;
use super::mover_poses::NavPose;
use crate::map_data::NavParams;
use crate::mover_pose::PosedBrush;

/// Slack on the overlap test, so a door face lying on a cell edge claims only
/// the cell it stands in.
const OVERLAP_EPS_M: f64 = 1.0e-4;

/// Per walkable span, the poses whose solid crosses its agent band. Region growth
/// keeps spans with different pose lists apart, which puts a posed mover's
/// footprint edge on a region boundary at cell granularity. Spans under no
/// pose are absent, so a map with no poses grows exactly as before.
#[derive(Default)]
pub(super) struct MoverFootprints {
    /// Keyed by column and floor bit pattern; pose indices ascending.
    spans: BTreeMap<(u32, u32, u32), Vec<u32>>,
}

impl MoverFootprints {
    pub(super) fn of_poses(grid: &WalkGrid, poses: &[NavPose], params: &NavParams) -> Self {
        let mut footprints = Self::default();
        let origin = grid.origin.as_dvec3();
        let cell = f64::from(grid.cell_size);
        let step = f64::from(params.step_height);
        let height = f64::from(params.agent_height);

        for (pose_index, pose) in poses.iter().enumerate() {
            let pose_index = pose_index as u32;
            for brush in &pose.brushes {
                let bounds = &brush.posed.bounds;
                let (x0, x1) = column_span(
                    bounds.min.x - origin.x,
                    bounds.max.x - origin.x,
                    cell,
                    grid.dim_x,
                );
                let (z0, z1) = column_span(
                    bounds.min.z - origin.z,
                    bounds.max.z - origin.z,
                    cell,
                    grid.dim_z,
                );
                for z in z0..z1 {
                    for x in x0..x1 {
                        let cell_x = origin.x + f64::from(x) * cell;
                        let cell_z = origin.z + f64::from(z) * cell;
                        for &floor in grid.heights_at(x, z) {
                            let floor_y = f64::from(floor);
                            let min = DVec3::new(cell_x, floor_y + step, cell_z);
                            let max = DVec3::new(cell_x + cell, floor_y + height, cell_z + cell);
                            if brush_crosses_box(&brush.posed, min, max) {
                                footprints.record(x, z, floor, pose_index);
                            }
                        }
                    }
                }
            }
        }
        footprints
    }

    /// Note a pose over a span. A pose's brushes arrive together and poses in
    /// ascending order, so a repeat can only be the last entry.
    fn record(&mut self, x: u32, z: u32, floor_y: f32, pose_index: u32) {
        let poses = self.spans.entry((x, z, floor_y.to_bits())).or_default();
        if poses.last() != Some(&pose_index) {
            poses.push(pose_index);
        }
    }

    /// The poses over the span at column `(x, z)`, floor `floor_y`.
    pub(super) fn poses_at(&self, x: u32, z: u32, floor_y: f32) -> &[u32] {
        self.spans
            .get(&(x, z, floor_y.to_bits()))
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }
}

/// The columns `[first, end)` a world extent offset from the grid origin touches.
fn column_span(min_offset: f64, max_offset: f64, cell: f64, dim: u32) -> (u32, u32) {
    let limit = f64::from(dim);
    let first = (min_offset / cell).floor().clamp(0.0, limit) as u32;
    let end = (max_offset / cell).ceil().clamp(0.0, limit) as u32;
    (first, end)
}

/// Whether the box `min..max` reaches into `brush`: bounds overlap and no brush
/// plane has the whole box in front of it. Conservative at oblique corners, where
/// it can claim an extra cell; that costs a region, never a wrong gate, since the
/// gate pass tests the portal itself.
fn brush_crosses_box(brush: &PosedBrush, min: DVec3, max: DVec3) -> bool {
    let bounds = &brush.bounds;
    let apart =
        bounds.max.cmple(min + OVERLAP_EPS_M).any() || bounds.min.cmpge(max - OVERLAP_EPS_M).any();
    if apart {
        return false;
    }
    !brush.planes.iter().any(|&(normal, distance)| {
        let deepest = DVec3::new(
            if normal.x >= 0.0 { min.x } else { max.x },
            if normal.y >= 0.0 { min.y } else { max.y },
            if normal.z >= 0.0 { min.z } else { max.z },
        );
        normal.dot(deepest) - distance >= -OVERLAP_EPS_M
    })
}
