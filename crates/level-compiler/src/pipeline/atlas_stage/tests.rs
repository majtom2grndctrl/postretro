// Atlas stage tests: the face-identity rebuild after an oversize-face cut.
// See: context/lib/build_pipeline.md §Compiler pipeline (atlas preparation)

use std::collections::BTreeSet;
use std::sync::Arc;

use glam::DVec3;
use postretro_level_format::bsp::BspLeafRecord;
use postretro_level_format::geometry::{FaceMeta, GeometrySection, Vertex};
use postretro_level_format::texture_names::TextureNamesSection;

use super::*;
use crate::geometry::FaceIndexRange;
use crate::governor::Governor;
use crate::map_data::{FalloffModel, LightType, MapLight, ShadowType};
use crate::reporter::StageProgress;

/// A floor quad `[x, x + size]²` at `y = 0`, facing `+Y`, in `leaf`.
fn push_quad(geometry: &mut GeometryResult, x: f32, size: f32, leaf: u32) {
    let base = geometry.geometry.vertices.len() as u32;
    for [dx, dz] in [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]] {
        geometry.geometry.vertices.push(Vertex::new(
            [x + dx * size, 0.0, dz * size],
            [dx, dz],
            [0.0, 1.0, 0.0],
            [1.0, 0.0, 0.0],
            true,
            [0.0, 0.0],
            0,
        ));
    }
    let index_offset = geometry.geometry.indices.len() as u32;
    geometry
        .geometry
        .indices
        .extend([base, base + 1, base + 2, base, base + 2, base + 3]);
    geometry.geometry.faces.push(FaceMeta {
        leaf_index: leaf,
        texture_index: 0,
    });
    geometry.face_index_ranges.push(FaceIndexRange {
        index_offset,
        index_count: 6,
    });
}

/// Leaf 0 solid; leaf 1 a small quad then a 9 m quad; leaf 2 a small quad.
/// At 0.1 m/texel the 9 m quad's chart passes a 64-texel test pool edge.
fn three_leaf_fixture() -> (GeometryResult, BspLeavesSection) {
    let mut geometry = GeometryResult {
        geometry: GeometrySection {
            vertices: Vec::new(),
            indices: Vec::new(),
            faces: Vec::new(),
        },
        texture_names: TextureNamesSection { names: Vec::new() },
        face_index_ranges: Vec::new(),
    };
    push_quad(&mut geometry, 0.0, 1.0, 1);
    push_quad(&mut geometry, 2.0, 9.0, 1);
    push_quad(&mut geometry, 20.0, 1.0, 2);
    let leaf = |face_start, face_count, is_solid| BspLeafRecord {
        face_start,
        face_count,
        bounds_min: [-50.0; 3],
        bounds_max: [50.0; 3],
        is_solid,
    };
    let leaves = BspLeavesSection {
        leaves: vec![leaf(0, 0, 1), leaf(0, 2, 0), leaf(2, 1, 0)],
    };
    (geometry, leaves)
}

fn lit() -> Vec<MapLight> {
    vec![MapLight {
        origin: DVec3::new(5.0, 3.0, 5.0),
        carrier: String::new(),
        light_type: LightType::Point,
        intensity: 1.0,
        color: [1.0; 3],
        falloff_model: FalloffModel::Linear,
        falloff_range: 20.0,
        light_size: 0.0,
        angular_diameter: 0.0,
        cone_angle_inner: None,
        cone_angle_outer: None,
        cone_direction: None,
        animation: None,
        bake_only: false,
        is_dynamic: false,
        casts_entity_shadows: false,
        is_animated: false,
        tags: Vec::new(),
        shadow_type: ShadowType::StaticLightMap,
    }]
}

