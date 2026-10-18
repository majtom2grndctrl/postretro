// Fluid surface grid: install-time cuts of each surface face along the SH
// probe lattice, so baked light evaluated per vertex varies within a cell.
// See: context/lib/rendering_pipeline.md §7.3a

use std::collections::HashMap;

use glam::{DVec2, DVec3, Vec3};
use postretro_level_format::environment_volumes::{
    EnvironmentSurfaceFace, EnvironmentVolumeRecord,
};

use super::{COLLINEAR_DISTANCE, face_normal, has_collinear_vertex};

/// Planes per probe cell along each axis at the finest spacing.
pub const FLUID_GRID_DIVISIONS: f64 = 2.0;
/// Most mesh vertices one level's fluid surfaces may pack into. Sized for an
/// authored ocean: about 500 m square at half-metre spacing.
pub const FLUID_GRID_VERTEX_CAP: u64 = 1 << 18;
/// Doublings of the finest spacing a level past the cap may take before it
/// packs ungridded: 2^20 cells, past any authorable level.
const FLUID_GRID_MAX_COARSENING: u32 = 20;
/// A vertex within this distance of a plane, along the plane's axis, lies on
/// it: the plane passes through the vertex and adds none beside it. The
/// compiler's collinear tolerance, so a plane never adds a vertex nearer an
/// existing one than the compiler would.
const PLANE_TOLERANCE: f64 = COLLINEAR_DISTANCE;
/// Plane counts past this saturate: the bound only has to exceed the cap.
const PLANE_COUNT_LIMIT: f64 = 1.0e9;

/// The SH probe lattice the fluid grid aligns to: the one the sampler reads.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FluidGridLattice {
    origin: DVec3,
    cell_size: DVec3,
}

impl FluidGridLattice {
    /// `None` for a non-finite origin, or a zero, negative or non-finite cell
    /// size on any axis: such a level packs ungridded.
    pub fn new(origin: [f32; 3], cell_size: [f32; 3]) -> Option<Self> {
        let origin = Vec3::from(origin).as_dvec3();
        let cell_size = Vec3::from(cell_size).as_dvec3();
        (origin.is_finite() && cell_size.is_finite() && cell_size.min_element() > 0.0).then_some(
            Self { origin, cell_size },
        )
    }

    /// The planes at `coarsening` doublings of the finest spacing. Each
    /// coarser set is a subset of the finer one, as both share the origin.
    fn planes(&self, coarsening: u32) -> FluidGridPlanes {
        FluidGridPlanes {
            origin: self.origin,
            spacing: self.cell_size / FLUID_GRID_DIVISIONS * f64::from(1u32 << coarsening),
            coarsening,
        }
    }
}

/// The world planes one packing cuts with: `origin[a] + k * spacing[a]` for
/// every integer `k` on each axis `a`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FluidGridPlanes {
    origin: DVec3,
    spacing: DVec3,
    /// Doublings past the finest spacing the cap forced.
    pub coarsening: u32,
}

impl FluidGridPlanes {
    pub fn spacing(&self) -> DVec3 {
        self.spacing
    }

    fn plane(&self, axis: usize, k: i64) -> f64 {
        self.origin[axis] + k as f64 * self.spacing[axis]
    }

    /// The planes on `axis` strictly inside `min..max` by more than the plane
    /// tolerance, which are exactly the planes splitting a polygon of that
    /// extent; `None` when there are none or too many to count.
    fn splitting(&self, axis: usize, min: f64, max: f64) -> Option<(i64, i64)> {
        let (low, high) = (min + PLANE_TOLERANCE, max - PLANE_TOLERANCE);
        let spacing = self.spacing[axis];
        let first = ((low - self.origin[axis]) / spacing).floor();
        let last = ((high - self.origin[axis]) / spacing).ceil();
        if !(low < high) || !(last - first <= PLANE_COUNT_LIMIT) || !first.is_finite() {
            return None;
        }
        let (mut first, mut last) = (first as i64, last as i64);
        // The plane's own formula decides, so every face agrees on it.
        while self.plane(axis, first) <= low {
            first += 1;
        }
        while self.plane(axis, last) >= high {
            last -= 1;
        }
        (first <= last).then_some((first, last))
    }

