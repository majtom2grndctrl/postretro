// Test-only exact oracle: the chain walk, bypass tightened, chains classified.
// See: context/lib/rendering_pipeline.md §2

use glam::Vec3;

use super::geometry::{distance_to_polygon, wholly_behind};
use super::{PORTAL_BYPASS_DISTANCE, WalkView, crosses_from_source_side};
use crate::portal_vis::{clip_polygon_to_frustum, narrow_frustum};
use crate::visibility::Frustum;
use postretro_level_loader::LevelWorld;

/// Far above the runtime cap: a camera that hits it is reported, not proven.
pub(crate) const ORACLE_STEP_LIMIT: u64 = 20_000_000;
const ORACLE_CHAIN_DEPTH: usize = 256;

/// What the exact walk reached. `clean[c]` marks a cell reached by at least
/// one chain that never crossed a portal from the side away from the eye and
/// whose final clipped polygon is not wholly outside the view frustum: the
/// cells the region walk must reach. The rest of `visible` are leaks of the
/// exact walk's float slop (`research.md` §Exact-walk floating-point leaks).
pub(crate) struct OracleWalk {
    pub visible: Vec<bool>,
    pub clean: Vec<bool>,
    pub considered: u64,
    pub step_limit_hit: bool,
    pub depth_limit_hit: bool,
}

impl OracleWalk {
    /// Proof holds only when the oracle finished its enumeration.
    pub(crate) fn complete(&self) -> bool {
        !self.step_limit_hit && !self.depth_limit_hit
    }
}

struct OracleState<'a> {
    world: &'a LevelWorld,
    blocked_portals: &'a [bool],
    view: &'a WalkView,
    result: OracleWalk,
    path: Vec<usize>,
    clip_a: Vec<Vec3>,
    clip_b: Vec<Vec3>,
}

pub(crate) fn oracle_from_camera(
    world: &LevelWorld,
    blocked_portals: &[bool],
    view: &WalkView,
    camera_cell: usize,
) -> OracleWalk {
    oracle_from_seeds(world, blocked_portals, view, &[camera_cell])
}

/// One flood per usable seed, unioned; the same seed skips as the walk.
pub(crate) fn oracle_from_seeds(
    world: &LevelWorld,
    blocked_portals: &[bool],
    view: &WalkView,
    seeds: &[usize],
) -> OracleWalk {
    let cell_count = world.cell_count();
    let mut state = OracleState {
        world,
        blocked_portals,
        view,
        result: OracleWalk {
            visible: vec![false; cell_count],
            clean: vec![false; cell_count],
            considered: 0,
            step_limit_hit: false,
            depth_limit_hit: false,
        },
        path: Vec::new(),
        clip_a: Vec::new(),
        clip_b: Vec::new(),
    };
    let frustum = view.frustum().clone();
    for &seed in seeds {
        let Some(cell) = world.cells.get(seed) else {
            continue;
        };
        if cell.is_solid || cell.is_exterior {
            continue;
        }
        if crate::visibility::is_aabb_outside_frustum(cell.bounds_min, cell.bounds_max, &frustum) {
            continue;
        }
        flood(&mut state, seed, &frustum, true, true);
    }
    state.result
}

/// `forward`: no hop so far crossed from the side away from the eye.
/// `on_screen`: the hop into `cell` left a polygon not wholly off screen.
fn flood(state: &mut OracleState, cell: usize, frustum: &Frustum, forward: bool, on_screen: bool) {
    state.result.visible[cell] = true;
    state.result.clean[cell] |= forward && on_screen;
    if state.path.len() >= ORACLE_CHAIN_DEPTH {
        state.result.depth_limit_hit = true;
        return;
    }
    let world = state.world;
    let view = state.view;
    let planes = world.portal_planes();
    for i in 0..world.cell_portal_count(cell) {
        if state.result.considered >= ORACLE_STEP_LIMIT {
            state.result.step_limit_hit = true;
            return;
        }
        state.result.considered += 1;
        let Some(portal_idx) = world.cell_portal_index(cell, i) else {
            continue;
        };
        let portal = &world.portals[portal_idx];
        let neighbor = if portal.front_cell == cell {
            portal.back_cell
        } else {
            portal.front_cell
        };
        if state.blocked_portals.get(portal_idx).copied().unwrap_or(false)
            || neighbor >= world.cell_count()
            || state.path.contains(&portal_idx)
            || world.cell_is_solid(neighbor)
        {
            continue;
        }
        if view.near_gates_bypass && wholly_behind(&view.near, &portal.polygon) {
            continue;
        }
        let plane = planes[portal_idx];
        let bypass =
            distance_to_polygon(view.eye, &portal.polygon, &plane) <= PORTAL_BYPASS_DISTANCE;
        let backward = !bypass && plane.oriented && {
            let distance = plane.signed_distance(view.eye);
            let source_side = if portal.front_cell == cell {
                distance
            } else {
                -distance
            };
            !crosses_from_source_side(source_side)
        };

        let (child, on_screen) = if bypass {
            // Keep the parent's planes: narrowing the unclipped polygon alone
            // drops the side planes and floods off screen.
            let Some(mut narrowed) = narrow_frustum(view.eye, &portal.polygon, frustum) else {
                continue;
            };
            narrowed.planes.extend_from_slice(&frustum.planes);
            (narrowed, !wholly_outside(&portal.polygon, view.frustum()))
        } else {
            let clipped =
                clip_polygon_to_frustum(&portal.polygon, frustum, &mut state.clip_a, &mut state.clip_b);
            if clipped.len() < 3 {
                continue;
            }
            let on_screen = !wholly_outside(clipped, view.frustum());
            let Some(narrowed) = narrow_frustum(view.eye, clipped, frustum) else {
                continue;
            };
            (narrowed, on_screen)
        };

        state.path.push(portal_idx);
        flood(state, neighbor, &child, forward && !backward, on_screen);
        state.path.pop();
        if state.result.step_limit_hit {
            return;
        }
    }
}

/// Some plane of `frustum` has every vertex strictly outside it.
fn wholly_outside(polygon: &[Vec3], frustum: &Frustum) -> bool {
    frustum
        .planes
        .iter()
        .any(|plane| polygon.iter().all(|v| plane.normal.dot(*v) + plane.dist < 0.0))
}
