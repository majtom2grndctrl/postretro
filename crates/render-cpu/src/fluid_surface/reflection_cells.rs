// SPIKE (cell-rate spike, never merged): GPU-free layout of the pixel-style
// reflection cell atlas and the cell pass's per-frame inputs, mirrored by
// `fluid_reflection_cells.wgsl`.
// See: context/lib/rendering_pipeline.md §7.3a (Reflection → Style)

use std::ops::Range;

use bytemuck::{Pod, Zeroable};
use glam::{DVec2, DVec3, Mat4, Vec2, Vec3, Vec4, Vec4Swizzles};
use postretro_level_format::environment_volumes::EnvironmentVolumeRecord;

use super::reflection::FLUID_REFLECTION_NO_PROBE;
use super::{FluidSurfaceMesh, material_index};

/// Cells per side of one work tile: the compute's 8 × 8 workgroup.
pub const FLUID_CELL_TILE: u32 = 8;
/// The vertex word's atlas origin for a face with no atlas entry: it keeps
/// the per-fragment reflection.
pub const FLUID_CELL_NO_ATLAS: u32 = u32::MAX;
/// Widest atlas the packer opens, before the device limit.
pub const FLUID_CELL_ATLAS_MAX_WIDTH: u32 = 4096;
/// Atlas origins and extents pack two 16-bit halves into one vertex word.
const FLUID_CELL_HALF_MAX: u32 = 0xFFFF;
/// A face whose affine texel fit misses a vertex by more than this many
/// metres, plus a relative share of its extent, keeps the per-fragment path.
const FLUID_CELL_FIT_TOLERANCE: f64 = 1.0e-3;
const FLUID_CELL_FIT_RELATIVE: f64 = 1.0e-5;
/// The analytic texels-per-pixel for a cell behind the eye or seen edge-on:
/// past every fade threshold, so the cell reads flat, as an edge-on
/// fragment's huge derivative does.
pub const FLUID_CELL_FLAT_TEXELS_PER_PIXEL: f32 = 1.0e30;
/// Squared sine floor between the two screen-axis texel steps, as the
/// fragment's `FLUID_REFLECTION_AXES_EPSILON`.
const FLUID_CELL_JACOBIAN_EPSILON: f32 = 1.0e-8;
const FLUID_CELL_W_EPSILON: f32 = 1.0e-6;

/// Byte stride of the fourth vertex buffer's per-vertex word.
pub const FLUID_CELL_VERTEX_STRIDE: usize = 16;

/// One atlas face as the compute reads it (`FluidCellFace`). `origin` is the
/// world position of texel corner `cell_min`, so a cell centre is
/// `origin + axis_u·(local.x + ½) + axis_v·(local.y + ½)`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Pod, Zeroable)]
pub struct FluidCellFaceGpu {
    pub origin: [f32; 3],
    pub probe: u32,
    pub axis_u: [f32; 3],
    pub cell_min_x: i32,
    pub axis_v: [f32; 3],
    pub cell_min_y: i32,
    pub outward: [f32; 3],
    /// `x | y << 16`.
    pub atlas_origin: u32,
    pub extent: [u32; 2],
    pub _pad: [u32; 2],
}
pub const FLUID_CELL_FACE_STRIDE: usize = 80;
const _: () = assert!(std::mem::size_of::<FluidCellFaceGpu>() == FLUID_CELL_FACE_STRIDE);

/// One 8 × 8 work tile of one face (`FluidCellTile`): the face, the tile's
/// position in it (`x | y << 16`, in tiles), and the face's volume
/// reflectivity this frame, patched per frame.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Pod, Zeroable)]
pub struct FluidCellTileGpu {
    pub face: u32,
    pub tile: u32,
    pub reflectivity: f32,
    pub _pad: u32,
}
pub const FLUID_CELL_TILE_STRIDE: usize = 16;
const _: () = assert!(std::mem::size_of::<FluidCellTileGpu>() == FLUID_CELL_TILE_STRIDE);

/// The cell pass's own uniform (`FluidCellPass`): the view frustum's planes
/// (inward, normalized), the scene extent in pixels, the tile count, the
/// dispatch's x width in workgroups, and the stats switch.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Pod, Zeroable)]
pub struct FluidCellPassUniform {
    pub planes: [[f32; 4]; 6],
    pub extent: [f32; 2],
    pub tile_count: u32,
    pub groups_x: u32,
    pub stats: u32,
    pub _pad: [u32; 3],
}
pub const FLUID_CELL_PASS_UNIFORM_BYTES: usize = 128;
const _: () =
    assert!(std::mem::size_of::<FluidCellPassUniform>() == FLUID_CELL_PASS_UNIFORM_BYTES);

