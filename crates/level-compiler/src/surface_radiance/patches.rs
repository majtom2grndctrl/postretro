// Surface radiance cache patches: square cells on each pre-cut static face's plane.
// See: context/lib/build_pipeline.md §Compiler pipeline

use std::collections::BTreeMap;

use glam::{DVec2, DVec3, Vec3};

use crate::geometry::GeometryResult;

/// Edge of a patch's square cell on its face's plane, in metres.
pub(crate) const PATCH_SIZE_M: f64 = 0.5;

/// A cell whose clipped area is below this (m²) holds no patch: it is a
/// rounding sliver along a face edge, and a hit there reads the face's
/// nearest patch.
pub(super) const MIN_PATCH_AREA_M2: f64 = 1.0e-6;

/// Which side of its face a patch record lights: the side the face's
/// winding normal points to, or the other. A gather ray reads the side it
/// arrives from, as the double-sided hit test shades it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Side {
    Front = 0,
    Back = 1,
}

impl Side {
    pub(crate) const BOTH: [Side; 2] = [Side::Front, Side::Back];

    /// The side a ray travelling along `ray_dir` hits a face of front
    /// normal `front` from.
    pub(crate) fn of_ray(front: Vec3, ray_dir: Vec3) -> Self {
        if ray_dir.dot(front) < 0.0 {
            Side::Front
        } else {
            Side::Back
        }
    }

    /// This side's outward normal on a face of front normal `front`.
    pub(crate) fn normal(self, front: Vec3) -> Vec3 {
        match self {
            Side::Front => front,
            Side::Back => -front,
        }
    }
}

/// One cell of a face's plane grid, clipped to the face.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Patch {
    /// Index into `geometry.faces` (pre-cut, static world faces only).
    pub(crate) face: u32,
    /// The cell's `floor(coordinate / size)` on the face's plane axes.
    pub(crate) cell: [i32; 2],
    /// Area-weighted centroid of the cell's clipped region, on the face.
    pub(crate) centroid: Vec3,
    /// The face's front (winding) normal.
    pub(crate) normal: Vec3,
    /// Clipped area in m².
    pub(crate) area: f32,
    pub(crate) texture_index: u32,
}

impl Patch {
    /// The patch's seed axis for shading noise, from the face and the cell,
    /// never from its rank in the patch set.
    pub(crate) fn seed_id(&self) -> u64 {
        patch_seed_id(self.face, self.cell)
    }
}