/// Cut `geometry` at `pool_edge` and, when a face was cut, rebuild.
fn cut_and_rebuild(
    geometry: &mut GeometryResult,
    leaves: &mut BspLeavesSection,
    pool_edge: u32,
) -> (
    Vec<crate::lightmap_bake::Chart>,
    Option<RebuiltFaceIdentity>,
) {
    let lights = lit();
    let static_lights = StaticBakedLights::from_lights(&lights);
    let cut = lightmap_bake::plan_cut_charts(geometry, &static_lights, 0.1, &[], pool_edge)
        .expect("fixture charts plan");
    let rebuilt = cut
        .face_remap
        .as_ref()
        .map(|remap| rebuild_face_identity(geometry, leaves, remap).expect("rebuild"));
    (cut.charts, rebuilt)
}

// P4: with nothing to cut the identity remap rebuilds exactly the leaf face
// ranges, BVH and CellDrawIndex the pre-atlas stages built.
#[test]
fn empty_cut_rebuilds_the_pre_atlas_face_identity_set_unchanged() {
    let (mut geometry, leaves) = three_leaf_fixture();
    let (_, _, bvh_before) = build_bvh(&geometry).unwrap();
    let draw_before = bake_cell_draw_index(&bvh_before.leaves, &leaves.leaves);

    let mut rebuilt_leaves = leaves.clone();
    let (charts, rebuilt) = cut_and_rebuild(&mut geometry, &mut rebuilt_leaves, 2048);
    assert!(rebuilt.is_none(), "nothing past the pool edge is cut");
    let identity: Vec<_> = (0..charts.len()).map(|face| face..face + 1).collect();
    let rebuilt = rebuild_face_identity(&geometry, &mut rebuilt_leaves, &identity).unwrap();
    assert_eq!(rebuilt_leaves, leaves);
    assert_eq!(rebuilt.bvh_section, bvh_before);
    assert_eq!(rebuilt.cell_draw_index, draw_before);
}

/// The cell partition planned over `leaves` and `bvh`, as atlas preparation
/// plans it (no portals, no streaming regions).
fn partition_over(leaves: &BspLeavesSection, bvh: &BvhSection) -> CellPartitionPlan {
    plan_cell_partition(CellPartitionInputs {
        generated_portals: &[],
        streaming_seam_regions: &[],
        stream_resident_regions: &[],
        stream_priority_regions: &[],
        leaves,
        exterior_leaves: &std::collections::HashSet::new(),
        bvh,
    })
    .expect("the fixture partitions")
}

