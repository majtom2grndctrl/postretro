// Fluid surface seam closure, level-wide: near-coincident corners weld to one
// position, then each face gains every other face's vertex lying on its edges,
// so two faces that meet carry bit-identical vertices along their boundary.
// See: context/lib/build_pipeline.md §Compiler pipeline

use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap, HashSet};

use glam::DVec3;

use super::surface::{SPLIT_EPSILON, SurfacePiece};

/// Two points this close, meters, are one; a point this close to a face's
/// boundary lies on it. The split epsilon: the renderer's collinear tolerance
/// and the grid's plane tolerance are derived from it.
pub(super) const SEAM_TOLERANCE: f64 = SPLIT_EPSILON;

/// Points per leaf of the insert candidate tree.
const LEAF_POINTS: usize = 8;

/// What closure did, for the build log and the bound tests.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub(super) struct ClosureStats {
    /// Vertices the weld moved onto another face's corner.
    pub(super) welded: usize,
    /// The furthest the weld moved a vertex, meters.
    pub(super) largest_move: f64,
    /// Vertices inserted on face edges.
    pub(super) inserted: usize,
    /// Vertex-to-vertex distance tests the weld made.
    pub(super) weld_tests: u64,
    /// Vertex-to-edge tests the inserts made.
    pub(super) insert_tests: u64,
}

/// One face vertex: `key` is its position as encoded (f32, widened), which
/// every judgement reads; `out` is what the face emits. A vertex closure
/// leaves alone keeps its f64 position, so its encoded bytes and UV are
/// unchanged; a moved or inserted one emits `key` exactly.
#[derive(Clone, Copy)]
struct Vertex {
    key: DVec3,
    out: DVec3,
}

