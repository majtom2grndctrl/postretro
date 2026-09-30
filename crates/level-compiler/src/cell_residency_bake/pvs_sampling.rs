//! Sampled potentially-visible sets per camera cell.
//!
//! The CellResidencySet bake's visible-set input; the test-only
//! `lightmap_residency_dry_run` measures the same sets.
//!
//! The engine bakes no PVS (id 14 is retired): the runtime walks portals from
//! the eye every frame. This estimates a cell's PVS by running that same walk
//! (`postretro_visibility::determine_visible_cells`) from a lattice of eye
//! points inside the cell, six cube faces per point. A sample can only miss
//! cells, never invent them, so every set is a lower bound on what the cell
//! can see.
//!
//! The bound is per cell volume, a free-fly camera anywhere in the cell, not
//! a standing player's eye: the lattice spans 10–90% of the AABB on every
//! axis, ceiling bands included. It holds for any runtime FOV up to the
//! maximum, since the cube faces tile every view direction. A cell with no
//! accepted eye point keeps the set {cell} alone, a much weaker bound.

use glam::{Mat4, Vec3};
use postretro_level_loader::LevelWorld;
use postretro_visibility::{
    TimingGate, VisibilityPath, VisibleCells, determine_visible_cells, portal_traverse,
};
use rayon::prelude::*;

use crate::bake_control::BakeControl;

/// Eye-point lattice per axis, as fractions of the cell AABB. Inset from the
/// faces so a point on a shared boundary doesn't locate into the neighbour.
pub(crate) const LATTICE_FRACTIONS: [f32; 3] = [0.1, 0.5, 0.9];
pub(crate) const LATTICE_STEPS: usize = LATTICE_FRACTIONS.len();
/// Lattice points per cell: every combination of `LATTICE_FRACTIONS`.
pub(crate) const LATTICE_POINTS: usize = LATTICE_STEPS * LATTICE_STEPS * LATTICE_STEPS;
/// A lattice point outside its cell (cells are convex, their AABBs are not
/// the cell) retries at these fractions of its offset from the centroid.
const INSET_SCALES: [f32; 2] = [0.5, 0.25];
/// 90° per cube face plus 2° either side, so cells on a face seam are inside
/// both neighbouring frusta rather than clipped by both.
pub(crate) const CUBE_FACE_FOV_DEGREES: f32 = 94.0;
/// Must match `MAX_FOV_DEG` in `crates/postretro/src/camera.rs`; the test-only
/// dry-run report states it as the widest camera the bound covers.
#[cfg(test)]
pub(crate) const RUNTIME_MAX_FOV_DEGREES: f32 = 130.0;
/// Must match `NEAR` / `FAR` in `crates/postretro/src/camera.rs`: the engine
/// has no draw distance past the far plane.
const NEAR: f32 = 0.1;
const FAR: f32 = 4096.0;

/// View direction and up vector of each cube face; +Y is world up.
const CUBE_FACES: [(Vec3, Vec3); 6] = [
    (Vec3::X, Vec3::Y),
    (Vec3::NEG_X, Vec3::Y),
    (Vec3::Z, Vec3::Y),
    (Vec3::NEG_Z, Vec3::Y),
    (Vec3::Y, Vec3::Z),
    (Vec3::NEG_Y, Vec3::Z),
];

/// Which lattice subset a PVS unions over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SampleDensity {
    /// Centroid plus the 8 inset corners.
    Sparse,
    /// The full inset lattice, a superset of `Sparse`. Only tests name it; the
    /// bake reads `SampledPvs::dense` directly.
    #[cfg_attr(not(test), allow(dead_code))]
    Dense,
}

impl SampleDensity {
    fn includes(self, lattice: [usize; 3]) -> bool {
        let centre = LATTICE_STEPS / 2;
        match self {
            SampleDensity::Dense => true,
            SampleDensity::Sparse => {
                lattice == [centre; 3]
                    || lattice
                        .iter()
                        .all(|&axis| axis == 0 || axis == LATTICE_STEPS - 1)
            }
        }
    }
}

