use super::*;
use glam::Mat4;
use postretro_render_data::cone_frustum::cone_frustum_planes;

/// One leaf: its box, cell and index range.
#[derive(Clone, Copy)]
struct L {
    min: [f32; 3],
    max: [f32; 3],
    cell: u32,
    offset: u32,
    count: u32,
}

fn leaf(l: L) -> BvhLeaf {
    BvhLeaf {
        aabb_min: l.min,
        material_bucket_id: 0,
        aabb_max: l.max,
        index_offset: l.offset,
        index_count: l.count,
        cell_id: l.cell,
        chunk_range_start: 0,
        chunk_range_count: 0,
    }
}

fn union(boxes: impl Iterator<Item = ([f32; 3], [f32; 3])>) -> ([f32; 3], [f32; 3]) {
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for (lo, hi) in boxes {
        for axis in 0..3 {
            min[axis] = min[axis].min(lo[axis]);
            max[axis] = max[axis].max(hi[axis]);
        }
    }
    (min, max)
}

/// Flat depth-first BVH over `slots` (leaf indices), median split on the
/// widest centroid axis, with skip indices that exit each subtree — the
/// layout `bvh_build::flatten` emits.
fn build_subtree(leaves: &[BvhLeaf], slots: &mut [u32], nodes: &mut Vec<BvhNode>) {
    let (min, max) = union(
        slots
            .iter()
            .map(|&s| (leaves[s as usize].aabb_min, leaves[s as usize].aabb_max)),
    );
    let at = nodes.len();
    if slots.len() == 1 {
        nodes.push(BvhNode {
            aabb_min: min,
            skip_index: at as u32 + 1,
            aabb_max: max,
            left_child_or_leaf_index: slots[0],
            flags: BVH_NODE_FLAG_LEAF,
        });
        return;
    }
    nodes.push(BvhNode {
        aabb_min: min,
        skip_index: 0,
        aabb_max: max,
        left_child_or_leaf_index: 0,
        flags: 0,
    });
    let extent: Vec<f32> = (0..3).map(|a| max[a] - min[a]).collect();
    let axis = (0..3)
        .max_by(|&a, &b| extent[a].total_cmp(&extent[b]))
        .unwrap();
    let centroid = |s: &u32| {
        let l = &leaves[*s as usize];
        l.aabb_min[axis] + l.aabb_max[axis]
    };
    slots.sort_by(|a, b| centroid(a).total_cmp(&centroid(b)));
    let mid = slots.len() / 2;
    let (left, right) = slots.split_at_mut(mid);
    build_subtree(leaves, left, nodes);
    build_subtree(leaves, right, nodes);
    nodes[at].skip_index = nodes.len() as u32;
}

fn tree(leaves: &[BvhLeaf]) -> Vec<BvhNode> {
    let mut nodes = Vec::new();
    if !leaves.is_empty() {
        let mut slots: Vec<u32> = (0..leaves.len() as u32).collect();
        build_subtree(leaves, &mut slots, &mut nodes);
    }
    nodes
}

fn index_of(ls: &[L]) -> ShadowReachIndex {
    let leaves: Vec<BvhLeaf> = ls.iter().copied().map(leaf).collect();
    ShadowReachIndex::new(&tree(&leaves), &leaves)
}

/// Cell-major unit boxes along +x: cell `c` spans `[c, c + 1]`, one leaf of
/// three indices each.
fn row(cells: u32) -> Vec<L> {
    (0..cells)
        .map(|c| L {
            min: [c as f32, 0.0, 0.0],
            max: [c as f32 + 1.0, 1.0, 1.0],
            cell: c,
            offset: c * 3,
            count: 3,
        })
        .collect()
}

/// Axis-aligned slab `x ∈ [x0, x1]` over the unit cross-section, as an
/// orthographic frustum looking down +x.
fn slab(x0: f32, x1: f32) -> [Vec4; 6] {
    let view = Mat4::look_at_rh(
        Vec3::new(x0 - 1.0, 0.5, 0.5),
        Vec3::new(x1, 0.5, 0.5),
        Vec3::Y,
    );
    cone_frustum_planes(&(Mat4::orthographic_rh(-2.0, 2.0, -2.0, 2.0, 1.0, 1.0 + x1 - x0) * view))
}