/// Close every seam between the fluid surface faces of all `volumes`.
///
/// Judged on encoded positions, so closure and any reader of the section agree
/// at the tolerance's edge. First the weld: vertices of different faces within
/// [`SEAM_TOLERANCE`] of each other take one position, one of their own, so no
/// vertex moves further than the tolerance and a chain of near points breaks
/// rather than drifting. Two vertices of one face never weld together, so a
/// face keeps its corners distinct. Then the inserts: each face gains every
/// other face's welded vertex within the tolerance of its boundary, in order
/// round it, repeated on the edges an insert bends until none is left to add.
/// Faces are neither added, dropped nor reordered.
pub(super) fn close_seams(volumes: &mut [Vec<SurfacePiece<'_>>]) -> ClosureStats {
    let mut stats = ClosureStats::default();
    let mut faces: Vec<Vec<Vertex>> = volumes
        .iter()
        .flatten()
        .map(|piece| {
            piece
                .vertices
                .iter()
                .map(|&position| Vertex {
                    key: encoded(position),
                    out: position,
                })
                .collect()
        })
        .collect();
    if faces.len() < 2 {
        return stats;
    }
    weld(&mut faces, &mut stats);
    insert(&mut faces, &mut stats);
    for (piece, face) in volumes.iter_mut().flatten().zip(faces) {
        piece.vertices = face.into_iter().map(|vertex| vertex.out).collect();
    }
    stats
}

/// Distance from `point` to the boundary of `polygon`, meters: closure's own
/// test, for the checks that read its output.
#[cfg(test)]
pub(super) fn boundary_distance(polygon: &[DVec3], point: DVec3) -> f64 {
    (0..polygon.len())
        .map(|edge| segment_distance(polygon[edge], polygon[(edge + 1) % polygon.len()], point))
        .fold(f64::INFINITY, f64::min)
}

/// `position` as the section encodes it.
pub(super) fn encoded(position: DVec3) -> DVec3 {
    position.as_vec3().as_dvec3()
}

fn bits(point: DVec3) -> [u64; 3] {
    point.to_array().map(f64::to_bits)
}

fn point_order(a: DVec3, b: DVec3) -> Ordering {
    a.x.total_cmp(&b.x)
        .then(a.y.total_cmp(&b.y))
        .then(a.z.total_cmp(&b.z))
}

/// A weld cluster: one position, and the faces whose vertex took it.
struct Anchor {
    position: DVec3,
    faces: Vec<usize>,
}

/// Weld vertices of different faces within the tolerance. Vertices are taken
/// in position order; each joins the nearest earlier anchor within the
/// tolerance that holds no vertex of its own face, or becomes an anchor.
fn weld(faces: &mut [Vec<Vertex>], stats: &mut ClosureStats) {
    let mut order: Vec<(usize, usize)> = faces
        .iter()
        .enumerate()
        .flat_map(|(face, vertices)| (0..vertices.len()).map(move |index| (face, index)))
        .collect();
    let key = |&(face, index): &(usize, usize)| faces[face][index].key;
    order.sort_by(|a, b| point_order(key(a), key(b)).then(a.cmp(b)));

    let cell = |point: DVec3| (point / SEAM_TOLERANCE).floor().as_i64vec3().to_array();
    let mut anchors: Vec<Anchor> = Vec::new();
    let mut cells: HashMap<[i64; 3], Vec<usize>> = HashMap::new();
    let mut assigned = Vec::with_capacity(order.len());
    for &(face, index) in &order {
        let point = faces[face][index].key;
        let [x, y, z] = cell(point);
        let mut best: Option<(f64, usize)> = None;
        for neighbour in (-1..=1).flat_map(|dx| {
            (-1..=1).flat_map(move |dy| (-1..=1).map(move |dz| [x + dx, y + dy, z + dz]))
        }) {
            for &anchor in cells.get(&neighbour).into_iter().flatten() {
                stats.weld_tests += 1;
                let distance = anchors[anchor].position.distance(point);
                if distance <= SEAM_TOLERANCE
                    && !anchors[anchor].faces.contains(&face)
                    && best.is_none_or(|(d, a)| (distance, anchor) < (d, a))
                {
                    best = Some((distance, anchor));
                }
            }
        }
        let anchor = match best {
            Some((_, anchor)) => anchor,
            None => {
                anchors.push(Anchor {
                    position: point,
                    faces: Vec::new(),
                });
                cells
                    .entry(cell(point))
                    .or_default()
                    .push(anchors.len() - 1);
                anchors.len() - 1
            }
        };
        anchors[anchor].faces.push(face);
        assigned.push(anchor);
    }
    for (&(face, index), anchor) in order.iter().zip(assigned) {
        let position = anchors[anchor].position;
        let vertex = &mut faces[face][index];
        if bits(position) != bits(vertex.key) {
            stats.largest_move = stats.largest_move.max(position.distance(vertex.key));
            *vertex = Vertex {
                key: position,
                out: position,
            };
            stats.welded += 1;
        }
    }
}

/// Insert in every face each other face's vertex within the tolerance of its
/// boundary, until no face gains one.
fn insert(faces: &mut [Vec<Vertex>], stats: &mut ClosureStats) {
    let mut points: Vec<DVec3> = faces.iter().flatten().map(|vertex| vertex.key).collect();
    points.sort_by(|a, b| point_order(*a, *b));
    points.dedup_by(|a, b| bits(*a) == bits(*b));
    let tree = PointTree::build(&points);

    let mut pending: Vec<usize> = (0..faces.len()).collect();
    let mut candidates = Vec::new();
    while !pending.is_empty() {
        let mut changed = Vec::new();
        for &face in &pending {
            if let Some(ring) =
                gain_boundary_points(&faces[face], &points, &tree, &mut candidates, stats)
            {
                faces[face] = ring;
                changed.push(face);
            }
        }
        pending = changed;
    }
}

/// `ring` with every point within the tolerance of its boundary that is not
/// already one of its vertices, or `None` when it gains none.
///
/// A point near two edges, as beside an acute corner or on a sliver, joins
/// the one whose sector about the face's area centroid holds it, so the
/// boundary keeps turning one way round that centroid; failing that, the
/// nearer. Within an edge, points take their order round the centroid.
fn gain_boundary_points(
    ring: &[Vertex],
    points: &[DVec3],
    tree: &PointTree<'_>,
    candidates: &mut Vec<usize>,
    stats: &mut ClosureStats,
) -> Option<Vec<Vertex>> {
    let own: HashSet<[u64; 3]> = ring.iter().map(|vertex| bits(vertex.key)).collect();
    // Each gained point with every edge it lies near, as (edge, distance).
    let mut near: BTreeMap<usize, Vec<(usize, f64)>> = BTreeMap::new();
    for edge in 0..ring.len() {
        let (start, end) = (ring[edge].key, ring[(edge + 1) % ring.len()].key);
        tree.near_segment(start, end, candidates);
        for &candidate in candidates.iter() {
            stats.insert_tests += 1;
            let point = points[candidate];
            if own.contains(&bits(point)) {
                continue;
            }
            let distance = segment_distance(start, end, point);
            if distance <= SEAM_TOLERANCE {
                near.entry(candidate).or_default().push((edge, distance));
            }
        }
    }
    if near.is_empty() {
        return None;
    }

    let keys: Vec<DVec3> = ring.iter().map(|vertex| vertex.key).collect();
    let (centre, normal) = centroid_and_normal(&keys);
    let angle = |point: DVec3, from: DVec3| {
        let (a, b) = (from - centre, point - centre);
        a.cross(b).dot(normal).atan2(a.dot(b) * normal.length())
    };
    // (edge, angle past the edge's start, point)
    let mut inserts: Vec<(usize, f64, DVec3)> = near
        .into_iter()
        .map(|(candidate, edges)| {
            let point = points[candidate];
            let in_sector = |&(edge, _): &(usize, f64)| {
                let (start, end) = (keys[edge], keys[(edge + 1) % keys.len()]);
                angle(point, start) >= 0.0 && angle(end, point) > 0.0
            };
            let (edge, _) = edges
                .iter()
                .copied()
                .find(in_sector)
                .or_else(|| {
                    edges
                        .iter()
                        .copied()
                        .min_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)))
                })
                .expect("a gained point lies near some edge");
            (edge, angle(point, keys[edge]), point)
        })
        .collect();
    inserts.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then(a.1.total_cmp(&b.1))
            .then(point_order(a.2, b.2))
    });
    stats.inserted += inserts.len();

    let mut inserts = inserts.into_iter().peekable();
    let mut gained = Vec::with_capacity(ring.len() + inserts.len());
    for (edge, &corner) in ring.iter().enumerate() {
        gained.push(corner);
        while let Some((_, _, point)) = inserts.next_if(|insert| insert.0 == edge) {
            gained.push(Vertex {
                key: point,
                out: point,
            });
        }
    }
    Some(gained)
}