/// Where eye points landed and how the walks from them ended.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct SamplingStats {
    pub candidates: usize,
    /// Lattice points the cell locator placed in their own cell.
    pub in_cell: usize,
    /// Lattice points that landed after moving toward the centroid.
    pub inset: usize,
    /// Lattice points no inset brought into the cell; never walked.
    pub rejected: usize,
    /// Where each lattice point that missed its cell first located.
    pub missed_into_solid: usize,
    pub missed_into_exterior: usize,
    pub missed_into_other_cell: usize,
    /// Camera cells with no accepted eye point: their PVS is the cell alone.
    pub cells_without_eye: usize,
    pub walks: usize,
    /// Walks that exceeded `MAX_PORTAL_WALK_STEPS`. The runtime would draw a
    /// frustum-culled superset; the sample keeps the truncated walk instead.
    pub step_limit_walks: usize,
    /// Walks the runtime answered with any other frustum-all fallback
    /// (solid, exterior, no portals). Their cells are not added.
    pub frustum_all_walks: usize,
}

impl SamplingStats {
    fn add(&mut self, other: &SamplingStats) {
        self.candidates += other.candidates;
        self.in_cell += other.in_cell;
        self.inset += other.inset;
        self.rejected += other.rejected;
        self.missed_into_solid += other.missed_into_solid;
        self.missed_into_exterior += other.missed_into_exterior;
        self.missed_into_other_cell += other.missed_into_other_cell;
        self.cells_without_eye += other.cells_without_eye;
        self.walks += other.walks;
        self.step_limit_walks += other.step_limit_walks;
        self.frustum_all_walks += other.frustum_all_walks;
    }
}

/// Sampled PVS of every camera cell at both densities.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SampledPvs {
    /// Indexed by cell id; ascending drawable cells plus the cell itself.
    /// Empty for cells that are not camera cells. Only tests read the sparse
    /// sets (the dry run's density convergence check); the bake reads `dense`.
    #[cfg_attr(not(test), allow(dead_code))]
    pub sparse: Vec<Vec<u32>>,
    pub dense: Vec<Vec<u32>>,
    pub stats: SamplingStats,
}

/// Sample every camera cell in parallel, one governor permit and one progress
/// unit per camera cell.
pub(crate) fn sample_pvs(
    world: &LevelWorld,
    camera_cells: &[u32],
    control: &BakeControl,
) -> SampledPvs {
    let samples: Vec<CellSample> = camera_cells
        .par_iter()
        .map_init(Vec::new, |scratch, &cell| {
            let _permit = control.governor().enter();
            let sample = sample_cell(world, cell, scratch);
            control.advance(1);
            sample
        })
        .collect();
    let mut result = SampledPvs {
        sparse: vec![Vec::new(); world.cell_count()],
        dense: vec![Vec::new(); world.cell_count()],
        stats: SamplingStats::default(),
    };
    for (&cell, sample) in camera_cells.iter().zip(samples) {
        result.stats.add(&sample.stats);
        result.sparse[cell as usize] = sample.sparse;
        result.dense[cell as usize] = sample.dense;
    }
    result
}

struct CellSample {
    sparse: Vec<u32>,
    dense: Vec<u32>,
    stats: SamplingStats,
}

fn sample_cell(world: &LevelWorld, cell: u32, scratch: &mut Vec<u32>) -> CellSample {
    let mut stats = SamplingStats::default();
    let mut sparse = vec![false; world.cell_count()];
    let mut dense = vec![false; world.cell_count()];
    let mut point_visible = vec![false; world.cell_count()];
    let mut walked_any = false;
    for (lattice, eye) in eye_points(world, cell, &mut stats) {
        walked_any = true;
        point_visible.fill(false);
        walk_cube_faces(world, cell, eye, scratch, &mut point_visible, &mut stats);
        for (index, &seen) in point_visible.iter().enumerate() {
            if seen {
                dense[index] = true;
                if SampleDensity::Sparse.includes(lattice) {
                    sparse[index] = true;
                }
            }
        }
    }
    if !walked_any {
        stats.cells_without_eye = 1;
    }
    let collect = |marks: &mut [bool]| {
        marks[cell as usize] = true;
        marks
            .iter()
            .enumerate()
            .filter(|&(_, &seen)| seen)
            .map(|(index, _)| index as u32)
            .collect()
    };
    CellSample {
        sparse: collect(&mut sparse),
        dense: collect(&mut dense),
        stats,
    }
}