// P1, P2, P4: after a cut, leaf face ranges, the BVH and CellDrawIndex all
// agree with the emitted geometry, sub-faces stay in their parent's leaf,
// and the cell partition counts the sub-faces, not the parent.
#[test]
fn rebuilt_face_identity_agrees_with_cut_geometry() {
    let (mut geometry, mut leaves) = three_leaf_fixture();
    let (_, _, bvh_before) = build_bvh(&geometry).unwrap();
    let partition_before = partition_over(&leaves, &bvh_before);
    let (charts, rebuilt) = cut_and_rebuild(&mut geometry, &mut leaves, 64);
    let rebuilt = rebuilt.expect("the 9 m quad is cut");
    let faces = geometry.face_index_ranges.len();
    assert!(faces > 3);
    assert_eq!(charts.len(), faces);

    // Leaf ranges tile the faces in order, each face in its own leaf.
    assert_eq!(leaves.leaves[1].face_start, 0);
    assert_eq!(leaves.leaves[1].face_count as usize, faces - 1);
    assert_eq!(leaves.leaves[2].face_start as usize, faces - 1);
    assert_eq!(leaves.leaves[2].face_count, 1);
    for (leaf_index, leaf) in leaves.leaves.iter().enumerate() {
        let range = leaf.face_start as usize..(leaf.face_start + leaf.face_count) as usize;
        for face in range {
            assert_eq!(
                geometry.geometry.faces[face].leaf_index as usize,
                leaf_index
            );
        }
    }

    // One BVH leaf per face, on the face's index range and cell.
    let face_offsets: BTreeSet<(u32, u32, u32)> = geometry
        .face_index_ranges
        .iter()
        .zip(&geometry.geometry.faces)
        .map(|(range, meta)| (range.index_offset, range.index_count, meta.leaf_index))
        .collect();
    let bvh_leaves: BTreeSet<(u32, u32, u32)> = rebuilt
        .bvh_section
        .leaves
        .iter()
        .map(|leaf| (leaf.index_offset, leaf.index_count, leaf.cell_id))
        .collect();
    assert_eq!(bvh_leaves, face_offsets);
    assert_eq!(rebuilt.primitives.len(), faces);

    // Every CellDrawIndex span names BVH leaves of its own cell, and the
    // spans cover every leaf once.
    let draw = rebuilt.cell_draw_index.expect("non-empty BVH");
    let mut covered = 0;
    for cell in 0..draw.cell_count as usize {
        let spans = &draw.spans
            [draw.cell_span_offset[cell] as usize..draw.cell_span_offset[cell + 1] as usize];
        for span in spans {
            for leaf in span.leaf_start..span.leaf_start + span.leaf_count {
                assert_eq!(
                    rebuilt.bvh_section.leaves[leaf as usize].cell_id as usize,
                    cell
                );
                covered += 1;
            }
        }
    }
    assert_eq!(covered, rebuilt.bvh_section.leaves.len());

    // The partition reads the rebuilt leaves and BVH: its clusters count
    // every face the cut emitted, where the pre-cut plan counted three.
    let primitives = |plan: &CellPartitionPlan| {
        plan.partition
            .clusters
            .iter()
            .map(|cluster| cluster.primitive_count as usize)
            .sum::<usize>()
    };
    assert_eq!(primitives(&partition_before), 3);
    let partition = partition_over(&leaves, &rebuilt.bvh_section);
    assert_eq!(primitives(&partition), faces);
    // The Cells it encodes, which the pack stage emits, carry the remapped
    // face ranges.
    let cell_ranges: Vec<(u32, u32)> = partition
        .cells
        .cells
        .iter()
        .map(|cell| (cell.face_start, cell.face_count))
        .collect();
    let leaf_ranges: Vec<(u32, u32)> = leaves
        .leaves
        .iter()
        .map(|leaf| (leaf.face_start, leaf.face_count))
        .collect();
    assert_eq!(cell_ranges, leaf_ranges);
}

// Face cuts and the rebuild are identical with one worker and with many.
#[test]
fn cut_and_rebuild_are_identical_with_one_worker_and_many() {
    let run = |workers: usize| {
        rayon::ThreadPoolBuilder::new()
            .num_threads(workers)
            .build()
            .unwrap()
            .install(|| {
                let (mut geometry, mut leaves) = three_leaf_fixture();
                let (charts, rebuilt) = cut_and_rebuild(&mut geometry, &mut leaves, 64);
                let rebuilt = rebuilt.expect("cut");
                (
                    geometry.geometry.vertices,
                    geometry.geometry.indices,
                    format!("{charts:?}"),
                    leaves,
                    rebuilt.bvh_section,
                    rebuilt.cell_draw_index,
                )
            })
    };
    assert_eq!(run(1), run(4));
}

// P19: without static lights nothing is cut; the oversize chart just goes
// unplaced, as before.
#[test]
fn map_without_static_lights_cuts_nothing() {
    let (mut geometry, _) = three_leaf_fixture();
    let before = geometry.geometry.vertices.clone();
    let static_lights = StaticBakedLights::from_lights(&[]);
    let cut =
        lightmap_bake::plan_cut_charts(&mut geometry, &static_lights, 0.1, &[], 64).expect("plans");
    assert!(cut.face_remap.is_none() && cut.pre_cut.is_none());
    assert_eq!(cut.charts.len(), 3);
    assert_eq!(geometry.geometry.vertices, before);
    let control = BakeControl::new(
        Arc::new(Governor::new(1, false)),
        &StageProgress::indeterminate(),
    );
    let prepared = lightmap_bake::pack_cut_charts(
        &mut geometry,
        &static_lights,
        cut.charts,
        BlockOrdering::by_cell_id(2),
        64,
        &control,
    )
    .expect("an unlit map compiles");
    assert!(
        prepared.placements.is_empty(),
        "the oversize chart is never packed"
    );
}