/// SplitMix-style avalanche of `(face, i, j)`: a pure function of the face
/// and the cell, so adding a face after this one leaves its patches' seeds.
pub(crate) fn patch_seed_id(face: u32, cell: [i32; 2]) -> u64 {
    let mut z = u64::from(face).wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ u64::from(cell[0] as u32).wrapping_mul(0xBF58_476D_1CE4_E5B9)
        ^ u64::from(cell[1] as u32).wrapping_mul(0x94D0_49BB_1331_11EB);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// A face's plane axes. Coordinates are absolute (`p · u`, `p · v`), so
/// coplanar faces of one normal share a grid.
#[derive(Clone, Copy, Debug)]
struct FaceFrame {
    u: DVec3,
    v: DVec3,
    normal: Vec3,
}

impl FaceFrame {
    /// The frame of front normal `normal`: `u` is the axis least aligned
    /// with the normal (first of x, y, z on a tie) projected onto the plane.
    fn new(normal: DVec3) -> Self {
        let abs = normal.abs();
        let axis = if abs.x <= abs.y && abs.x <= abs.z {
            DVec3::X
        } else if abs.y <= abs.z {
            DVec3::Y
        } else {
            DVec3::Z
        };
        let u = (axis - normal * normal.dot(axis)).normalize();
        Self {
            u,
            v: normal.cross(u),
            normal: normal.as_vec3(),
        }
    }

    fn project(&self, p: DVec3) -> DVec2 {
        DVec2::new(p.dot(self.u), p.dot(self.v))
    }
}

/// Every patch over a level's pre-cut static faces, grouped by face.
pub(crate) struct PatchSet {
    size: f64,
    patches: Vec<Patch>,
    /// CSR: face `f`'s patches are `patches[face_patches[f]..face_patches[f + 1]]`,
    /// ascending by cell.
    face_patches: Vec<u32>,
    frames: Vec<Option<FaceFrame>>,
    /// Exclusive end of each face's index range, ascending: a triangle's
    /// index offset names its face by binary search.
    face_index_ends: Vec<u32>,
    #[cfg(test)]
    _held: crate::live_bytes::Held,
}

impl PatchSet {
    pub(crate) fn build(geo: &GeometryResult) -> Self {
        Self::build_with_size(geo, PATCH_SIZE_M)
    }

    /// [`Self::build`] at a cell edge of `size` metres.
    pub(crate) fn build_with_size(geo: &GeometryResult, size: f64) -> Self {
        assert!(size > 0.0 && size.is_finite(), "patch size {size}");
        let section = &geo.geometry;
        let mut patches = Vec::new();
        let mut face_patches = Vec::with_capacity(section.faces.len() + 1);
        let mut frames = Vec::with_capacity(section.faces.len());
        let mut face_index_ends = Vec::with_capacity(section.faces.len());
        face_patches.push(0);
        for (face, meta) in section.faces.iter().enumerate() {
            let range = geo.face_index_ranges[face];
            face_index_ends.push(range.index_offset + range.index_count);
            let triangles: Vec<[DVec3; 3]> = (range.index_offset
                ..range.index_offset + range.index_count)
                .step_by(3)
                .map(|tri| {
                    std::array::from_fn(|k| {
                        let index = section.indices[tri as usize + k] as usize;
                        Vec3::from(section.vertices[index].position).as_dvec3()
                    })
                })
                .collect();
            #[cfg(test)]
            let _triangles_held =
                crate::live_bytes::hold_vec("patch set face triangles", &triangles);
            let frame = face_normal(&triangles).map(FaceFrame::new);
            frames.push(frame);
            if let Some(frame) = frame {
                push_face_patches(
                    &mut patches,
                    face as u32,
                    meta.texture_index,
                    &frame,
                    &triangles,
                    size,
                );
            }
            face_patches.push(patches.len() as u32);
        }
        debug_assert!(face_index_ends.windows(2).all(|pair| pair[0] <= pair[1]));
        // Resident through the SH base stage and priced by capacity.
        #[cfg(test)]
        let grown = crate::live_bytes::hold_vec("patch set before shrinking", &patches);
        patches.shrink_to_fit();
        let set = Self {
            size,
            patches,
            face_patches,
            frames,
            face_index_ends,
            #[cfg(test)]
            _held: Default::default(),
        };
        #[cfg(test)]
        let set = {
            let held = crate::live_bytes::hold("patch set", set.heap_bytes());
            drop(grown);
            Self { _held: held, ..set }
        };
        set
    }

    pub(crate) fn patches(&self) -> &[Patch] {
        &self.patches
    }

    pub(crate) fn len(&self) -> usize {
        self.patches.len()
    }

    /// Host bytes the set holds, by allocated capacity.
    pub(crate) fn heap_bytes(&self) -> u64 {
        (self.patches.capacity() * size_of::<Patch>()
            + self.face_patches.capacity() * size_of::<u32>()
            + self.frames.capacity() * size_of::<Option<FaceFrame>>()
            + self.face_index_ends.capacity() * size_of::<u32>()) as u64
    }

    /// The most [`Self::build`] held over `geo` by capacity, or `None` on
    /// overflow: the finished set, the patch vector's growth before it
    /// shrinks (at most twice its length, or four), and the largest face's
    /// triangles. A face's cell map is not counted.
    pub(crate) fn build_peak_bytes(&self, geo: &GeometryResult) -> Option<u64> {
        let grown = (2 * self.patches.len()).max(4) as u64;
        let triangles = geo
            .face_index_ranges
            .iter()
            .map(|range| u64::from(range.index_count / 3))
            .max()
            .unwrap_or(0);
        grown
            .checked_mul(size_of::<Patch>() as u64)?
            .checked_add(triangles.checked_mul(size_of::<[DVec3; 3]>() as u64)?)?
            .checked_add(self.heap_bytes())
    }

    /// Face `face`'s patches, ascending by cell.
    pub(crate) fn face_patches(&self, face: u32) -> &[Patch] {
        let face = face as usize;
        &self.patches[self.face_patches[face] as usize..self.face_patches[face + 1] as usize]
    }

    /// The face whose index range holds the triangle at index offset
    /// `triangle`.
    pub(crate) fn face_of_triangle(&self, triangle: u32) -> Option<u32> {
        let face = self.face_index_ends.partition_point(|&end| end <= triangle);
        (face < self.face_index_ends.len()).then_some(face as u32)
    }

    /// The patch a gather ray along `ray_dir` reads where it hits `point` on
    /// the triangle at index offset `triangle`, and the side it arrives
    /// from: the patch whose cell holds the point, or the face's nearest
    /// patch when that cell holds none (a sliver along an edge). `None` for
    /// a face with no patch. [`super::PatchLookup::Nearest`] reads it.
    #[cfg(test)]
    pub(crate) fn lookup(&self, triangle: u32, point: Vec3, ray_dir: Vec3) -> Option<(u32, Side)> {
        let face = self.face_of_triangle(triangle)?;
        let frame = self.frames[face as usize]?;
        let first = self.face_patches[face as usize];
        let patches = self.face_patches(face);
        if patches.is_empty() {
            return None;
        }
        let at = frame.project(point.as_dvec3()) / self.size;
        let cell = [at.x.floor() as i32, at.y.floor() as i32];
        let local = patches
            .binary_search_by(|patch| patch.cell.cmp(&cell))
            .unwrap_or_else(|_| nearest(patches, point));
        Some((first + local as u32, Side::of_ray(frame.normal, ray_dir)))
    }

    /// [`Self::lookup`] blended: up to four patches of the hit's face whose
    /// cell centres surround the point, with bilinear weights over the cell
    /// grid, renormalized over the cells that hold a patch. A point no
    /// surrounding cell covers takes the face's nearest patch at weight 1.
    pub(crate) fn lookup_bilinear(
        &self,
        triangle: u32,
        point: Vec3,
        ray_dir: Vec3,
    ) -> Option<(BilinearPatches, Side)> {
        let face = self.face_of_triangle(triangle)?;
        let frame = self.frames[face as usize]?;
        let first = self.face_patches[face as usize];
        let patches = self.face_patches(face);
        if patches.is_empty() {
            return None;
        }
        let side = Side::of_ray(frame.normal, ray_dir);
        // Cell centres sit at `cell + 0.5`; the blend's lower corner cell
        // and the point's offset from its centre, in cell units.
        let at = frame.project(point.as_dvec3()) / self.size - DVec2::splat(0.5);
        let corner = at.floor();
        let t = at - corner;
        let mut blend = BilinearPatches::default();
        let mut total = 0.0;
        for (di, dj, w) in [
            (0, 0, (1.0 - t.x) * (1.0 - t.y)),
            (1, 0, t.x * (1.0 - t.y)),
            (0, 1, (1.0 - t.x) * t.y),
            (1, 1, t.x * t.y),
        ] {
            let cell = [corner.x as i32 + di, corner.y as i32 + dj];
            if w <= 0.0 {
                continue;
            }
            if let Ok(local) = patches.binary_search_by(|patch| patch.cell.cmp(&cell)) {
                blend.entries[blend.len] = (first + local as u32, w as f32);
                blend.len += 1;
                total += w;
            }
        }
        if blend.len == 0 {
            blend.entries[0] = (first + nearest(patches, point) as u32, 1.0);
            blend.len = 1;
        } else {
            for entry in &mut blend.entries[..blend.len] {
                entry.1 = (f64::from(entry.1) / total) as f32;
            }
        }
        Some((blend, side))
    }
}

/// Up to four `(patch, weight)` pairs whose weights sum to one.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct BilinearPatches {
    entries: [(u32, f32); 4],
    len: usize,
}

