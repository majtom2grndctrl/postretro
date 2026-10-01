// Oversize-face cut: a face whose chart exceeds one pool layer becomes sub-faces whose charts window its grid.
// See: context/lib/build_pipeline.md §Compiler pipeline (atlas preparation)

use std::ops::Range;

use glam::{DVec3, Vec3};
use postretro_level_format::geometry::Vertex;

use super::charts::{Chart, ChartWindow};
use crate::chart_raster::CHART_PADDING_TEXELS;
use crate::geometry::{FaceIndexRange, GeometryResult};

/// Texels each sub-chart extends past its cut lines: the bilinear footprint
/// needs one, the second absorbs the fragment snap and vertex UV
/// quantization, so taps near a cut read real surface on both sides.
pub(crate) const CUT_OVERLAP_TEXELS: u32 = 2;

/// One oversize face's cut: grid lines `0 = c_0 < … < c_n = interior` on each
/// chart axis. An uncut axis is `[0, interior]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FaceCut {
    pub face: usize,
    pub u_lines: Vec<u32>,
    pub v_lines: Vec<u32>,
}

impl FaceCut {
    /// Windows (in parent-grid texels) in emission order: rows of `v`, then
    /// `u` within a row.
    pub(crate) fn windows(&self) -> impl Iterator<Item = ([Range<u32>; 2], [usize; 2])> + '_ {
        let u_windows = axis_windows(&self.u_lines);
        let v_windows = axis_windows(&self.v_lines);
        v_windows.into_iter().enumerate().flat_map(move |(vi, v)| {
            u_windows
                .clone()
                .into_iter()
                .enumerate()
                .map(move |(ui, u)| ([u, v.clone()], [ui, vi]))
        })
    }

    /// Sub-charts this cut can emit, at most (windows a polygon misses emit
    /// none).
    #[cfg(test)]
    pub(crate) fn window_count(&self) -> usize {
        (self.u_lines.len() - 1) * (self.v_lines.len() - 1)
    }
}

/// Interior texels a padded window may hold within `pool_edge`.
fn max_window_interior(pool_edge: u32) -> u32 {
    pool_edge - 2 * CHART_PADDING_TEXELS
}

/// Grid lines cutting an axis of `interior` texels into the fewest pieces of
/// near-equal extent whose windows, overlap included, fit `pool_edge`.
pub(crate) fn axis_cut_lines(interior: u32, pool_edge: u32) -> Vec<u32> {
    let max_interior = max_window_interior(pool_edge);
    if interior <= max_interior {
        return vec![0, interior];
    }
    let pieces = (2..)
        .find(|&n: &u32| {
            let longest = interior.div_ceil(n);
            let overlap = if n == 2 { 1 } else { 2 } * CUT_OVERLAP_TEXELS;
            longest + overlap <= max_interior
        })
        .expect("enough pieces always fit");
    (0..=pieces)
        .map(|k| (u64::from(k) * u64::from(interior) / u64::from(pieces)) as u32)
        .collect()
}

/// Each piece's window: its segment widened by the overlap past every inner
/// cut line.
fn axis_windows(lines: &[u32]) -> Vec<Range<u32>> {
    let last = *lines.last().expect("a cut axis has two lines");
    lines
        .windows(2)
        .map(|pair| {
            pair[0].saturating_sub(CUT_OVERLAP_TEXELS)..(pair[1] + CUT_OVERLAP_TEXELS).min(last)
        })
        .collect()
}

/// The cut of every face whose own chart exceeds `pool_edge`, in face order.
/// Degenerate and already-windowed charts are never cut.
pub(crate) fn plan_face_cuts(charts: &[Chart], pool_edge: u32) -> Vec<FaceCut> {
    charts
        .iter()
        .enumerate()
        .filter(|(_, chart)| {
            chart.window.is_none()
                && chart.uv_extent[0] > 0.0
                && chart.uv_extent[1] > 0.0
                && (chart.width_texels > pool_edge || chart.height_texels > pool_edge)
        })
        .map(|(face, chart)| {
            let padding = 2 * CHART_PADDING_TEXELS;
            FaceCut {
                face,
                u_lines: axis_cut_lines(chart.width_texels - padding, pool_edge),
                v_lines: axis_cut_lines(chart.height_texels - padding, pool_edge),
            }
        })
        .collect()
}

/// Padded texel area of the charts once `cuts` apply, every window counted
/// as if its polygon reached it: an upper bound on the cut atlas's area,
/// computed before any geometry is cut.
pub(crate) fn planned_chart_area(charts: &[Chart], cuts: &[FaceCut]) -> u64 {
    let padding = u64::from(2 * CHART_PADDING_TEXELS);
    let mut area: u64 = charts
        .iter()
        .map(|chart| u64::from(chart.width_texels) * u64::from(chart.height_texels))
        .sum();
    for cut in cuts {
        let chart = &charts[cut.face];
        area -= u64::from(chart.width_texels) * u64::from(chart.height_texels);
        area += cut
            .windows()
            .map(|([u, v], _)| {
                (u64::from(u.len() as u32) + padding) * (u64::from(v.len() as u32) + padding)
            })
            .sum::<u64>();
    }
    area
}

