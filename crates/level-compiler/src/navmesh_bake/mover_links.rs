// Mover links: boarding edges from a posed mover's walkable top to landing regions.
// See: context/lib/build_pipeline.md §Navigation bake

use glam::DVec3;
use postretro_level_format::navmesh::{NavMoverLink, NavRegion, sorted_unique_links};

use super::mover_poses::{NavPose, WalkableSurface, lane_quad};
use super::{STEP_EPS, Triangle, WalkGrid, region_ref_height};
use crate::map_data::NavParams;

/// Horizontal reach from a surface rim to a landing region, past the agent
/// radius, in cells. Erosion removes spans out to `radius + cell / √2` from the
/// last floor cell, and that cell's centre can stand a whole cell back from the
/// true rim, so a landing can start `radius + 2.21 cells` out. The extra quarter
/// cell admits a mover docked a hand's width short of the rim.
const LINK_REACH_CELLS: f64 = 2.5;

/// An edge shorter than this has no lanes worth walking.
const MIN_EDGE_M: f64 = 1.0e-6;

/// A ray must run this far through a region to count as landing on it, so one
/// grazing a corner does not.
const MIN_LANDING_DEPTH_M: f64 = 1.0e-6;

/// How far over a surface to look for a brush lying on it.
const SURFACE_CLEARANCE_M: f64 = 1.0e-2;

/// Wall probes stay this far inside the agent band, off its exact limits.
const WALL_PROBE_INSET_M: f64 = 1.0e-2;

/// A region as a landing: its world footprint and floor level.
struct Landing {
    region: u32,
    floor: f64,
    min_x: f64,
    max_x: f64,
    min_z: f64,
    max_z: f64,
}

impl Landing {
    /// Whether this region can land any lane of `surface`.
    fn near_surface(&self, surface: &WalkableSurface, reach: f64, step: f64) -> bool {
        self.min_x <= surface.max.x + reach
            && self.max_x >= surface.min.x - reach
            && self.min_z <= surface.max.z + reach
            && self.max_z >= surface.min.z - reach
            && self.floor >= surface.min.y - step
            && self.floor <= surface.max.y + step
    }

    /// Where the ray from `at` along `direction` enters and leaves this region's
    /// footprint, or `None` when it enters beyond `reach` or misses. A lane looks
    /// outward only: a region set inside the rim (a shaft floor under a lift) is
    /// no landing for a rim whose outward side is a wall.
    fn ray_span(&self, at: DVec3, direction: DVec3, reach: f64) -> Option<(f64, f64)> {
        let mut enter = 0.0_f64;
        let mut exit = f64::INFINITY;
        for (origin, heading, low, high) in [
            (at.x, direction.x, self.min_x, self.max_x),
            (at.z, direction.z, self.min_z, self.max_z),
        ] {
            if heading.abs() <= f64::EPSILON {
                if origin < low || origin > high {
                    return None;
                }
            } else {
                let (first, second) = ((low - origin) / heading, (high - origin) / heading);
                enter = enter.max(first.min(second));
                exit = exit.min(first.max(second));
            }
        }
        (enter <= reach && exit - enter > MIN_LANDING_DEPTH_M).then_some((enter, exit))
    }
}

/// The link records for every pose, sorted and unique. A lane walks off a surface
/// edge to the nearest landing it reaches; adjacent lanes onto one region share a
/// link. Each edge lies at its surface's own height, which is what tells a rising
/// room's floor from its roof.
pub(super) fn bake_links(
    grid: &WalkGrid,
    regions: &[NavRegion],
    poses: &[NavPose],
    triangles: &[Triangle],
    params: &NavParams,
) -> Vec<NavMoverLink> {
    let cell = f64::from(grid.cell_size);
    let reach = f64::from(params.agent_radius) + LINK_REACH_CELLS * cell;
    let step = f64::from(params.step_height) + f64::from(STEP_EPS);
    let landings = landings(grid, regions);

    let mut links = Vec::new();
    for pose in poses {
        // Most poses land nowhere, so the wall gather waits for the first that does.
        let mut pose_walls: Option<Vec<&Triangle>> = None;
        for surface in &pose.surfaces {
            let candidates: Vec<&Landing> = landings
                .iter()
                .filter(|landing| landing.near_surface(surface, reach, step))
                .collect();
            if candidates.is_empty() {
                continue;
            }
            let walls =
                pose_walls.get_or_insert_with(|| walls_near_pose(triangles, pose, reach, params));
            SurfaceBoarding {
                pose,
                surface,
                candidates: &candidates,
                walls,
                params,
                cell,
                reach,
                step,
            }
            .collect_links(&mut links);
        }
    }
    sorted_unique_links(&links)
}

fn landings(grid: &WalkGrid, regions: &[NavRegion]) -> Vec<Landing> {
    let origin = grid.origin.as_dvec3();
    let cell = f64::from(grid.cell_size);
    regions
        .iter()
        .enumerate()
        .map(|(index, region)| Landing {
            region: index as u32,
            floor: f64::from(region_ref_height(region)),
            min_x: origin.x + f64::from(region.x0) * cell,
            max_x: origin.x + f64::from(region.x1) * cell,
            min_z: origin.z + f64::from(region.z0) * cell,
            max_z: origin.z + f64::from(region.z1) * cell,
        })
        .collect()
}

/// The static triangles a lane leaving this pose can cross.
fn walls_near_pose<'t>(
    triangles: &'t [Triangle],
    pose: &NavPose,
    reach: f64,
    params: &NavParams,
) -> Vec<&'t Triangle> {
    let margin = DVec3::new(reach, f64::from(params.agent_height), reach);
    let (near_min, near_max) = (pose.bounds.min - margin, pose.bounds.max + margin);
    triangles
        .iter()
        .filter(|triangle| {
            let min = triangle.a.min(triangle.b).min(triangle.c).as_dvec3();
            let max = triangle.a.max(triangle.b).max(triangle.c).as_dvec3();
            min.cmple(near_max).all() && max.cmpge(near_min).all()
        })
        .collect()
}