/// The spike's debug levers. All off is the shipped per-fragment path.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct FluidReflectionCellLevers {
    /// Pixel-style reflecting draws read the cell atlas the cell pass fills.
    pub cells: bool,
    /// The cell pass skips cells outside the view frustum.
    pub frustum_cull: bool,
    /// The live-lighting ratio is exactly 1, in the fragment and the pass.
    pub ratio_one: bool,
}

/// The world map of a planar face's texel space: `world = origin + u·t.x +
/// v·t.y`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FluidCellTexelMap {
    pub origin: DVec3,
    pub u: DVec3,
    pub v: DVec3,
}

/// Least-squares affine fit of `positions` against texel coordinates
/// `texels`, or `None` when the texels are degenerate (collinear) or the
/// fit misses a vertex past tolerance.
pub fn fit_texel_map(positions: &[Vec3], texels: &[Vec2]) -> Option<FluidCellTexelMap> {
    if positions.len() < 3 || positions.len() != texels.len() {
        return None;
    }
    let count = positions.len() as f64;
    let mean_t = texels
        .iter()
        .fold(DVec2::ZERO, |sum, t| sum + t.as_dvec2())
        / count;
    let mean_p = positions
        .iter()
        .fold(DVec3::ZERO, |sum, p| sum + p.as_dvec3())
        / count;
    // Centred normal equations: [sxx sxy; sxy syy] [u v]ᵀ = [sxp syp]ᵀ.
    let (mut sxx, mut sxy, mut syy) = (0.0, 0.0, 0.0);
    let (mut sxp, mut syp) = (DVec3::ZERO, DVec3::ZERO);
    for (p, t) in positions.iter().zip(texels) {
        let dt = t.as_dvec2() - mean_t;
        let dp = p.as_dvec3() - mean_p;
        sxx += dt.x * dt.x;
        sxy += dt.x * dt.y;
        syy += dt.y * dt.y;
        sxp += dp * dt.x;
        syp += dp * dt.y;
    }
    let det = sxx * syy - sxy * sxy;
    if !(det > 1.0e-12 * (sxx * syy).max(1.0e-300)) {
        return None;
    }
    let u = (sxp * syy - syp * sxy) / det;
    let v = (syp * sxx - sxp * sxy) / det;
    let origin = mean_p - u * mean_t.x - v * mean_t.y;
    let map = FluidCellTexelMap { origin, u, v };
    let extent = positions
        .iter()
        .map(|p| (p.as_dvec3() - mean_p).length())
        .fold(0.0, f64::max);
    let tolerance = FLUID_CELL_FIT_TOLERANCE + FLUID_CELL_FIT_RELATIVE * extent;
    let fits = positions.iter().zip(texels).all(|(p, t)| {
        let t = t.as_dvec2();
        (map.origin + map.u * t.x + map.v * t.y - p.as_dvec3()).length() <= tolerance
    });
    fits.then_some(map)
}

/// One face's cell rectangle in its texel grid: cells `min ..= min + extent - 1`.
/// Cells floor from the texel coordinate as pixel style keys them; the max
/// corner is `ceil(max t) − 1`, so a face edge on a whole texel adds no
/// empty column of cells.
pub fn cell_rect(texels: &[Vec2]) -> (glam::IVec2, glam::UVec2) {
    let min = texels.iter().fold(Vec2::INFINITY, |acc, t| acc.min(*t));
    let max = texels.iter().fold(Vec2::NEG_INFINITY, |acc, t| acc.max(*t));
    let cell_min = min.floor().as_ivec2();
    let cell_max = (max.ceil().as_ivec2() - glam::IVec2::ONE).max(cell_min);
    (cell_min, (cell_max - cell_min + glam::IVec2::ONE).as_uvec2())
}

