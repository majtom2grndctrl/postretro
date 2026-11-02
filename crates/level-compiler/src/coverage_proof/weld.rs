// Coverage proof weld: copies of one corner, from different triangles and no more than δ apart, merged into one vertex.
// See: context/lib/build_pipeline.md §Compiler pipeline step 12 (Exact skips)

use std::collections::HashMap;

use glam::DVec3;

use super::edges::face_triangles;
use crate::geometry::GeometryResult;
use crate::ray_traversal::triangle_points;

/// A stored `f32` corner by its bits, `-0.0` read as `0.0`, so two corners
/// share bits exactly when they share a position.
pub(super) type Bits = [u32; 3];

/// A triangle's stored corners by their bits.
pub(super) fn corner_bits(geometry: &GeometryResult, tri: usize) -> [Bits; 3] {
    triangle_points(geometry, tri).map(|c| c.to_array().map(|v| (v + 0.0).to_bits()))
}

pub(super) fn position(bits: Bits) -> DVec3 {
    let [x, y, z] = bits.map(|b| f64::from(f32::from_bits(b)));
    DVec3::new(x, y, z)
}

/// How corners weld.
#[derive(Clone, Copy)]
pub(super) struct WeldRule {
    /// δ: corners this close (3D, `f64`) link. Zero turns the weld off.
    pub(super) radius: f64,
    /// Refuse a cluster holding two corners of one triangle, so no edge
    /// collapses and no face folds away. Off only in a negative control.
    pub(super) keep_triangles_whole: bool,
}

/// Why a cluster of linked corners welds or stays apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Verdict {
    Welded,
    /// Its diameter exceeds δ: linking chained along a line.
    TooWide,
    /// It holds two corners of one triangle.
    SharesTriangle,
}

/// A connected group of two or more corners linked within δ.
pub(super) struct Cluster {
    /// Corner bit keys, ascending: the first is the representative.
    pub(super) members: Vec<Bits>,
    /// Largest distance between two members.
    pub(super) diameter: f64,
    pub(super) verdict: Verdict,
}

/// Each welded corner's representative. Corners outside an accepted
/// cluster map to themselves.
pub(super) struct Weld {
    representative: HashMap<Bits, Bits>,
}

impl Weld {
    pub(super) fn new(geometry: &GeometryResult, rule: WeldRule) -> Self {
        let mut representative = HashMap::new();
        let (mut widest, mut too_wide, mut shares) = (0.0f64, 0, 0);
        let clusters = clusters(geometry, rule);
        for cluster in &clusters {
            match cluster.verdict {
                Verdict::Welded => {
                    widest = widest.max(cluster.diameter);
                    let first = cluster.members[0];
                    for &member in &cluster.members[1..] {
                        representative.insert(member, first);
                    }
                }
                Verdict::TooWide => too_wide += 1,
                Verdict::SharesTriangle => shares += 1,
            }
        }
        log::debug!(
            "[Compiler] Coverage proof weld: {} corner cluster(s) welded within {:.1} µm (widest {:.3} µm), {too_wide} refused as wider, {shares} refused as holding two corners of one triangle",
            clusters.len() - too_wide - shares,
            rule.radius * 1e6,
            widest * 1e6,
        );
        Self { representative }
    }

    pub(super) fn of(&self, corner: Bits) -> Bits {
        self.representative.get(&corner).copied().unwrap_or(corner)
    }
}

/// Every group of two or more distinct stored corners linked, pair by pair,
/// within `rule.radius`: single linkage over all pairs, so the groups do
/// not depend on face or corner order. Each carries its verdict.
pub(super) fn clusters(geometry: &GeometryResult, rule: WeldRule) -> Vec<Cluster> {
    if rule.radius <= 0.0 {
        return Vec::new();
    }
    // Every (corner, triangle) incidence, by corner bits then triangle.
    let mut incidence: Vec<(Bits, usize)> = geometry
        .face_index_ranges
        .iter()
        .flat_map(face_triangles)
        .flat_map(|tri| corner_bits(geometry, tri).map(|bits| (bits, tri)))
        .collect();
    incidence.sort_unstable();
    incidence.dedup();
    let mut corners: Vec<Bits> = incidence.iter().map(|&(bits, _)| bits).collect();
    corners.dedup();
    let points: Vec<DVec3> = corners.iter().map(|&bits| position(bits)).collect();

    let cell = |p: DVec3| (p / rule.radius).floor().to_array().map(|c| c as i64);
    let mut grid: HashMap<[i64; 3], Vec<usize>> = HashMap::new();
    for (id, &p) in points.iter().enumerate() {
        grid.entry(cell(p)).or_default().push(id);
    }
    let within = |a: usize, b: usize| (points[a] - points[b]).length() <= rule.radius;
    let mut parent: Vec<usize> = (0..corners.len()).collect();
    for (id, &p) in points.iter().enumerate() {
        let [x, y, z] = cell(p);
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    let Some(near) = grid.get(&[x + dx, y + dy, z + dz]) else {
                        continue;
                    };
                    for &other in near {
                        if other > id && within(id, other) {
                            union(&mut parent, id, other);
                        }
                    }
                }
            }
        }
    }

    let mut groups: HashMap<usize, Vec<usize>> = HashMap::new();
    for id in 0..corners.len() {
        let r = root(&mut parent, id);
        groups.entry(r).or_default().push(id);
    }
    let mut out: Vec<Cluster> = groups
        .into_values()
        .filter(|members| members.len() > 1)
        .map(|mut members| {
            members.sort_unstable();
            let diameter = diameter(&points, &members);
            let verdict = if diameter > rule.radius {
                Verdict::TooWide
            } else if rule.keep_triangles_whole && shares_triangle(&incidence, &corners, &members) {
                Verdict::SharesTriangle
            } else {
                Verdict::Welded
            };
            Cluster {
                members: members.iter().map(|&id| corners[id]).collect(),
                diameter,
                verdict,
            }
        })
        .collect();
    out.sort_unstable_by_key(|cluster| cluster.members[0]);
    out
}

fn diameter(points: &[DVec3], members: &[usize]) -> f64 {
    let mut widest = 0.0f64;
    for (k, &a) in members.iter().enumerate() {
        for &b in &members[k + 1..] {
            widest = widest.max((points[a] - points[b]).length());
        }
    }
    widest
}

/// Two members are corners of one triangle.
fn shares_triangle(incidence: &[(Bits, usize)], corners: &[Bits], members: &[usize]) -> bool {
    let mut triangles: Vec<usize> = members
        .iter()
        .flat_map(|&id| {
            let bits = corners[id];
            let start = incidence.partition_point(|&(b, _)| b < bits);
            incidence[start..]
                .iter()
                .take_while(move |&&(b, _)| b == bits)
                .map(|&(_, tri)| tri)
        })
        .collect();
    let all = triangles.len();
    triangles.sort_unstable();
    triangles.dedup();
    triangles.len() != all
}

fn union(parent: &mut [usize], a: usize, b: usize) {
    let (ra, rb) = (root(parent, a), root(parent, b));
    if ra != rb {
        parent[ra.max(rb)] = ra.min(rb);
    }
}

fn root(parent: &mut [usize], mut node: usize) -> usize {
    while parent[node] != node {
        parent[node] = parent[parent[node]];
        node = parent[node];
    }
    node
}