fn sorted(mut cells: Vec<u32>) -> Vec<u32> {
    cells.sort_unstable();
    cells
}

fn indices(ranges: &[Range<u32>]) -> Vec<u32> {
    ranges.iter().flat_map(|r| r.clone()).collect()
}

fn spot(origin: Vec3, direction: Vec3, outer: f32, range: f32) -> [Vec4; 6] {
    let light = postretro_level_loader::MapLight {
        origin: [origin.x as f64, origin.y as f64, origin.z as f64],
        light_type: postretro_level_loader::LightType::Spot,
        intensity: 1.0,
        color: [1.0; 3],
        falloff_model: postretro_level_loader::FalloffModel::Linear,
        falloff_range: range,
        cone_angle_inner: outer * 0.5,
        cone_angle_outer: outer,
        cone_direction: [direction.x, direction.y, direction.z],
        is_dynamic: true,
        casts_entity_shadows: false,
        animated_slot: None,
        tags: vec![],
        cell_index: 0,
        shadow_type: postretro_level_loader::ShadowType::StaticLightMap,
    };
    cone_frustum_planes(&postretro_lighting::light_space_matrix(&light))
}

#[test]
fn spot_reach_draws_exactly_cells_owning_a_leaf_in_its_frustum() {
    // A 12-cell row along +x; a narrow spot at the origin end aims down +x.
    let index = index_of(&row(12));
    let planes = spot(Vec3::new(-1.0, 0.5, 0.5), Vec3::X, 0.25, 6.0);
    let mut scratch = index.scratch();
    let ranges = index.reach(&planes, &mut scratch).to_vec();
    let reached = sorted(scratch.cells().to_vec());
    assert_eq!(reached, index.brute_force_reached_cells(&planes));
    assert!(!reached.is_empty() && reached.len() < 12, "{reached:?}");
    let expected: Vec<u32> = reached
        .iter()
        .flat_map(|&c| index.cell_ranges(c).iter().flat_map(|r| r.clone()))
        .collect();
    assert_eq!(indices(&ranges), expected);
}

/// xorshift64*: a fixed-seed stream so the randomized oracle is reproducible.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        ((self.0.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 40) as f32) / (1u64 << 24) as f32
    }

    fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.next()
    }
}

/// A 3-D grid of cells, each with one to three leaves that may overhang the
/// cell by up to 0.1 m, laid out cell-major.
fn random_world(rng: &mut Rng, dims: [u32; 3]) -> Vec<L> {
    let mut leaves = Vec::new();
    let mut offset = 0;
    let mut cell = 0;
    for x in 0..dims[0] {
        for y in 0..dims[1] {
            for z in 0..dims[2] {
                let base = Vec3::new(x as f32, y as f32, z as f32) * 4.0;
                let count = 1 + (rng.next() * 3.0) as u32;
                for _ in 0..count {
                    let a = base
                        + Vec3::new(
                            rng.range(-0.1, 3.0),
                            rng.range(-0.1, 3.0),
                            rng.range(-0.1, 3.0),
                        );
                    let size = Vec3::new(
                        rng.range(0.05, 1.1),
                        rng.range(0.05, 1.1),
                        rng.range(0.05, 1.1),
                    );
                    let n = 3 * (1 + (rng.next() * 4.0) as u32);
                    leaves.push(L {
                        min: a.to_array(),
                        max: (a + size).to_array(),
                        cell,
                        offset,
                        count: n,
                    });
                    offset += n;
                }
                cell += 1;
            }
        }
    }
    leaves
}

fn random_frustum(rng: &mut Rng, extent: f32) -> [Vec4; 6] {
    let eye = Vec3::new(
        rng.range(-4.0, extent + 4.0),
        rng.range(-4.0, extent + 4.0),
        rng.range(-4.0, extent + 4.0),
    );
    let mut dir = Vec3::new(
        rng.range(-1.0, 1.0),
        rng.range(-1.0, 1.0),
        rng.range(-1.0, 1.0),
    );
    if dir.length_squared() < 1e-3 {
        dir = Vec3::Z;
    }
    let up = if dir.normalize().y.abs() > 0.9 {
        Vec3::X
    } else {
        Vec3::Y
    };
    let view = Mat4::look_at_rh(eye, eye + dir, up);
    let proj = Mat4::perspective_rh(
        rng.range(0.1, 2.6),
        rng.range(0.5, 2.0),
        rng.range(0.05, 1.0),
        rng.range(2.0, extent * 1.5),
    );
    cone_frustum_planes(&(proj * view))
}

