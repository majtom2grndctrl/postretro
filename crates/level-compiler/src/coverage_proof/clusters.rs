// Coverage proof candidates: which edges may partner, and the connected groups they form.
// See: context/lib/build_pipeline.md §Compiler pipeline step 12 (Exact skips)

use bvh::aabb::{Aabb, Bounded};
use bvh::bounding_hierarchy::BHShape;
use bvh::bvh::Bvh;
use glam::DVec3;
use nalgebra::Point3;
use rayon::prelude::*;

use super::ITEMS_PER_PERMIT;
use super::edges::DirectedEdge;
use crate::governor::Governor;

/// Two edges run opposite ways when their directions' dot product is at
/// most minus this (120° apart or more). A pair matches only then, so a
/// matched pair never runs the same way, whatever the rounding.
const MIN_OPPOSITION: f64 = 0.5;

/// Read on stored directions, not welded ones. Welding never reverses an
/// edge: its corners lie in different clusters, so each welded end stays
/// nearer its own stored corner than the other's, and the weld turns the
/// edge by less than 90°. But a short edge can turn by nearly that much,
/// so two edges running one way, turned about 85° in opposite senses,
/// would read as opposed welded and could pair.
pub(super) fn opposed(a: &DirectedEdge, b: &DirectedEdge) -> bool {
    a.stored_direction().dot(b.stored_direction()) <= -MIN_OPPOSITION
}

/// The edges sharing a line, near enough: groups linked by opposed,
/// overlapping pairs whose shorter edge lies within `link_radius` of the
/// longer's line, all read at their welded corners. Each group lists its
/// edges ascending; groups come in order of their first edge. Linking only
/// decides which edges a matching considers together, never whether a pair
/// is proven. Each batch of edges links under one `governor` permit.
pub(super) fn clusters(
    edges: &[DirectedEdge],
    link_radius: f64,
    governor: &Governor,
) -> Vec<Vec<usize>> {
    if edges.is_empty() {
        return Vec::new();
    }
    let mut boxes: Vec<EdgeBox> = edges
        .iter()
        .enumerate()
        .map(|(edge, e)| EdgeBox {
            aabb: outward_box(e.weld_from.min(e.weld_to), e.weld_from.max(e.weld_to)),
            edge,
            node_index: 0,
        })
        .collect();
    let index = Bvh::build(&mut boxes);
    let links: Vec<(usize, usize)> = (0..edges.len().div_ceil(ITEMS_PER_PERMIT))
        .into_par_iter()
        .flat_map_iter(|batch| {
            let _permit = governor.enter();
            let first = batch * ITEMS_PER_PERMIT;
            let mut batch_links = Vec::new();
            for i in first..edges.len().min(first + ITEMS_PER_PERMIT) {
                let e = &edges[i];
                let pad = DVec3::splat(link_radius);
                let query = outward_box(
                    e.weld_from.min(e.weld_to) - pad,
                    e.weld_from.max(e.weld_to) + pad,
                );
                let mut found: Vec<usize> = index
                    .traverse(&query, &boxes)
                    .into_iter()
                    .map(|shape| shape.edge)
                    .filter(|&j| j > i && linked(e, &edges[j], link_radius))
                    .collect();
                found.sort_unstable();
                batch_links.extend(found.into_iter().map(|j| (i, j)));
            }
            batch_links
        })
        .collect();

    let mut parent: Vec<usize> = (0..edges.len()).collect();
    for (a, b) in links {
        let (ra, rb) = (root(&mut parent, a), root(&mut parent, b));
        if ra != rb {
            // The lower index roots the group, so roots do not depend on
            // link order.
            let (low, high) = if ra < rb { (ra, rb) } else { (rb, ra) };
            parent[high] = low;
        }
    }
    let mut slot = vec![usize::MAX; edges.len()];
    let mut groups: Vec<Vec<usize>> = Vec::new();
    for edge in 0..edges.len() {
        let r = root(&mut parent, edge);
        if slot[r] == usize::MAX {
            slot[r] = groups.len();
            groups.push(Vec::new());
        }
        groups[slot[r]].push(edge);
    }
    groups
}

fn root(parent: &mut [usize], mut node: usize) -> usize {
    while parent[node] != node {
        parent[node] = parent[parent[node]];
        node = parent[node];
    }
    node
}

/// Opposed, overlapping along each other's lines, and on one line: the
/// shorter edge lies wholly within `radius` of the longer's line. A chain
/// of links can still join two lines into one cluster; that only costs
/// pairs, since every pair is checked in 3D.
fn linked(a: &DirectedEdge, b: &DirectedEdge, radius: f64) -> bool {
    let (long, short) = if a.length() >= b.length() {
        (a, b)
    } else {
        (b, a)
    };
    let axis = long.direction();
    let off_line = |p: DVec3| {
        let v = p - long.weld_from;
        (v - axis * v.dot(axis)).length()
    };
    opposed(a, b)
        && overlaps(a, b)
        && overlaps(b, a)
        && off_line(short.weld_from) <= radius
        && off_line(short.weld_to) <= radius
}

/// `b` projects onto `a`'s line over a stretch of `a` of positive length.
fn overlaps(a: &DirectedEdge, b: &DirectedEdge) -> bool {
    let (axis, length) = (a.direction(), a.length());
    let (t0, t1) = (
        (b.weld_from - a.weld_from).dot(axis),
        (b.weld_to - a.weld_from).dot(axis),
    );
    t0.max(t1).min(length) > t0.min(t1).max(0.0)
}

/// An `f32` box holding the `f64` box `lo`..`hi`, rounded outward.
fn outward_box(lo: DVec3, hi: DVec3) -> Aabb<f32, 3> {
    let down = |v: f64| (v as f32).next_down();
    let up = |v: f64| (v as f32).next_up();
    Aabb::with_bounds(
        Point3::new(down(lo.x), down(lo.y), down(lo.z)),
        Point3::new(up(hi.x), up(hi.y), up(hi.z)),
    )
}

struct EdgeBox {
    aabb: Aabb<f32, 3>,
    edge: usize,
    node_index: usize,
}

impl Bounded<f32, 3> for EdgeBox {
    fn aabb(&self) -> Aabb<f32, 3> {
        self.aabb
    }
}

impl BHShape<f32, 3> for EdgeBox {
    fn set_bh_node_index(&mut self, index: usize) {
        self.node_index = index;
    }

    fn bh_node_index(&self) -> usize {
        self.node_index
    }
}
