use super::*;
use crate::lighting::cube_shadow::{CUBE_FACES, cube_face_matrices};
use crate::render::dynamic_depth_cache::DynamicDepthCache;
use glam::Vec3;
use postretro_level_format::cell_draw_index::Span;
use postretro_level_loader::{CellDrawIndex, FalloffModel, LightType, MapLight, ShadowType};
use postretro_render_data::geometry::{BVH_NODE_FLAG_LEAF, BvhLeaf, BvhNode};
use postretro_sim::alloc_probe::AllocSnapshot;
use postretro_visibility::VisibleCells;

/// One leaf per cell, cell-major: cell `c` owns indices `[3c, 3c + 3)` and the
/// unit box centred on `centres[c]`. A root over threaded leaf children.
fn world(centres: &[Vec3]) -> BvhTree {
    let leaves: Vec<BvhLeaf> = centres
        .iter()
        .enumerate()
        .map(|(cell, centre)| BvhLeaf {
            aabb_min: (*centre - 0.5).to_array(),
            material_bucket_id: (cell % 2) as u32,
            aabb_max: (*centre + 0.5).to_array(),
            index_offset: cell as u32 * 3,
            index_count: 3,
            cell_id: cell as u32,
            chunk_range_start: 0,
            chunk_range_count: 0,
        })
        .collect();
    let mut min = Vec3::INFINITY;
    let mut max = Vec3::NEG_INFINITY;
    for leaf in &leaves {
        min = min.min(Vec3::from(leaf.aabb_min));
        max = max.max(Vec3::from(leaf.aabb_max));
    }
    let mut nodes = vec![BvhNode {
        aabb_min: min.to_array(),
        skip_index: leaves.len() as u32 + 1,
        aabb_max: max.to_array(),
        left_child_or_leaf_index: 0,
        flags: 0,
    }];
    nodes.extend(leaves.iter().enumerate().map(|(slot, leaf)| BvhNode {
        aabb_min: leaf.aabb_min,
        skip_index: slot as u32 + 2,
        aabb_max: leaf.aabb_max,
        left_child_or_leaf_index: slot as u32,
        flags: BVH_NODE_FLAG_LEAF,
    }));
    BvhTree {
        nodes,
        leaves,
        root_node_index: 0,
    }
}

fn index_count(tree: &BvhTree) -> u32 {
    tree.leaves.len() as u32 * 3
}

fn light(light_type: LightType, origin: Vec3, direction: Vec3) -> MapLight {
    MapLight {
        origin: [origin.x as f64, origin.y as f64, origin.z as f64],
        light_type,
        intensity: 1.0,
        color: [1.0; 3],
        falloff_model: FalloffModel::Linear,
        falloff_range: 12.0,
        cone_angle_inner: 0.2,
        cone_angle_outer: 0.4,
        cone_direction: direction.to_array(),
        is_dynamic: true,
        casts_entity_shadows: false,
        animated_slot: None,
        tags: vec![],
        cell_index: 0,
        shadow_type: ShadowType::StaticLightMap,
    }
}

fn spot(origin: Vec3, direction: Vec3) -> Mat4 {
    postretro_lighting::light_space_matrix(&light(LightType::Spot, origin, direction))
}

fn cube(origin: Vec3) -> [Mat4; CUBE_FACES] {
    cube_face_matrices(&light(LightType::Point, origin, Vec3::NEG_Z))
}

/// The cells a region's ranges cover: one leaf of three indices per cell.
fn cells_of(ranges: &[Range<u32>]) -> Vec<u32> {
    ranges
        .iter()
        .flat_map(|range| range.clone().step_by(3).map(|index| index / 3))
        .collect()
}

/// Brute force over every leaf, as merged index ranges.
fn oracle(tree: &BvhTree, matrix: &Mat4) -> Vec<u32> {
    ShadowReachIndex::new(&tree.nodes, &tree.leaves)
        .brute_force_reached_cells(&cone_frustum_planes(matrix))
}