#[test]
fn walk_matches_brute_force_cells_over_randomized_frusta() {
    // Fixed seed and count: 6 worlds × 200 frusta.
    let mut rng = Rng(0x5eed_5ad0_f111_c057);
    let mut nonempty = 0;
    let mut partial = 0;
    for _ in 0..6 {
        let ls = random_world(&mut rng, [7, 3, 6]);
        let index = index_of(&ls);
        let mut scratch = index.scratch();
        for _ in 0..200 {
            let planes = random_frustum(&mut rng, 28.0);
            let ranges = index.reach(&planes, &mut scratch).to_vec();
            let walked = sorted(scratch.cells().to_vec());
            let oracle = index.brute_force_reached_cells(&planes);
            assert_eq!(walked, oracle, "walk and brute force disagree");
            let expected: Vec<u32> = oracle
                .iter()
                .flat_map(|&c| index.cell_ranges(c).iter().flat_map(|r| r.clone()))
                .collect();
            let mut drawn = indices(&ranges);
            drawn.sort_unstable();
            let mut expected_sorted = expected;
            expected_sorted.sort_unstable();
            assert_eq!(drawn, expected_sorted, "ranges cover exactly reached cells");
            nonempty += usize::from(!oracle.is_empty());
            partial += usize::from(!oracle.is_empty() && oracle.len() < index.cell_count());
        }
    }
    // The sample must exercise partial reach, not only all-or-nothing.
    assert!(partial > 100, "nonempty {nonempty}, partial {partial}");
}

#[test]
fn leaf_overhanging_its_cell_bounds_reaches_its_cell() {
    // Cell 0's polytope is x ∈ [0, 1], but its leaf extends to 1.08 (inside the
    // 0.1 m split epsilon). Cell 1 starts at 1.1. The frustum touches only the
    // overhang.
    let index = index_of(&[
        L {
            min: [0.0, 0.0, 0.0],
            max: [1.08, 1.0, 1.0],
            cell: 0,
            offset: 0,
            count: 3,
        },
        L {
            min: [1.1, 0.0, 0.0],
            max: [2.0, 1.0, 1.0],
            cell: 1,
            offset: 3,
            count: 3,
        },
    ]);
    let planes = slab(1.02, 1.06);
    let mut scratch = index.scratch();
    assert_eq!(index.reach(&planes, &mut scratch), &[0..3]);
    assert_eq!(scratch.cells(), &[0]);
}

#[test]
fn cell_is_reached_by_any_one_of_its_leaves_and_not_otherwise() {
    // Cell 0: both leaves far outside. Cell 1: one leaf inside, one outside.
    let index = index_of(&[
        L {
            min: [10.0, 0.0, 0.0],
            max: [11.0, 1.0, 1.0],
            cell: 0,
            offset: 0,
            count: 3,
        },
        L {
            min: [12.0, 0.0, 0.0],
            max: [13.0, 1.0, 1.0],
            cell: 0,
            offset: 3,
            count: 3,
        },
        L {
            min: [0.0, 0.0, 0.0],
            max: [1.0, 1.0, 1.0],
            cell: 1,
            offset: 6,
            count: 3,
        },
        L {
            min: [20.0, 0.0, 0.0],
            max: [21.0, 1.0, 1.0],
            cell: 1,
            offset: 9,
            count: 3,
        },
    ]);
    let mut scratch = index.scratch();
    let ranges = index.reach(&slab(0.0, 2.0), &mut scratch).to_vec();
    assert_eq!(scratch.cells(), &[1]);
    // The whole cell draws, including its out-of-frustum leaf.
    assert_eq!(ranges, vec![6..12]);
}