/// Distance from `point` to the closed segment `start`→`end`, meters.
pub(super) fn segment_distance(start: DVec3, end: DVec3, point: DVec3) -> f64 {
    let direction = end - start;
    let length_squared = direction.length_squared();
    let along = if length_squared == 0.0 {
        0.0
    } else {
        ((point - start).dot(direction) / length_squared).clamp(0.0, 1.0)
    };
    point.distance(start + direction * along)
}

/// The area centroid of `polygon` and its Newell normal, which points the way
/// the ring winds counter-clockwise round. Positions are taken relative to the
/// first vertex, so precision holds far from the origin. A polygon with no
/// area takes its vertex mean.
pub(super) fn centroid_and_normal(polygon: &[DVec3]) -> (DVec3, DVec3) {
    let first = polygon[0];
    let mut normal = DVec3::ZERO;
    for (i, &a) in polygon.iter().enumerate() {
        normal += (a - first).cross(polygon[(i + 1) % polygon.len()] - first);
    }
    let (mut weighted, mut weight) = (DVec3::ZERO, 0.0);
    for pair in polygon[1..].windows(2) {
        let (a, b) = (pair[0] - first, pair[1] - first);
        let twice_area = a.cross(b).dot(normal);
        weighted += twice_area * (a + b) / 3.0;
        weight += twice_area;
    }
    if weight > 0.0 && weighted.is_finite() {
        return (first + weighted / weight, normal);
    }
    let sum: DVec3 = polygon.iter().map(|&point| point - first).sum();
    (first + sum / polygon.len() as f64, normal)
}