/// Six cells, one on each axis 5 m from the origin, plus two on the +X/+Z
/// diagonal that straddles two cube faces, plus one far away.
fn axes() -> BvhTree {
    world(&[
        Vec3::new(5.0, 0.0, 0.0),
        Vec3::new(-5.0, 0.0, 0.0),
        Vec3::new(0.0, 5.0, 0.0),
        Vec3::new(0.0, -5.0, 0.0),
        Vec3::new(0.0, 0.0, 5.0),
        Vec3::new(0.0, 0.0, -5.0),
        Vec3::new(4.0, 0.0, 4.0),
        Vec3::new(4.5, 0.0, 4.5),
        Vec3::new(400.0, 0.0, 0.0),
    ])
}

#[test]
fn cube_faces_each_draw_exactly_cells_owning_a_leaf_in_their_face_frustum() {
    let tree = axes();
    let mut draws = ShadowWorldDraws::install(Some(&tree), index_count(&tree));
    let faces = cube(Vec3::ZERO);
    let mut covered = Vec::new();
    for (face, matrix) in faces.iter().enumerate() {
        let cells = cells_of(draws.ranges(ShadowRegion::CubeFace(face as u32), matrix));
        assert_eq!(cells, oracle(&tree, matrix), "face {face}");
        covered.extend(cells);
    }
    covered.sort_unstable();
    covered.dedup();
    // Every in-range cell is reached by some face; the far cell by none.
    assert_eq!(covered, (0..8).collect::<Vec<_>>());
}

#[test]
fn spot_reach_includes_cells_the_camera_cannot_see_and_skips_cells_it_can() {
    // Cell 0 sits in the spot cone; cell 1 behind the light. The camera sees
    // only cell 1.
    let tree = world(&[Vec3::new(0.0, 0.0, -5.0), Vec3::new(0.0, 0.0, 5.0)]);
    let camera_index = CellDrawIndex {
        cell_count: 2,
        span_count: 2,
        cell_span_offset: vec![0, 1, 2],
        spans: vec![
            Span {
                leaf_start: 0,
                leaf_count: 1,
            },
            Span {
                leaf_start: 1,
                leaf_count: 1,
            },
        ],
    };
    let mut camera = crate::VisibleSpanRanges::default();
    camera.rebuild(
        Some(&camera_index),
        &VisibleCells::Culled(vec![1]),
        &tree.derive_bucket_ranges(),
    );
    let camera_leaves: Vec<u32> = camera
        .ranges()
        .iter()
        .flat_map(|range| range.first_leaf..range.first_leaf + range.leaf_count)
        .collect();
    assert_eq!(camera_leaves, vec![1]);

    let mut draws = ShadowWorldDraws::install(Some(&tree), index_count(&tree));
    let matrix = spot(Vec3::ZERO, Vec3::NEG_Z);
    assert_eq!(
        cells_of(draws.ranges(ShadowRegion::Spot(0), &matrix)),
        vec![0]
    );
}