/// The faces and charts after a cut. `face_remap[old]` is the range of new
/// face indices that replaced old face `old` (one face when it was not cut,
/// possibly none when every window missed it).
#[derive(Debug)]
pub(crate) struct CutFaces {
    pub face_remap: Vec<Range<usize>>,
}

/// Replace each cut face in `geometry` and `charts` with its sub-faces, at
/// its own slot, in window order. Every face owns its vertices afterwards;
/// uncut faces keep their triangles and attributes. A no-op without cuts.
pub(crate) fn apply_face_cuts(
    geometry: &mut GeometryResult,
    charts: &mut Vec<Chart>,
    cuts: &[FaceCut],
) -> CutFaces {
    let face_count = geometry.face_index_ranges.len();
    if cuts.is_empty() {
        return CutFaces {
            face_remap: (0..face_count).map(|face| face..face + 1).collect(),
        };
    }
    let section = &geometry.geometry;
    let mut vertices: Vec<Vertex> = Vec::with_capacity(section.vertices.len());
    let mut indices: Vec<u32> = Vec::with_capacity(section.indices.len());
    let mut faces = Vec::with_capacity(face_count + cuts.len());
    let mut ranges = Vec::with_capacity(face_count + cuts.len());
    let mut new_charts = Vec::with_capacity(charts.len() + cuts.len());
    let mut face_remap = Vec::with_capacity(face_count);
    let mut cuts = cuts.iter().peekable();

    for face in 0..face_count {
        let range = geometry.face_index_ranges[face];
        let start = range.index_offset as usize;
        let face_indices = &section.indices[start..start + range.index_count as usize];
        let first_new = faces.len();
        let cut = cuts.next_if(|cut| cut.face == face);
        match cut {
            None => {
                // Copy the face, giving it its own vertices in first-use order.
                let mut local: Vec<(u32, u32)> = Vec::new();
                let index_offset = indices.len() as u32;
                for &index in face_indices {
                    let new = match local.iter().find(|(old, _)| *old == index) {
                        Some(&(_, new)) => new,
                        None => {
                            let new = vertices.len() as u32;
                            vertices.push(section.vertices[index as usize].clone());
                            local.push((index, new));
                            new
                        }
                    };
                    indices.push(new);
                }
                faces.push(section.faces[face].clone());
                ranges.push(FaceIndexRange {
                    index_offset,
                    index_count: face_indices.len() as u32,
                });
                new_charts.push(charts[face].clone());
            }
            Some(cut) => {
                let parent = &charts[face];
                let polygon = fan_polygon(face_indices, &section.vertices);
                for (window, piece) in cut.windows() {
                    let clipped = clip_to_piece(&polygon, parent, cut, piece);
                    if clipped.len() < 3 {
                        continue;
                    }
                    let base = vertices.len() as u32;
                    let index_offset = indices.len() as u32;
                    vertices.extend(clipped.iter().cloned());
                    for i in 1..clipped.len() as u32 - 1 {
                        indices.extend([base, base + i, base + i + 1]);
                    }
                    faces.push(section.faces[face].clone());
                    ranges.push(FaceIndexRange {
                        index_offset,
                        index_count: indices.len() as u32 - index_offset,
                    });
                    new_charts.push(sub_chart(parent, &window));
                }
            }
        }
        face_remap.push(first_new..faces.len());
    }

    geometry.geometry.vertices = vertices;
    geometry.geometry.indices = indices;
    geometry.geometry.faces = faces;
    geometry.face_index_ranges = ranges;
    *charts = new_charts;
    CutFaces { face_remap }
}

/// A fan-triangulated face's polygon: `(0, 1, 2), (0, 2, 3), …` back to its
/// vertex loop.
fn fan_polygon(face_indices: &[u32], vertices: &[Vertex]) -> Vec<Vertex> {
    let mut loop_indices = vec![face_indices[0], face_indices[1], face_indices[2]];
    loop_indices.extend(face_indices.chunks_exact(3).skip(1).map(|tri| tri[2]));
    loop_indices
        .into_iter()
        .map(|index| vertices[index as usize].clone())
        .collect()
}

/// The parent chart's grid coordinate of a position along one axis, in
/// texels.
fn grid_coord(parent: &Chart, axis: usize, position: [f32; 3]) -> f64 {
    let interior = [
        f64::from(parent.width_texels - 2 * CHART_PADDING_TEXELS),
        f64::from(parent.height_texels - 2 * CHART_PADDING_TEXELS),
    ];
    let axis_vec = if axis == 0 {
        parent.u_axis
    } else {
        parent.v_axis
    };
    let rel = DVec3::from(Vec3::from(position)) - DVec3::from(parent.origin);
    let local = rel.dot(DVec3::from(axis_vec)) - f64::from(parent.uv_min[axis]);
    local * interior[axis] / f64::from(parent.uv_extent[axis])
}