/// Shelf placements for `rects` (width, height) in an atlas `width` wide and
/// at most `max_height` tall, tallest first. `None` for a rect that does not
/// fit. Returns the placements and the atlas height used.
pub fn shelf_pack(rects: &[[u32; 2]], width: u32, max_height: u32) -> (Vec<Option<[u32; 2]>>, u32) {
    let mut order: Vec<usize> = (0..rects.len()).collect();
    order.sort_by(|&a, &b| {
        rects[b][1]
            .cmp(&rects[a][1])
            .then(rects[b][0].cmp(&rects[a][0]))
    });
    let mut placements = vec![None; rects.len()];
    let (mut x, mut y, mut shelf) = (0u32, 0u32, 0u32);
    for index in order {
        let [w, h] = rects[index];
        if w == 0 || h == 0 || w > width {
            continue;
        }
        if x + w > width {
            y += shelf;
            x = 0;
            shelf = 0;
        }
        if y + h > max_height {
            continue;
        }
        placements[index] = Some([x, y]);
        x += w;
        shelf = shelf.max(h);
    }
    (placements, y + shelf)
}

/// The atlas width for `rects`: about square by area, at least the widest
/// rect, a multiple of the tile, at most `max_width`.
fn atlas_width(rects: &[[u32; 2]], max_width: u32) -> u32 {
    let area: u64 = rects.iter().map(|[w, h]| u64::from(*w) * u64::from(*h)).sum();
    let widest = rects.iter().map(|[w, _]| *w).max().unwrap_or(1);
    let square = ((area as f64 * 1.1).sqrt().ceil() as u32).max(widest);
    square.div_ceil(FLUID_CELL_TILE).saturating_mul(FLUID_CELL_TILE).clamp(1, max_width)
}

/// Every reflecting face's cell rectangle packed into one atlas, one texel
/// per cell, with the per-vertex words, the per-slot work tiles, and counts.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FluidCellAtlas {
    pub width: u32,
    pub height: u32,
    /// Indexed by the tiles' `face`.
    pub faces: Vec<FluidCellFaceGpu>,
    /// One per mesh vertex: `[origin, extent, cell_min.x, cell_min.y]`, or
    /// [`FLUID_CELL_NO_ATLAS`] in the first word.
    pub vertex_words: Vec<[u32; 4]>,
    /// Every face's tiles, grouped by draw slot; `slot_tiles[slot]` is the
    /// slot's range. Reflectivity is 0 until the frame patches it.
    pub tiles: Vec<FluidCellTileGpu>,
    pub slot_tiles: Vec<Range<u32>>,
    /// Cells (rect area sums) per slot.
    pub slot_cells: Vec<u64>,
    /// Reflecting faces left on the per-fragment path: a failed fit, or no room.
    pub skipped_faces: u32,
}

impl FluidCellAtlas {
    /// Whether any face has an atlas entry.
    pub fn is_empty(&self) -> bool {
        self.faces.is_empty()
    }

    /// RGBA16F bytes the atlas texture holds.
    pub fn bytes(&self) -> u64 {
        u64::from(self.width) * u64::from(self.height) * 8
    }

    /// Append the tiles of this frame's reflecting slots to `out`, each with
    /// its slot's reflectivity; returns the cells those slots hold.
    pub fn fill_frame_tiles(
        &self,
        reflecting: impl Iterator<Item = (u32, f32)>,
        out: &mut Vec<FluidCellTileGpu>,
    ) -> u64 {
        out.clear();
        let mut cells = 0;
        for (slot, reflectivity) in reflecting {
            let Some(range) = self.slot_tiles.get(slot as usize) else {
                continue;
            };
            cells += self.slot_cells[slot as usize];
            out.extend(
                self.tiles[range.start as usize..range.end as usize]
                    .iter()
                    .map(|tile| FluidCellTileGpu {
                        reflectivity,
                        ..*tile
                    }),
            );
        }
        cells
    }
}