#[test]
fn regions_in_one_frame_draw_their_own_reach_through_one_scratch() {
    let tree = axes();
    let mut draws = ShadowWorldDraws::install(Some(&tree), index_count(&tree));
    let faces = cube(Vec3::ZERO);
    let toward_x = spot(Vec3::ZERO, Vec3::X);
    let toward_neg_x = spot(Vec3::ZERO, Vec3::NEG_X);
    draws.begin_frame();

    // Disjoint frusta.
    assert_eq!(
        cells_of(draws.ranges(ShadowRegion::Spot(0), &toward_x)),
        vec![0]
    );
    assert_eq!(
        cells_of(draws.ranges(ShadowRegion::Spot(1), &toward_neg_x)),
        vec![1]
    );

    // O1: adjacent faces +X (layer 0) and +Z (layer 4) share the diagonal
    // cells; each keeps its own axis cell.
    let plus_x = cells_of(draws.ranges(ShadowRegion::CubeFace(0), &faces[0]));
    let plus_z = cells_of(draws.ranges(ShadowRegion::CubeFace(4), &faces[4]));
    assert!(plus_x.contains(&0) && !plus_x.contains(&4), "{plus_x:?}");
    assert!(plus_z.contains(&4) && !plus_z.contains(&0), "{plus_z:?}");
    let shared: Vec<u32> = plus_x
        .iter()
        .copied()
        .filter(|cell| plus_z.contains(cell))
        .collect();
    assert!(!shared.is_empty(), "the diagonal cells straddle both faces");
    assert_eq!(plus_x, oracle(&tree, &faces[0]));
    assert_eq!(plus_z, oracle(&tree, &faces[4]));

    // O6: spot slot 3 (looking -Y) then cube layer 3 (looking +Y), recorded
    // in that order.
    let down = spot(Vec3::ZERO, Vec3::NEG_Y);
    assert_eq!(
        cells_of(draws.ranges(ShadowRegion::Spot(3), &down)),
        vec![3]
    );
    assert_eq!(
        cells_of(draws.ranges(ShadowRegion::CubeFace(3), &faces[3])),
        oracle(&tree, &faces[3])
    );
    assert_eq!(
        draws
            .trace
            .iter()
            .map(|entry| entry.region)
            .collect::<Vec<_>>(),
        vec![
            ShadowRegion::Spot(0),
            ShadowRegion::Spot(1),
            ShadowRegion::CubeFace(0),
            ShadowRegion::CubeFace(4),
            ShadowRegion::Spot(3),
            ShadowRegion::CubeFace(3),
        ]
    );
    assert_ne!(draws.trace[4].ranges, draws.trace[5].ranges);
}

#[test]
fn moving_light_draws_each_frames_own_reach_including_a_strict_subset() {
    // A row of cells down -Z; the light backs away along +Z with a fixed
    // range, so its far plane reaches fewer cells.
    let centres: Vec<Vec3> = (0..8)
        .map(|i| Vec3::new(0.0, 0.0, -2.0 * i as f32))
        .collect();
    let tree = world(&centres);
    let mut draws = ShadowWorldDraws::install(Some(&tree), index_count(&tree));
    let wide = cells_of(draws.ranges(
        ShadowRegion::Spot(0),
        &spot(Vec3::new(0.0, 0.0, 1.0), Vec3::NEG_Z),
    ));
    let narrow = cells_of(draws.ranges(
        ShadowRegion::Spot(0),
        &spot(Vec3::new(0.0, 0.0, 5.0), Vec3::NEG_Z),
    ));
    assert!(narrow.iter().all(|cell| wide.contains(cell)));
    assert!(narrow.len() < wide.len(), "{narrow:?} ⊂ {wide:?}");
    assert_eq!(
        narrow,
        oracle(&tree, &spot(Vec3::new(0.0, 0.0, 5.0), Vec3::NEG_Z))
    );
}

#[test]
fn level_without_bvh_draws_its_whole_index_buffer() {
    let empty = BvhTree {
        nodes: vec![],
        leaves: vec![],
        root_node_index: 0,
    };
    for bvh in [None, Some(&empty)] {
        let mut draws = ShadowWorldDraws::install(bvh, 42);
        assert_eq!(
            draws.ranges(ShadowRegion::Spot(0), &spot(Vec3::ZERO, Vec3::NEG_Z)),
            std::slice::from_ref(&(0..42))
        );
        assert_eq!(draws.walks, 0, "a whole-buffer draw is not a walk");
    }
}

#[test]
fn reach_built_from_leaves_alone_needs_no_cell_draw_index() {
    // O8: `install` takes only the BVH; a level without the per-cell draw
    // index has the same reach as one with it.
    let tree = axes();
    let mut draws = ShadowWorldDraws::install(Some(&tree), index_count(&tree));
    let matrix = spot(Vec3::ZERO, Vec3::X);
    assert_eq!(
        cells_of(draws.ranges(ShadowRegion::Spot(0), &matrix)),
        vec![0]
    );
}

