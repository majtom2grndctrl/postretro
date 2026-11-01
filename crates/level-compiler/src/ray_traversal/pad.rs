// Bake-tree box padding: every box grows to hold every point the hit test accepts.
// See: context/lib/build_pipeline.md §Ray traversal

use bvh::aabb::{Aabb, Bounded};
use bvh::bvh::{Bvh, BvhNode};

use super::TRACER_TOLERANCE_METERS;
use crate::bvh_build::BvhPrimitive;

/// Grow every box of a bake tree, node child and primitive alike, outward
/// by [`box_pad`] of the primitives' extent. A hit the test accepts lies
/// within τ of its triangle in the triangle's plane, plus the test's own
/// `f64` rounding, which one `f32` ulp of the extent covers; so it lies in its
/// padded box, and neither the box test nor the distance prune drops it.
///
/// Each grown coordinate steps one `f32` further out after the add: a
/// round-to-nearest add can land half a step short of the exact sum. The pad
/// is uniform and the rounding monotone, so a parent's stored box still holds
/// its children's, and the topology, and with it the closest-hit tie order,
/// is unchanged. The shipped BVH section is flattened from the exact boxes
/// before this runs.
pub(crate) fn pad_bake_tree(bvh: &mut Bvh<f32, 3>, primitives: &mut [BvhPrimitive]) {
    let pad = box_pad(extent(primitives));
    for prim in primitives.iter_mut() {
        for axis in 0..3 {
            prim.aabb_min[axis] = grow_down(prim.aabb_min[axis], pad);
            prim.aabb_max[axis] = grow_up(prim.aabb_max[axis], pad);
        }
    }
    for node in &mut bvh.nodes {
        if let BvhNode::Node {
            child_l_aabb,
            child_r_aabb,
            ..
        } = node
        {
            grow_box(child_l_aabb, pad);
            grow_box(child_r_aabb, pad);
        }
    }
    #[cfg(debug_assertions)]
    assert_parents_contain_children(bvh, primitives);
}

/// The bake tree's box pad for geometry whose largest coordinate magnitude
/// is `extent`: τ plus one `f32` ulp of the extent, rounded up.
pub(crate) fn box_pad(extent: f32) -> f32 {
    let ulp = extent.next_up() - extent;
    let pad = TRACER_TOLERANCE_METERS + ulp;
    if f64::from(pad) < f64::from(TRACER_TOLERANCE_METERS) + f64::from(ulp) {
        pad.next_up()
    } else {
        pad
    }
}

/// The largest coordinate magnitude of the primitives' boxes.
fn extent(primitives: &[BvhPrimitive]) -> f32 {
    primitives
        .iter()
        .flat_map(|prim| prim.aabb_min.iter().chain(&prim.aabb_max))
        .fold(0.0f32, |extent, &c| extent.max(c.abs()))
}

fn grow_down(c: f32, pad: f32) -> f32 {
    (c - pad).next_down()
}

fn grow_up(c: f32, pad: f32) -> f32 {
    (c + pad).next_up()
}

fn grow_box(aabb: &mut Aabb<f32, 3>, pad: f32) {
    for axis in 0..3 {
        aabb.min[axis] = grow_down(aabb.min[axis], pad);
        aabb.max[axis] = grow_up(aabb.max[axis], pad);
    }
}

/// Panic unless every stored box holds the boxes below it: each interior
/// child box holds that child's own two stored boxes, and each leaf's stored
/// box holds its primitive's box. The traversal reaches a leaf only through
/// its ancestors' boxes, so a remembered occluder gated on its leaf box is
/// one of the traversal's own candidates only while this holds.
#[cfg(debug_assertions)]
pub(crate) fn assert_parents_contain_children(bvh: &Bvh<f32, 3>, primitives: &[BvhPrimitive]) {
    let holds = |outer: &Aabb<f32, 3>, inner: &Aabb<f32, 3>| {
        (0..3).all(|axis| outer.min[axis] <= inner.min[axis] && inner.max[axis] <= outer.max[axis])
    };
    for (index, node) in bvh.nodes.iter().enumerate() {
        let BvhNode::Node {
            child_l_index,
            child_l_aabb,
            child_r_index,
            child_r_aabb,
            ..
        } = node
        else {
            continue;
        };
        for (child, stored) in [
            (*child_l_index, child_l_aabb),
            (*child_r_index, child_r_aabb),
        ] {
            let below: Vec<Aabb<f32, 3>> = match &bvh.nodes[child] {
                BvhNode::Node {
                    child_l_aabb,
                    child_r_aabb,
                    ..
                } => vec![*child_l_aabb, *child_r_aabb],
                BvhNode::Leaf { shape_index, .. } => vec![primitives[*shape_index].aabb()],
            };
            for inner in below {
                assert!(
                    holds(stored, &inner),
                    "bake BVH: node {index}'s box for child {child} {:?}..{:?} does not hold {:?}..{:?}",
                    stored.min,
                    stored.max,
                    inner.min,
                    inner.max,
                );
            }
        }
    }
}

/// The box the traversal tests before it yields the leaf at node `leaf`:
/// its parent's stored child box, or, for a root leaf, its primitive's own
/// box (`bvh` 0.12 writes a root leaf's parent index as itself).
pub(super) fn leaf_box(
    bvh: &Bvh<f32, 3>,
    primitives: &[BvhPrimitive],
    leaf: usize,
) -> Aabb<f32, 3> {
    let BvhNode::Leaf {
        parent_index,
        shape_index,
    } = bvh.nodes[leaf]
    else {
        unreachable!("an occluder names a leaf node");
    };
    if parent_index == leaf {
        return primitives[shape_index].aabb();
    }
    match bvh.nodes[parent_index] {
        BvhNode::Node {
            child_l_index,
            child_l_aabb,
            child_r_aabb,
            ..
        } => {
            if child_l_index == leaf {
                child_l_aabb
            } else {
                child_r_aabb
            }
        }
        BvhNode::Leaf { .. } => unreachable!("a leaf's parent is an interior node"),
    }
}

/// Panic unless every primitive's box holds its face's exact box grown by τ
/// on every face: the tree is one a bake may trace.
#[cfg(test)]
pub(crate) fn assert_padded(
    primitives: &[BvhPrimitive],
    geometry: &crate::geometry::GeometryResult,
) {
    let mut exact = crate::bvh_build::collect_primitives(geometry);
    exact.sort_by_key(|prim| prim.index_offset);
    assert_eq!(primitives.len(), exact.len(), "one primitive per face");
    let tau = f64::from(TRACER_TOLERANCE_METERS);
    for prim in primitives {
        let slot = exact
            .binary_search_by_key(&prim.index_offset, |e| e.index_offset)
            .expect("every primitive is a face's");
        let face = &exact[slot];
        for axis in 0..3 {
            assert!(
                f64::from(prim.aabb_min[axis]) <= f64::from(face.aabb_min[axis]) - tau
                    && f64::from(prim.aabb_max[axis]) >= f64::from(face.aabb_max[axis]) + tau,
                "primitive at {} is not padded: {:?}..{:?} around {:?}..{:?}",
                prim.index_offset,
                prim.aabb_min,
                prim.aabb_max,
                face.aabb_min,
                face.aabb_max
            );
        }
    }
}