// P11: a scale region extreme enough to need more bake layers than allowed
// fails by name before anything is cut — promptly, even when the region's
// faces would cut into millions of pieces and their area overflows a u64.
#[test]
fn scale_region_past_the_bake_layer_cap_fails_by_name_before_cutting() {
    for scale in [2000.0, 1.0e6, 1.0e7] {
        let (mut geometry, _) = three_leaf_fixture();
        let before = geometry.geometry.vertices.len();
        let lights = lit();
        let static_lights = StaticBakedLights::from_lights(&lights);
        let region = crate::map_data::MapLightmapScaleRegion {
            min: [1.0, -1.0, -1.0],
            max: [12.0, 1.0, 12.0],
            planes: Vec::new(),
            scale,
        };
        let started = std::time::Instant::now();
        let error = match lightmap_bake::plan_cut_charts(
            &mut geometry,
            &static_lights,
            0.1,
            std::slice::from_ref(&region),
            2048,
        ) {
            Err(error) => error,
            Ok(_) => panic!("scale {scale}: the region must overflow the bake layers"),
        };
        assert!(
            matches!(error, crate::lightmap_bake::LightmapBakeError::LayerOverflow { max, .. } if max == crate::lightmap_bake::MAX_ATLAS_LAYERS),
            "scale {scale}: {error}"
        );
        assert!(error.to_string().contains("layer overflow"), "{error}");
        assert!(
            geometry.face_index_ranges.len() == 3 && geometry.geometry.vertices.len() >= before
        );
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "scale {scale}: the limit took {:?}",
            started.elapsed()
        );
    }
}