/// The recorder loop's world decision, region by region: classify each
/// occupied spot slot and walk reach for exactly those that draw world.
fn record_spot_frame(
    draws: &mut ShadowWorldDraws,
    slots: &[(u32, Mat4)],
    promoted: &PromotedDepthCacheFramePlan,
    dynamic: &DynamicDepthCachePlan,
) -> Vec<u32> {
    draws.begin_frame();
    for &(slot, matrix) in slots {
        if classify_spot(slot, promoted, dynamic).draws_world() {
            draws.ranges(ShadowRegion::Spot(slot), &matrix);
        }
    }
    draws
        .trace
        .iter()
        .map(|entry| match entry.region {
            ShadowRegion::Spot(slot) => slot,
            ShadowRegion::CubeFace(_) => unreachable!(),
        })
        .collect()
}

fn promoted_plan(spots: &[(u32, bool)]) -> PromotedDepthCacheFramePlan {
    PromotedDepthCacheFramePlan {
        spot: spots
            .iter()
            .enumerate()
            .map(
                |(layer, &(slot, needs_world_render))| PromotedSpotCachePlan {
                    slot,
                    cache_layer: layer as u32,
                    needs_world_render,
                },
            )
            .collect(),
        ..Default::default()
    }
}

#[test]
fn mixed_frame_walks_reach_once_per_cold_fill_and_uncached_region_only() {
    // O5. Slots: 0 promoted warm, 1 dynamic (warm next frame), 2 dynamic,
    // 3 promoted cold, 4 dynamic, 5 dynamic past the 3-layer cache. A
    // promoted light the promoted cache drops holds no slot at all, so it
    // never reaches the recorder (`renderer_light_slots`
    // `missing_cache_plan_layer_drops_record_and_zeros_weight_before_metadata_pack`).
    let tree = axes();
    let mut draws = ShadowWorldDraws::install(Some(&tree), index_count(&tree));
    let m = |direction: Vec3| spot(Vec3::ZERO, direction);
    let mut cache = DynamicDepthCache::default();
    let dynamic_inputs = [
        (1, 11, m(Vec3::X)),
        (2, 12, m(Vec3::NEG_X)),
        (4, 14, m(Vec3::Z)),
        (5, 15, m(Vec3::NEG_Z)),
    ];
    // Warm slot 1's layer first, as an earlier frame would have.
    let earlier = cache.plan_frame(&dynamic_inputs[..1], &[]);
    cache.mark_spot_world_rendered(earlier.spot()[0]);
    let dynamic = cache.plan_frame(&dynamic_inputs, &[]);
    assert_eq!(dynamic.spot().len(), 3, "slot 5 is past capacity");
    let promoted = promoted_plan(&[(0, false), (3, true)]);
    let slots = [
        (0, m(Vec3::Y)),
        (1, m(Vec3::X)),
        (2, m(Vec3::NEG_X)),
        (3, m(Vec3::NEG_Y)),
        (4, m(Vec3::Z)),
        (5, m(Vec3::NEG_Z)),
    ];
    let walked = record_spot_frame(&mut draws, &slots, &promoted, &dynamic);
    assert_eq!(walked, vec![2, 3, 4, 5]);
    assert_eq!(draws.walks, 4);

    // Next frame: the cold fills are warm, the promoted cold fill is warm, and
    // the uncached slot walks again with an unchanged matrix.
    for plan in dynamic.spot() {
        cache.mark_spot_world_rendered(*plan);
    }
    let dynamic = cache.plan_frame(&dynamic_inputs, &[]);
    let promoted = promoted_plan(&[(0, false), (3, false)]);
    assert_eq!(
        record_spot_frame(&mut draws, &slots, &promoted, &dynamic),
        vec![5]
    );
}