/// Accepted eye points of `cell` with their lattice coordinates. Every point
/// returned locates to `cell`; the rest are counted and dropped.
pub(crate) fn eye_points(
    world: &LevelWorld,
    cell: u32,
    stats: &mut SamplingStats,
) -> Vec<([usize; 3], Vec3)> {
    let info = &world.cells[cell as usize];
    let (min, max) = (info.bounds_min, info.bounds_max);
    let centroid = (min + max) * 0.5;
    let in_cell = |point: Vec3| world.locate_cell(point) == cell as usize;
    let steps = || LATTICE_FRACTIONS.iter().copied().enumerate();
    let lattice = steps().flat_map(|(ix, fx)| {
        steps().flat_map(move |(iy, fy)| {
            steps().map(move |(iz, fz)| ([ix, iy, iz], Vec3::new(fx, fy, fz)))
        })
    });
    let mut points = Vec::with_capacity(LATTICE_POINTS);
    for (index, fraction) in lattice {
        stats.candidates += 1;
        let point = min + (max - min) * fraction;
        if in_cell(point) {
            stats.in_cell += 1;
            points.push((index, point));
            continue;
        }
        let landed = &world.cells[world.locate_cell(point)];
        if landed.is_solid {
            stats.missed_into_solid += 1;
        } else if landed.is_exterior {
            stats.missed_into_exterior += 1;
        } else {
            stats.missed_into_other_cell += 1;
        }
        let inset = INSET_SCALES
            .iter()
            .map(|&scale| centroid + (point - centroid) * scale)
            .find(|&candidate| in_cell(candidate));
        match inset {
            Some(candidate) => {
                stats.inset += 1;
                points.push((index, candidate));
            }
            None => stats.rejected += 1,
        }
    }
    points
}

/// Mark every drawable cell the runtime walk reaches from `eye` across the six
/// cube faces.
fn walk_cube_faces(
    world: &LevelWorld,
    cell: u32,
    eye: Vec3,
    scratch: &mut Vec<u32>,
    visible: &mut [bool],
    stats: &mut SamplingStats,
) {
    let projection = Mat4::perspective_rh(CUBE_FACE_FOV_DEGREES.to_radians(), 1.0, NEAR, FAR);
    for (direction, up) in CUBE_FACES {
        stats.walks += 1;
        let view = Mat4::look_at_rh(eye, eye + direction, up);
        let (result, frustum) = determine_visible_cells(
            eye,
            projection * view,
            world,
            &[],
            false,
            scratch,
            TimingGate::OFF,
        );
        match result.stats.path {
            VisibilityPath::PrlPortal { .. } => {
                if let VisibleCells::Culled(cells) = result.visible_cells {
                    for &visible_cell in &cells {
                        visible[visible_cell as usize] = true;
                    }
                    // Hand the allocation back for the next walk.
                    *scratch = cells;
                }
            }
            VisibilityPath::PortalStepLimitFallback { .. } => {
                stats.step_limit_walks += 1;
                // The truncated walk: a subset of the full walk, so the PVS
                // stays a lower bound, unlike the runtime's frustum superset.
                let reached = portal_traverse(eye, cell as usize, &frustum, world, false);
                for (index, &seen) in reached.iter().enumerate() {
                    if seen && world.cells[index].is_drawable {
                        visible[index] = true;
                    }
                }
            }
            VisibilityPath::EmptyWorldFallback
            | VisibilityPath::SolidCellFallback
            | VisibilityPath::ExteriorCellFallback
            | VisibilityPath::NoPortalsFallback => stats.frustum_all_walks += 1,
        }
    }
}
