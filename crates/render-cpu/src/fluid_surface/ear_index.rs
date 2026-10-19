// Fluid surface ear clip containment index: which reflex ring vertices lie
// inside a candidate ear, over a static tree of the ring's packed positions.
// See: context/lib/rendering_pipeline.md §7.3a

use glam::DVec3;

/// Most slots a leaf holds.
const LEAF_SLOTS: usize = 8;
/// How far past an ear edge's line, meters, a slot still counts as on the
/// edge, and a node's box is still searched: far above the rounding of an
/// offset computed in f64 from f32 positions, far below any vertex spacing,
/// so a slot on the edge, its chord included, is never missed.
const EDGE_SLACK: f64 = 1.0e-9;

/// One node of the tree: slots `start..end` of [`ReflexIndex::order`], their
/// bounding box, and its two halves unless a leaf.
#[derive(Clone, Copy)]
struct Node {
    start: usize,
    end: usize,
    min: DVec3,
    max: DVec3,
    halves: Option<(usize, usize)>,
}

/// A static median-split tree over every slot of a ring, each slot marked
/// active or not; the ear clipper keeps exactly its live reflex slots active.
/// Positions never move, so the tree is built once, in n log n of the ring's
/// slots; marking a slot walks one root-to-leaf path, log n; a search skips
/// every node holding no active slot.
pub(super) struct ReflexIndex {
    /// Slots in tree order: each node holds a contiguous range.
    order: Vec<usize>,
    /// Each slot's place in `order`.
    rank: Vec<usize>,
    nodes: Vec<Node>,
    /// Active slots under each node.
    active_under: Vec<u32>,
    active: Vec<bool>,
    /// Nodes still to visit, kept across searches.
    stack: Vec<usize>,
}

impl ReflexIndex {
    /// The tree over `points`, one per slot, with no slot active.
    pub(super) fn new(points: &[DVec3]) -> Self {
        let mut index = Self {
            order: (0..points.len()).collect(),
            rank: vec![0; points.len()],
            nodes: Vec::new(),
            active_under: Vec::new(),
            active: vec![false; points.len()],
            stack: Vec::new(),
        };
        index.build(points, 0, points.len());
        for (place, &slot) in index.order.iter().enumerate() {
            index.rank[slot] = place;
        }
        index
    }

    /// Builds the node over `order[start..end]`, splitting at the median of
    /// its box's longest axis; returns its index.
    fn build(&mut self, points: &[DVec3], start: usize, end: usize) -> usize {
        super::count_install_steps(end - start);
        let (min, max) = self.order[start..end].iter().fold(
            (DVec3::INFINITY, DVec3::NEG_INFINITY),
            |(min, max), &slot| (min.min(points[slot]), max.max(points[slot])),
        );
        let node = self.nodes.len();
        self.nodes.push(Node {
            start,
            end,
            min,
            max,
            halves: None,
        });
        self.active_under.push(0);
        if end - start > LEAF_SLOTS {
            let extent = max - min;
            let axis = (0..3)
                .max_by(|&a, &b| extent[a].total_cmp(&extent[b]))
                .expect("three axes");
            let middle = start + (end - start) / 2;
            self.order[start..end].select_nth_unstable_by(middle - start, |&a, &b| {
                points[a][axis].total_cmp(&points[b][axis])
            });
            let lower = self.build(points, start, middle);
            let upper = self.build(points, middle, end);
            self.nodes[node].halves = Some((lower, upper));
        }
        node
    }

    /// Whether any slot is active.
    pub(super) fn any_active(&self) -> bool {
        self.active_under[0] > 0
    }

    /// Marks `slot` active or not.
    pub(super) fn set_active(&mut self, slot: usize, active: bool) {
        if self.active[slot] == active {
            return;
        }
        self.active[slot] = active;
        let rank = self.rank[slot];
        let mut node = 0;
        loop {
            super::count_install_steps(1);
            if active {
                self.active_under[node] += 1;
            } else {
                self.active_under[node] -= 1;
            }
            match self.nodes[node].halves {
                Some((lower, upper)) => {
                    node = if rank < self.nodes[lower].end {
                        lower
                    } else {
                        upper
                    };
                }
                None => break,
            }
        }
    }

    /// An active slot inside the closed triangle on slots `ear`, wound CCW
    /// about unit `normal` and judged in its plane, other than the ear's own
    /// slots and any slot repeating one of their positions; `None` when there
    /// is none. A slot on an edge, within [`EDGE_SLACK`], counts as inside.
    pub(super) fn inside(
        &mut self,
        ear: [usize; 3],
        points: &[DVec3],
        normal: DVec3,
    ) -> Option<usize> {
        let corners = ear.map(|slot| points[slot]);
        // Each edge's start, outward in-plane normal (as long as the edge)
        // and slack along it: a point is inside the edge where its offset
        // along the normal is at most the slack.
        let edges: [(DVec3, DVec3, f64); 3] = std::array::from_fn(|i| {
            let (from, to) = (corners[i], corners[(i + 1) % 3]);
            let outward = (to - from).cross(normal);
            (from, outward, EDGE_SLACK * outward.length())
        });
        let low = corners[0].min(corners[1]).min(corners[2]);
        let high = corners[0].max(corners[1]).max(corners[2]);
        self.stack.clear();
        self.stack.push(0);
        while let Some(node) = self.stack.pop() {
            super::count_install_steps(1);
            let Node {
                start,
                end,
                min,
                max,
                halves,
            } = self.nodes[node];
            if self.active_under[node] == 0
                || min.cmpgt(high).any()
                || max.cmplt(low).any()
                || edges.iter().any(|&(from, outward, slack)| {
                    // The box's least offset along the edge's outward normal.
                    let nearest = DVec3::select(outward.cmpgt(DVec3::ZERO), min, max);
                    (nearest - from).dot(outward) > slack
                })
            {
                continue;
            }
            if let Some((lower, upper)) = halves {
                self.stack.extend([lower, upper]);
                continue;
            }
            for &slot in &self.order[start..end] {
                super::count_install_steps(1);
                let point = points[slot];
                if self.active[slot]
                    && !ear.contains(&slot)
                    && !corners.contains(&point)
                    && edges
                        .iter()
                        .all(|&(from, outward, slack)| (point - from).dot(outward) <= slack)
                {
                    return Some(slot);
                }
            }
        }
        None
    }
}