    /// Planes splitting `min..max` on `axis`, saturating far past any cap.
    fn splitting_count(&self, axis: usize, min: f64, max: f64) -> u64 {
        let span = (max - min) / self.spacing[axis];
        if !(span <= PLANE_COUNT_LIMIT) {
            return PLANE_COUNT_LIMIT as u64;
        }
        self.splitting(axis, min, max)
            .map_or(0, |(first, last)| (last - first + 1) as u64)
    }
}

fn position(face: &EnvironmentSurfaceFace, i: usize) -> DVec3 {
    Vec3::from(face.vertices[i].position).as_dvec3()
}

fn face_extent(face: &EnvironmentSurfaceFace) -> (DVec3, DVec3) {
    (0..face.vertices.len()).fold(
        (DVec3::INFINITY, DVec3::NEG_INFINITY),
        |(min, max), i| (min.min(position(face, i)), max.max(position(face, i))),
    )
}

/// Mesh vertices a drawable face packs into, today's triangulation.
fn ungridded_vertex_count(face: &EnvironmentSurfaceFace) -> u64 {
    face.vertices.len() as u64 + u64::from(has_collinear_vertex(&face.vertices))
}

/// An upper bound on the mesh vertices `face` packs into under `planes`,
/// from its extent alone, before any cut. Exact for a face no plane crosses.
/// A cut face keeps its own vertices and gains at most two boundary crossings
/// per splitting line, plus one interior crossing per pair of lines from
/// different axes; its triangulation adds none.
fn face_vertex_bound(face: &EnvironmentSurfaceFace, planes: &FluidGridPlanes) -> u64 {
    let (min, max) = face_extent(face);
    let lines: [u64; 3] = std::array::from_fn(|axis| planes.splitting_count(axis, min[axis], max[axis]));
    if lines == [0; 3] {
        return ungridded_vertex_count(face);
    }
    let boundary = 2 * (lines[0] + lines[1] + lines[2]);
    let interior = lines[0]
        .saturating_mul(lines[1])
        .saturating_add(lines[1].saturating_mul(lines[2]))
        .saturating_add(lines[0].saturating_mul(lines[2]));
    (face.vertices.len() as u64)
        .saturating_add(boundary)
        .saturating_add(interior)
}

fn drawable_faces(
    volumes: &[EnvironmentVolumeRecord],
) -> impl Iterator<Item = &EnvironmentSurfaceFace> {
    volumes
        .iter()
        .flat_map(|volume| &volume.surface_faces)
        .filter(|face| face_normal(face).is_some())
}

/// The planes a level's fluid surfaces are cut with: the finest spacing
/// whose vertex bound, summed over every drawable face before any is cut,
/// stays within `cap`. `None` packs every face as without a grid: no lattice,
/// no drawable face, or no spacing fits. Warns once when the cap coarsens the
/// grid or leaves it out, however many spacings the search tried.
pub fn choose_grid_planes(
    volumes: &[EnvironmentVolumeRecord],
    lattice: Option<&FluidGridLattice>,
    cap: u64,
) -> Option<FluidGridPlanes> {
    let lattice = lattice?;
    if drawable_faces(volumes).next().is_none() {
        return None;
    }
    let ungridded: u64 = drawable_faces(volumes).map(ungridded_vertex_count).sum();
    if ungridded > cap {
        log::warn!(
            "[Renderer] Fluid surfaces need {ungridded} vertices before any grid, past the cap \
             of {cap}; water takes its baked light at face vertices only"
        );
        return None;
    }
    for coarsening in 0..=FLUID_GRID_MAX_COARSENING {
        let planes = lattice.planes(coarsening);
        let mut bound = 0u64;
        for face in drawable_faces(volumes) {
            bound = bound.saturating_add(face_vertex_bound(face, &planes));
            if bound > cap {
                break;
            }
        }
        if bound <= cap {
            if coarsening > 0 {
                let finest = lattice.planes(0).spacing;
                log::warn!(
                    "[Renderer] Fluid surface grid coarsened from {finest:.3?} m to {:.3?} m \
                     spacing to stay within {cap} vertices",
                    planes.spacing,
                );
            }
            return Some(planes);
        }
    }
    log::warn!(
        "[Renderer] Fluid surface grid does not fit {cap} vertices at any spacing; water takes \
         its baked light at face vertices only"
    );
    None
}

