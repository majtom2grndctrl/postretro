// Fluid surface grid: install-time cuts of each surface face along the SH
// probe lattice, so baked light evaluated per vertex varies within a cell.
// See: context/lib/rendering_pipeline.md §7.3a

use std::collections::HashMap;

use glam::{DVec2, DVec3, Vec3};
use postretro_level_format::environment_volumes::{
    EnvironmentSurfaceFace, EnvironmentVolumeRecord,
};

use super::ear_clip::triangulate_convex;
use super::{COLLINEAR_DISTANCE, face_normal};

/// Planes per probe cell along each axis at the finest spacing.
pub const FLUID_GRID_DIVISIONS: f64 = 2.0;
/// Most mesh vertices one level's fluid surfaces may pack into. Sized for an
/// authored ocean: about 256 m square at half-metre spacing.
pub const FLUID_GRID_VERTEX_CAP: u64 = 1 << 18;
/// Doublings of the finest spacing a level past the cap may take before it
/// packs ungridded: 2^20 cells, past any authorable level.
const FLUID_GRID_MAX_COARSENING: u32 = 20;
/// A vertex within this distance of a plane, along the plane's axis, lies on
/// it: the plane passes through the vertex and adds none beside it. The
/// renderer's collinear tolerance (`COLLINEAR_DISTANCE`, derived from the
/// compiler's split epsilon), so a plane never adds a vertex nearer an
/// existing one than the compiler would.
const PLANE_TOLERANCE: f64 = COLLINEAR_DISTANCE;
/// Plane counts past this saturate: the bound only has to exceed the cap.
const PLANE_COUNT_LIMIT: f64 = 1.0e9;
/// Plane indices at or past this magnitude are not counted: `k as f64`
/// stops holding every integer.
const PLANE_INDEX_LIMIT: f64 = (1u64 << 53) as f64;
/// Steps the plane range search may take from its estimate. More means the
/// spacing is below the precision of the planes' formula.
const PLANE_SEARCH_STEPS: u32 = 4;

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
        (origin.is_finite() && cell_size.is_finite() && cell_size.min_element() > 0.0)
            .then_some(Self { origin, cell_size })
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

    /// The planes on `axis` splitting a polygon whose extent on it is
    /// `min..max`: those with a vertex below and one above, by [`side`], as
    /// [`level`] and [`cut_axis`] judge each vertex.
    fn splitting(&self, axis: usize, min: f64, max: f64) -> Splitting {
        let (origin, spacing) = (self.origin[axis], self.spacing[axis]);
        let first = ((min + PLANE_TOLERANCE - origin) / spacing).floor() + 1.0;
        let last = ((max - PLANE_TOLERANCE - origin) / spacing).ceil() - 1.0;
        // NaN fails the comparisons too.
        if !(first.abs() < PLANE_INDEX_LIMIT && last.abs() < PLANE_INDEX_LIMIT)
            || last - first > PLANE_COUNT_LIMIT
        {
            return Splitting::Uncountable;
        }
        // The plane's own formula settles each estimate, so every face agrees.
        let first = first_where(first as i64, |k| {
            side(min, self.plane(axis, k)) == Side::Below
        });
        let last = first_where(last as i64 + 1, |k| {
            side(max, self.plane(axis, k)) != Side::Above
        });
        match (first, last) {
            (Some(first), Some(end)) if first < end => Splitting::Planes(first, end - 1),
            (Some(_), Some(_)) => Splitting::Nothing,
            _ => Splitting::Uncountable,
        }
    }

    /// Planes splitting `min..max` on `axis`, saturating far past any cap.
    fn splitting_count(&self, axis: usize, min: f64, max: f64) -> u64 {
        match self.splitting(axis, min, max) {
            Splitting::Nothing => 0,
            Splitting::Planes(first, last) => (last - first + 1) as u64,
            Splitting::Uncountable => PLANE_COUNT_LIMIT as u64,
        }
    }
}

/// The planes on one axis that split an extent.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Splitting {
    Nothing,
    /// Planes `first..=last`.
    Planes(i64, i64),
    /// Too many to count, or a spacing below the planes' precision: the
    /// bound saturates, and a cut takes none of them.
    Uncountable,
}