#[test]
fn retenanted_layer_cold_fills_from_its_new_tenants_matrix() {
    // O3.
    let tree = axes();
    let mut draws = ShadowWorldDraws::install(Some(&tree), index_count(&tree));
    let promoted = PromotedDepthCacheFramePlan::default();
    let toward_x = spot(Vec3::ZERO, Vec3::X);
    let toward_z = spot(Vec3::ZERO, Vec3::Z);
    let mut cache = DynamicDepthCache::default();
    let first = cache.plan_frame(&[(0, 10, toward_x)], &[]);
    cache.mark_spot_world_rendered(first.spot()[0]);

    // Light 10 leaves; light 20 arrives in the same frame and takes layer 0.
    let next = cache.plan_frame(&[(0, 20, toward_z)], &[]);
    assert_eq!(next.spot()[0].cache_layer, first.spot()[0].cache_layer);
    assert_eq!(
        record_spot_frame(&mut draws, &[(0, toward_z)], &promoted, &next),
        vec![0]
    );
    assert_eq!(cells_of(&draws.trace[0].ranges), vec![4]);

    // Light 20 keeps its matrix but moves to slot 6: still warm, no walk.
    cache.mark_spot_world_rendered(next.spot()[0]);
    let moved = cache.plan_frame(&[(6, 20, toward_z)], &[]);
    assert!(record_spot_frame(&mut draws, &[(6, toward_z)], &promoted, &moved).is_empty());
}

#[test]
fn cold_fill_then_warm_then_rekey_or_relight_draws_reach_again() {
    let tree = axes();
    let mut draws = ShadowWorldDraws::install(Some(&tree), index_count(&tree));
    let promoted = PromotedDepthCacheFramePlan::default();
    let a = spot(Vec3::ZERO, Vec3::X);
    let b = spot(Vec3::ZERO, Vec3::Y);
    let mut cache = DynamicDepthCache::default();
    let mut frame = |cache: &mut DynamicDepthCache, inputs: &[(u32, usize, Mat4)]| {
        let plan = cache.plan_frame(inputs, &[]);
        let slots: Vec<_> = inputs.iter().map(|&(slot, _, m)| (slot, m)).collect();
        let walked = record_spot_frame(&mut draws, &slots, &promoted, &plan);
        for spot_plan in plan.spot() {
            cache.mark_spot_world_rendered(*spot_plan);
        }
        walked
    };
    assert_eq!(frame(&mut cache, &[(0, 10, a)]), vec![0], "cold fill");
    assert!(frame(&mut cache, &[(0, 10, a)]).is_empty(), "warm");
    assert_eq!(frame(&mut cache, &[(0, 10, b)]), vec![0], "re-key");
    assert!(frame(&mut cache, &[(0, 10, b)]).is_empty(), "warm again");
    // Below the brightness gate the light holds no slot for a frame.
    assert!(frame(&mut cache, &[]).is_empty());
    assert_eq!(frame(&mut cache, &[(0, 10, b)]), vec![0], "re-lit");
}

#[test]
fn building_draw_lists_after_warmup_allocates_nothing_even_for_a_record_reach() {
    let centres: Vec<Vec3> = (0..64)
        .map(|i| Vec3::new((i % 8) as f32 * 2.0, 0.0, -2.0 * (i / 8) as f32 - 2.0))
        .collect();
    let tree = world(&centres);
    let index = ShadowReachIndex::new(&tree.nodes, &tree.leaves);
    let mut scratch = index.scratch();
    // Warm up on a small reach only.
    let small = cone_frustum_planes(&spot(Vec3::new(0.0, 0.0, 1.0), Vec3::NEG_Z));
    let small_len = index.reach(&small, &mut scratch).len();
    let everything = cone_frustum_planes(&cube(Vec3::new(7.0, 0.0, -8.0))[5]);
    let wide = cone_frustum_planes(&glam::camera::rh::proj::directx::orthographic(
        -100.0, 100.0, -100.0, 100.0, -100.0, 100.0,
    ));
    let probe = AllocSnapshot::arm();
    let mut most_cells = 0;
    let mut most_ranges = 0;
    for planes in [&wide, &everything, &small, &wide] {
        most_ranges = most_ranges.max(index.reach(planes, &mut scratch).len());
        most_cells = most_cells.max(scratch.cells().len());
    }
    assert_eq!(probe.allocs_since(), 0);
    assert_eq!(most_cells, 64, "a record reach: every cell");
    assert!(most_ranges >= small_len);
}