// P8, and the animated term of the overlap contract: a cut face lit by an
// animated light compiles through the animated path — chunks, weight maps,
// the compact atlas layout and its shared-vertex and footprint guards — and
// overlap texels carry bit-identical animated weights on both sides.
#[test]
fn cut_face_lit_by_an_animated_light_passes_the_guards_with_matching_overlap_weights() {
    use std::collections::HashMap;

    use crate::animated_light_chunks::build_placed_animated_light_chunks;
    use crate::animated_light_weight_maps::{
        WeightMapInputs, bake_animated_light_weight_maps_controlled,
    };
    use crate::chart_raster::CHART_PADDING_TEXELS;
    use crate::light_namespaces::AnimatedBakedLights;
    use crate::map_data::LightAnimation;

    // A 9 m floor under a 2 m occluder straddling its cut lines, in cell 1.
    let (mut geometry, _) = three_leaf_fixture();
    geometry.geometry.vertices.truncate(0);
    geometry.geometry.indices.truncate(0);
    geometry.geometry.faces.truncate(0);
    geometry.face_index_ranges.truncate(0);
    push_quad(&mut geometry, 0.0, 9.0, 1);
    push_quad(&mut geometry, 3.5, 2.0, 1);
    for vertex in &mut geometry.geometry.vertices[4..] {
        vertex.position[1] = 1.0;
        vertex.position[2] += 3.5;
    }

    let mut lights = lit();
    let mut animated = lights[0].clone();
    animated.origin = DVec3::new(4.3, 2.5, 4.6);
    animated.light_size = 1.0;
    animated.falloff_range = 9.0;
    animated.animation = Some(LightAnimation {
        period: 1.0,
        phase: 0.0,
        brightness: Some(vec![1.0, 0.5]),
        color: None,
        direction: None,
        start_active: true,
    });
    lights.push(animated);
    let static_lights = StaticBakedLights::from_lights(&lights);
    let animated_lights = AnimatedBakedLights::from_lights(&lights);
    assert_eq!(animated_lights.entries().len(), 1);

    let prepared = lightmap_bake::prepare_atlas_within(
        &mut geometry,
        &static_lights,
        0.1,
        &[],
        BlockOrdering::by_cell_id(2),
        64,
        &BakeControl::unrestricted(),
    )
    .expect("the cut floor prepares");
    assert!(
        prepared
            .charts
            .iter()
            .filter(|c| c.window.is_some())
            .count()
            >= 4
    );
    let (bvh, primitives, bvh_section) = build_bvh(&geometry).unwrap();
    let (chunks, _) = build_placed_animated_light_chunks(
        &bvh_section,
        &animated_lights,
        &prepared.charts,
        &prepared.placements,
        &geometry.face_index_ranges,
        0.1,
    );
    assert!(!chunks.chunks.is_empty());
    let (animated_list, _) = animated_lights.to_parallel_vecs();
    let mut weight_maps = bake_animated_light_weight_maps_controlled(
        &WeightMapInputs {
            bvh: &bvh,
            primitives: &primitives,
            geometry: &geometry,
            chunk_section: &chunks,
            lights: &animated_list,
            face_charts: &prepared.charts,
            face_placements: &prepared.placements,
            layout: &prepared.layout,
            atlas_width: prepared.atlas_width,
            atlas_height: prepared.atlas_height,
            static_atlas_layer_count: prepared.layer_count,
            area_sample_count: 8,
        },
        &BakeControl::unrestricted(),
    )
    .expect("the animated weights bake");

    // Every texel's weights, keyed by its face's parent-grid texel.
    type TexelWeights = Vec<(u32, u32, [u16; 2])>;
    let mut by_grid: HashMap<[u32; 2], Vec<(usize, TexelWeights)>> = HashMap::new();
    for (index, (chunk, rect)) in chunks
        .chunks
        .iter()
        .zip(&weight_maps.chunk_rects)
        .enumerate()
    {
        let face = chunk.face_index as usize;
        let Some(origin) = prepared.charts[face].window.map(|w| w.origin) else {
            continue;
        };
        let (_, block_x, block_y) = weight_maps
            .chunk_block_origin(index)
            .expect("chunk inside its block");
        let (local_x, local_y) = prepared
            .layout
            .local_placement(face, &prepared.placements[face]);
        for ry in 0..rect.height {
            for rx in 0..rect.width {
                let entry =
                    weight_maps.offset_counts[(rect.texel_offset + ry * rect.width + rx) as usize];
                let weights = weight_maps.texel_lights
                    [entry.offset as usize..(entry.offset + entry.count) as usize]
                    .iter()
                    .map(|t| (t.light_index, t.weight.to_bits(), t.direction_oct))
                    .collect();
                let (cx, cy) = (block_x + rx, block_y + ry);
                if cx < local_x + CHART_PADDING_TEXELS || cy < local_y + CHART_PADDING_TEXELS {
                    continue;
                }
                let grid = [
                    cx - local_x - CHART_PADDING_TEXELS + origin[0],
                    cy - local_y - CHART_PADDING_TEXELS + origin[1],
                ];
                by_grid.entry(grid).or_default().push((face, weights));
            }
        }
    }
    let mut compared = 0;
    for texels in by_grid.values() {
        for pair in texels.windows(2) {
            if pair[0].0 != pair[1].0 {
                assert_eq!(pair[0].1, pair[1].1, "animated weights differ across a cut");
                compared += usize::from(!pair[0].1.is_empty());
            }
        }
    }
    assert!(compared > 0, "lit overlap texels were compared");

    let layout = super::super::animated_atlas_stage::layout_animated_atlas(
        &mut weight_maps,
        &chunks,
        &geometry,
        &prepared.layout,
        prepared.atlas_width,
        false,
    )
    .expect("the shared-vertex and footprint guards pass");
    assert!(layout.is_some_and(|blocks| blocks.iter().flatten().count() >= 4));
}

