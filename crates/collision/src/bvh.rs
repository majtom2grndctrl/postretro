// Load-time bounding-volume tree over axis-aligned entry bounds, generic over the leaf payload.
// See: context/plans/in-progress/E24--water-and-environment-volumes/research.md §Bake versus load-time build

use glam::Vec3;

/// A node holding at most this many entries is a leaf; a larger one is split.
const MAX_LEAF_ENTRIES: usize = 4;

/// Binned-SAH bins per axis.
const SAH_BINS: usize = 16;

/// Deepest node the builder creates (the root is depth 0). A node at this depth is
/// a leaf whatever its entry count, so the traversal stack bound below always holds.
const MAX_DEPTH: usize = 64;

/// From this depth on the builder splits at the median entry instead of by SAH.
/// A median split halves the entry count, so the remaining
/// `MAX_DEPTH - MEDIAN_SPLIT_DEPTH` levels resolve 2^24 entries per node down to
/// leaf size; SAH alone can produce a chain of one-entry peels on adversarial
/// input.
const MEDIAN_SPLIT_DEPTH: usize = 40;

/// Traversal stack capacity. Popping a node at depth `d` leaves at most one pending
/// sibling per depth `1..=d` on the stack, then pushes two children; internal nodes
/// sit at depth at most `MAX_DEPTH - 1`, so the stack never holds more than
/// `MAX_DEPTH + 1` entries.
const STACK_CAPACITY: usize = MAX_DEPTH + 1;

/// One leaf entry: closed bounds plus the payload a query hands back.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct TreeEntry<P> {
    pub min: Vec3,
    pub max: Vec3,
    pub payload: P,
}

impl<P> TreeEntry<P> {
    fn centroid(&self) -> Vec3 {
        (self.min + self.max) * 0.5
    }
}

/// A node's bounds enclose every entry beneath it. `count == 0` marks an internal
/// node whose children are `first` and `first + 1`; otherwise the node is a leaf
/// over `entries[first..first + count]`.
#[derive(Debug, Clone, Copy, PartialEq)]
struct TreeNode {
    min: Vec3,
    max: Vec3,
    first: u32,
    count: u32,
}

/// A bounding-volume tree built once from its entries. Queries walk it on a
/// fixed-size stack and never allocate; an entry is visited when its own bounds
/// pass the query's overlap test, so the visited set never depends on tree shape.
/// Distinct from the render BVH (PRL section 19): this one is built at load and
/// holds arbitrary payloads.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct AabbTree<P> {
    nodes: Vec<TreeNode>,
    entries: Vec<TreeEntry<P>>,
}

impl<P> AabbTree<P> {
    /// A tree over no entries; every query visits nothing.
    pub(crate) const fn empty() -> Self {
        Self {
            nodes: Vec::new(),
            entries: Vec::new(),
        }
    }

    /// Build over `entries` with a binned surface-area-heuristic split. The build
    /// is a pure function of the entries and their order, so every peer builds the
    /// same tree. Bounds should be finite; a non-finite entry is still placed but
    /// may never be visited.
    pub(crate) fn build(mut entries: Vec<TreeEntry<P>>) -> Self {
        if entries.is_empty() {
            return Self::empty();
        }
        u32::try_from(entries.len().saturating_mul(2))
            .expect("environment volume tree: entry count exceeds u32 node indexing");
        let mut nodes = Vec::with_capacity(entries.len() * 2);
        nodes.push(placeholder_node());
        let len = entries.len();
        let deepest = build_node(&mut nodes, &mut entries, 0, 0, len, 0);
        debug_assert!(deepest <= MAX_DEPTH, "builder exceeded MAX_DEPTH");
        Self { nodes, entries }
    }

    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    /// Visit every entry whose closed bounds contain `point`.
    pub(crate) fn visit_point(&self, point: Vec3, visit: impl FnMut(&P)) {
        self.visit_overlapping(
            |min, max| point.cmpge(min).all() && point.cmple(max).all(),
            visit,
        );
    }

    /// Visit every entry whose closed bounds meet the line segment
    /// `origin + t * dir` for `t` in `[t_min, t_max]`. `dir` need not be unit
    /// length; a zero `dir` tests the point `origin`.
    pub(crate) fn visit_line(
        &self,
        origin: Vec3,
        dir: Vec3,
        t_min: f32,
        t_max: f32,
        visit: impl FnMut(&P),
    ) {
        self.visit_overlapping(
            |min, max| line_overlaps_box(origin, dir, t_min, t_max, min, max),
            visit,
        );
    }

