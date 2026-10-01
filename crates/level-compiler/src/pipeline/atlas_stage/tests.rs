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

// P1, P2: after a cut, leaf face ranges, the BVH and CellDrawIndex all
// agree with the emitted geometry, and sub-faces stay in their parent's leaf.
#[test]
fn rebuilt_face_identity_agrees_with_cut_geometry() {
    let (mut geometry, mut leaves) = three_leaf_fixture();
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
// fails by name before anything is cut.
#[test]
fn scale_region_past_the_bake_layer_cap_fails_by_name_before_cutting() {
    let (mut geometry, _) = three_leaf_fixture();
    let before = geometry.geometry.vertices.len();
    let lights = lit();
    let static_lights = StaticBakedLights::from_lights(&lights);
    let region = crate::map_data::MapLightmapScaleRegion {
        min: [1.0, -1.0, -1.0],
        max: [12.0, 1.0, 12.0],
        planes: Vec::new(),
        scale: 2000.0,
    };
    let error = match lightmap_bake::plan_cut_charts(
        &mut geometry,
        &static_lights,
        0.1,
        std::slice::from_ref(&region),
        2048,
    ) {
        Err(error) => error,
        Ok(_) => panic!("the region must overflow the bake layers"),
    };
    assert!(
        matches!(error, crate::lightmap_bake::LightmapBakeError::LayerOverflow { max, .. } if max == crate::lightmap_bake::MAX_ATLAS_LAYERS),
        "{error}"
    );
    assert!(error.to_string().contains("layer overflow"), "{error}");
    assert!(geometry.face_index_ranges.len() == 3 && geometry.geometry.vertices.len() >= before);
}