// More animated lights than a chunk holds, over a sub-chart thousands of
// texels from its frame's origin: chunks split inside the window, and each
// chunk's atlas rect spans exactly the texels its UV range holds — no
// sibling overlap (a release panic) and no texel row left without weights.
#[test]
fn chunks_split_far_inside_a_cut_face_own_every_texel_once() {
    use crate::animated_light_chunks::build_placed_animated_light_chunks;
    use crate::animated_light_weight_maps::{
        WeightMapInputs, bake_animated_light_weight_maps_controlled,
    };
    use crate::chart_raster::chart_interior_dims;
    use crate::light_namespaces::AnimatedBakedLights;
    use crate::map_data::LightAnimation;
    use postretro_level_format::animated_light_chunks::MAX_ANIMATED_LIGHTS_PER_CHUNK;

    // A 400 m × 4 m strip at 0.1 m/texel: 4000 texels, cut once.
    let (mut geometry, _) = three_leaf_fixture();
    geometry.geometry.vertices.clear();
    geometry.geometry.indices.clear();
    geometry.geometry.faces.clear();
    geometry.face_index_ranges.clear();
    push_quad(&mut geometry, 0.0, 400.0, 1);
    for vertex in &mut geometry.geometry.vertices {
        vertex.position[2] *= 0.01;
    }

    let mut lights = lit();
    for i in 0..6 {
        let mut animated = lights[0].clone();
        animated.origin = DVec3::new(263.7 + 7.3 * f64::from(i), 1.0, 2.0);
        animated.falloff_range = 4.0;
        animated.animation = Some(LightAnimation {
            period: 1.0,
            phase: 0.1 * i as f32,
            brightness: Some(vec![1.0, 0.5]),
            color: None,
            direction: None,
            start_active: true,
        });
        lights.push(animated);
    }
    let static_lights = StaticBakedLights::from_lights(&lights);
    let animated_lights = AnimatedBakedLights::from_lights(&lights);
    assert!(animated_lights.entries().len() > MAX_ANIMATED_LIGHTS_PER_CHUNK);

    let prepared = lightmap_bake::prepare_atlas_within(
        &mut geometry,
        &static_lights,
        0.1,
        &[],
        BlockOrdering::by_cell_id(2),
        2048,
        &BakeControl::unrestricted(),
    )
    .expect("the cut strip prepares");
    let far = prepared
        .charts
        .iter()
        .position(|c| c.window.is_some_and(|w| w.origin[0] > 1000))
        .expect("a far sub-chart");
    let (bvh, primitives, bvh_section) = build_bvh(&geometry).unwrap();
    let (chunks, _) = build_placed_animated_light_chunks(
        &bvh_section,
        &animated_lights,
        &prepared.charts,
        &prepared.placements,
        &geometry.face_index_ranges,
        0.1,
    );
    let far_chunks = chunks
        .chunks
        .iter()
        .filter(|c| c.face_index as usize == far)
        .count();
    assert!(far_chunks > 1, "chunks split in the far sub-chart");

    let (animated_list, _) = animated_lights.to_parallel_vecs();
    let mut weight_maps = bake_animated_light_weight_maps_controlled(
        &WeightMapInputs {
            bvh: &bvh,
            primitives: &primitives,
            geometry: &geometry,
            chunk_section: &chunks,
            lights: &animated_list,
            face_charts: &prepared.charts,
            face_placements: &prepared.placements,
            layout: &prepared.layout,
            atlas_width: prepared.atlas_width,
            atlas_height: prepared.atlas_height,
            static_atlas_layer_count: prepared.layer_count,
            area_sample_count: 1,
        },
        &BakeControl::unrestricted(),
    )
    .expect("sibling chunk rects do not overlap");

    for (chunk, rect) in chunks.chunks.iter().zip(&weight_maps.chunk_rects) {
        let chart = &prepared.charts[chunk.face_index as usize];
        let (iw, ih) = chart_interior_dims(chart);
        let texels = |axis: usize, interior: i32| {
            let scale = f64::from(interior) / f64::from(chart.uv_extent[axis]);
            (f64::from(chunk.uv_max[axis] - chunk.uv_min[axis]) * scale).round() as u32
        };
        assert_eq!(
            (rect.width, rect.height),
            (texels(0, iw), texels(1, ih)),
            "face {} chunk {:?}..{:?}",
            chunk.face_index,
            chunk.uv_min,
            chunk.uv_max
        );
    }

    super::super::animated_atlas_stage::layout_animated_atlas(
        &mut weight_maps,
        &chunks,
        &geometry,
        &prepared.layout,
        prepared.atlas_width,
        false,
    )
    .expect("the shared-vertex and footprint guards pass");
}