    fn visit_overlapping(&self, overlaps: impl Fn(Vec3, Vec3) -> bool, mut visit: impl FnMut(&P)) {
        if self.nodes.is_empty() {
            return;
        }
        let mut stack = [0u32; STACK_CAPACITY];
        let mut len = 1;
        while len > 0 {
            len -= 1;
            let node = &self.nodes[stack[len] as usize];
            if !overlaps(node.min, node.max) {
                continue;
            }
            if node.count > 0 {
                let first = node.first as usize;
                for entry in &self.entries[first..first + node.count as usize] {
                    if overlaps(entry.min, entry.max) {
                        visit(&entry.payload);
                    }
                }
            } else {
                // Indexing past STACK_CAPACITY would panic rather than drop a
                // subtree; the depth cap makes it unreachable.
                stack[len] = node.first;
                stack[len + 1] = node.first + 1;
                len += 2;
            }
        }
    }

    /// Depth of the deepest node (root = 0); test support for the stack bound.
    #[cfg(test)]
    pub(crate) fn depth(&self) -> usize {
        fn walk(nodes: &[TreeNode], index: usize, depth: usize) -> usize {
            let node = nodes[index];
            if node.count > 0 {
                depth
            } else {
                let first = node.first as usize;
                walk(nodes, first, depth + 1).max(walk(nodes, first + 1, depth + 1))
            }
        }
        if self.nodes.is_empty() {
            0
        } else {
            walk(&self.nodes, 0, 0)
        }
    }
}

/// Closed segment-versus-box overlap by slabs. An axis the segment does not move
/// along passes when `origin` lies inside that slab. Monotone in the box: a box
/// enclosing another passes whenever the inner one does, so a node test never
/// rejects an entry beneath it that would pass.
pub(crate) fn line_overlaps_box(
    origin: Vec3,
    dir: Vec3,
    t_min: f32,
    t_max: f32,
    min: Vec3,
    max: Vec3,
) -> bool {
    let mut low = t_min;
    let mut high = t_max;
    for axis in 0..3 {
        let o = origin[axis];
        let d = dir[axis];
        if d == 0.0 {
            if o < min[axis] || o > max[axis] {
                return false;
            }
            continue;
        }
        let inverse = 1.0 / d;
        let mut near = (min[axis] - o) * inverse;
        let mut far = (max[axis] - o) * inverse;
        if near > far {
            std::mem::swap(&mut near, &mut far);
        }
        low = low.max(near);
        high = high.min(far);
        if low > high {
            return false;
        }
    }
    true
}

fn placeholder_node() -> TreeNode {
    TreeNode {
        min: Vec3::ZERO,
        max: Vec3::ZERO,
        first: 0,
        count: 0,
    }
}

/// Fill `nodes[node_index]` over `entries[start..end]` and recurse. Returns the
/// deepest depth reached beneath it.
fn build_node<P>(
    nodes: &mut Vec<TreeNode>,
    entries: &mut [TreeEntry<P>],
    node_index: usize,
    start: usize,
    end: usize,
    depth: usize,
) -> usize {
    let (min, max) = bounds_of(&entries[start..end]);
    let count = end - start;
    if count <= MAX_LEAF_ENTRIES || depth >= MAX_DEPTH {
        nodes[node_index] = TreeNode {
            min,
            max,
            first: start as u32,
            count: count as u32,
        };
        return depth;
    }

    let split = if depth < MEDIAN_SPLIT_DEPTH {
        sah_split(&mut entries[start..end])
    } else {
        None
    };
    let mid = start + split.unwrap_or_else(|| median_split(&mut entries[start..end]));

    let left = nodes.len();
    nodes.push(placeholder_node());
    nodes.push(placeholder_node());
    nodes[node_index] = TreeNode {
        min,
        max,
        first: left as u32,
        count: 0,
    };
    let left_depth = build_node(nodes, entries, left, start, mid, depth + 1);
    let right_depth = build_node(nodes, entries, left + 1, mid, end, depth + 1);
    left_depth.max(right_depth)
}

fn bounds_of<P>(entries: &[TreeEntry<P>]) -> (Vec3, Vec3) {
    entries.iter().fold(
        (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY)),
        |(min, max), entry| (min.min(entry.min), max.max(entry.max)),
    )
}

fn centroid_bounds<P>(entries: &[TreeEntry<P>]) -> (Vec3, Vec3) {
    entries.iter().fold(
        (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY)),
        |(min, max), entry| {
            let centroid = entry.centroid();
            (min.min(centroid), max.max(centroid))
        },
    )
}