#[test]
fn empty_and_full_reach_transitions_through_one_scratch() {
    let index = index_of(&row(8));
    let mut scratch = index.scratch();
    let miss = slab(100.0, 101.0);
    let part = slab(2.2, 4.8);
    let all = slab(-1.0, 9.0);

    assert!(index.reach(&miss, &mut scratch).is_empty(), "empty first");
    assert_eq!(index.reach(&part, &mut scratch), &[6..15]);
    assert!(
        index.reach(&miss, &mut scratch).is_empty(),
        "empty after nonempty"
    );
    assert_eq!(scratch.stats().collected_cells, 0);
    assert_eq!(
        index.reach(&part, &mut scratch),
        &[6..15],
        "nonempty after empty draws in full"
    );
    // Every drawable leaf's indices exactly once.
    assert_eq!(
        indices(index.reach(&all, &mut scratch)),
        (0..24).collect::<Vec<_>>()
    );
}

#[test]
fn abutting_ranges_merge_and_a_one_index_gap_splits() {
    // Cells 0–2 abut; cell 3 starts one index after cell 2 ends; cell 4 abuts
    // cell 3.
    let index = index_of(&[
        L {
            min: [0.0, 0.0, 0.0],
            max: [1.0, 1.0, 1.0],
            cell: 0,
            offset: 0,
            count: 3,
        },
        L {
            min: [1.0, 0.0, 0.0],
            max: [2.0, 1.0, 1.0],
            cell: 1,
            offset: 3,
            count: 3,
        },
        L {
            min: [2.0, 0.0, 0.0],
            max: [3.0, 1.0, 1.0],
            cell: 2,
            offset: 6,
            count: 3,
        },
        L {
            min: [3.0, 0.0, 0.0],
            max: [4.0, 1.0, 1.0],
            cell: 3,
            offset: 10,
            count: 3,
        },
        L {
            min: [4.0, 0.0, 0.0],
            max: [5.0, 1.0, 1.0],
            cell: 4,
            offset: 13,
            count: 3,
        },
    ]);
    let mut scratch = index.scratch();
    assert_eq!(
        index.reach(&slab(-1.0, 6.0), &mut scratch),
        &[0..9, 10..16],
        "one draw per maximal run"
    );
    // Cells 0 and 2 without cell 1 between them: two draws.
    let mut split = index.reach(&slab(0.1, 0.9), &mut scratch).to_vec();
    split.extend_from_slice(index.reach(&slab(2.1, 2.9), &mut scratch));
    assert_eq!(split, vec![0..3, 6..9]);
}

#[test]
fn noncontiguous_cell_draws_each_leaf_range_and_nothing_between() {
    // Cell 0 owns [0, 3) and [6, 9); cell 1 owns [3, 6) between them.
    let index = index_of(&[
        L {
            min: [0.0, 0.0, 0.0],
            max: [1.0, 1.0, 1.0],
            cell: 0,
            offset: 0,
            count: 3,
        },
        L {
            min: [5.0, 0.0, 0.0],
            max: [6.0, 1.0, 1.0],
            cell: 1,
            offset: 3,
            count: 3,
        },
        L {
            min: [0.5, 0.0, 0.0],
            max: [1.5, 1.0, 1.0],
            cell: 0,
            offset: 6,
            count: 3,
        },
    ]);
    assert_eq!(index.cell_ranges(0), &[0..3, 6..9]);
    let mut scratch = index.scratch();
    assert_eq!(index.reach(&slab(0.0, 2.0), &mut scratch), &[0..3, 6..9]);
}

/// Root over `[inside subtree, outside subtree]`. The inside subtree is the
/// same for every `outside` count; the outside subtree holds `outside` cells
/// far down +x, out of any frustum near the origin.
fn split_world(outside: u32) -> ShadowReachIndex {
    let inside = row(6);
    let mut ls = inside.clone();
    for i in 0..outside {
        let x = 1000.0 + i as f32;
        ls.push(L {
            min: [x, 0.0, 0.0],
            max: [x + 1.0, 1.0, 1.0],
            cell: 6 + i,
            offset: (6 + i) * 3,
            count: 3,
        });
    }
    let leaves: Vec<BvhLeaf> = ls.iter().copied().map(leaf).collect();
    let mut nodes = vec![BvhNode {
        aabb_min: [0.0; 3],
        skip_index: 0,
        aabb_max: [0.0; 3],
        left_child_or_leaf_index: 0,
        flags: 0,
    }];
    let mut inside_slots: Vec<u32> = (0..6).collect();
    build_subtree(&leaves, &mut inside_slots, &mut nodes);
    let mut outside_slots: Vec<u32> = (6..6 + outside).collect();
    build_subtree(&leaves, &mut outside_slots, &mut nodes);
    let (min, max) = union(leaves.iter().map(|l| (l.aabb_min, l.aabb_max)));
    nodes[0].aabb_min = min;
    nodes[0].aabb_max = max;
    nodes[0].skip_index = nodes.len() as u32;
    ShadowReachIndex::new(&nodes, &leaves)
}