/// One face cut along the grid: its vertices (the face's own first, in ring
/// order, then the crossings) and triangles over them, wound as the face.
#[derive(Debug, Default)]
pub(super) struct GridCut {
    pub positions: Vec<DVec3>,
    pub uvs: Vec<DVec2>,
    pub triangles: Vec<[u32; 3]>,
}

#[derive(Clone, Copy, PartialEq)]
enum Side {
    Below,
    On,
    Above,
}

/// Cut `face` (outward unit normal `normal`, normalized vertex UVs `uvs`)
/// along every plane splitting it; `None` when none does, so it packs as
/// without a grid.
///
/// A crossing is a function of its edge and plane alone, computed from the
/// edge's lexicographically lower endpoint, and whether a vertex lies on a
/// plane is a function of that vertex alone. Two faces sharing an edge
/// therefore cut it at the same points, so the grid adds no T-junction.
/// Axes cut in order, planes ascending, so a shared edge's sub-segments
/// match too.
pub(super) fn cut_face(
    face: &EnvironmentSurfaceFace,
    normal: Vec3,
    uvs: &[DVec2],
    planes: &FluidGridPlanes,
) -> Option<GridCut> {
    let (min, max) = face_extent(face);
    if (0..3).all(|axis| planes.splitting(axis, min[axis], max[axis]).is_none()) {
        return None;
    }
    let count = face.vertices.len();
    let mut cut = GridCut {
        positions: (0..count).map(|i| position(face, i)).collect(),
        uvs: uvs.to_vec(),
        triangles: Vec::new(),
    };
    let mut polygons: Vec<Vec<u32>> = vec![(0..count as u32).collect()];
    let mut crossings: HashMap<(i64, u32, u32), u32> = HashMap::new();
    for axis in 0..3 {
        crossings.clear();
        let mut next = Vec::with_capacity(polygons.len());
        for polygon in polygons {
            let (low, high) = polygon.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(l, h), &i| {
                let p = cut.positions[i as usize][axis];
                (l.min(p), h.max(p))
            });
            let Some((first, last)) = planes.splitting(axis, low, high) else {
                next.push(polygon);
                continue;
            };
            let mut rest = polygon;
            for k in first..=last {
                if let Some((below, above)) =
                    split(&rest, axis, k, planes.plane(axis, k), &mut cut, &mut crossings)
                {
                    next.push(below);
                    rest = above;
                }
            }
            next.push(rest);
        }
        polygons = next;
    }
    let normal = normal.as_dvec3();
    let magnitude = cut
        .positions
        .iter()
        .fold(1.0f64, |m, p| m.max(p.abs().max_element()));
    // Heights below the f32 rounding of the packed positions draw no area.
    let epsilon = 16.0 * f64::from(f32::EPSILON) * magnitude;
    for polygon in &polygons {
        triangulate_convex(polygon, &cut.positions, normal, epsilon, &mut cut.triangles);
    }
    Some(cut)
}

/// Split a convex polygon at plane `k` (`axis` = `plane`) into the parts
/// below and above it; `None` unless vertices lie on both sides beyond the
/// plane tolerance. A vertex on the plane joins both parts.
fn split(
    polygon: &[u32],
    axis: usize,
    k: i64,
    plane: f64,
    cut: &mut GridCut,
    crossings: &mut HashMap<(i64, u32, u32), u32>,
) -> Option<(Vec<u32>, Vec<u32>)> {
    let side = |cut: &GridCut, i: u32| {
        let d = cut.positions[i as usize][axis] - plane;
        if d < -PLANE_TOLERANCE {
            Side::Below
        } else if d > PLANE_TOLERANCE {
            Side::Above
        } else {
            Side::On
        }
    };
    let sides: Vec<Side> = polygon.iter().map(|&i| side(cut, i)).collect();
    if !sides.contains(&Side::Below) || !sides.contains(&Side::Above) {
        return None;
    }
    let (mut below, mut above) = (Vec::new(), Vec::new());
    for (n, &vertex) in polygon.iter().enumerate() {
        let following = (n + 1) % polygon.len();
        if sides[n] != Side::Above {
            below.push(vertex);
        }
        if sides[n] != Side::Below {
            above.push(vertex);
        }
        if matches!(
            (sides[n], sides[following]),
            (Side::Below, Side::Above) | (Side::Above, Side::Below)
        ) {
            let crossing = crossing(vertex, polygon[following], axis, k, plane, cut, crossings);
            below.push(crossing);
            above.push(crossing);
        }
    }
    Some((below, above))
}