impl BilinearPatches {
    pub(crate) fn entries(&self) -> &[(u32, f32)] {
        &self.entries[..self.len]
    }
}

/// The face's front normal from its fan's summed cross products (Newell),
/// so it points along the winding the hit test's normal takes. `None` for a
/// degenerate face.
fn face_normal(triangles: &[[DVec3; 3]]) -> Option<DVec3> {
    let sum: DVec3 = triangles
        .iter()
        .map(|[a, b, c]| (*b - *a).cross(*c - *a))
        .sum();
    let normal = sum.normalize_or_zero();
    (normal != DVec3::ZERO).then_some(normal)
}

/// Clip each of the face's triangles against the grid cells its bounds
/// cover, accumulating each cell's area and area-weighted centroid, and push
/// one patch per cell above [`MIN_PATCH_AREA_M2`], ascending by cell.
fn push_face_patches(
    patches: &mut Vec<Patch>,
    face: u32,
    texture_index: u32,
    frame: &FaceFrame,
    triangles: &[[DVec3; 3]],
    size: f64,
) {
    // Plane offset along the normal, averaged over the face's corners.
    let offset = triangles
        .iter()
        .flatten()
        .map(|p| p.dot(frame.normal.as_dvec3()))
        .sum::<f64>()
        / (3 * triangles.len()) as f64;
    let mut cells: BTreeMap<[i32; 2], (f64, DVec2)> = BTreeMap::new();
    for triangle in triangles {
        let flat = triangle.map(|p| frame.project(p) / size);
        let lo = flat[0].min(flat[1]).min(flat[2]).floor();
        let hi = flat[0].max(flat[1]).max(flat[2]).floor();
        for i in lo.x as i32..=hi.x as i32 {
            for j in lo.y as i32..=hi.y as i32 {
                let clipped = clip_to_cell(&flat, i, j);
                let (area, centroid) = area_and_centroid(&clipped);
                if area > 0.0 {
                    let entry = cells.entry([i, j]).or_insert((0.0, DVec2::ZERO));
                    entry.0 += area;
                    entry.1 += centroid * area;
                }
            }
        }
    }
    let unit_area = size * size;
    for (cell, (area, weighted)) in cells {
        let area_m2 = area * unit_area;
        if area_m2 <= MIN_PATCH_AREA_M2 {
            continue;
        }
        let at = weighted / area * size;
        let centroid = frame.u * at.x + frame.v * at.y + frame.normal.as_dvec3() * offset;
        patches.push(Patch {
            face,
            cell,
            centroid: centroid.as_vec3(),
            normal: frame.normal,
            area: area_m2 as f32,
            texture_index,
        });
    }
}