/// `polygon` clipped to piece `piece`'s cut-line segments (outer chart
/// edges never clip). Edge crossings are computed from the edge's endpoints
/// in a canonical order, so the two faces sharing a cut produce bit-identical
/// vertices on it.
fn clip_to_piece(
    polygon: &[Vertex],
    parent: &Chart,
    cut: &FaceCut,
    piece: [usize; 2],
) -> Vec<Vertex> {
    let mut clipped = polygon.to_vec();
    for axis in 0..2 {
        let lines = if axis == 0 {
            &cut.u_lines
        } else {
            &cut.v_lines
        };
        let (lo, hi) = (lines[piece[axis]], lines[piece[axis] + 1]);
        if lo > 0 {
            clipped = clip_half_plane(&clipped, parent, axis, f64::from(lo), true);
        }
        if hi < *lines.last().expect("two lines") {
            clipped = clip_half_plane(&clipped, parent, axis, f64::from(hi), false);
        }
        if clipped.len() < 3 {
            return Vec::new();
        }
    }
    clipped.dedup_by(|a, b| a.position == b.position);
    if clipped.len() > 1
        && clipped.first().map(|v| v.position) == clipped.last().map(|v| v.position)
    {
        clipped.pop();
    }
    clipped
}

/// Sutherland–Hodgman against grid line `line` on `axis`, keeping the side
/// `>= line` when `keep_above`, else `<= line`.
fn clip_half_plane(
    polygon: &[Vertex],
    parent: &Chart,
    axis: usize,
    line: f64,
    keep_above: bool,
) -> Vec<Vertex> {
    let inside = |v: &Vertex| {
        let c = grid_coord(parent, axis, v.position);
        if keep_above { c >= line } else { c <= line }
    };
    let mut out = Vec::with_capacity(polygon.len() + 1);
    for i in 0..polygon.len() {
        let current = &polygon[i];
        let next = &polygon[(i + 1) % polygon.len()];
        let (current_in, next_in) = (inside(current), inside(next));
        if current_in {
            out.push(current.clone());
        }
        if current_in != next_in {
            out.push(crossing(current, next, parent, axis, line));
        }
    }
    out
}

/// Where edge `a`–`b` crosses grid line `line`, interpolating position and
/// texture UV. Endpoints are ordered by position bits first, so `a`–`b` and
/// `b`–`a` yield the same vertex.
fn crossing(a: &Vertex, b: &Vertex, parent: &Chart, axis: usize, line: f64) -> Vertex {
    let key = |v: &Vertex| v.position.map(f32::to_bits);
    let (p, q) = if key(a) <= key(b) { (a, b) } else { (b, a) };
    let (cp, cq) = (
        grid_coord(parent, axis, p.position),
        grid_coord(parent, axis, q.position),
    );
    let t = ((line - cp) / (cq - cp)).clamp(0.0, 1.0);
    let lerp = |x: f32, y: f32| (f64::from(x) + t * (f64::from(y) - f64::from(x))) as f32;
    let mut vertex = p.clone();
    vertex.position = [
        lerp(p.position[0], q.position[0]),
        lerp(p.position[1], q.position[1]),
        lerp(p.position[2], q.position[2]),
    ];
    vertex.uv = [lerp(p.uv[0], q.uv[0]), lerp(p.uv[1], q.uv[1])];
    vertex
}

/// The sub-chart of `parent` covering `window` (parent-grid texels).
fn sub_chart(parent: &Chart, window: &[Range<u32>; 2]) -> Chart {
    let padding = 2 * CHART_PADDING_TEXELS;
    let interior = [
        parent.width_texels - padding,
        parent.height_texels - padding,
    ];
    let pitch = [
        parent.uv_extent[0] / interior[0] as f32,
        parent.uv_extent[1] / interior[1] as f32,
    ];
    let len = [window[0].len() as u32, window[1].len() as u32];
    Chart {
        uv_min: [
            parent.uv_min[0] + window[0].start as f32 * pitch[0],
            parent.uv_min[1] + window[1].start as f32 * pitch[1],
        ],
        uv_extent: [len[0] as f32 * pitch[0], len[1] as f32 * pitch[1]],
        width_texels: len[0] + padding,
        height_texels: len[1] + padding,
        window: Some(ChartWindow {
            grid_uv_min: parent.uv_min,
            grid_uv_extent: parent.uv_extent,
            grid_interior: interior,
            origin: [window[0].start, window[1].start],
        }),
        ..parent.clone()
    }
}

#[cfg(test)]
mod tests;
