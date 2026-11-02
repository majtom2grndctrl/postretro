// Coverage proof input: every traced face's directed boundary edges, each with the triangle it bounds.
// See: context/lib/build_pipeline.md §Compiler pipeline step 12 (Exact skips)

use glam::DVec3;

use super::weld::{Bits, Weld, corner_bits, position};
use crate::geometry::{FaceIndexRange, GeometryResult};

/// One directed edge of a traced face's boundary, running as the face winds.
pub(super) struct DirectedEdge {
    /// Its stored corners: every distance is measured from these.
    pub(super) from: DVec3,
    pub(super) to: DVec3,
    /// Its corners' weld representatives: where it sits on its line.
    pub(super) weld_from: DVec3,
    pub(super) weld_to: DVec3,
    /// The edge's face, an index into the geometry's faces.
    pub(super) face: u32,
    /// Index-buffer offset of the triangle the edge bounds.
    pub(super) triangle: usize,
    pub(super) brush: Option<u32>,
}

impl DirectedEdge {
    /// Welded length. Never zero: the weld keeps a triangle's corners
    /// apart.
    pub(super) fn length(&self) -> f64 {
        (self.weld_to - self.weld_from).length()
    }

    /// Welded unit direction: for placing edges on a line only.
    pub(super) fn direction(&self) -> DVec3 {
        (self.weld_to - self.weld_from) / self.length()
    }

    /// Stored unit direction: which way the edge runs. Never zero: corners
    /// sharing a position share bits, so they weld as one and the edge is
    /// left out.
    pub(super) fn stored_direction(&self) -> DVec3 {
        (self.to - self.from).normalize()
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

/// A triangle edge within one face: its welded corners, then its stored
/// corners and triangle.
type FaceEdge = (Bits, Bits, Bits, Bits, usize);

/// Every face's triangle edges, wound as the triangles are, less each pair
/// inside one face that runs both ways between the same two welded corners:
/// a fan diagonal, which lies on both its triangles and so is matched
/// exactly. Edges whose welded corners coincide bound nothing and are left
/// out. Sorted by their stored corners, the corners of the triangle they
/// bound, their brush and their face, so the order depends on the geometry,
/// and only geometry-identical edges fall back to face order.
pub(super) fn boundary_edges(
    geometry: &GeometryResult,
    face_brushes: &[Option<u32>],
    weld: &Weld,
) -> Vec<DirectedEdge> {
    type Key = ([Bits; 2], [Bits; 3], Option<u32>, u32);
    let mut keyed: Vec<(Key, DirectedEdge)> = Vec::new();
    let mut face_edges: Vec<FaceEdge> = Vec::new();
    for (face, range) in geometry.face_index_ranges.iter().enumerate() {
        face_edges.clear();
        for tri in face_triangles(range) {
            let corners = corner_bits(geometry, tri);
            for k in 0..3 {
                let (a, b) = (corners[k], corners[(k + 1) % 3]);
                let (wa, wb) = (weld.of(a), weld.of(b));
                if wa != wb {
                    face_edges.push((wa, wb, a, b, tri));
                }
            }
        }
        cancel_reversed_pairs(&mut face_edges);
        let brush = face_brushes.get(face).copied().flatten();
        for &(wa, wb, a, b, tri) in &face_edges {
            keyed.push((
                ([a, b], corner_bits(geometry, tri), brush, face as u32),
                DirectedEdge {
                    from: position(a),
                    to: position(b),
                    weld_from: position(wa),
                    weld_to: position(wb),
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

/// Drop, between each two welded corners, as many edges running one way as
/// run the other, keeping the surplus direction's last edges in face order.
fn cancel_reversed_pairs(edges: &mut Vec<FaceEdge>) {
    let mut order: Vec<usize> = (0..edges.len()).collect();
    let undirected = |&(a, b, ..): &FaceEdge| if a <= b { (a, b) } else { (b, a) };
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