// P11: more faces within animated reach than the animated block table holds
// fails at atlas preparation, before any bake, on the animated cap's error.
#[test]
fn animated_reach_past_the_block_cap_fails_by_name_at_atlas_preparation() {
    use crate::light_namespaces::AnimatedBakedLights;
    use crate::map_data::LightAnimation;

    let (mut geometry, _) = three_leaf_fixture();
    let lights = lit();
    let static_lights = StaticBakedLights::from_lights(&lights);
    let chart = lightmap_bake::plan_cut_charts(&mut geometry, &static_lights, 0.1, &[], 2048)
        .unwrap()
        .charts
        .remove(0);
    let mut animated = lights[0].clone();
    animated.animation = Some(LightAnimation {
        period: 1.0,
        phase: 0.0,
        brightness: Some(vec![1.0, 0.5]),
        color: None,
        direction: None,
        start_active: true,
    });
    let animated = vec![animated];
    let animated_lights = AnimatedBakedLights::from_lights(&animated);

    let range = geometry.face_index_ranges[0];
    let ranges = vec![range; ANIMATED_BLOCK_CAP as usize + 1];
    let at_cap = vec![chart.clone(); ANIMATED_BLOCK_CAP as usize];
    assert!(check_animated_block_bound(&animated_lights, &at_cap, &ranges[1..]).is_ok());
    let past_cap = vec![chart; ANIMATED_BLOCK_CAP as usize + 1];
    // Faces without geometry emit no chunk, so they count toward nothing.
    let mut empty = ranges.clone();
    empty[0].index_count = 0;
    assert!(check_animated_block_bound(&animated_lights, &past_cap, &empty).is_ok());
    let error = check_animated_block_bound(&animated_lights, &past_cap, &ranges)
        .expect_err("one face past the cap fails")
        .to_string();
    assert!(error.contains("exceeds the block-table cap"), "{error}");
    assert!(error.contains(&ANIMATED_BLOCK_CAP.to_string()), "{error}");
}

/// A `size`-metre floor quad at `x`, facing `+Y`, as brush-side projection
/// emits it: engine-space winding, no geometry or index ranges yet.
fn floor_face(x: f64, size: f64, texture: &str) -> crate::map_data::Face {
    crate::map_data::Face {
        vertices: [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]]
            .into_iter()
            .map(|[dx, dz]| DVec3::new(x + dx * size, 0.0, dz * size))
            .collect(),
        normal: DVec3::Y,
        distance: 0.0,
        texture: texture.to_string(),
        tex_projection: crate::map_data::TextureProjection::Standard {
            u_offset: 0.0,
            v_offset: 0.0,
            angle: 0.0,
            scale_u: 1.0,
            scale_v: 1.0,
        },
        brush_index: 0,
    }
}