/// Build the atlas for `mesh` (as `pack_fluid_surfaces` packed `volumes`).
/// `probe_indices` holds one probe per mesh vertex; `texture_dimensions` are
/// the installed diffuse sizes the fragment reads by material. A face with
/// no probe gets no entry; a reflecting face whose fit fails or that does not
/// fit keeps the per-fragment path and counts as skipped.
pub fn build_cell_atlas(
    volumes: &[EnvironmentVolumeRecord],
    mesh: &FluidSurfaceMesh,
    probe_indices: &[u32],
    texture_dimensions: &[[u32; 2]],
    material_count: usize,
    max_dimension: u32,
) -> FluidCellAtlas {
    struct Candidate {
        slot: u32,
        probe: u32,
        vertices: Range<usize>,
        map: FluidCellTexelMap,
        cell_min: glam::IVec2,
        extent: glam::UVec2,
        outward: Vec3,
    }
    let mut slot_of_volume = vec![u32::MAX; volumes.len()];
    for (slot, volume) in mesh.volumes.iter().enumerate() {
        if let Some(entry) = slot_of_volume.get_mut(volume.volume_index as usize) {
            *entry = slot as u32;
        }
    }
    let mut atlas = FluidCellAtlas {
        vertex_words: vec![[FLUID_CELL_NO_ATLAS, 0, 0, 0]; mesh.vertices.len()],
        slot_tiles: vec![0..0; mesh.volumes.len()],
        slot_cells: vec![0; mesh.volumes.len()],
        ..Default::default()
    };
    let mut candidates = Vec::new();
    let mut maps = mesh.face_drift_maps.iter();
    for (volume_index, volume) in volumes.iter().enumerate() {
        for face in &volume.surface_faces {
            let Some(Some(map)) = maps.next() else {
                continue;
            };
            let vertices = map.vertex_start as usize..(map.vertex_start + map.vertex_count) as usize;
            let probe = probe_indices
                .get(vertices.start)
                .copied()
                .unwrap_or(FLUID_REFLECTION_NO_PROBE);
            let slot = slot_of_volume[volume_index];
            if probe == FLUID_REFLECTION_NO_PROBE || slot == u32::MAX {
                continue;
            }
            let dims = texture_dimensions
                .get(material_index(face.texture_index, material_count))
                .map(|&[w, h]| Vec2::new(w as f32, h as f32));
            let Some(dims) = dims.filter(|d| d.x > 0.0 && d.y > 0.0) else {
                atlas.skipped_faces += 1;
                continue;
            };
            let positions: Vec<Vec3> = mesh.vertices[vertices.clone()]
                .iter()
                .map(|vertex| Vec3::from(vertex.position))
                .collect();
            // The fragment's own product: `in.uv * dims`.
            let texels: Vec<Vec2> = mesh.vertices[vertices.clone()]
                .iter()
                .map(|vertex| Vec2::from(vertex.uv) * dims)
                .collect();
            let Some(fit) = fit_texel_map(&positions, &texels) else {
                log::warn!(
                    "[Renderer] Fluid cell spike: volume {volume_index} face keeps the \
                     per-fragment reflection (texel map fit failed)"
                );
                atlas.skipped_faces += 1;
                continue;
            };
            let (cell_min, extent) = cell_rect(&texels);
            let outward = Vec3::from(mesh.vertices[vertices.start].normal);
            candidates.push(Candidate {
                slot,
                probe,
                vertices,
                map: fit,
                cell_min,
                extent,
                outward,
            });
        }
    }
    if candidates.is_empty() {
        return atlas;
    }
    let max_dimension = max_dimension.min(FLUID_CELL_HALF_MAX);
    let rects: Vec<[u32; 2]> = candidates.iter().map(|c| c.extent.to_array()).collect();
    let width = atlas_width(&rects, FLUID_CELL_ATLAS_MAX_WIDTH.min(max_dimension));
    let (placements, height) = shelf_pack(&rects, width, max_dimension);
    let mut slot_tiles: Vec<Vec<FluidCellTileGpu>> = vec![Vec::new(); mesh.volumes.len()];
    for (candidate, placement) in candidates.iter().zip(&placements) {
        let Some([x, y]) = *placement else {
            log::warn!(
                "[Renderer] Fluid cell spike: a {}×{}-cell face keeps the per-fragment \
                 reflection (no room in a {width}-wide atlas)",
                candidate.extent.x,
                candidate.extent.y,
            );
            atlas.skipped_faces += 1;
            continue;
        };
        let face_index = atlas.faces.len() as u32;
        let corner = candidate.map.origin
            + candidate.map.u * f64::from(candidate.cell_min.x)
            + candidate.map.v * f64::from(candidate.cell_min.y);
        let atlas_origin = x | (y << 16);
        let extent_word = candidate.extent.x | (candidate.extent.y << 16);
        atlas.faces.push(FluidCellFaceGpu {
            origin: corner.as_vec3().to_array(),
            probe: candidate.probe,
            axis_u: candidate.map.u.as_vec3().to_array(),
            cell_min_x: candidate.cell_min.x,
            axis_v: candidate.map.v.as_vec3().to_array(),
            cell_min_y: candidate.cell_min.y,
            outward: candidate.outward.to_array(),
            atlas_origin,
            extent: candidate.extent.to_array(),
            _pad: [0; 2],
        });
        for word in &mut atlas.vertex_words[candidate.vertices.clone()] {
            *word = [
                atlas_origin,
                extent_word,
                candidate.cell_min.x as u32,
                candidate.cell_min.y as u32,
            ];
        }
        let tiles = &mut slot_tiles[candidate.slot as usize];
        for ty in 0..candidate.extent.y.div_ceil(FLUID_CELL_TILE) {
            for tx in 0..candidate.extent.x.div_ceil(FLUID_CELL_TILE) {
                tiles.push(FluidCellTileGpu {
                    face: face_index,
                    tile: tx | (ty << 16),
                    reflectivity: 0.0,
                    _pad: 0,
                });
            }
        }
        atlas.slot_cells[candidate.slot as usize] +=
            u64::from(candidate.extent.x) * u64::from(candidate.extent.y);
    }
    for (slot, tiles) in slot_tiles.into_iter().enumerate() {
        let start = atlas.tiles.len() as u32;
        atlas.tiles.extend(tiles);
        atlas.slot_tiles[slot] = start..atlas.tiles.len() as u32;
    }
    if !atlas.faces.is_empty() {
        atlas.width = width;
        atlas.height = height.max(1);
    }
    atlas
}

