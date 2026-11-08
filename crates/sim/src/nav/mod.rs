// Runtime navigation graph: region records + per-region portals, the query
// surface that the pathfinding query (`nav/path.rs`) and steering sit on.
// See: context/lib/build_pipeline.md §Navigation bake

mod gate;
mod path;
// One-shot path queries. `find_path_from` is the production entry: steering, AI
// reachability and combat scoring route from a body's position and ground.
// `find_path_gated` assumes world ground, and `find_path` also ignores gates.
pub(crate) use gate::NavMoverBound;
pub use gate::{MoverNav, NavGate, install_mover_records};
pub use path::{NavStart, find_path, find_path_from, find_path_gated};

// Bake→runtime funnel contract tests: bake fixture floors with the real
// `navmesh_bake::bake_navmesh`, then run the real `find_path` over the result.
// The bake and the funnel meet in CI for the first time here (they held
// self-consistent but disagreeing handedness conventions before). Kept in its
// own sibling module (not `path.rs`) so it stays disjoint from the query code.
#[cfg(test)]
mod bake_contract_tests;

use glam::Vec3;
#[cfg(any(test, feature = "test-support"))]
use postretro_level_format::navmesh::NAVMESH_VERSION;
use postretro_level_format::navmesh::{
    NavMeshSection, NavMoverGate, NavMoverLink, NavPortal, NavRegion,
};

use postretro_foundation::GroundRef;
pub use postretro_foundation::NavAgentParams;

/// XZ-plane (ground) distance between two world positions, ignoring Y. Shared by
/// the pathfinding query (edge cost, heuristic) and downstream steering/AI so the
/// engine has one definition of "ground distance".
pub fn distance_xz(a: Vec3, b: Vec3) -> f32 {
    let dx = a.x - b.x;
    let dz = a.z - b.z;
    (dx * dx + dz * dz).sqrt()
}

/// Read-back of the navigation grid header. Regions are stored in cell space;
/// the grid header decodes a cell index to a world coordinate via
/// `origin + cell_size * index`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NavGrid {
    /// World-space minimum corner of the navigation grid.
    pub origin: [f32; 3],
    pub cell_size: f32,
    pub dim_x: u32,
    pub dim_z: u32,
}

/// World-space axis-aligned footprint of a region on the XZ plane plus its
/// floor-height band. Decoded once at load from the region's cell-space rect.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NavRegionRecord {
    /// Source cell-space rectangle (min inclusive / max exclusive).
    pub cell: NavRegion,
    /// World-space min corner on the XZ plane (`origin.x + cell_size * x0`, etc.).
    pub world_min_xz: [f32; 2],
    /// World-space max corner on the XZ plane (`origin.x + cell_size * x1`, etc.).
    pub world_max_xz: [f32; 2],
    pub floor_y_min: f32,
    pub floor_y_max: f32,
}

impl NavRegionRecord {
    /// Whether a world-space XZ position falls inside this region's footprint.
    /// Min inclusive / max exclusive, matching the cell-space convention.
    fn contains_xz(&self, x: f32, z: f32) -> bool {
        x >= self.world_min_xz[0]
            && x < self.world_max_xz[0]
            && z >= self.world_min_xz[1]
            && z < self.world_max_xz[1]
    }
}

/// Runtime navigation graph. Built once at load from a `NavMeshSection`: region
/// records carry world-space footprints decoded from the grid header, and a
/// per-region adjacency list flattens the section's portal array for O(1)
/// per-region portal iteration. The query surface (point→region lookup, portal
/// iteration, header/agent read-back) is what pathfinding sits on top of.
#[derive(Debug, Clone)]
pub struct NavGraph {
    grid: NavGrid,
    agent: NavAgentParams,
    regions: Vec<NavRegionRecord>,
    portals: Vec<NavPortal>,
    /// `region_portals[i]` = portal indices touching region `i`.
    region_portals: Vec<Vec<usize>>,
    /// Connected-component id per region. Built once at load so callers can
    /// distinguish a disconnected destination from a path-repair refusal.
    region_components: Vec<usize>,
    /// Baked mover gates, unvalidated: the section cannot see the movers, so
    /// references are checked after mover spawn.
    gates: Vec<NavMoverGate>,
    /// Baked mover links, unvalidated like `gates`, in wire order
    /// ([`sorted_links`]).
    links: Vec<NavMoverLink>,
    /// `portal_gates[i]` = the (mover, waypoint) docks that close portal `i`.
    portal_gates: Vec<Vec<(u32, u32)>>,
    /// `region_links[i]` = link indices landing in region `i`.
    region_links: Vec<Vec<usize>>,
    /// Waypoint heights of each mover with records, once validated.
    mover_heights: Vec<(u32, Vec<f32>)>,
}

/// Per-region link lists from the link table.
fn region_links(links: &[NavMoverLink], region_count: usize) -> Vec<Vec<usize>> {
    let mut by_region = vec![Vec::new(); region_count];
    for (index, link) in links.iter().enumerate() {
        if let Some(list) = by_region.get_mut(link.region as usize) {
            list.push(index);
        }
    }
    by_region
}

/// Links in wire order: by mover, waypoint, region, then the boarding edge's
/// bits. [`NavGraph::mover_links`] and the component merge read each mover's
/// links as one run, and `from_bytes` checks counts and strides only, so the
/// order is made here rather than trusted. Stable, so equal keys keep section
/// order.
fn sorted_links(links: &[NavMoverLink]) -> Vec<NavMoverLink> {
    let mut sorted = links.to_vec();
    sorted.sort_by_key(|link| {
        (
            link.mover_id,
            link.waypoint,
            link.region,
            link.left.map(f32::to_bits),
            link.right.map(f32::to_bits),
        )
    });
    sorted
}

