//! Offline teleporter source/image cell couplings for id51.
//! Convex SAT preserves closed contacts; AABBs only reject candidates.
//! See: context/lib/build_pipeline.md §PRL section IDs (CellResidencySet, Teleporters)
use super::portal_distance::Neighbors;
use crate::partition::{Aabb, BspTree, region_polytope::RegionPolytope};
use glam::{DQuat, DVec3, Vec3};
use postretro_level_format::cells::CellsSection;
use postretro_level_format::environment_volumes::EnvironmentBrush;
use postretro_level_format::teleporters::{TeleportMode, TeleportersSection};
use postretro_level_loader::LevelWorld;

/// The compiler keeps BSP half-spaces only until packing. Id55 supplies the
/// canonical f32 geometry; no source-format vocabulary enters this stage.
#[derive(Clone, Copy)]
pub struct TeleportResidencyInput<'a> {
    pub section: &'a TeleportersSection,
    pub tree: &'a BspTree,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TeleportResidencyRoute {
    pub source_cells: Vec<u32>,
    pub arrival_cells: Vec<u32>,
}

/// Match final playable cell regions, preserving their runtime IDs. No geometry
/// work is done for an empty section. Armed state is deliberately never read.
pub(crate) fn build_routes(
    input: TeleportResidencyInput<'_>,
    cells: &CellsSection,
    world: &LevelWorld,
) -> anyhow::Result<Vec<TeleportResidencyRoute>> {
    build_routes_with(input, cells, world, LeafPrefilter::On)
}

/// Whether cell hulls are built only for leaves a teleporter region can reach.
#[derive(Clone, Copy, PartialEq, Eq)]
enum LeafPrefilter {
    On,
    /// Every playable leaf: the exhaustive reference the prefilter must match.
    #[cfg(test)]
    Off,
}

/// A teleporter's arrival before cell matching: a located cell, or the
/// source union carried onto the target.
enum Arrival {
    Cell(usize),
    Image(Vec<Hull>),
}

fn build_routes_with(
    input: TeleportResidencyInput<'_>,
    cells: &CellsSection,
    world: &LevelWorld,
    prefilter: LeafPrefilter,
) -> anyhow::Result<Vec<TeleportResidencyRoute>> {
    if input.section.teleporters.is_empty() {
        return Ok(Vec::new());
    }
    anyhow::ensure!(
        input.tree.leaves.len() == cells.cells.len(),
        "teleporter residency: BSP leaf count differs from runtime cells"
    );
    // Regions first: they need no cell geometry.
    let regions: Vec<(Vec<Hull>, Arrival)> = input
        .section
        .teleporters
        .iter()
        .map(|teleporter| {
            let source: Vec<Hull> = teleporter.brushes.iter().filter_map(brush_hull).collect();
            let arrival = match teleporter.mode {
                TeleportMode::Classic => {
                    Arrival::Cell(world.locate_cell(Vec3::from(teleporter.destination_origin)))
                }
                TeleportMode::Seamless => {
                    let yaw = teleporter.destination_yaw - teleporter.landmark_yaw;
                    let rotation = DQuat::from_rotation_y(f64::from(yaw));
                    let landmark = DVec3::from(teleporter.landmark_origin.map(f64::from));
                    let target = DVec3::from(teleporter.destination_origin.map(f64::from));
                    Arrival::Image(
                        source
                            .iter()
                            .map(|hull| hull.transformed(rotation, target - rotation * landmark))
                            .collect(),
                    )
                }
            };
            (source, arrival)
        })
        .collect();
    // Clipping every leaf dominates on large maps. A leaf's hull lies inside
    // its bounds, so a leaf whose bounds miss every region's (with margin over
    // the SAT's own AABB reject) cannot touch one: skipping it changes nothing.
    let needed: Vec<bool> = match prefilter {
        LeafPrefilter::On => {
            let reach: Vec<&Aabb> = regions
                .iter()
                .flat_map(|(source, arrival)| {
                    let image = match arrival {
                        Arrival::Image(image) => image.as_slice(),
                        Arrival::Cell(_) => &[],
                    };
                    source.iter().chain(image).map(|hull| &hull.bounds)
                })
                .collect();
            let mut needed: Vec<bool> = input
                .tree
                .leaves
                .iter()
                .map(|leaf| reach.iter().any(|region| may_touch(&leaf.bounds, region)))
                .collect();
            for (_, arrival) in &regions {
                if let &Arrival::Cell(cell) = arrival
                    && let Some(slot) = needed.get_mut(cell)
                {
                    *slot = true;
                }
            }
            needed
        }
        #[cfg(test)]
        LeafPrefilter::Off => vec![true; cells.cells.len()],
    };
    let hulls: Vec<Option<Hull>> = cells
        .cells
        .iter()
        .zip(&input.tree.leaves)
        .zip(&needed)
        .map(|((cell, leaf), &needed)| {
            if !needed || cell.is_solid() || cell.is_exterior() {
                return None;
            }
            let mut polytope = RegionPolytope::from_aabb(&leaf.bounds);
            for &(normal, distance) in &leaf.defining_planes {
                polytope = polytope.clip(normal, distance).0;
            }
            Hull::from_polytope(&polytope, leaf.defining_planes.iter().map(|&(n, _)| n))
        })
        .collect();
    Ok(regions
        .iter()
        .map(|(source, arrival)| TeleportResidencyRoute {
            source_cells: touched_cells(source, &hulls),
            arrival_cells: match arrival {
                Arrival::Cell(cell) => {
                    if hulls.get(*cell).is_some_and(Option::is_some) {
                        vec![*cell as u32]
                    } else {
                        Vec::new()
                    }
                }
                Arrival::Image(image) => touched_cells(image, &hulls),
            },
        })
        .collect())
}