/// The vertex where plane `k` crosses edge `a`–`b`, shared by both polygons
/// that hold the edge.
fn crossing(
    a: u32,
    b: u32,
    axis: usize,
    k: i64,
    plane: f64,
    cut: &mut GridCut,
    crossings: &mut HashMap<(i64, u32, u32), u32>,
) -> u32 {
    *crossings.entry((k, a.min(b), a.max(b))).or_insert_with(|| {
        let (pa, pb) = (cut.positions[a as usize], cut.positions[b as usize]);
        let lower_first = pa.to_array().partial_cmp(&pb.to_array()) != Some(std::cmp::Ordering::Greater);
        let (lo, hi) = if lower_first { (a, b) } else { (b, a) };
        let (lo_p, hi_p) = (cut.positions[lo as usize], cut.positions[hi as usize]);
        let t = (plane - lo_p[axis]) / (hi_p[axis] - lo_p[axis]);
        let mut point = lo_p + (hi_p - lo_p) * t;
        point[axis] = plane;
        // UV is affine along the edge, as across the face.
        let (lo_uv, hi_uv) = (cut.uvs[lo as usize], cut.uvs[hi as usize]);
        cut.positions.push(point);
        cut.uvs.push(lo_uv + (hi_uv - lo_uv) * t);
        cut.positions.len() as u32 - 1
    })
}

/// Triangulate a convex polygon over its own vertices. A grid cut leaves
/// collinear vertices on a part's edges (the compiler's T-junction inserts,
/// and planes passing near a corner); each stays a triangle corner. Each step
/// clips a corner beside a collinear vertex, which turns that vertex into a
/// corner, so three corners always remain; with none left, any corner.
/// Heights at or below `epsilon` count as collinear. A part with no corner
/// left has no area and draws nothing.
fn triangulate_convex(
    polygon: &[u32],
    positions: &[DVec3],
    normal: DVec3,
    epsilon: f64,
    triangles: &mut Vec<[u32; 3]>,
) {
    let height = |prev: u32, at: u32, next: u32| {
        let (p, v, n) = (
            positions[prev as usize],
            positions[at as usize],
            positions[next as usize],
        );
        let chord = (n - p).length();
        if chord == 0.0 {
            return 0.0;
        }
        (v - p).cross(n - v).dot(normal) / chord
    };
    let mut ring = polygon.to_vec();
    let mut corner = Vec::with_capacity(ring.len());
    while ring.len() > 3 {
        let count = ring.len();
        corner.clear();
        corner.extend((0..count).map(|i| {
            height(ring[(i + count - 1) % count], ring[i], ring[(i + 1) % count]) > epsilon
        }));
        let beside_collinear =
            |i: usize| corner[i] && (!corner[(i + count - 1) % count] || !corner[(i + 1) % count]);
        let Some(tip) = (0..count)
            .find(|&i| beside_collinear(i))
            .or_else(|| (0..count).find(|&i| corner[i]))
        else {
            return;
        };
        triangles.push([ring[(tip + count - 1) % count], ring[tip], ring[(tip + 1) % count]]);
        ring.remove(tip);
    }
    if ring.len() == 3 && height(ring[0], ring[1], ring[2]) > epsilon {
        triangles.push([ring[0], ring[1], ring[2]]);
    }
}

#[cfg(test)]
#[path = "grid_tests.rs"]
mod tests;