/// A static bounding-volume tree over points, answering which points may lie
/// within the tolerance of a segment.
struct PointTree<'a> {
    points: &'a [DVec3],
    /// Point indices, grouped so each node covers a contiguous range.
    order: Vec<u32>,
    nodes: Vec<Node>,
}

struct Node {
    min: DVec3,
    max: DVec3,
    /// Range of `order` the node covers.
    start: u32,
    end: u32,
    /// Child node indices, or `None` for a leaf.
    children: Option<[u32; 2]>,
}

impl<'a> PointTree<'a> {
    fn build(points: &'a [DVec3]) -> Self {
        let mut tree = Self {
            points,
            order: (0..points.len() as u32).collect(),
            nodes: Vec::new(),
        };
        if !points.is_empty() {
            tree.build_node(0, points.len());
        }
        tree
    }

    fn build_node(&mut self, start: usize, end: usize) -> u32 {
        let (mut min, mut max) = (DVec3::INFINITY, DVec3::NEG_INFINITY);
        for &index in &self.order[start..end] {
            let point = self.points[index as usize];
            (min, max) = (min.min(point), max.max(point));
        }
        let node = self.nodes.len() as u32;
        self.nodes.push(Node {
            min,
            max,
            start: start as u32,
            end: end as u32,
            children: None,
        });
        if end - start > LEAF_POINTS {
            let extent = max - min;
            let axis = if extent.x >= extent.y && extent.x >= extent.z {
                0
            } else if extent.y >= extent.z {
                1
            } else {
                2
            };
            let middle = start + (end - start) / 2;
            let points = self.points;
            self.order[start..end].select_nth_unstable_by(middle - start, |&a, &b| {
                points[a as usize][axis]
                    .total_cmp(&points[b as usize][axis])
                    .then(a.cmp(&b))
            });
            let left = self.build_node(start, middle);
            let right = self.build_node(middle, end);
            self.nodes[node as usize].children = Some([left, right]);
        }
        node
    }

    /// Every point whose box, grown by the tolerance, the segment crosses: a
    /// superset of the points within the tolerance of it.
    fn near_segment(&self, start: DVec3, end: DVec3, found: &mut Vec<usize>) {
        found.clear();
        if self.nodes.is_empty() {
            return;
        }
        let margin = DVec3::splat(SEAM_TOLERANCE);
        let mut stack = vec![0u32];
        while let Some(node) = stack.pop() {
            let node = &self.nodes[node as usize];
            if !segment_crosses_box(start, end, node.min - margin, node.max + margin) {
                continue;
            }
            match node.children {
                Some([left, right]) => stack.extend([right, left]),
                None => found.extend(
                    self.order[node.start as usize..node.end as usize]
                        .iter()
                        .map(|&index| index as usize),
                ),
            }
        }
    }
}

/// Whether segment `start`→`end` meets the closed box `min`..`max`.
fn segment_crosses_box(start: DVec3, end: DVec3, min: DVec3, max: DVec3) -> bool {
    let direction = end - start;
    let (mut enter, mut leave) = (0.0f64, 1.0f64);
    for axis in 0..3 {
        if direction[axis] == 0.0 {
            if start[axis] < min[axis] || start[axis] > max[axis] {
                return false;
            }
            continue;
        }
        let inverse = 1.0 / direction[axis];
        let (mut near, mut far) = (
            (min[axis] - start[axis]) * inverse,
            (max[axis] - start[axis]) * inverse,
        );
        if near > far {
            std::mem::swap(&mut near, &mut far);
        }
        enter = enter.max(near);
        leave = leave.min(far);
        if enter > leave {
            return false;
        }
    }
    true
}
