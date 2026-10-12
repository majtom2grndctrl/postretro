// AABB tree over the static world brushes, so each fluid face tests only the
// solids near it. See: context/lib/build_pipeline.md §Compiler pipeline

use crate::partition::Aabb;

/// Brushes per leaf before a node splits.
const LEAF_SIZE: usize = 4;

enum Node {
    Leaf {
        bounds: Aabb,
        start: usize,
        end: usize,
    },
    Inner {
        bounds: Aabb,
        left: usize,
        right: usize,
    },
}

impl Node {
    fn bounds(&self) -> &Aabb {
        match self {
            Node::Leaf { bounds, .. } | Node::Inner { bounds, .. } => bounds,
        }
    }
}

/// Median-split bounding-volume tree over brush AABBs. Built once per compile;
/// a query returns brush indices in ascending order, independent of tree shape.
pub(super) struct SolidIndex {
    nodes: Vec<Node>,
    /// Brush indices, permuted so each leaf owns a contiguous run.
    order: Vec<usize>,
    boxes: Vec<Aabb>,
}

impl SolidIndex {
    pub(super) fn build(boxes: Vec<Aabb>) -> Self {
        let mut index = Self {
            nodes: Vec::new(),
            order: (0..boxes.len()).collect(),
            boxes,
        };
        if !index.order.is_empty() {
            index.build_node(0, index.order.len());
        }
        index
    }

    fn build_node(&mut self, start: usize, end: usize) -> usize {
        let mut bounds = Aabb::empty();
        let mut centroids = Aabb::empty();
        for &brush in &self.order[start..end] {
            bounds.expand_aabb(&self.boxes[brush]);
            centroids.expand_point(self.boxes[brush].centroid());
        }
        let slot = self.nodes.len();
        if end - start <= LEAF_SIZE {
            self.nodes.push(Node::Leaf { bounds, start, end });
            return slot;
        }
        let extent = centroids.max - centroids.min;
        let axis = if extent.x >= extent.y && extent.x >= extent.z {
            0
        } else if extent.y >= extent.z {
            1
        } else {
            2
        };
        let mid = start + (end - start) / 2;
        let boxes = &self.boxes;
        self.order[start..end].select_nth_unstable_by(mid - start, |a, b| {
            boxes[*a].centroid()[axis].total_cmp(&boxes[*b].centroid()[axis])
        });
        // Reserve the slot; children are pushed after it.
        self.nodes.push(Node::Leaf {
            bounds: Aabb::empty(),
            start,
            end,
        });
        let left = self.build_node(start, mid);
        let right = self.build_node(mid, end);
        self.nodes[slot] = Node::Inner {
            bounds,
            left,
            right,
        };
        slot
    }

    /// Indices of every brush whose AABB touches `query`, ascending.
    pub(super) fn overlapping(&self, query: &Aabb, out: &mut Vec<usize>) {
        out.clear();
        if self.nodes.is_empty() {
            return;
        }
        let mut stack = vec![0usize];
        while let Some(slot) = stack.pop() {
            let node = &self.nodes[slot];
            // Closed-interval overlap: boxes sharing only a face still touch,
            // which is the flush case the surface stage must see.
            if !node.bounds().intersects(query) {
                continue;
            }
            match node {
                Node::Leaf { start, end, .. } => out.extend(
                    self.order[*start..*end]
                        .iter()
                        .copied()
                        .filter(|&brush| self.boxes[brush].intersects(query)),
                ),
                Node::Inner { left, right, .. } => {
                    stack.push(*left);
                    stack.push(*right);
                }
            }
        }
        out.sort_unstable();
    }
}