#[test]
fn walk_cost_follows_reach_not_cells_outside_the_frustum() {
    let planes = slab(1.5, 3.5);
    let mut baseline = None;
    for outside in [1, 16, 300] {
        let index = split_world(outside);
        let mut scratch = index.scratch();
        let ranges = index.reach(&planes, &mut scratch).to_vec();
        let stats = scratch.stats();
        assert_eq!(ranges, vec![3..12]);
        assert_eq!(stats.collected_cells, stats.reset_cells);
        // Root, the inside subtree's visited nodes, and the outside subtree's
        // rejected root — never a node below it.
        let shape = (
            stats.visited_nodes,
            stats.collected_cells,
            stats.reset_cells,
        );
        match baseline {
            None => baseline = Some(shape),
            Some(baseline) => assert_eq!(shape, baseline, "{outside} outside cells"),
        }
    }
}

#[test]
fn malformed_skip_or_leaf_slot_ends_or_skips_without_reaching_out_of_range() {
    let leaves = vec![leaf(L {
        min: [0.0; 3],
        max: [1.0; 3],
        cell: 0,
        offset: 0,
        count: 3,
    })];
    let nodes = vec![
        // Leaf node pointing past the leaf array; its skip loops back.
        BvhNode {
            aabb_min: [0.0; 3],
            skip_index: 0,
            aabb_max: [1.0; 3],
            left_child_or_leaf_index: 7,
            flags: BVH_NODE_FLAG_LEAF,
        },
        BvhNode {
            aabb_min: [0.0; 3],
            skip_index: 2,
            aabb_max: [1.0; 3],
            left_child_or_leaf_index: 0,
            flags: BVH_NODE_FLAG_LEAF,
        },
    ];
    let index = ShadowReachIndex::new(&nodes, &leaves);
    let mut scratch = index.scratch();
    assert!(index.reach(&slab(-1.0, 2.0), &mut scratch).is_empty());
}

#[test]
fn zero_count_leaf_reaches_its_cell_without_adding_a_range() {
    // Cell 1's only in-frustum leaf is empty; its other leaf lies outside but
    // still draws, because reach is per cell.
    let index = index_of(&[
        L {
            min: [0.0, 0.0, 0.0],
            max: [1.0, 1.0, 1.0],
            cell: 0,
            offset: 0,
            count: 3,
        },
        L {
            min: [2.0, 0.0, 0.0],
            max: [3.0, 1.0, 1.0],
            cell: 1,
            offset: 3,
            count: 0,
        },
        L {
            min: [40.0, 0.0, 0.0],
            max: [41.0, 1.0, 1.0],
            cell: 1,
            offset: 3,
            count: 3,
        },
    ]);
    let mut scratch = index.scratch();
    assert_eq!(index.reach(&slab(2.2, 2.8), &mut scratch), &[3..6]);
    assert_eq!(scratch.cells(), &[1]);
}

#[test]
fn scratch_from_a_larger_level_walks_a_smaller_one() {
    let large = index_of(&row(200));
    let small = index_of(&row(3));
    let mut scratch = large.scratch();
    assert_eq!(large.reach(&slab(150.2, 151.8), &mut scratch), &[450..456]);
    assert_eq!(small.reach(&slab(-1.0, 4.0), &mut scratch), &[0..9]);
}

#[test]
fn scratch_from_a_smaller_level_refits_before_walking() {
    let small = index_of(&row(2));
    let large = index_of(&row(200));
    let mut scratch = small.scratch();
    assert_eq!(large.reach(&slab(150.2, 151.8), &mut scratch), &[450..456]);
}
