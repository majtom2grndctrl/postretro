// Coverage proof input: every traced face's directed boundary edges, each with the triangle it bounds.
// See: context/lib/build_pipeline.md §Compiler pipeline step 12 (Exact skips)

use glam::DVec3;

use crate::geometry::{FaceIndexRange, GeometryResult};
use crate::ray_traversal::triangle_points;

/// A stored `f32` corner, by its bits.
type Bits = [u32; 3];

/// One directed edge of a traced face's boundary, running as the face winds.
pub(super) struct DirectedEdge {
    pub(super) from: DVec3,
    pub(super) to: DVec3,
    /// The edge's face, an index into the geometry's faces.
    pub(super) face: u32,
    /// Index-buffer offset of the triangle the edge bounds.
    pub(super) triangle: usize,
    pub(super) brush: Option<u32>,
}

impl DirectedEdge {
    pub(super) fn length(&self) -> f64 {
        (self.to - self.from).length()
    }

    /// Unit direction. Edges are never degenerate: their corners differ.
    pub(super) fn direction(&self) -> DVec3 {
        (self.to - self.from) / self.length()
    }
}

/// Index-buffer offsets of a face's triangles.
pub(super) fn face_triangles(range: &FaceIndexRange) -> impl Iterator<Item = usize> {
    let start = range.index_offset as usize;
    let end = start + range.index_count as usize;
    (start..end)
        .step_by(3)
        .take_while(move |&tri| tri + 3 <= end)
}

/// Every face's triangle edges, wound as the triangles are, less each pair
/// inside one face that runs both ways between the same two stored corners:
/// a fan diagonal, which lies on both its triangles and so is matched exactly.
/// Edges with bit-identical corners bound nothing and are left out. Sorted
/// by their corners, the corners of the triangle they bound, their brush and
/// their face, so the order depends on the geometry, and only geometry-identical
/// edges fall back to face order.
pub(super) fn boundary_edges(
    geometry: &GeometryResult,
    face_brushes: &[Option<u32>],
) -> Vec<DirectedEdge> {
    type Key = ([Bits; 2], [Bits; 3], Option<u32>, u32);
    let mut keyed: Vec<(Key, DirectedEdge)> = Vec::new();
    let mut face_edges: Vec<(Bits, Bits, usize)> = Vec::new();
    for (face, range) in geometry.face_index_ranges.iter().enumerate() {
        face_edges.clear();
        for tri in face_triangles(range) {
            let corners = corner_bits(geometry, tri);
            for k in 0..3 {
                let (a, b) = (corners[k], corners[(k + 1) % 3]);
                if a != b {
                    face_edges.push((a, b, tri));
                }
            }
        }
        cancel_reversed_pairs(&mut face_edges);
        let brush = face_brushes.get(face).copied().flatten();
        for &(a, b, tri) in &face_edges {
            let corners = corner_bits(geometry, tri);
            keyed.push((
                ([a, b], corners, brush, face as u32),
                DirectedEdge {
                    from: position(a),
                    to: position(b),
                    face: face as u32,
                    triangle: tri,
                    brush,
                },
            ));
        }
    }
    keyed.sort_by_key(|(key, _)| *key);
    keyed.into_iter().map(|(_, edge)| edge).collect()
}

/// A triangle's stored corners by their bits, `-0.0` read as `0.0`, so two
/// corners share bits exactly when they share a position.
fn corner_bits(geometry: &GeometryResult, tri: usize) -> [Bits; 3] {
    triangle_points(geometry, tri).map(|c| c.to_array().map(|v| (v + 0.0).to_bits()))
}

fn position(bits: Bits) -> DVec3 {
    let [x, y, z] = bits.map(|b| f64::from(f32::from_bits(b)));
    DVec3::new(x, y, z)
}

/// Drop, between each two corners, as many edges running one way as run the
/// other, keeping the surplus direction's last edges in face order.
fn cancel_reversed_pairs(edges: &mut Vec<(Bits, Bits, usize)>) {
    let mut order: Vec<usize> = (0..edges.len()).collect();
    let undirected = |(a, b, _): &(Bits, Bits, usize)| if a <= b { (*a, *b) } else { (*b, *a) };
    order.sort_by_key(|&i| (undirected(&edges[i]), i));
    let mut keep = vec![false; edges.len()];
    let mut group = 0;
    while group < order.len() {
        let key = undirected(&edges[order[group]]);
        let end = group
            + order[group..]
                .iter()
                .take_while(|&&i| undirected(&edges[i]) == key)
                .count();
        let forward: Vec<usize> = order[group..end]
            .iter()
            .copied()
            .filter(|&i| edges[i].0 == key.0)
            .collect();
        let backward: Vec<usize> = order[group..end]
            .iter()
            .copied()
            .filter(|&i| edges[i].0 != key.0)
            .collect();
        let cancelled = forward.len().min(backward.len());
        for &i in forward[cancelled..].iter().chain(&backward[cancelled..]) {
            keep[i] = true;
        }
        group = end;
    }
    let mut index = 0;
    edges.retain(|_| {
        index += 1;
        keep[index - 1]
    });
}