fn half_area(min: Vec3, max: Vec3) -> f32 {
    let extent = (max - min).max(Vec3::ZERO);
    extent.x * extent.y + extent.y * extent.z + extent.z * extent.x
}

fn bin_of(centroid: f32, low: f32, scale: f32) -> usize {
    // `as usize` saturates and maps NaN to 0, so a degenerate entry still lands in a bin.
    (((centroid - low) * scale) as usize).min(SAH_BINS - 1)
}

/// Best binned-SAH split of `entries`, partitioned in place. Returns the count of
/// the left side, or `None` when every centroid coincides on every axis. Ties keep
/// the first candidate in axis-then-bin order, so the choice is deterministic.
fn sah_split<P>(entries: &mut [TreeEntry<P>]) -> Option<usize> {
    let (centroid_min, centroid_max) = centroid_bounds(entries);
    let mut best: Option<(f32, usize, usize)> = None;
    for axis in 0..3 {
        let extent = centroid_max[axis] - centroid_min[axis];
        if !(extent > 0.0 && extent.is_finite()) {
            continue;
        }
        let scale = SAH_BINS as f32 / extent;
        let mut counts = [0usize; SAH_BINS];
        let mut bin_min = [Vec3::splat(f32::INFINITY); SAH_BINS];
        let mut bin_max = [Vec3::splat(f32::NEG_INFINITY); SAH_BINS];
        for entry in entries.iter() {
            let bin = bin_of(entry.centroid()[axis], centroid_min[axis], scale);
            counts[bin] += 1;
            bin_min[bin] = bin_min[bin].min(entry.min);
            bin_max[bin] = bin_max[bin].max(entry.max);
        }
        // Right-side sweep: cost contribution of bins (split, SAH_BINS).
        let mut right_cost = [0.0f32; SAH_BINS];
        let mut right_count = [0usize; SAH_BINS];
        let (mut run_min, mut run_max, mut run_count) = (
            Vec3::splat(f32::INFINITY),
            Vec3::splat(f32::NEG_INFINITY),
            0usize,
        );
        for bin in (1..SAH_BINS).rev() {
            run_min = run_min.min(bin_min[bin]);
            run_max = run_max.max(bin_max[bin]);
            run_count += counts[bin];
            right_cost[bin] = half_area(run_min, run_max) * run_count as f32;
            right_count[bin] = run_count;
        }
        let (mut run_min, mut run_max, mut run_count) = (
            Vec3::splat(f32::INFINITY),
            Vec3::splat(f32::NEG_INFINITY),
            0usize,
        );
        for split in 1..SAH_BINS {
            run_min = run_min.min(bin_min[split - 1]);
            run_max = run_max.max(bin_max[split - 1]);
            run_count += counts[split - 1];
            if run_count == 0 || right_count[split] == 0 {
                continue;
            }
            let cost = half_area(run_min, run_max) * run_count as f32 + right_cost[split];
            if best.is_none_or(|(best_cost, _, _)| cost < best_cost) {
                best = Some((cost, axis, split));
            }
        }
    }

    let (_, axis, split) = best?;
    let extent = centroid_max[axis] - centroid_min[axis];
    let scale = SAH_BINS as f32 / extent;
    let low = centroid_min[axis];
    let mut left = 0;
    for index in 0..entries.len() {
        if bin_of(entries[index].centroid()[axis], low, scale) < split {
            entries.swap(left, index);
            left += 1;
        }
    }
    Some(left)
}