/// Per-portal dock lists from the gate table. Out-of-range records wait for
/// [`NavGraph::validate_mover_records`] to name and drop them.
fn portal_gates(gates: &[NavMoverGate], portal_count: usize) -> Vec<Vec<(u32, u32)>> {
    let mut by_portal = vec![Vec::new(); portal_count];
    for gate in gates {
        if let Some(list) = by_portal.get_mut(gate.portal as usize) {
            list.push((gate.mover_id, gate.waypoint));
        }
    }
    by_portal
}

impl NavGraph {
    /// Build the runtime graph from a decoded section. Region rects are
    /// cell-space integers; each is decoded to a world footprint here so a
    /// world-space query position resolves without re-deriving the grid math.
    pub fn from_section(section: &NavMeshSection) -> Self {
        let grid = NavGrid {
            origin: section.origin,
            cell_size: section.cell_size,
            dim_x: section.dim_x,
            dim_z: section.dim_z,
        };
        let agent = NavAgentParams {
            radius: section.agent_radius,
            height: section.agent_height,
            step_height: section.step_height,
            max_slope_deg: section.max_slope_deg,
        };

        let regions: Vec<NavRegionRecord> = section
            .regions
            .iter()
            .map(|r| {
                let cs = section.cell_size;
                NavRegionRecord {
                    cell: *r,
                    world_min_xz: [
                        section.origin[0] + cs * r.x0 as f32,
                        section.origin[2] + cs * r.z0 as f32,
                    ],
                    world_max_xz: [
                        section.origin[0] + cs * r.x1 as f32,
                        section.origin[2] + cs * r.z1 as f32,
                    ],
                    floor_y_min: r.floor_y_min,
                    floor_y_max: r.floor_y_max,
                }
            })
            .collect();

        let mut region_portals = vec![Vec::new(); regions.len()];
        for (portal_idx, portal) in section.portals.iter().enumerate() {
            // `from_bytes` already range-checks portal endpoints, but guard
            // anyway so a hand-built section can't index out of bounds.
            if let Some(list) = region_portals.get_mut(portal.region_a as usize) {
                list.push(portal_idx);
            }
            if let Some(list) = region_portals.get_mut(portal.region_b as usize) {
                list.push(portal_idx);
            }
        }

        let mut region_components = vec![usize::MAX; regions.len()];
        let mut component_count = 0;
        for start in 0..regions.len() {
            if region_components[start] != usize::MAX {
                continue;
            }
            let mut pending = vec![start];
            region_components[start] = component_count;
            while let Some(region_index) = pending.pop() {
                for &portal_index in &region_portals[region_index] {
                    let portal = &section.portals[portal_index];
                    let neighbor = if portal.region_a as usize == region_index {
                        portal.region_b as usize
                    } else {
                        portal.region_a as usize
                    };
                    let Some(neighbor_component) = region_components.get_mut(neighbor) else {
                        continue;
                    };
                    if *neighbor_component == usize::MAX {
                        *neighbor_component = component_count;
                        pending.push(neighbor);
                    }
                }
            }
            component_count += 1;
        }
        // A lift joins its landings, so components stay a superset of what any
        // dock state can connect. Sorted links put each mover's in one run, so
        // neighbouring pairs chain all of them.
        let links = sorted_links(&section.links);
        for pair in links.windows(2) {
            if pair[0].mover_id != pair[1].mover_id {
                continue;
            }
            let joined = (
                region_components.get(pair[0].region as usize).copied(),
                region_components.get(pair[1].region as usize).copied(),
            );
            if let (Some(keep), Some(merge)) = joined {
                for component in region_components.iter_mut() {
                    if *component == merge {
                        *component = keep;
                    }
                }
            }
        }

        Self {
            grid,
            agent,
            regions,
            portals: section.portals.clone(),
            region_portals,
            region_components,
            gates: section.gates.clone(),
            portal_gates: portal_gates(&section.gates, section.portals.len()),
            region_links: region_links(&links, section.regions.len()),
            links,
            mover_heights: Vec::new(),
        }
    }

    /// Recreate the immutable source section for the sim unit-test crate-identity
    /// adapter. Production never serializes or rebuilds a graph per tick.
    #[cfg(any(test, feature = "test-support"))]
    pub fn to_section_for_test(&self) -> NavMeshSection {
        NavMeshSection {
            version: NAVMESH_VERSION,
            origin: self.grid.origin,
            cell_size: self.grid.cell_size,
            dim_x: self.grid.dim_x,
            dim_z: self.grid.dim_z,
            agent_radius: self.agent.radius,
            agent_height: self.agent.height,
            step_height: self.agent.step_height,
            max_slope_deg: self.agent.max_slope_deg,
            regions: self.regions.iter().map(|region| region.cell).collect(),
            portals: self.portals.clone(),
            gates: self.gates.clone(),
            links: self.links.clone(),
        }
    }

    /// Grid header read-back. Navmesh query surface; no production caller in the
    /// default build (the navmesh debug overlay and tests read it).
    #[allow(dead_code)]
    pub fn grid(&self) -> NavGrid {
        self.grid
    }

    /// Cell size read-back (convenience; also on `grid()`). Navmesh query
    /// surface; no production caller in the default build.
    #[allow(dead_code)]
    pub fn cell_size(&self) -> f32 {
        self.grid.cell_size
    }

    /// Canonical-agent parameter read-back. Called at level load to seed
    /// descriptor-spawned agent capsules (`startup/lifecycle.rs`) and by the
    /// dev-tools chase-agent spawner.
    pub fn agent_params(&self) -> NavAgentParams {
        self.agent
    }

    #[cfg(test)]
    pub fn region_count(&self) -> usize {
        self.regions.len()
    }

