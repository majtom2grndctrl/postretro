// Planar plane selection: the seed-in-reach filter, scoring and hysteresis, and the mirror walk's seed cells.
// See: context/lib/rendering_pipeline.md §7.3a

use glam::{Mat4, Vec3};
use postretro_level_loader::{CellData, LevelWorld};

use super::{
    PLANE_SELECTION_HYSTERESIS, PlanarCandidate, PlanarCandidates, aabb_in_planes, view_proj_planes,
};
use crate::fluid_surface::fog::FLUID_FOG_BAND;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SelectedPlane {
    pub candidate_index: usize,
    pub h: f32,
    pub bounds_min: Vec3,
    pub bounds_max: Vec3,
    pub score: f32,
}

/// This frame's plane (`rendering_pipeline.md` §7.3a Selection), by plane
/// identity, from world geometry and the eye position; the main frustum
/// only decides which faces count. In order:
/// - **Filters, before scoring.** A plane competes only when `seeded` marks
///   it (a seed cell in this frame's camera reach, from
///   [`mark_seeded_planes`]; a missing entry reads as unseeded) and the eye
///   sits more than `FLUID_FOG_BAND` above it. The area floor applied at
///   install. A filtered plane scores zero.
/// - **Score.** Estimated projected solid angle of its faces whose bounds
///   pass the main frustum (the same test the crop takes its faces by);
///   ties keep the lower candidate index.
/// - **Hysteresis.** The held plane (`previous`) keeps selection while it
///   scores above zero and no challenger beats it by more than
///   [`PLANE_SELECTION_HYSTERESIS`] ([`challenger_switches`]). A held plane
///   scoring zero (filtered, or out of the frustum) yields that frame.
pub fn select_plane(
    candidates: &PlanarCandidates,
    eye: Vec3,
    main_vp: Mat4,
    seeded: &[bool],
    previous: Option<SelectedPlane>,
) -> Option<SelectedPlane> {
    let main = view_proj_planes(main_vp);
    let score = |index: usize, plane: &PlanarCandidate| -> f32 {
        if !seeded.get(index).copied().unwrap_or(false) || eye.y - plane.h <= FLUID_FOG_BAND {
            return 0.0;
        }
        plane
            .faces
            .iter()
            .filter(|face| aabb_in_planes(face.bounds_min, face.bounds_max, &main))
            .map(|face| {
                let delta = eye - face.centroid;
                face.area * delta.y.abs() / delta.length_squared().max(0.0001).powf(1.5)
            })
            .sum()
    };
    let held_index = previous.map(|previous| previous.candidate_index);
    let mut best: Option<SelectedPlane> = None;
    let mut held: Option<SelectedPlane> = None;
    for (candidate_index, plane) in candidates.planes.iter().enumerate() {
        let score = score(candidate_index, plane);
        let entry = SelectedPlane {
            candidate_index,
            h: plane.h,
            bounds_min: plane.bounds_min,
            bounds_max: plane.bounds_max,
            score,
        };
        if held_index == Some(candidate_index) {
            held = Some(entry);
        }
        if score > 0.0 && best.is_none_or(|best| score > best.score) {
            best = Some(entry);
        }
    }
    match (held, best) {
        (Some(held), Some(best))
            if held.score > 0.0 && !challenger_switches(held.score, best.score) =>
        {
            Some(held)
        }
        _ => best,
    }
}

/// Whether a challenger scoring `challenger` takes selection from a held
/// plane scoring `held` above zero: only above [`PLANE_SELECTION_HYSTERESIS`]
/// times the held score. At or below it, the held plane stays.
pub fn challenger_switches(held: f32, challenger: f32) -> bool {
    challenger > held * PLANE_SELECTION_HYSTERESIS
}

/// Mark, one flag per candidate in `seeded` (reused across frames), each
/// plane with a seed cell in `reach`: this frame's camera reach, the walk's
/// fog reach on a walk path and the drawn set on a fallback. A seed cell is
/// one [`collect_mirror_seed_cells`] takes for that plane, so a selected
/// plane always seeds its mirror walk. Stops once every plane is marked.
pub fn mark_seeded_planes(
    candidates: &PlanarCandidates,
    reach: &[u32],
    world: &LevelWorld,
    eye: Vec3,
    r: Option<f32>,
    seeded: &mut Vec<bool>,
) {
    seeded.clear();
    seeded.resize(candidates.planes.len(), false);
    let mut unmarked = candidates.planes.len();
    for &id in reach {
        if unmarked == 0 {
            return;
        }
        let Some(cell) = world.cells.get(id as usize) else {
            continue;
        };
        for (plane, flag) in candidates.planes.iter().zip(seeded.iter_mut()) {
            if !*flag && seeds_plane(cell, plane, eye, r) {
                *flag = true;
                unmarked -= 1;
            }
        }
    }
}

/// The cells of `main_cells` that seed the mirror walk of `plane` (the
/// selected candidate): main-visible water cells, whose mirror apex may lie
/// in solid. `main_cells` may include empty reachable cells and is
/// ascending, as both camera reach sets are, so `output` is too. Output
/// storage is reused.
pub fn collect_mirror_seed_cells(
    main_cells: &[u32],
    world: &LevelWorld,
    plane: &PlanarCandidate,
    eye: Vec3,
    r: Option<f32>,
    output: &mut Vec<u32>,
) {
    debug_assert!(
        main_cells.windows(2).all(|pair| pair[0] < pair[1]),
        "the camera reach is ascending"
    );
    output.clear();
    output.extend(main_cells.iter().copied().filter(|&id| {
        world
            .cells
            .get(id as usize)
            .is_some_and(|cell| seeds_plane(cell, plane, eye, r))
    }));
}

/// Whether `cell` seeds the mirror of `plane`: non-solid, interior,
/// straddling the water within the fog band, and overlapping one of the
/// plane's faces in x and z, with, under a receiver reach `r`, that face's
/// water inside the cell within `r` of the eye. Faces, not the plane's union
/// bounds: a dry cell inside an L-shaped lake's bounding box holds none of
/// its water, and seeding it would let a lake behind a wall win selection
/// over a visible pool.
fn seeds_plane(cell: &CellData, plane: &PlanarCandidate, eye: Vec3, r: Option<f32>) -> bool {
    let overlaps_xz = |min: Vec3, max: Vec3| {
        cell.bounds_max.x >= min.x
            && cell.bounds_min.x <= max.x
            && cell.bounds_max.z >= min.z
            && cell.bounds_min.z <= max.z
    };
    if cell.is_solid
        || cell.is_exterior
        || cell.bounds_max.y < plane.h - FLUID_FOG_BAND
        || cell.bounds_min.y > plane.h + FLUID_FOG_BAND
        || !overlaps_xz(plane.bounds_min, plane.bounds_max)
    {
        return false;
    }
    plane.faces.iter().any(|face| {
        if !overlaps_xz(face.bounds_min, face.bounds_max) {
            return false;
        }
        let Some(r) = r else {
            return true;
        };
        let water_min = Vec3::new(
            cell.bounds_min.x.max(face.bounds_min.x),
            plane.h,
            cell.bounds_min.z.max(face.bounds_min.z),
        );
        let water_max = Vec3::new(
            cell.bounds_max.x.min(face.bounds_max.x),
            plane.h,
            cell.bounds_max.z.min(face.bounds_max.z),
        );
        eye.distance_squared(eye.clamp(water_min, water_max)) <= r * r
    })
}