/// Each cell's leaves, sorted by offset, abut into one range; consecutive
/// cells' ranges abut; and the leaves cover every index exactly once.
fn assert_cell_major_leaf_ranges(
    leaves: &[postretro_level_format::bvh::BvhLeaf],
    index_count: usize,
) {
    let mut by_cell: std::collections::BTreeMap<u32, Vec<(u32, u32)>> = Default::default();
    for leaf in leaves {
        by_cell
            .entry(leaf.cell_id)
            .or_default()
            .push((leaf.index_offset, leaf.index_count));
    }
    let mut next_cell_start = 0;
    for (cell, ranges) in &mut by_cell {
        ranges.sort_unstable();
        for pair in ranges.windows(2) {
            assert_eq!(
                pair[0].0 + pair[0].1,
                pair[1].0,
                "cell {cell}: leaf ranges {pair:?} leave a gap or overlap"
            );
        }
        assert_eq!(
            ranges[0].0, next_cell_start,
            "cell {cell} does not start where the previous cell ended"
        );
        let (last_offset, last_count) = *ranges.last().expect("a grouped cell has a leaf");
        next_cell_start = last_offset + last_count;
    }
    assert_eq!(next_cell_start as usize, index_count);

    let mut coverage = vec![0u32; index_count];
    for leaf in leaves {
        for slot in leaf.index_offset..leaf.index_offset + leaf.index_count {
            coverage[slot as usize] += 1;
        }
    }
    assert!(
        coverage.iter().all(|&count| count == 1),
        "every index is covered by exactly one leaf: {coverage:?}"
    );
}

// D3 (shadow-fill-cost): runtime shadow reach (`render-cpu` `shadow_reach`)
// merges each reached cell's leaf ranges into one draw on this cell-major order.
#[test]
fn face_cut_bvh_leaves_keep_each_cell_one_contiguous_index_range() {
    use crate::partition::{Aabb, BspLeaf, BspTree};

    // Input order fixes texture indices, so material buckets (`ceiling` 0,
    // `floor` 1, `trim` 2) cut across cell order. Cell 1 holds three faces
    // with the 9 m floor, cut at a 64-texel pool edge, in the middle.
    let faces = vec![
        floor_face(40.0, 1.0, "ceiling"),
        floor_face(0.0, 1.0, "floor"),
        floor_face(2.0, 9.0, "floor"),
        floor_face(12.0, 1.0, "trim"),
        floor_face(20.0, 1.0, "trim"),
        floor_face(22.0, 1.0, "ceiling"),
    ];
    let leaf = |face_indices: Vec<usize>, is_solid| BspLeaf {
        face_indices,
        bounds: Aabb {
            min: DVec3::splat(-50.0),
            max: DVec3::splat(50.0),
        },
        is_solid,
        defining_planes: Vec::new(),
    };
    let tree = BspTree {
        nodes: Vec::new(),
        leaves: vec![
            leaf(Vec::new(), true),
            leaf(vec![1, 2, 3], false),
            leaf(vec![4, 5], false),
            leaf(vec![0], false),
        ],
    };

    // Production order: leaves and geometry from the tree, the pre-atlas
    // BVH, then atlas preparation's cut and face-identity rebuild.
    let exterior = std::collections::HashSet::new();
    let mut leaves = crate::visibility::encode_vis(&tree, &exterior).leaves_section;
    let mut geometry = crate::geometry::extract_geometry(&faces, &tree, &exterior);
    let faces_before = geometry.face_index_ranges.len();
    let (_, _, bvh_before) = build_bvh(&geometry).unwrap();
    assert_cell_major_leaf_ranges(&bvh_before.leaves, geometry.geometry.indices.len());

    let (_, rebuilt) = cut_and_rebuild(&mut geometry, &mut leaves, 64);
    let rebuilt = rebuilt.expect("the 9 m floor is cut");
    let cell_one_faces = geometry
        .geometry
        .faces
        .iter()
        .filter(|face| face.leaf_index == 1)
        .count();
    assert!(
        geometry.face_index_ranges.len() > faces_before && cell_one_faces > 3,
        "the cut emits sub-faces in cell 1: {cell_one_faces} faces there"
    );

    let bvh_leaves = &rebuilt.bvh_section.leaves;
    assert_eq!(bvh_leaves.len(), geometry.face_index_ranges.len());
    assert!(
        bvh_leaves
            .windows(2)
            .any(|pair| pair[0].cell_id > pair[1].cell_id),
        "bucket-sorted leaves must not already be in cell order"
    );
    assert_cell_major_leaf_ranges(bvh_leaves, geometry.geometry.indices.len());
}