    /// All region records (world-space footprints). Read by the navmesh debug
    /// overlay (dev-tools) and tests; no production caller in the default build.
    #[allow(dead_code)]
    pub fn regions(&self) -> &[NavRegionRecord] {
        &self.regions
    }

    pub fn region(&self, index: usize) -> Option<&NavRegionRecord> {
        self.regions.get(index)
    }

    /// All portals (world-space segments).
    pub fn portals(&self) -> &[NavPortal] {
        &self.portals
    }

    /// Baked mover gates, in section order. Empty for a map with no movers or a
    /// version 2 section.
    #[cfg(any(test, feature = "test-support"))]
    pub fn gates(&self) -> &[NavMoverGate] {
        &self.gates
    }

    /// The (mover, waypoint) docks that close `portal_index`.
    pub(crate) fn portal_gates(&self, portal_index: usize) -> &[(u32, u32)] {
        self.portal_gates
            .get(portal_index)
            .map_or(&[], Vec::as_slice)
    }

    /// Links landing in `region`.
    pub(crate) fn region_links(&self, region: usize) -> &[usize] {
        self.region_links.get(region).map_or(&[], Vec::as_slice)
    }

    /// Link indices of one mover: one run, since [`sorted_links`] orders them
    /// by mover first.
    pub(crate) fn mover_links(&self, mover_id: u32) -> std::ops::Range<usize> {
        let start = self.links.partition_point(|link| link.mover_id < mover_id);
        let end = self.links.partition_point(|link| link.mover_id <= mover_id);
        start..end
    }

    fn waypoint_height(&self, mover_id: u32, waypoint: u32) -> Option<f32> {
        self.mover_heights
            .iter()
            .find(|(id, _)| *id == mover_id)
            .and_then(|(_, heights)| heights.get(waypoint as usize).copied())
    }

    /// A link's boarding surface in mover-local height, which is what stays
    /// constant from dock to dock: a floor and a roof differ, the same floor at
    /// two docks does not.
    pub(crate) fn link_surface(&self, link_index: usize) -> f32 {
        let link = &self.links[link_index];
        link.left[1]
            - self
                .waypoint_height(link.mover_id, link.waypoint)
                .unwrap_or(0.0)
    }

    /// `height` in the same frame, for a body on `mover_id` docked at `waypoint`.
    pub(crate) fn mover_surface(&self, mover_id: u32, waypoint: u32, height: f32) -> f32 {
        height - self.waypoint_height(mover_id, waypoint).unwrap_or(0.0)
    }

    /// Whether a self-driven mover waits here: only a path's two ends qualify.
    pub(crate) fn is_terminus(&self, mover_id: u32, waypoint: u32) -> bool {
        self.mover_heights
            .iter()
            .find(|(id, _)| *id == mover_id)
            .is_none_or(|(_, heights)| waypoint == 0 || waypoint as usize + 1 == heights.len())
    }

    /// Drop every record naming a mover, waypoint, portal or region this level
    /// lacks, with one warning each, so the rest of navigation still works.
    pub(crate) fn validate_mover_records(&mut self, movers: &[NavMoverBound]) {
        let waypoint_exists = |mover_id: u32, waypoint: u32| {
            movers.iter().any(|mover| {
                mover.mover_id == mover_id && (waypoint as usize) < mover.waypoint_heights.len()
            })
        };
        let portal_count = self.portals.len();
        self.gates.retain(|gate| {
            let valid = waypoint_exists(gate.mover_id, gate.waypoint)
                && (gate.portal as usize) < portal_count;
            if !valid {
                log::warn!(
                    "[Nav] dropped mover gate: mover {} waypoint {} portal {} is not in this level",
                    gate.mover_id,
                    gate.waypoint,
                    gate.portal
                );
            }
            valid
        });
        let region_count = self.regions.len();
        self.links.retain(|link| {
            let valid = waypoint_exists(link.mover_id, link.waypoint)
                && (link.region as usize) < region_count;
            if !valid {
                log::warn!(
                    "[Nav] dropped mover link: mover {} waypoint {} region {} is not in this level",
                    link.mover_id,
                    link.waypoint,
                    link.region
                );
            }
            valid
        });
        self.portal_gates = portal_gates(&self.gates, portal_count);
        self.region_links = region_links(&self.links, region_count);
        self.mover_heights = movers
            .iter()
            .map(|mover| (mover.mover_id, mover.waypoint_heights.clone()))
            .collect();
    }

    /// Baked mover links, in wire order ([`sorted_links`]). Empty for a map with
    /// no movers or a version 2 section.
    pub(crate) fn links(&self) -> &[NavMoverLink] {
        &self.links
    }

    /// World-space point → region index. Uses only the XZ footprint (regions
    /// partition the floor on the XZ plane); the floor band disambiguates only
    /// when footprints overlap on XZ. Returns the region whose floor band is
    /// nearest the query Y among all overlapping regions. When two regions are
    /// equidistant in Y, the lower-index region wins (first-wins by iteration
    /// order). `None` when no region covers the point.
    pub fn region_at(&self, position: Vec3) -> Option<usize> {
        let mut best: Option<(usize, f32)> = None;
        for (i, region) in self.regions.iter().enumerate() {
            if !region.contains_xz(position.x, position.z) {
                continue;
            }
            // Distance from the query Y to the region's floor band; 0 when the
            // point sits within the band.
            let dy = if position.y < region.floor_y_min {
                region.floor_y_min - position.y
            } else if position.y > region.floor_y_max {
                position.y - region.floor_y_max
            } else {
                0.0
            };
            match best {
                Some((_, best_dy)) if best_dy <= dy => {}
                _ => best = Some((i, dy)),
            }
        }
        best.map(|(i, _)| i)
    }