/// The least `k` near `estimate` where `holds`, false below some index and
/// true from it on, turns true; `None` when it is not within a few steps.
fn first_where(estimate: i64, holds: impl Fn(i64) -> bool) -> Option<i64> {
    let mut k = estimate;
    for _ in 0..PLANE_SEARCH_STEPS {
        match (holds(k - 1), holds(k)) {
            (false, true) => return Some(k),
            (true, _) => k -= 1,
            (false, false) => k += 1,
        }
    }
    None
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Side {
    Below,
    On,
    Above,
}

/// Which side of `plane` a vertex at `position` on the plane's axis lies:
/// on it within the plane tolerance. The one side test, for the planes a
/// polygon's extent spans and for the cut alike, so the two never disagree.
fn side(position: f64, plane: f64) -> Side {
    let offset = position - plane;
    if offset < -PLANE_TOLERANCE {
        Side::Below
    } else if offset > PLANE_TOLERANCE {
        Side::Above
    } else {
        Side::On
    }
}

fn position(face: &EnvironmentSurfaceFace, i: usize) -> DVec3 {
    Vec3::from(face.vertices[i].position).as_dvec3()
}

fn face_extent(face: &EnvironmentSurfaceFace) -> (DVec3, DVec3) {
    (0..face.vertices.len()).fold((DVec3::INFINITY, DVec3::NEG_INFINITY), |(min, max), i| {
        (min.min(position(face, i)), max.max(position(face, i)))
    })
}

/// Mesh vertices a drawable face packs into under the ungridded
/// triangulation (`triangulate_convex_face`), which adds none.
fn ungridded_vertex_count(face: &EnvironmentSurfaceFace) -> u64 {
    face.vertices.len() as u64
}

/// An upper bound on the mesh vertices `face` packs into under `planes`,
/// from its extent alone, before any cut. Exact for a face no plane crosses.
/// A cut face keeps its own vertices and gains at most two boundary crossings
/// per splitting line, plus one interior crossing per pair of lines from
/// different axes; its triangulation adds none.
fn face_vertex_bound(face: &EnvironmentSurfaceFace, planes: &FluidGridPlanes) -> u64 {
    let (min, max) = face_extent(face);
    let lines: [u64; 3] =
        std::array::from_fn(|axis| planes.splitting_count(axis, min[axis], max[axis]));
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
    drawable_faces(volumes).next()?;
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

/// Cut `face` (outward unit normal `normal`, normalized vertex UVs `uvs`)
/// along every plane splitting it; `None` when none does, so it packs as
/// without a grid.
///
/// A crossing is a function of its edge and plane alone, computed from the
/// edge's lexicographically lower endpoint, and whether a vertex lies on a
/// plane is a function of that vertex alone. Two faces sharing an edge
/// therefore cut it at the same points, so the grid adds no T-junction.
/// Axes cut in order. Within an axis each crossing is keyed by its edge and
/// plane, so the order planes are walked in cannot change where a shared
/// edge is cut.
pub(super) fn cut_face(
    face: &EnvironmentSurfaceFace,
    normal: Vec3,
    uvs: &[DVec2],
    planes: &FluidGridPlanes,
) -> Option<GridCut> {
    let (min, max) = face_extent(face);
    if (0..3).all(|axis| {
        !matches!(
            planes.splitting(axis, min[axis], max[axis]),
            Splitting::Planes(..)
        )
    }) {
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
            let (low, high) =
                polygon
                    .iter()
                    .fold((f64::INFINITY, f64::NEG_INFINITY), |(l, h), &i| {
                        let p = cut.positions[i as usize][axis];
                        (l.min(p), h.max(p))
                    });
            let Splitting::Planes(first, last) = planes.splitting(axis, low, high) else {
                next.push(polygon);
                continue;
            };
            next.extend(cut_axis(
                &polygon,
                axis,
                first,
                last,
                planes,
                &mut cut,
                &mut crossings,
            ));
        }
        polygons = next;
    }
    let normal = normal.as_dvec3();
    for polygon in &polygons {
        triangulate_convex(polygon, &cut.positions, normal, &mut cut.triangles);
    }
    Some(cut)
}

/// Cut a convex polygon along planes `first..=last` on `axis` in one walk of
/// its ring, returning its parts from below the first plane up, each wound as
/// the polygon.
///
/// Each vertex lies in one slab between planes, or on one plane within the
/// plane tolerance. An edge gains a crossing at each plane strictly between
/// its ends, computed from that edge alone. A run of vertices on a plane
/// joins the slab on the side the ring came from, except where the ring
/// passes on to the other side: there the run's last vertex is the cut and
/// joins both. So the parts partition the polygon, as successive splits at
/// each plane would, and no vertex is added within the tolerance of one on a
/// plane. Linear in the polygon's vertices and crossings, however many
/// planes cross it.
fn cut_axis(
    polygon: &[u32],
    axis: usize,
    first: i64,
    last: i64,
    planes: &FluidGridPlanes,
    cut: &mut GridCut,
    crossings: &mut HashMap<(i64, u32, u32), u32>,
) -> Vec<Vec<u32>> {
    let count = polygon.len();
    super::count_install_steps(count);
    // A vertex's level: 2s in slab s (below plane `first + s`, above the one
    // before), 2j + 1 on plane `first + j`.
    let levels: Vec<i64> = polygon
        .iter()
        .map(|&vertex| {
            level(
                cut.positions[vertex as usize][axis],
                axis,
                first,
                last,
                planes,
            )
        })
        .collect();
    let runs = on_plane_runs(&levels);
    let slabs = (last - first + 2) as usize;
    let mut parts: Vec<Vec<u32>> = vec![Vec::new(); slabs];
    for n in 0..count {
        let vertex = polygon[n];
        let here = levels[n];
        if here % 2 == 0 {
            parts[(here / 2) as usize].push(vertex);
        } else {
            let (from, onward, last_of_run) = runs[n];
            let (below, above) = ((here / 2) as usize, (here / 2 + 1) as usize);
            match (from < here, onward < here) {
                (true, true) => parts[below].push(vertex),
                (false, false) => parts[above].push(vertex),
                (true, false) => {
                    parts[below].push(vertex);
                    if last_of_run {
                        parts[above].push(vertex);
                    }
                }
                (false, true) => {
                    parts[above].push(vertex);
                    if last_of_run {
                        parts[below].push(vertex);
                    }
                }
            }
        }
        let following = (n + 1) % count;
        let there = levels[following];
        // Planes strictly between the edge's ends, in ring order.
        let (low, high) = (here.min(there), here.max(there));
        super::count_install_steps((high - low) as usize);
        let mut between: Vec<i64> = (low + 1..high).filter(|height| height % 2 == 1).collect();
        if there < here {
            between.reverse();
        }
        for plane_level in between {
            let j = plane_level / 2;
            let k = first + j;
            let point = crossing(
                vertex,
                polygon[following],
                axis,
                k,
                planes.plane(axis, k),
                cut,
                crossings,
            );
            parts[j as usize].push(point);
            parts[j as usize + 1].push(point);
        }
    }
    parts.retain(|part| !part.is_empty());
    parts
}

/// The level of a vertex at `position` on `axis` among planes
/// `first..=last` (see [`cut_axis`]), judged by [`side`] against each
/// plane's own formula, so every face agrees.
fn level(position: f64, axis: usize, first: i64, last: i64, planes: &FluidGridPlanes) -> i64 {
    let (origin, spacing) = (planes.origin[axis], planes.spacing[axis]);
    // The first plane the position is not above: estimated, then settled.
    let estimate = ((position - PLANE_TOLERANCE - origin) / spacing).ceil();
    let mut k = if estimate.is_finite() {
        (estimate as i64).clamp(first, last + 1)
    } else {
        first
    };
    while k > first && side(position, planes.plane(axis, k - 1)) != Side::Above {
        super::count_install_steps(1);
        k -= 1;
    }
    while k <= last && side(position, planes.plane(axis, k)) == Side::Above {
        super::count_install_steps(1);
        k += 1;
    }
    let slab = 2 * (k - first);
    if k <= last && side(position, planes.plane(axis, k)) == Side::On {
        slab + 1
    } else {
        slab
    }
}

/// For each ring position on a plane, the levels of the vertices before and
/// after its run of vertices on that plane, and whether it ends the run; one
/// walk of the ring. Other positions read their own level and `false`.
fn on_plane_runs(levels: &[i64]) -> Vec<(i64, i64, bool)> {
    let count = levels.len();
    let mut runs: Vec<(i64, i64, bool)> =
        levels.iter().map(|&level| (level, level, false)).collect();
    // Start where a run begins, so no run wraps past the walk's end.
    let Some(start) = (0..count).find(|&n| levels[n] != levels[(n + count - 1) % count]) else {
        return runs;
    };
    let mut offset = 0;
    while offset < count {
        let begin = (start + offset) % count;
        let run_level = levels[begin];
        let mut length = 1;
        while length < count && levels[(begin + length) % count] == run_level {
            length += 1;
        }
        if run_level % 2 == 1 {
            let before = levels[(begin + count - 1) % count];
            let after = levels[(begin + length) % count];
            for i in 0..length {
                runs[(begin + i) % count] = (before, after, i + 1 == length);
            }
        }
        offset += length;
    }
    runs
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
        let lower_first =
            pa.to_array().partial_cmp(&pb.to_array()) != Some(std::cmp::Ordering::Greater);
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

#[cfg(test)]
#[path = "grid_tests.rs"]
mod tests;
