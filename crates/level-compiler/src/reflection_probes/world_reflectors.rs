// Glossy world reflector faces: static id-17 faces whose material passes the opt-in, with their plane and outline.
// See: context/lib/resource_management.md §5.4

use std::collections::HashMap;

use glam::{DVec2, DVec3};

use crate::geometry::GeometryResult;

use super::glossy_materials::GlossyMaterials;
use super::placement::FaceGeometry;
use super::plane_basis::PlaneBasis;

/// A static world face whose material reflects, in Geometry (id 17)
/// numbering.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct WorldReflectorFace {
    pub face: u32,
    /// Unit normal (the face's winding, its front side), area and area
    /// centroid.
    pub geometry: FaceGeometry,
    /// The plane's offset along the normal: `normal · p` for every point `p`
    /// on it, taken at the area centroid.
    pub plane_offset: f64,
    /// The frame [`Self::outline`] is expressed in, from the face's own
    /// normal.
    pub basis: PlaneBasis,
    /// The polygon in `basis`'s plane coordinates, in winding order.
    pub outline: Vec<DVec2>,
}

impl WorldReflectorFace {
    /// The outline's vertices in world space, on the face's plane.
    pub(crate) fn outline_points(&self) -> impl Iterator<Item = DVec3> + '_ {
        self.outline
            .iter()
            .map(|&p| self.basis.lift(p, self.plane_offset))
    }
}

/// The glossy faces of the static world, in face order.
pub(crate) struct WorldReflectors {
    pub faces: Vec<WorldReflectorFace>,
    /// Glossy faces whose triangles form no single simple polygon or no area:
    /// they bind nothing.
    pub unrecovered: usize,
}

/// Every id-17 face whose material reflects. Id 17 holds only static world
/// faces (worldspawn, `func_group`, `switch` and `light_area` brushes):
/// movers live in id 43 and fluid surfaces in id 52, never here.
pub(crate) fn world_reflector_faces(
    geometry: &GeometryResult,
    glossy: &GlossyMaterials,
) -> WorldReflectors {
    let section = &geometry.geometry;
    let mut faces = Vec::new();
    let mut unrecovered = 0;
    for (index, (meta, range)) in section
        .faces
        .iter()
        .zip(&geometry.face_index_ranges)
        .enumerate()
    {
        if !glossy.reflects(meta.texture_index) {
            continue;
        }
        let start = range.index_offset as usize;
        let end = start + range.index_count as usize;
        let polygon = section
            .indices
            .get(start..end)
            .and_then(boundary_loop)
            .map(|loop_| {
                loop_
                    .iter()
                    .map(|&v| DVec3::from(section.vertices[v as usize].position.map(f64::from)))
                    .collect::<Vec<_>>()
            });
        let Some(face) = polygon.and_then(|points| reflector_face(index as u32, &points)) else {
            unrecovered += 1;
            continue;
        };
        faces.push(face);
    }
    WorldReflectors { faces, unrecovered }
}

fn reflector_face(face: u32, points: &[DVec3]) -> Option<WorldReflectorFace> {
    let geometry = FaceGeometry::from_polygon(points)?;
    let basis = PlaneBasis::new(geometry.normal);
    Some(WorldReflectorFace {
        face,
        geometry,
        plane_offset: basis.height(geometry.centroid),
        basis,
        outline: points.iter().map(|&p| basis.project(p)).collect(),
    })
}

/// A face's polygon from its triangles, in winding order: the directed edges
/// no triangle walks the other way, chained. The compiler fans each face over
/// its own vertices, but this reads any triangulation of one simple polygon.
/// `None` when the boundary is not one loop.
fn boundary_loop(indices: &[u32]) -> Option<Vec<u32>> {
    let (triangles, rest) = indices.as_chunks::<3>();
    if triangles.is_empty() || !rest.is_empty() {
        return None;
    }
    let edges: Vec<(u32, u32)> = triangles
        .iter()
        .flat_map(|&[a, b, c]| [(a, b), (b, c), (c, a)])
        .collect();
    let mut next: HashMap<u32, u32> = HashMap::new();
    for &(a, b) in &edges {
        if edges.contains(&(b, a)) {
            continue;
        }
        if next.insert(a, b).is_some() {
            return None;
        }
    }
    let start = *edges
        .iter()
        .map(|(a, _)| a)
        .find(|a| next.contains_key(a))?;
    let mut out = vec![start];
    let mut at = next[&start];
    while at != start {
        if out.len() >= next.len() {
            return None;
        }
        out.push(at);
        at = *next.get(&at)?;
    }
    (out.len() == next.len()).then_some(out)
}