/// Conservative AABB overlap: twice the SAT contact epsilon, so a clipped
/// hull's rounding past its leaf bounds can never be culled here.
fn may_touch(leaf: &Aabb, region: &Aabb) -> bool {
    let margin = DVec3::splat(2.0 * CONTACT_EPSILON);
    !((leaf.max + margin).cmplt(region.min).any() || (region.max + margin).cmplt(leaf.min).any())
}

/// Extra origins per camera cell. One source→arrival edge only: arriving at
/// another teleporter does not recursively traverse it. Distances use the
/// same untruncated hub metric as ordinary movement lead.
pub(crate) fn arrival_origins(
    routes: &[TeleportResidencyRoute],
    neighbors: &Neighbors,
    cameras: &[u32],
    cell_count: usize,
    max_lead: u32,
) -> Vec<Vec<(u32, u32)>> {
    if routes.is_empty() {
        return Vec::new();
    }
    let mut origins = vec![Vec::new(); cell_count];
    let mut camera_mask = vec![false; cell_count];
    for &camera in cameras {
        camera_mask[camera as usize] = true;
    }
    let mut distances = vec![u32::MAX; cell_count];
    let mut touched = Vec::new();
    for route in routes {
        if route.arrival_cells.is_empty() {
            continue;
        }
        for &source in &route.source_cells {
            for (cell, lead) in
                std::iter::once((source, 0)).chain(neighbors.within_distances(source, max_lead))
            {
                if !camera_mask[cell as usize] {
                    continue;
                }
                let distance = &mut distances[cell as usize];
                if *distance == u32::MAX {
                    touched.push(cell);
                }
                *distance = (*distance).min(lead);
            }
        }
        for camera in touched.drain(..) {
            let distance = &mut distances[camera as usize];
            origins[camera as usize].extend(
                route
                    .arrival_cells
                    .iter()
                    .map(|&arrival| (arrival, *distance)),
            );
            *distance = u32::MAX;
        }
    }
    for cells in &mut origins {
        cells.sort_unstable_by_key(|&(cell, lead)| (lead, cell));
        cells.dedup();
    }
    origins
}