/// One walkable surface of one pose, with what its lanes test against.
struct SurfaceBoarding<'a> {
    pose: &'a NavPose,
    surface: &'a WalkableSurface,
    candidates: &'a [&'a Landing],
    walls: &'a [&'a Triangle],
    params: &'a NavParams,
    cell: f64,
    reach: f64,
    step: f64,
}

impl SurfaceBoarding<'_> {
    fn collect_links(&self, links: &mut Vec<NavMoverLink>) {
        let vertices = &self.surface.vertices;
        for (index, &start) in vertices.iter().enumerate() {
            self.collect_edge_links(start, vertices[(index + 1) % vertices.len()], links);
        }
    }

    fn collect_edge_links(&self, start: DVec3, end: DVec3, links: &mut Vec<NavMoverLink>) {
        let edge = end - start;
        let length = edge.x.hypot(edge.z);
        if length <= MIN_EDGE_M {
            return;
        }
        // Brush side winding is not a contract here, so outward is read from
        // the centroid of this convex polygon.
        let mut outward = DVec3::new(edge.z, 0.0, -edge.x) / length;
        if outward.dot((start + end) * 0.5 - self.surface.centroid) < 0.0 {
            outward = -outward;
        }

        let lanes = ((length / self.cell).ceil() as usize).max(1);
        let along = |lane: f64| start + edge * (lane / lanes as f64);
        let mut run: Option<(u32, usize)> = None;
        // One step past the last lane closes a run still open at the edge's end.
        for lane in 0..=lanes {
            let landing = (lane < lanes)
                .then(|| self.lane_landing(along(lane as f64 + 0.5), outward))
                .flatten();
            if run.is_some_and(|(region, _)| landing == Some(region)) {
                continue;
            }
            if let Some((region, first)) = run {
                links.push(self.link(region, along(first as f64), along(lane as f64), outward));
            }
            run = landing.map(|region| (region, lane));
        }
    }

    /// The region a lane at rim point `at` boards, or `None` when another brush
    /// lies on the surface there, the mover's own solid stands in the way, or a
    /// static wall does. The mover's own test is what leaves only a hollow
    /// mover's doorway open.
    fn lane_landing(&self, at: DVec3, outward: DVec3) -> Option<u32> {
        let (landing, enter, exit) = self
            .candidates
            .iter()
            .filter(|landing| (landing.floor - at.y).abs() <= self.step)
            .filter_map(|landing| {
                let (enter, exit) = landing.ray_span(at, outward, self.reach)?;
                Some((*landing, enter, exit))
            })
            .min_by(|a, b| a.1.total_cmp(&b.1).then(a.0.region.cmp(&b.0.region)))?;

        let onto = at + outward * (enter + ((exit - enter) * 0.5).min(self.cell * 0.5));
        let behind = at - outward * (self.cell * 0.5);
        let on_surface = DVec3::new(behind.x, at.y + SURFACE_CLEARANCE_M, behind.z);
        let boxed_in = self.pose.covers(on_surface)
            || self
                .pose
                .blocks(&lane_quad(behind, at.y, onto, at.y, self.params));
        if boxed_in || self.wall_between(at.y, onto) {
            return None;
        }
        Some(landing.region)
    }

    /// Whether static geometry stands between the surface and `onto`, probed at
    /// both limits of the agent band. The surface's centroid is open space even
    /// where its rim sits inside a wall, so the probe starts there.
    fn wall_between(&self, floor: f64, onto: DVec3) -> bool {
        let low = floor + f64::from(self.params.step_height) + WALL_PROBE_INSET_M;
        let high = floor + f64::from(self.params.agent_height) - WALL_PROBE_INSET_M;
        let centroid = self.surface.centroid;
        [low, high].into_iter().any(|y| {
            let from = DVec3::new(centroid.x, y, centroid.z);
            let to = DVec3::new(onto.x, y, onto.z);
            self.walls
                .iter()
                .any(|wall| segment_crosses_triangle(from, to, wall))
        })
    }

    /// A link over `from..to`, oriented for travel along `outward`.
    fn link(&self, region: u32, from: DVec3, to: DVec3, outward: DVec3) -> NavMoverLink {
        let left_of_travel = DVec3::new(-outward.z, 0.0, outward.x);
        let (left, right) = if (to - from).dot(left_of_travel) >= 0.0 {
            (to, from)
        } else {
            (from, to)
        };
        NavMoverLink {
            mover_id: self.pose.mover_id,
            waypoint: self.pose.waypoint,
            region,
            left: left.as_vec3().to_array(),
            right: right.as_vec3().to_array(),
        }
    }
}

/// Möller–Trumbore over the segment `from..=to`, either facing.
fn segment_crosses_triangle(from: DVec3, to: DVec3, triangle: &Triangle) -> bool {
    let a = triangle.a.as_dvec3();
    let ab = triangle.b.as_dvec3() - a;
    let ac = triangle.c.as_dvec3() - a;
    let direction = to - from;
    let p = direction.cross(ac);
    let determinant = ab.dot(p);
    if determinant.abs() <= f64::EPSILON {
        return false;
    }
    let from_a = from - a;
    let u = from_a.dot(p) / determinant;
    if !(0.0..=1.0).contains(&u) {
        return false;
    }
    let q = from_a.cross(ab);
    let v = direction.dot(q) / determinant;
    if v < 0.0 || u + v > 1.0 {
        return false;
    }
    (0.0..=1.0).contains(&(ac.dot(q) / determinant))
}