    /// Maximum XZ distance an off-mesh query position may sit from a region and
    /// still resolve to it via [`resolve_region_at`].
    ///
    /// The bake erodes the walkable grid conservatively: every cell any point of
    /// which lies within `agent_radius` of a boundary column is removed, so the
    /// walkable set can start up to `agent_radius + cell diagonal` away from the
    /// actual wall. A capsule legitimately standing against that wall (its
    /// center at roughly its radius plus a collision skin from the surface)
    /// therefore sits INSIDE the eroded band — off every region — even though it
    /// is physically on walkable floor. Chase targets (the player hugging a
    /// corner) and steered agents (pushed wall-ward by collide-and-slide,
    /// separation, or full-speed corner rounding) hit this constantly, and a
    /// routing query that returns `None` for them freezes pursuit.
    ///
    /// `radius + 1.5 * cell_size` covers the conservative quantization
    /// (`cell * sqrt(2)`) with slack for query capsules fatter than the baked
    /// agent (the player pawn). It stays far below any authored corridor width,
    /// so snapping cannot jump a wall: positions in an eroded band are always
    /// nearer the band's own region than anything across the solid geometry that
    /// caused the erosion.
    pub(crate) fn region_snap_tolerance(&self) -> f32 {
        self.agent.radius + 1.5 * self.grid.cell_size
    }

    /// [`region_at`], with off-mesh tolerance: a position outside every region
    /// resolves to the NEAREST region within [`region_snap_tolerance`] (XZ
    /// distance to the region footprint; ties break toward the nearer floor
    /// band, then the lower index). `None` when the position is farther than the
    /// tolerance from everything — genuinely off the mesh, not merely inside the
    /// eroded wall margin.
    ///
    /// This is the routing definition of "which region does this world position
    /// belong to": the route queries (`find_path_from` and its wrappers) resolve
    /// the goal through it, and the start too unless the body stands on a lift
    /// it can ride ([`Self::route_mover`]). The steering replan gates use it so
    /// a target skirting in and out of the eroded band does not flap between
    /// "routable" and "off-mesh".
    pub(crate) fn resolve_region_at(&self, position: Vec3) -> Option<usize> {
        if let Some(region) = self.region_at(position) {
            return Some(region);
        }

        let tolerance = self.region_snap_tolerance();
        let mut best: Option<(usize, f32, f32)> = None;
        for (i, region) in self.regions.iter().enumerate() {
            let dx = (region.world_min_xz[0] - position.x)
                .max(position.x - region.world_max_xz[0])
                .max(0.0);
            let dz = (region.world_min_xz[1] - position.z)
                .max(position.z - region.world_max_xz[1])
                .max(0.0);
            let dist_xz = (dx * dx + dz * dz).sqrt();
            if dist_xz > tolerance {
                continue;
            }
            let dy = if position.y < region.floor_y_min {
                region.floor_y_min - position.y
            } else if position.y > region.floor_y_max {
                position.y - region.floor_y_max
            } else {
                0.0
            };
            let better = match best {
                Some((_, best_dist, best_dy)) => {
                    dist_xz < best_dist || (dist_xz == best_dist && dy < best_dy)
                }
                None => true,
            };
            if better {
                best = Some((i, dist_xz, dy));
            }
        }
        best.map(|(i, _, _)| i)
    }

    /// Whether two resolved endpoints connect through the portals and links
    /// open now. Deliberately weaker than [`find_path_from`]: it ignores funnel
    /// clearance and link surfaces, so it tells a genuinely disconnected
    /// destination from a transient funnel-clearance refusal without rerunning
    /// a graph search.
    pub(crate) fn endpoints_are_topologically_connected(
        &self,
        gate: NavGate<'_>,
        start: Vec3,
        goal: Vec3,
    ) -> bool {
        let Some(start_region) = self.resolve_region_at(start) else {
            return false;
        };
        let Some(goal_region) = self.resolve_region_at(goal) else {
            return false;
        };
        // Components join every portal and link whatever the dock state, so
        // they are a superset of what connects now, and exact for a level
        // with no mover records.
        if self.region_components.get(start_region) != self.region_components.get(goal_region) {
            return false;
        }
        (self.gates.is_empty() && self.links.is_empty())
            || self.connected_now(gate, start_region, goal_region)
    }

    /// Flood from `start` across open portals and open links: a link is open
    /// while its mover docks at its waypoint, and leads to the mover's other
    /// links as A* would, at the same dock or by a ride between termini. Off
    /// the per-tick path: only a refused funnel in a level with mover records
    /// reaches it.
    fn connected_now(&self, gate: NavGate<'_>, start: usize, goal: usize) -> bool {
        if start == goal {
            return true;
        }
        let mut seen = vec![false; self.regions.len()];
        let mut frontier = vec![start];
        seen[start] = true;
        let mut reach = |region: usize, frontier: &mut Vec<usize>| {
            if let Some(slot) = seen.get_mut(region)
                && !*slot
            {
                *slot = true;
                frontier.push(region);
            }
        };
        while let Some(region) = frontier.pop() {
            if region == goal {
                return true;
            }
            for &portal_index in self.region_portal_indices(region) {
                if gate.portal_closed(self, portal_index) {
                    continue;
                }
                let portal = &self.portals[portal_index];
                let next = if portal.region_a as usize == region {
                    portal.region_b
                } else {
                    portal.region_a
                };
                reach(next as usize, &mut frontier);
            }
            for &boarding in self.region_links(region) {
                let board = &self.links[boarding];
                if !gate.docked_at(board.mover_id, board.waypoint) {
                    continue;
                }
                let rides = gate.rests_at_stops(board.mover_id)
                    && gate.moves_on_its_own(board.mover_id)
                    && self.is_terminus(board.mover_id, board.waypoint);
                for exit in self.mover_links(board.mover_id) {
                    let exit = &self.links[exit];
                    let same_dock = exit.waypoint == board.waypoint;
                    if same_dock || (rides && self.is_terminus(exit.mover_id, exit.waypoint)) {
                        reach(exit.region as usize, &mut frontier);
                    }
                }
            }
        }
        false
    }