/// The triangle clipped to cell `(i, j)`'s unit square (Sutherland–Hodgman),
/// in cell units.
fn clip_to_cell(triangle: &[DVec2; 3], i: i32, j: i32) -> Vec<DVec2> {
    let (x0, y0) = (f64::from(i), f64::from(j));
    let mut polygon = triangle.to_vec();
    // Each edge keeps the points where `inside(p) >= 0`.
    let edges: [&dyn Fn(DVec2) -> f64; 4] =
        [&|p| p.x - x0, &|p| x0 + 1.0 - p.x, &|p| p.y - y0, &|p| {
            y0 + 1.0 - p.y
        }];
    for inside in edges {
        if polygon.is_empty() {
            break;
        }
        let mut next = Vec::with_capacity(polygon.len() + 2);
        for (k, &a) in polygon.iter().enumerate() {
            let b = polygon[(k + 1) % polygon.len()];
            let (da, db) = (inside(a), inside(b));
            if da >= 0.0 {
                next.push(a);
            }
            if (da >= 0.0) != (db >= 0.0) {
                next.push(a + (b - a) * (da / (da - db)));
            }
        }
        polygon = next;
    }
    polygon
}

/// Unsigned area and centroid of a simple polygon (shoelace).
fn area_and_centroid(polygon: &[DVec2]) -> (f64, DVec2) {
    if polygon.len() < 3 {
        return (0.0, DVec2::ZERO);
    }
    let (mut twice_area, mut weighted) = (0.0, DVec2::ZERO);
    for (k, &a) in polygon.iter().enumerate() {
        let b = polygon[(k + 1) % polygon.len()];
        let cross = a.perp_dot(b);
        twice_area += cross;
        weighted += (a + b) * cross;
    }
    if twice_area.abs() < f64::EPSILON {
        return (0.0, DVec2::ZERO);
    }
    (twice_area.abs() / 2.0, weighted / (3.0 * twice_area))
}

/// The index of the patch whose centroid lies nearest `point`, the first on
/// a tie.
fn nearest(patches: &[Patch], point: Vec3) -> usize {
    patches
        .iter()
        .enumerate()
        .min_by(|(_, a), (_, b)| {
            a.centroid
                .distance_squared(point)
                .total_cmp(&b.centroid.distance_squared(point))
        })
        .map_or(0, |(index, _)| index)
}