/// Split at the median entry along the widest centroid axis. Both sides are
/// non-empty because the caller only splits nodes larger than a leaf.
fn median_split<P>(entries: &mut [TreeEntry<P>]) -> usize {
    let (centroid_min, centroid_max) = centroid_bounds(entries);
    let extent = (centroid_max - centroid_min).max(Vec3::ZERO);
    let axis = if extent.x >= extent.y && extent.x >= extent.z {
        0
    } else if extent.y >= extent.z {
        1
    } else {
        2
    };
    let mid = entries.len() / 2;
    entries.select_nth_unstable_by(mid, |a, b| {
        a.centroid()[axis].total_cmp(&b.centroid()[axis])
    });
    mid
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(min: Vec3, max: Vec3, payload: usize) -> TreeEntry<usize> {
        TreeEntry { min, max, payload }
    }

    fn collect_point(tree: &AabbTree<usize>, point: Vec3) -> Vec<usize> {
        let mut hits = Vec::new();
        tree.visit_point(point, |payload| hits.push(*payload));
        hits.sort_unstable();
        hits
    }

    #[test]
    fn empty_tree_visits_nothing() {
        let tree: AabbTree<usize> = AabbTree::build(Vec::new());
        assert_eq!(tree.len(), 0);
        assert_eq!(tree, AabbTree::empty());
        assert!(collect_point(&tree, Vec3::ZERO).is_empty());
        let mut hits = 0;
        tree.visit_line(Vec3::ZERO, Vec3::X, 0.0, 1.0e9, |_| hits += 1);
        assert_eq!(hits, 0);
    }

    #[test]
    fn tree_point_query_matches_entry_bounds_including_shared_faces() {
        // A 6x6 grid of unit cells sharing faces: a point on a shared face or
        // corner reaches every cell whose closed bounds hold it.
        let mut entries = Vec::new();
        for x in 0..6 {
            for z in 0..6 {
                let min = Vec3::new(x as f32, 0.0, z as f32);
                entries.push(entry(min, min + Vec3::ONE, (x * 6 + z) as usize));
            }
        }
        let tree = AabbTree::build(entries);
        assert_eq!(collect_point(&tree, Vec3::new(0.5, 0.5, 0.5)), vec![0]);
        assert_eq!(collect_point(&tree, Vec3::new(1.0, 0.5, 0.5)), vec![0, 6]);
        assert_eq!(
            collect_point(&tree, Vec3::new(1.0, 1.0, 1.0)),
            vec![0, 1, 6, 7]
        );
        assert!(collect_point(&tree, Vec3::new(6.5, 0.5, 0.5)).is_empty());
    }

    #[test]
    fn tree_depth_stays_within_stack_bound_for_degenerate_input() {
        // Nested boxes all centred on the origin: no centroid split exists, and a
        // long chain of thin slabs invites one-entry SAH peels. The builder must
        // stay within MAX_DEPTH either way.
        let nested: Vec<_> = (0..5000)
            .map(|i| {
                entry(
                    Vec3::splat(-(i as f32) - 1.0),
                    Vec3::splat(i as f32 + 1.0),
                    i,
                )
            })
            .collect();
        let tree = AabbTree::build(nested);
        assert!(tree.depth() <= MAX_DEPTH);
        assert_eq!(collect_point(&tree, Vec3::splat(4999.5)), vec![4999]);

        let slabs: Vec<_> = (0..5000)
            .map(|i| {
                let x = (1u64 << (i % 40)) as f32;
                entry(Vec3::new(x, 0.0, 0.0), Vec3::new(x, 1.0, 1.0), i)
            })
            .collect();
        let tree = AabbTree::build(slabs);
        assert!(tree.depth() <= MAX_DEPTH);
    }

    #[test]
    fn tree_build_is_deterministic() {
        let entries: Vec<_> = (0..300)
            .map(|i| {
                let f = i as f32;
                let min = Vec3::new((f * 7.3) % 50.0, (f * 3.1) % 20.0, (f * 11.7) % 50.0);
                entry(min, min + Vec3::new(1.0 + f % 3.0, 2.0, 1.5), i)
            })
            .collect();
        assert_eq!(
            AabbTree::build(entries.clone()),
            AabbTree::build(entries.clone())
        );
    }

    #[test]
    fn line_overlap_handles_axis_parallel_and_touching_segments() {
        let (min, max) = (Vec3::ZERO, Vec3::ONE);
        // Vertical segment through the box's face plane x = 1: touching counts.
        assert!(line_overlaps_box(
            Vec3::new(1.0, -1.0, 0.5),
            Vec3::Y,
            0.0,
            3.0,
            min,
            max
        ));
        assert!(!line_overlaps_box(
            Vec3::new(1.001, -1.0, 0.5),
            Vec3::Y,
            0.0,
            3.0,
            min,
            max
        ));
        // A segment ending exactly on the near face meets it; one stopping short does not.
        assert!(line_overlaps_box(
            Vec3::new(-1.0, 0.5, 0.5),
            Vec3::X,
            0.0,
            1.0,
            min,
            max
        ));
        assert!(!line_overlaps_box(
            Vec3::new(-1.0, 0.5, 0.5),
            Vec3::X,
            0.0,
            0.999,
            min,
            max
        ));
        // Zero direction tests the origin point.
        assert!(line_overlaps_box(
            Vec3::splat(0.5),
            Vec3::ZERO,
            0.0,
            1.0,
            min,
            max
        ));
    }
}