    /// The lift a route from `ground` starts on: a mover with links that rests
    /// at its stops. A search from it leaves through its links, so the floor
    /// under it in XZ says nothing about where the body can go. `None` for
    /// plain ground, a mover with no links or one that never rests included:
    /// that route starts from the region under the body.
    pub(crate) fn route_mover(&self, gate: NavGate<'_>, ground: GroundRef) -> Option<u32> {
        match ground {
            GroundRef::Mover(mover_id)
                if !self.mover_links(mover_id).is_empty() && gate.rests_at_stops(mover_id) =>
            {
                Some(mover_id)
            }
            GroundRef::Mover(_) | GroundRef::World | GroundRef::Airborne => None,
        }
    }

    /// Whether a body on `ground` has no route start now: it stands on a lift
    /// ([`Self::route_mover`]) while the lift is between docks. A route from
    /// there starts only once the lift docks, so a cached verdict about where
    /// the body can go should hold until then.
    pub fn between_docks(&self, gate: NavGate<'_>, ground: GroundRef) -> bool {
        self.route_mover(gate, ground)
            .is_some_and(|mover_id| gate.docked_waypoint(mover_id).is_none())
    }

    /// Iterate the portals touching `region_index` (the per-region adjacency
    /// built at load). Empty when the region is isolated or the index is out of
    /// range. Navmesh query surface; pathfinding uses `region_portal_indices`
    /// (it needs the index), so this convenience iterator has no production
    /// caller in the default build.
    #[allow(dead_code)]
    pub fn region_portal_iter(&self, region_index: usize) -> impl Iterator<Item = &NavPortal> + '_ {
        self.region_portal_indices(region_index)
            .iter()
            .filter_map(move |&idx| self.portals.get(idx))
    }

    /// Portal indices touching `region_index`, into `portals()`. The index (not
    /// just the `&NavPortal`) is what pathfinding records per A* hop so a region
    /// pair joined by two distinct portals resolves to the exact portal chosen.
    /// Empty when the region is isolated or the index is out of range.
    pub(crate) fn region_portal_indices(&self, region_index: usize) -> &[usize] {
        self.region_portals
            .get(region_index)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nav_graph_is_thread_shareable() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<NavGraph>();
    }
    use crate::kinematic_mover::{MoverDockTable, MoverTickStateTable};
    use postretro_entities::{KinematicMoverComponent, KinematicMoverConfig, KinematicMoverMode};
    use postretro_level_format::navmesh::{NAVMESH_VERSION, NavPortal, NavRegion};

    /// Single region, non-zero origin and cell size, so the cell→world decode
    /// is exercised (region rect [2,4) cells at cell_size 0.5, origin x=-4).
    fn single_region_section() -> NavMeshSection {
        NavMeshSection {
            version: NAVMESH_VERSION,
            origin: [-4.0, 1.0, -8.0],
            cell_size: 0.5,
            dim_x: 32,
            dim_z: 64,
            agent_radius: 0.3,
            agent_height: 1.8,
            step_height: 0.4,
            max_slope_deg: 45.0,
            regions: vec![NavRegion {
                x0: 2,
                z0: 2,
                x1: 6,
                z1: 6,
                floor_y_min: 0.0,
                floor_y_max: 0.25,
            }],
            portals: Vec::new(),
            gates: Vec::new(),
            links: Vec::new(),
        }
    }

    /// Two regions adjacent in z (cell_size 1.0, origin at world zero) joined by
    /// one portal. Region 0 covers z in [0,4); region 1 covers z in [4,8).
    fn stacked_region_section() -> NavMeshSection {
        NavMeshSection {
            version: NAVMESH_VERSION,
            origin: [0.0, 0.0, 0.0],
            cell_size: 1.0,
            dim_x: 16,
            dim_z: 16,
            agent_radius: 0.35,
            agent_height: 2.0,
            step_height: 0.5,
            max_slope_deg: 50.0,
            regions: vec![
                NavRegion {
                    x0: 0,
                    z0: 0,
                    x1: 4,
                    z1: 4,
                    floor_y_min: 0.0,
                    floor_y_max: 0.1,
                },
                NavRegion {
                    x0: 0,
                    z0: 4,
                    x1: 4,
                    z1: 8,
                    floor_y_min: 0.5,
                    floor_y_max: 0.6,
                },
            ],
            portals: vec![NavPortal {
                region_a: 0,
                region_b: 1,
                left: [0.0, 0.05, 4.0],
                right: [4.0, 0.55, 4.0],
            }],
            gates: Vec::new(),
            links: Vec::new(),
        }
    }

    // The `ai` test macro rebuilds graphs through `to_section_for_test`, so a table
    // it dropped would vanish from those tests silently.
    #[test]
    fn mover_records_survive_the_graph_and_its_test_section_round_trip() {
        let mut section = stacked_region_section();
        section.gates = vec![NavMoverGate {
            mover_id: 3,
            waypoint: 1,
            portal: 0,
        }];
        section.links = vec![NavMoverLink {
            mover_id: 4,
            waypoint: 0,
            region: 1,
            left: [1.0, 0.5, 4.0],
            right: [2.0, 0.5, 4.0],
        }];

        let graph = NavGraph::from_section(&section);

        assert_eq!(graph.gates(), section.gates);
        assert_eq!(graph.links(), section.links);
        assert_eq!(graph.to_section_for_test(), section);
    }

    #[test]
    fn nav_grid_and_agent_params_read_back() {
        let graph = NavGraph::from_section(&single_region_section());
        let grid = graph.grid();
        assert_eq!(grid.origin, [-4.0, 1.0, -8.0]);
        assert_eq!(grid.cell_size, 0.5);
        assert_eq!(grid.dim_x, 32);
        assert_eq!(grid.dim_z, 64);
        assert_eq!(graph.cell_size(), 0.5);

        let agent = graph.agent_params();
        assert_eq!(agent.radius, 0.3);
        assert_eq!(agent.height, 1.8);
        assert_eq!(agent.step_height, 0.4);
        assert_eq!(agent.max_slope_deg, 45.0);
    }

    #[test]
    fn region_record_decodes_cell_rect_to_world() {
        let graph = NavGraph::from_section(&single_region_section());
        let region = graph.region(0).unwrap();
        // x: origin.x(-4) + cell(0.5) * [2,6) = [-3.0, -1.0)
        // z: origin.z(-8) + cell(0.5) * [2,6) = [-7.0, -5.0)
        assert_eq!(region.world_min_xz, [-3.0, -7.0]);
        assert_eq!(region.world_max_xz, [-1.0, -5.0]);
        assert_eq!(region.floor_y_min, 0.0);
        assert_eq!(region.floor_y_max, 0.25);
    }

    #[test]
    fn region_at_resolves_world_point_via_decoded_rect() {
        let graph = NavGraph::from_section(&single_region_section());
        // A point inside the decoded world footprint resolves to region 0.
        assert_eq!(graph.region_at(Vec3::new(-2.0, 0.1, -6.0)), Some(0));
        // The cell-space rect would (wrongly) contain (3, *) — confirm the
        // raw cell coordinate is NOT what the lookup uses.
        assert_eq!(graph.region_at(Vec3::new(3.0, 0.1, 3.0)), None);
    }

    #[test]
    fn region_at_returns_none_outside_any_region() {
        let graph = NavGraph::from_section(&single_region_section());
        assert_eq!(graph.region_at(Vec3::new(100.0, 0.0, 100.0)), None);
    }

    #[test]
    fn region_at_max_edge_is_exclusive() {
        let graph = NavGraph::from_section(&single_region_section());
        // Max XZ corner is exclusive; the min corner is inclusive.
        assert_eq!(graph.region_at(Vec3::new(-3.0, 0.0, -7.0)), Some(0));
        assert_eq!(graph.region_at(Vec3::new(-1.0, 0.0, -5.0)), None);
    }

    #[test]
    fn region_at_picks_correct_stacked_region() {
        let graph = NavGraph::from_section(&stacked_region_section());
        // z in [0,4) → region 0; z in [4,8) → region 1.
        assert_eq!(graph.region_at(Vec3::new(2.0, 0.0, 1.0)), Some(0));
        assert_eq!(graph.region_at(Vec3::new(2.0, 0.5, 5.0)), Some(1));
    }

    #[test]
    fn resolve_region_at_matches_region_at_for_on_mesh_points() {
        let graph = NavGraph::from_section(&single_region_section());
        let inside = Vec3::new(-2.0, 0.1, -6.0);
        assert_eq!(graph.resolve_region_at(inside), graph.region_at(inside));
        assert_eq!(graph.resolve_region_at(inside), Some(0));
    }

    #[test]
    fn resolve_region_at_snaps_eroded_band_point_to_nearest_region() {
        // Region footprint x [-3,-1), z [-7,-5); agent radius 0.3, cell 0.5 →
        // tolerance 0.3 + 0.75 = 1.05. A point 0.4 outside the footprint (the
        // eroded wall margin a capsule can legitimately stand in) resolves.
        let graph = NavGraph::from_section(&single_region_section());
        assert_eq!(graph.region_at(Vec3::new(-0.6, 0.1, -6.0)), None);
        assert_eq!(graph.resolve_region_at(Vec3::new(-0.6, 0.1, -6.0)), Some(0));
        // Diagonal off the corner within tolerance also resolves.
        assert_eq!(graph.resolve_region_at(Vec3::new(-0.5, 0.1, -4.5)), Some(0));
    }

    #[test]
    fn resolve_region_at_rejects_points_beyond_the_snap_tolerance() {
        let graph = NavGraph::from_section(&single_region_section());
        // 1.5 beyond the footprint edge > tolerance 1.05: genuinely off-mesh.
        assert_eq!(graph.resolve_region_at(Vec3::new(0.5, 0.1, -6.0)), None);
        assert_eq!(graph.resolve_region_at(Vec3::new(100.0, 0.0, 100.0)), None);
    }

    #[test]
    fn resolve_region_at_picks_the_nearer_of_two_regions() {
        // Two stacked regions; a point just below region 0's footprint is nearer
        // region 0 than region 1 and must snap to it.
        let graph = NavGraph::from_section(&stacked_region_section());
        // (-0.5, 0, 3.5): left of both footprints (x0 = 0), within the 1.85
        // tolerance of both (0.5 from region 0, ~0.71 from region 1); the
        // nearer region 0 must win.
        assert_eq!(graph.region_at(Vec3::new(-0.5, 0.0, 3.5)), None);
        assert_eq!(graph.resolve_region_at(Vec3::new(-0.5, 0.0, 3.5)), Some(0));
        // Mirrored just above the shared edge: region 1 is nearer.
        assert_eq!(graph.resolve_region_at(Vec3::new(-0.5, 0.5, 4.5)), Some(1));
    }

    #[test]
    fn endpoints_topological_connectivity_distinguishes_portal_components() {
        let connected = NavGraph::from_section(&stacked_region_section());
        assert!(connected.endpoints_are_topologically_connected(
            NavGate::OPEN,
            Vec3::new(2.0, 0.0, 1.0),
            Vec3::new(2.0, 0.0, 5.0),
        ));

        let mut disconnected_section = stacked_region_section();
        disconnected_section.portals.clear();
        let disconnected = NavGraph::from_section(&disconnected_section);
        assert!(!disconnected.endpoints_are_topologically_connected(
            NavGate::OPEN,
            Vec3::new(2.0, 0.0, 1.0),
            Vec3::new(2.0, 0.0, 5.0),
        ));
        assert!(!connected.endpoints_are_topologically_connected(
            NavGate::OPEN,
            Vec3::new(100.0, 0.0, 100.0),
            Vec3::new(2.0, 0.0, 5.0),
        ));
    }

    const LIFT: u32 = 7;
    const DOOR: u32 = 8;

    fn lift_component(at_rest: bool) -> KinematicMoverComponent {
        let mut lift = KinematicMoverComponent::new(
            LIFT,
            KinematicMoverConfig {
                waypoints: vec![Vec3::ZERO, Vec3::Y * 4.0],
                waypoint_names: vec!["low".into(), "high".into()],
                speed_mps: 2.0,
                wait_ms: 500.0,
                mode: KinematicMoverMode::PingPong,
                started: true,
                spin_axis: Vec3::ZERO,
                initial_spin_rate_rad_s: 0.0,
                spin_accel_rad_s2: 0.0,
                carry_yaw: false,
            },
        );
        if !at_rest {
            lift.segment_elapsed_ms = 100.0;
            lift.current_linear_velocity = Vec3::Y * 2.0;
        }
        lift
    }

    fn lift_link(waypoint: u32, region: u32, x: f32) -> NavMoverLink {
        NavMoverLink {
            mover_id: LIFT,
            waypoint,
            region,
            left: [x, 0.0, 0.0],
            right: [x, 0.0, 4.0],
        }
    }

    /// Regions 0 and 1 share no portal, so only the lift joins them. Region 2
    /// meets region 0 through a portal a door gates when `gated`.
    fn lift_only_section(links: Vec<NavMoverLink>, gated: bool) -> NavMeshSection {
        let region = |x0, z0, x1, z1| NavRegion {
            x0,
            z0,
            x1,
            z1,
            floor_y_min: 0.0,
            floor_y_max: 0.1,
        };
        NavMeshSection {
            version: NAVMESH_VERSION,
            origin: [0.0, 0.0, 0.0],
            cell_size: 1.0,
            dim_x: 16,
            dim_z: 16,
            agent_radius: 0.35,
            agent_height: 2.0,
            step_height: 0.5,
            max_slope_deg: 50.0,
            regions: vec![region(0, 0, 4, 4), region(8, 0, 12, 4), region(0, 4, 4, 8)],
            portals: vec![NavPortal {
                region_a: 0,
                region_b: 2,
                left: [0.0, 0.05, 4.0],
                right: [4.0, 0.05, 4.0],
            }],
            gates: if gated {
                vec![NavMoverGate {
                    mover_id: DOOR,
                    waypoint: 0,
                    portal: 0,
                }]
            } else {
                Vec::new()
            },
            links,
        }
    }

    /// The lift's dock table, with the door watched too when `gated`.
    fn lift_docks(links: &[NavMoverLink], gated: bool, at_rest: bool) -> MoverDockTable {
        let mut movers = MoverTickStateTable::default();
        let door: &[(u32, u32)] = if gated { &[(DOOR, 0)] } else { &[] };
        let linked: Vec<(u32, u32)> = links
            .iter()
            .map(|link| (link.mover_id, link.waypoint))
            .collect();
        movers.watch_mover_docks(door, &linked);
        let mut docks = movers.docks().clone();
        docks.update(&lift_component(at_rest));
        docks
    }

    #[test]
    fn a_lift_only_connection_reads_the_same_with_or_without_gates_in_the_level() {
        let west = Vec3::new(2.0, 0.05, 2.0);
        let east = Vec3::new(10.0, 0.05, 2.0);
        let walk_across = vec![lift_link(0, 0, 4.0), lift_link(0, 1, 8.0)];
        let ride = vec![lift_link(0, 0, 4.0), lift_link(1, 1, 8.0)];
        for (route, links) in [("walk across", walk_across), ("ride", ride)] {
            for at_rest in [true, false] {
                let [without, with] = [false, true].map(|gated| {
                    let graph = NavGraph::from_section(&lift_only_section(links.clone(), gated));
                    let docks = lift_docks(&links, gated, at_rest);
                    graph.endpoints_are_topologically_connected(NavGate::new(&docks), west, east)
                });
                assert_eq!(
                    without, with,
                    "{route}, docked {at_rest}: gates elsewhere change nothing"
                );
                assert_eq!(
                    with, at_rest,
                    "{route}: connected only while the lift docks"
                );
            }
        }
    }

    #[test]
    fn a_body_is_between_docks_only_on_a_linked_mover_in_motion() {
        let links = vec![lift_link(0, 0, 4.0), lift_link(1, 1, 8.0)];
        let graph = NavGraph::from_section(&lift_only_section(links.clone(), false));
        let moving = lift_docks(&links, false, false);
        let docked = lift_docks(&links, false, true);

        assert!(graph.between_docks(NavGate::new(&moving), GroundRef::Mover(LIFT)));
        assert!(!graph.between_docks(NavGate::new(&docked), GroundRef::Mover(LIFT)));
        assert!(!graph.between_docks(NavGate::new(&moving), GroundRef::World));
        assert!(!graph.between_docks(NavGate::new(&moving), GroundRef::Mover(DOOR)));
    }

    /// The lift's dock table with no wait at its stops: never resting. Docked
    /// at its first stop while started, as a pin holds it there, when
    /// `at_rest`; otherwise between docks.
    fn restless_lift_docks(links: &[NavMoverLink], at_rest: bool) -> MoverDockTable {
        let mut movers = MoverTickStateTable::default();
        let linked: Vec<(u32, u32)> = links
            .iter()
            .map(|link| (link.mover_id, link.waypoint))
            .collect();
        movers.watch_mover_docks(&[], &linked);
        let mut docks = movers.docks().clone();
        let mut lift = lift_component(at_rest);
        lift.wait_ms = 0.0;
        docks.update(&lift);
        docks
    }

    // A lift that never rests at its stops is plain ground: no ride across it,
    // and a body on it routes from the floor under it, docked or not.
    #[test]
    fn a_lift_that_never_rests_offers_no_ride_and_is_plain_ground_to_a_route_start() {
        let west = Vec3::new(2.0, 0.05, 2.0);
        let east = Vec3::new(10.0, 0.05, 2.0);
        let links = vec![lift_link(0, 0, 4.0), lift_link(1, 1, 8.0)];
        let graph = NavGraph::from_section(&lift_only_section(links.clone(), false));
        let resting = lift_docks(&links, false, true);
        let docked = restless_lift_docks(&links, true);
        let moving = restless_lift_docks(&links, false);
        assert!(
            NavGate::new(&docked).moves_on_its_own(LIFT),
            "it would leave"
        );
        assert!(!NavGate::new(&docked).rests_at_stops(LIFT));

        let ride = find_path_gated(&graph, NavGate::new(&resting), west, east)
            .expect("a resting lift rides across");
        assert!(ride.ride().is_some());
        assert!(
            find_path_gated(&graph, NavGate::new(&docked), west, east).is_none(),
            "no ride on a lift that never rests"
        );
        assert!(!graph.endpoints_are_topologically_connected(NavGate::new(&docked), west, east));

        // Over the west landing, on the lift.
        let aboard = NavStart {
            position: Vec3::new(4.5, 1.0, 2.0),
            ground: GroundRef::Mover(LIFT),
            height: 2.0,
        };
        for (state, docks) in [("docked", &docked), ("moving", &moving)] {
            let gate = NavGate::new(docks);
            assert!(!graph.between_docks(gate, aboard.ground), "{state}");
            assert_eq!(aboard.region(&graph, gate), Some(0), "{state}");
            assert!(
                find_path_from(&graph, gate, aboard, west).is_some(),
                "{state}: routes from the floor under it"
            );
        }
        // A lift that rests is the route start, and has none between docks.
        let resting_moving = lift_docks(&links, false, false);
        assert!(graph.between_docks(NavGate::new(&resting_moving), aboard.ground));
        assert!(find_path_from(&graph, NavGate::new(&resting_moving), aboard, west).is_none());
    }

    // A hand-built or corrupt section can list links in any order; the graph
    // still reads each mover's links as one run.
    #[test]
    fn links_out_of_wire_order_still_read_as_one_run_per_mover() {
        let mut section = stacked_region_section();
        section.portals.clear();
        let link = |mover_id, waypoint, region| NavMoverLink {
            mover_id,
            waypoint,
            region,
            left: [1.0, 0.0, 4.0],
            right: [2.0, 0.0, 4.0],
        };
        section.links = vec![link(5, 0, 1), link(2, 1, 1), link(5, 1, 0), link(2, 0, 0)];

        let graph = NavGraph::from_section(&section);

        let movers_of = |mover_id| {
            graph
                .mover_links(mover_id)
                .map(|index| graph.links()[index].mover_id)
                .collect::<Vec<_>>()
        };
        assert_eq!(movers_of(2), vec![2, 2]);
        assert_eq!(movers_of(5), vec![5, 5]);
        assert!(
            graph
                .links()
                .windows(2)
                .all(|pair| (pair[0].mover_id, pair[0].waypoint)
                    <= (pair[1].mover_id, pair[1].waypoint)),
            "links sit in wire order"
        );
        for region in 0..2 {
            assert!(
                graph
                    .region_links(region)
                    .iter()
                    .all(|&index| graph.links()[index].region as usize == region),
                "region {region} indexes the sorted table"
            );
        }
        assert_eq!(
            graph.region_components[0], graph.region_components[1],
            "each lift joins the two regions no portal does"
        );
    }

    #[test]
    fn region_portal_iter_lists_each_regions_portals() {
        let graph = NavGraph::from_section(&stacked_region_section());
        // The single portal touches both regions.
        let from_a: Vec<_> = graph.region_portal_iter(0).collect();
        let from_b: Vec<_> = graph.region_portal_iter(1).collect();
        assert_eq!(from_a.len(), 1);
        assert_eq!(from_b.len(), 1);
        assert_eq!(from_a[0].region_a, 0);
        assert_eq!(from_a[0].region_b, 1);
        // Same portal seen from either side.
        assert_eq!(from_a[0], from_b[0]);
        assert_eq!(from_a[0].left, [0.0, 0.05, 4.0]);
        assert_eq!(from_a[0].right, [4.0, 0.55, 4.0]);
    }

    #[test]
    fn region_portal_iter_empty_for_isolated_region() {
        let graph = NavGraph::from_section(&single_region_section());
        assert_eq!(graph.region_count(), 1);
        assert_eq!(graph.region_portal_iter(0).count(), 0);
    }

    #[test]
    fn region_portal_iter_out_of_range_is_empty() {
        let graph = NavGraph::from_section(&stacked_region_section());
        assert_eq!(graph.region_portal_iter(99).count(), 0);
    }
}