/// Use the compiler clipper to construct full-dimensional hulls, then SAT for
/// intersection. Clipping alone discards boundary-only regions, which would
/// miss a source brush touching a cell face, edge or vertex.
struct Hull {
    bounds: Aabb,
    vertices: Vec<DVec3>,
    normals: Vec<DVec3>,
    edges: Vec<DVec3>,
}
const CONTACT_EPSILON: f64 = 1.0e-6; // Same metre tolerance as RegionPolytope.
impl Hull {
    fn from_polytope(
        polytope: &RegionPolytope,
        normals: impl Iterator<Item = DVec3>,
    ) -> Option<Self> {
        let vertices: Vec<_> = polytope.vertices().collect();
        if vertices.is_empty() {
            return None;
        }
        let normals = normals.chain([DVec3::X, DVec3::Y, DVec3::Z]).collect();
        let edges = polytope
            .facets()
            .flat_map(|(_, _, winding)| {
                let count = winding.len();
                (0..count).map(move |i| winding[(i + 1) % count] - winding[i])
            })
            .filter(|edge| edge.length_squared() > 0.0)
            .collect();
        Some(Self {
            bounds: polytope.vertex_aabb(),
            vertices,
            normals,
            edges,
        })
    }
    fn transformed(&self, rotation: DQuat, translation: DVec3) -> Self {
        let vertices: Vec<_> = self
            .vertices
            .iter()
            .map(|&v| rotation * v + translation)
            .collect();
        let mut bounds = Aabb::empty();
        for &vertex in &vertices {
            bounds.expand_point(vertex);
        }
        Self {
            bounds,
            vertices,
            normals: self.normals.iter().map(|&n| rotation * n).collect(),
            edges: self.edges.iter().map(|&e| rotation * e).collect(),
        }
    }
    fn intersects(&self, other: &Self) -> bool {
        if (self.bounds.max + DVec3::splat(CONTACT_EPSILON))
            .cmplt(other.bounds.min)
            .any()
            || (other.bounds.max + DVec3::splat(CONTACT_EPSILON))
                .cmplt(self.bounds.min)
                .any()
        {
            return false;
        }
        let separated = |axis: DVec3| {
            let length = axis.length();
            if length <= f64::EPSILON {
                return false;
            }
            let axis = axis / length;
            let span = |vertices: &[DVec3]| {
                vertices
                    .iter()
                    .map(|v| v.dot(axis))
                    .fold((f64::INFINITY, f64::NEG_INFINITY), |(min, max), v| {
                        (min.min(v), max.max(v))
                    })
            };
            let (a0, a1) = span(&self.vertices);
            let (b0, b1) = span(&other.vertices);
            a1 < b0 - CONTACT_EPSILON || b1 < a0 - CONTACT_EPSILON
        };
        !self
            .normals
            .iter()
            .chain(&other.normals)
            .copied()
            .any(separated)
            && !self
                .edges
                .iter()
                .any(|a| other.edges.iter().any(|b| separated(a.cross(*b))))
    }
}
fn brush_hull(brush: &EnvironmentBrush) -> Option<Hull> {
    let bounds = Aabb {
        min: DVec3::from(brush.aabb_min.map(f64::from)),
        max: DVec3::from(brush.aabb_max.map(f64::from)),
    };
    let mut polytope = RegionPolytope::from_aabb(&bounds);
    for plane in &brush.planes {
        polytope = polytope
            .clip(
                DVec3::from(plane.normal.map(f64::from)),
                f64::from(plane.distance),
            )
            .1;
    }
    Hull::from_polytope(
        &polytope,
        brush
            .planes
            .iter()
            .map(|p| DVec3::from(p.normal.map(f64::from))),
    )
}
fn touched_cells(regions: &[Hull], cells: &[Option<Hull>]) -> Vec<u32> {
    cells
        .iter()
        .enumerate()
        .filter_map(|(id, cell)| {
            cell.as_ref()
                .filter(|cell| regions.iter().any(|region| region.intersects(cell)))
                .map(|_| id as u32)
        })
        .collect()
}

#[cfg(test)]
mod tests;