/// The view frustum's six inward planes from a wgpu (z in 0..1)
/// view-projection, normalized: a point is inside when
/// `dot(plane.xyz, p) + plane.w >= 0` for all six.
pub fn frustum_planes(view_proj: Mat4) -> [Vec4; 6] {
    let r = [
        view_proj.row(0),
        view_proj.row(1),
        view_proj.row(2),
        view_proj.row(3),
    ];
    [r[3] + r[0], r[3] - r[0], r[3] + r[1], r[3] - r[1], r[2], r[3] - r[2]].map(|plane| {
        let length = plane.xyz().length();
        if length > 0.0 { plane / length } else { plane }
    })
}

/// Whether a sphere touches the frustum `planes` bound.
pub fn sphere_in_frustum(planes: &[Vec4; 6], centre: Vec3, radius: f32) -> bool {
    planes
        .iter()
        .all(|plane| plane.xyz().dot(centre) + plane.w >= -radius)
}

/// The bounding radius of a cell with texel axes `u`, `v`: half its longer
/// diagonal.
pub fn cell_radius(u: Vec3, v: Vec3) -> f32 {
    0.5 * (u + v).length().max((u - v).length())
}

/// Texels per screen pixel at a cell centre, analytically: the inverse of
/// the texel-to-pixel Jacobian under `view_proj` at `extent` pixels, the
/// longer of its two columns, as the fragment's
/// `max(length(dpdx(uv)·dims), length(dpdy(uv)·dims))`. Flat (past every
/// fade threshold) behind the eye or edge-on. Mirrored by the compute's
/// `fluid_cell_texels_per_pixel`.
pub fn cell_texels_per_pixel(view_proj: Mat4, extent: Vec2, centre: Vec3, u: Vec3, v: Vec3) -> f32 {
    let c = view_proj * centre.extend(1.0);
    if !(c.w > FLUID_CELL_W_EPSILON) {
        return FLUID_CELL_FLAT_TEXELS_PER_PIXEL;
    }
    let cu = view_proj * u.extend(0.0);
    let cv = view_proj * v.extend(0.0);
    let scale = extent * 0.5 / (c.w * c.w);
    let ju = (cu.xy() * c.w - c.xy() * cu.w) * scale;
    let jv = (cv.xy() * c.w - c.xy() * cv.w) * scale;
    let det = ju.x * jv.y - jv.x * ju.y;
    if det * det <= FLUID_CELL_JACOBIAN_EPSILON * ju.length_squared() * jv.length_squared() {
        return FLUID_CELL_FLAT_TEXELS_PER_PIXEL;
    }
    let per_pixel_x = Vec2::new(jv.y, -ju.y) / det;
    let per_pixel_y = Vec2::new(-jv.x, ju.x) / det;
    per_pixel_x.length().max(per_pixel_y.length())
}

/// The dispatch for `tile_count` tiles: workgroups (x, y), x at most
/// `max_per_dimension`; the compute drops `y·x + x ≥ tile_count`.
pub fn cell_dispatch(tile_count: u32, max_per_dimension: u32) -> (u32, u32) {
    if tile_count == 0 {
        return (0, 0);
    }
    let x = tile_count.min(max_per_dimension.max(1));
    (x, tile_count.div_ceil(x))
}

#[cfg(test)]
#[path = "reflection_cells_tests.rs"]
mod tests;
