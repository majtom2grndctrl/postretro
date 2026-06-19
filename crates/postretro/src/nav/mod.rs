// Runtime navigation graph: region records + per-region portals, the query
// surface that the future baked-pathfinding plan extends.
// See: context/lib/build_pipeline.md §Navigation bake

mod path;
// One-shot path query. Gains its production caller when path-following lands;
// re-exported now so that caller imports `crate::nav::find_path`.
#[allow(unused_imports)]
pub use path::find_path;

use glam::Vec3;
use postretro_level_format::navmesh::{NavMeshSection, NavPortal, NavRegion};

/// XZ-plane (ground) distance between two world positions, ignoring Y. Shared by
/// the pathfinding query (edge cost, heuristic) and downstream steering/AI so the
/// engine has one definition of "ground distance".
pub(crate) fn distance_xz(a: Vec3, b: Vec3) -> f32 {
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

/// The four canonical-agent parameters the navmesh was baked for.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NavAgentParams {
    pub radius: f32,
    pub height: f32,
    pub step_height: f32,
    pub max_slope_deg: f32,
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
/// iteration, header/agent read-back) is what pathfinding will sit on top of.
#[derive(Debug, Clone)]
pub struct NavGraph {
    grid: NavGrid,
    agent: NavAgentParams,
    regions: Vec<NavRegionRecord>,
    portals: Vec<NavPortal>,
    /// `region_portals[i]` = portal indices touching region `i`.
    region_portals: Vec<Vec<usize>>,
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

        Self {
            grid,
            agent,
            regions,
            portals: section.portals.clone(),
            region_portals,
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

    /// Canonical-agent parameter read-back. Navmesh query surface; no production
    /// caller in the default build (agent-aware path-following will consume it).
    #[allow(dead_code)]
    pub fn agent_params(&self) -> NavAgentParams {
        self.agent
    }

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
        }
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
