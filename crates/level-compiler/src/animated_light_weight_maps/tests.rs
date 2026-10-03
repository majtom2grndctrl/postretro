// Animated-light weight-map tests: chunk rects, soft weights, culling, budget, and cache keys.
// See: context/lib/build_pipeline.md §Build Cache

use super::*;
use crate::bvh_build::build_bvh;
use crate::geometry::FaceIndexRange;
use crate::lightmap_bake::{BlockLayout, pack_layers};
use crate::map_data::{FalloffModel, LightAnimation, LightType};
use glam::DVec3;
use log::Level;
use postretro_level_format::animated_light_chunks::{
    AnimatedLightChunk, AnimatedLightChunksSection,
};
use postretro_level_format::animated_light_weight_maps::AnimatedBlock;
use postretro_level_format::geometry::{FaceMeta, GeometrySection, Vertex};
use postretro_level_format::texture_names::TextureNamesSection;
use postretro_test_log_capture::LogCapture;

fn xz_quad_face(y: f32, normal_y: f32, vertex_base: f32) -> Vec<Vertex> {
    let n = [0.0, normal_y, 0.0];
    let t = [1.0, 0.0, 0.0];
    vec![
        Vertex::new([vertex_base, y, 0.0], [0.0, 0.0], n, t, true, [0.0, 0.0], 0),
        Vertex::new(
            [vertex_base + 1.0, y, 0.0],
            [1.0, 0.0],
            n,
            t,
            true,
            [0.0, 0.0],
            0,
        ),
        Vertex::new(
            [vertex_base + 1.0, y, 1.0],
            [1.0, 1.0],
            n,
            t,
            true,
            [0.0, 0.0],
            0,
        ),
        Vertex::new([vertex_base, y, 1.0], [0.0, 1.0], n, t, true, [0.0, 0.0], 0),
    ]
}

fn unit_floor_geometry() -> GeometryResult {
    GeometryResult {
        geometry: GeometrySection {
            vertices: xz_quad_face(0.0, 1.0, 0.0),
            indices: vec![0, 1, 2, 0, 2, 3],
            faces: vec![FaceMeta {
                leaf_index: 0,
                texture_index: 0,
            }],
        },
        texture_names: TextureNamesSection { names: Vec::new() },
        face_index_ranges: vec![FaceIndexRange {
            index_offset: 0,
            index_count: 6,
        }],
    }
}

fn two_separate_floor_geometry() -> GeometryResult {
    let mut vertices = xz_quad_face(0.0, 1.0, 0.0);
    vertices.extend(xz_quad_face(0.0, 1.0, 2.0));
    GeometryResult {
        geometry: GeometrySection {
            vertices,
            indices: vec![0, 1, 2, 0, 2, 3, 4, 5, 6, 4, 6, 7],
            faces: vec![
                FaceMeta {
                    leaf_index: 0,
                    texture_index: 0,
                },
                FaceMeta {
                    leaf_index: 1,
                    texture_index: 0,
                },
            ],
        },
        texture_names: TextureNamesSection { names: Vec::new() },
        face_index_ranges: vec![
            FaceIndexRange {
                index_offset: 0,
                index_count: 6,
            },
            FaceIndexRange {
                index_offset: 6,
                index_count: 6,
            },
        ],
    }
}

/// Floor (face 0) at y=0; ceiling blocker (face 1) at y=0.5 between light and floor.
fn floor_plus_blocker_geometry() -> GeometryResult {
    let mut vertices = xz_quad_face(0.0, 1.0, 0.0);
    // Ceiling larger than the floor so the shadow ray always hits it.
    let ceiling = vec![
        Vertex::new(
            [-1.0, 0.5, -1.0],
            [0.0, 0.0],
            [0.0, -1.0, 0.0],
            [1.0, 0.0, 0.0],
            true,
            [0.0, 0.0],
            0,
        ),
        Vertex::new(
            [2.0, 0.5, -1.0],
            [1.0, 0.0],
            [0.0, -1.0, 0.0],
            [1.0, 0.0, 0.0],
            true,
            [0.0, 0.0],
            0,
        ),
        Vertex::new(
            [2.0, 0.5, 2.0],
            [1.0, 1.0],
            [0.0, -1.0, 0.0],
            [1.0, 0.0, 0.0],
            true,
            [0.0, 0.0],
            0,
        ),
        Vertex::new(
            [-1.0, 0.5, 2.0],
            [0.0, 1.0],
            [0.0, -1.0, 0.0],
            [1.0, 0.0, 0.0],
            true,
            [0.0, 0.0],
            0,
        ),
    ];
    vertices.extend(ceiling);
    GeometryResult {
        geometry: GeometrySection {
            vertices,
            indices: vec![0, 1, 2, 0, 2, 3, 4, 5, 6, 4, 6, 7],
            faces: vec![
                FaceMeta {
                    leaf_index: 0,
                    texture_index: 0,
                },
                FaceMeta {
                    leaf_index: 0,
                    texture_index: 0,
                },
            ],
        },
        texture_names: TextureNamesSection { names: Vec::new() },
        face_index_ranges: vec![
            FaceIndexRange {
                index_offset: 0,
                index_count: 6,
            },
            FaceIndexRange {
                index_offset: 6,
                index_count: 6,
            },
        ],
    }
}

/// Floor (face 0) at y=0; a *partial* blocker (face 1) at y=0.5 covering
/// only x ∈ [-1, 0.5] (z ∈ [-1, 2]). With a large-`light_size` point light at
/// (0.5, 1, 0.5), floor texels near the blocker edge see part of the light's
/// area disk past the edge and part occluded → fractional soft visibility.
fn floor_plus_partial_blocker_geometry() -> GeometryResult {
    let mut vertices = xz_quad_face(0.0, 1.0, 0.0);
    let blocker = vec![
        Vertex::new(
            [-1.0, 0.5, -1.0],
            [0.0, 0.0],
            [0.0, -1.0, 0.0],
            [1.0, 0.0, 0.0],
            true,
            [0.0, 0.0],
            0,
        ),
        Vertex::new(
            [0.5, 0.5, -1.0],
            [1.0, 0.0],
            [0.0, -1.0, 0.0],
            [1.0, 0.0, 0.0],
            true,
            [0.0, 0.0],
            0,
        ),
        Vertex::new(
            [0.5, 0.5, 2.0],
            [1.0, 1.0],
            [0.0, -1.0, 0.0],
            [1.0, 0.0, 0.0],
            true,
            [0.0, 0.0],
            0,
        ),
        Vertex::new(
            [-1.0, 0.5, 2.0],
            [0.0, 1.0],
            [0.0, -1.0, 0.0],
            [1.0, 0.0, 0.0],
            true,
            [0.0, 0.0],
            0,
        ),
    ];
    vertices.extend(blocker);
    GeometryResult {
        geometry: GeometrySection {
            vertices,
            indices: vec![0, 1, 2, 0, 2, 3, 4, 5, 6, 4, 6, 7],
            faces: vec![
                FaceMeta {
                    leaf_index: 0,
                    texture_index: 0,
                },
                FaceMeta {
                    leaf_index: 0,
                    texture_index: 0,
                },
            ],
        },
        texture_names: TextureNamesSection { names: Vec::new() },
        face_index_ranges: vec![
            FaceIndexRange {
                index_offset: 0,
                index_count: 6,
            },
            FaceIndexRange {
                index_offset: 6,
                index_count: 6,
            },
        ],
    }
}

fn animated_point_light_above() -> MapLight {
    MapLight {
        origin: DVec3::new(0.5, 1.0, 0.5),
        carrier: String::new(),
        light_type: LightType::Point,
        intensity: 1.0,
        color: [1.0, 1.0, 1.0],
        falloff_model: FalloffModel::Linear,
        falloff_range: 5.0,
        light_size: 0.0,
        angular_diameter: 0.0,
        cone_angle_inner: None,
        cone_angle_outer: None,
        cone_direction: None,
        animation: Some(LightAnimation {
            period: 1.0,
            phase: 0.0,
            brightness: Some(vec![1.0, 0.5]),
            color: None,
            direction: None,
            start_active: true,
        }),
        bake_only: false,
        is_dynamic: false,
        is_animated: false,
        casts_entity_shadows: false,
        tags: vec![],
        shadow_type: crate::map_data::ShadowType::StaticLightMap,
    }
}

/// Same as `animated_point_light_above` but with a large `light_size` so the
/// soft-visibility path activates (the disk subtends enough that a partial
/// occluder yields a fractional unoccluded fraction rather than a hard 0/1).
fn soft_animated_point_light_above() -> MapLight {
    MapLight {
        light_size: 0.5,
        ..animated_point_light_above()
    }
}

fn bake_with_geometry_and_chunks<F>(
    geo: GeometryResult,
    lights: Vec<MapLight>,
    build_chunks: F,
) -> AnimatedLightWeightMapsSection
where
    F: FnOnce(&[Chart]) -> AnimatedLightChunksSection,
{
    bake_with_sample_count(
        geo,
        lights,
        crate::lightmap_bake::DEFAULT_AREA_SAMPLE_COUNT,
        build_chunks,
    )
}

fn bake_with_sample_count<F>(
    mut geo: GeometryResult,
    lights: Vec<MapLight>,
    area_sample_count: u32,
    build_chunks: F,
) -> AnimatedLightWeightMapsSection
where
    F: FnOnce(&[Chart]) -> AnimatedLightChunksSection,
{
    let (bvh, prims, _) = build_bvh(&geo).unwrap();
    let static_lights = crate::light_namespaces::StaticBakedLights::from_lights(&lights);
    let mut lm_ctx = crate::lightmap_bake::LightmapBakeCtx {
        bvh: &bvh,
        primitives: &prims,
        geometry: &mut geo,
        lights: &static_lights,
        scale_regions: &[],
    };
    let lm_output = crate::lightmap_bake::bake_lightmap(
        &mut lm_ctx,
        &crate::lightmap_bake::LightmapConfig {
            lightmap_density: 0.25,
            area_sample_count: crate::lightmap_bake::DEFAULT_AREA_SAMPLE_COUNT,
            direction_texel_scale: crate::lightmap_bake::DIRECTION_TEXEL_SCALE,
            uncompressed_irradiance: false,
        },
    )
    .unwrap();

    let chunk_section = build_chunks(&lm_output.charts);

    let inputs = WeightMapInputs {
        bvh: &bvh,
        primitives: &prims,
        geometry: &geo,
        chunk_section: &chunk_section,
        lights: &lights,
        face_charts: &lm_output.charts,
        face_placements: &lm_output.placements,
        layout: &lm_output.layout,
        atlas_width: lm_output.atlas_width,
        atlas_height: lm_output.atlas_height,
        static_atlas_layer_count: lm_output.layer_count,
        area_sample_count,
    };
    let progress = crate::reporter::StageProgress::indeterminate();
    let control = BakeControl::new(
        std::sync::Arc::new(crate::governor::Governor::new(2, false)),
        &progress,
    );
    let section = bake_animated_light_weight_maps_controlled(&inputs, &control)
        .expect("fixture charts stay inside their cell blocks");
    if chunk_section.chunks.is_empty() {
        assert_eq!(progress.total(), None);
        assert_eq!(progress.completed(), 0);
    } else {
        assert_eq!(progress.total(), Some(chunk_section.chunks.len()));
        assert_eq!(progress.completed(), chunk_section.chunks.len());
    }
    section
}

fn full_face_chunk(
    charts: &[Chart],
    face_index: u32,
    light_indices: Vec<u32>,
) -> AnimatedLightChunksSection {
    let chart = &charts[face_index as usize];
    let uv_min = chart.uv_min;
    let uv_max = [
        chart.uv_min[0] + chart.uv_extent[0],
        chart.uv_min[1] + chart.uv_extent[1],
    ];
    let index_count = light_indices.len() as u32;
    AnimatedLightChunksSection {
        chunks: vec![AnimatedLightChunk {
            aabb_min: [0.0, 0.0, 0.0],
            face_index,
            aabb_max: [1.0, 0.0, 1.0],
            index_offset: 0,
            uv_min,
            uv_max,
            index_count,
            _padding: 0,
        }],
        light_indices,
    }
}

/// Two faces in two leaves, packed onto bake layers 0 and 1, each lit by
/// its own animated light overhead. Each layer is one whole-layer cell block.
/// `first_face_normal` feeds face 0's chart: `-Y` turns it away from its light
/// so its chunk bakes unlit.
fn bake_real_multi_layer_fixture(
    first_face_normal: glam::Vec3,
) -> (
    AnimatedLightChunksSection,
    AnimatedLightWeightMapsSection,
    BlockLayout,
) {
    let geometry = two_separate_floor_geometry();
    let (bvh, primitives, _) = build_bvh(&geometry).expect("fixture BVH");
    let charts = vec![
        Chart {
            origin: glam::Vec3::new(0.0, 0.0, 0.0),
            u_axis: glam::Vec3::X,
            v_axis: glam::Vec3::Z,
            uv_min: [0.0, 0.0],
            uv_extent: [1.0, 1.0],
            normal: first_face_normal,
            width_texels: 64,
            height_texels: 64,
            leaf_index: 0,
            window: None,
        },
        Chart {
            origin: glam::Vec3::new(2.0, 0.0, 0.0),
            u_axis: glam::Vec3::X,
            v_axis: glam::Vec3::Z,
            uv_min: [0.0, 0.0],
            uv_extent: [1.0, 1.0],
            normal: glam::Vec3::Y,
            width_texels: 64,
            height_texels: 64,
            leaf_index: 1,
            window: None,
        },
    ];

    // This is a genuine two-layer pack: each leaf fills a 64x64 layer, so
    // `pack_layers` must open layer 1 without changing placement fields.
    let pack = pack_layers(&charts, 64).expect("fixture charts must pack");
    assert_eq!(pack.layer_count, 2, "fixture must exercise both layers");
    let layout = BlockLayout::whole_layers(
        pack.atlas_width,
        pack.atlas_height,
        &pack.placements,
        crate::lightmap_bake::DIRECTION_TEXEL_SCALE,
    );

    let mut second_light = animated_point_light_above();
    second_light.origin = DVec3::new(2.5, 1.0, 0.5);
    let lights = vec![animated_point_light_above(), second_light];
    let chunks = charts
        .iter()
        .enumerate()
        .map(|(face_index, chart)| AnimatedLightChunk {
            aabb_min: [face_index as f32 * 2.0, 0.0, 0.0],
            face_index: face_index as u32,
            aabb_max: [face_index as f32 * 2.0 + 1.0, 0.0, 1.0],
            index_offset: face_index as u32,
            uv_min: chart.uv_min,
            uv_max: [
                chart.uv_min[0] + chart.uv_extent[0],
                chart.uv_min[1] + chart.uv_extent[1],
            ],
            index_count: 1,
            _padding: 0,
        })
        .collect();
    let chunk_section = AnimatedLightChunksSection {
        chunks,
        light_indices: vec![0, 1],
    };
    let inputs = WeightMapInputs {
        bvh: &bvh,
        primitives: &primitives,
        geometry: &geometry,
        chunk_section: &chunk_section,
        lights: &lights,
        face_charts: &charts,
        face_placements: &pack.placements,
        layout: &layout,
        atlas_width: pack.atlas_width,
        atlas_height: pack.atlas_height,
        static_atlas_layer_count: pack.layer_count,
        area_sample_count: 1,
    };

    let section = bake_animated_light_weight_maps(&inputs).expect("fixture rects fit their blocks");
    (chunk_section, section, layout)
}

#[test]
fn real_multi_layer_pack_bakes_one_identity_page_per_covered_layer() {
    let (_, section, _) = bake_real_multi_layer_fixture(glam::Vec3::Y);

    let lightmap_blocks: Vec<u32> = section.blocks.iter().map(|b| b.lightmap_block).collect();
    assert_eq!(
        lightmap_blocks,
        vec![0, 1],
        "one block per face, in face order, keyed to its cell block"
    );
    assert_eq!(
        section.compact_layers, 2,
        "one identity page per covered layer"
    );
    assert_eq!(
        section.page_size, 64,
        "identity pages are the bake layer size"
    );
    for (index, block) in section.blocks.iter().enumerate() {
        assert_eq!(block.compact_layer, index as u32);
        assert_eq!(
            (block.compact_x, block.compact_y),
            (u32::from(block.block_x), u32::from(block.block_y)),
            "whole-layer cell blocks put block-local keys at bake-layer coords"
        );
        assert_eq!(
            (block.width, block.height),
            (64, 64),
            "block is the whole placement"
        );
    }
    for (index, rect) in section.chunk_rects.iter().enumerate() {
        assert!(rect.width > 1 && rect.height > 1, "no skip sentinel rect");
        let (layer, x, y) = section.chunk_block_origin(index).unwrap();
        assert_eq!(
            layer, index as u32,
            "real cell blocks survive into section 25"
        );
        assert_eq!(
            (x, y),
            (rect.compact_x, rect.compact_y),
            "identity keeps bake-layer coords"
        );
        let start = rect.texel_offset as usize;
        let end = start + (rect.width * rect.height) as usize;
        assert!(
            section.offset_counts[start..end]
                .iter()
                .any(|entry| entry.count > 0),
            "lightmap block {layer} must retain covered animated texels",
        );
    }
    assert_eq!(section.consistency_error(), None);

    let (_, repeated, _) = bake_real_multi_layer_fixture(glam::Vec3::Y);
    assert_eq!(section.to_bytes(), repeated.to_bytes());
}

#[test]
fn real_multi_layer_bake_logs_bake_layer_and_block_counts() {
    let capture = LogCapture::start();

    bake_real_multi_layer_fixture(glam::Vec3::Y);

    capture.assert_logged_once(Level::Info, "[AnimatedLightWeightMaps] 2 bake layers");
    capture.assert_logged_once(Level::Info, "2 animated blocks on 2 identity pages");
}

#[test]
fn cull_keeps_every_chunk_when_all_are_lit() {
    let (chunks, section, layout) = bake_real_multi_layer_fixture(glam::Vec3::Y);
    let leaf_ranges = vec![(0, 1), (1, 1)];

    let culled = cull_unlit_chunks(&chunks, section.clone(), &leaf_ranges, &layout, 64);

    assert_eq!(culled.chunk_section, chunks);
    assert_eq!(culled.weight_maps, section);
    assert_eq!(culled.leaf_chunk_ranges, leaf_ranges);
}

#[test]
fn cull_drops_unlit_chunk_and_rebases_every_parallel_table() {
    // Face 0 faces away from its light, so chunk 0 bakes an all-zero rect
    // and is the only occupant of bake layer 0.
    let (chunks, section, layout) = bake_real_multi_layer_fixture(glam::Vec3::NEG_Y);
    let rect_entries = |s: &AnimatedLightWeightMapsSection, i: usize| {
        let rect = s.chunk_rects[i];
        let start = rect.texel_offset as usize;
        s.offset_counts[start..start + (rect.width * rect.height) as usize].to_vec()
    };
    assert!(
        rect_entries(&section, 0).iter().all(|e| e.count == 0),
        "fixture precondition: face 0 must bake unlit",
    );
    assert_eq!(section.compact_layers, 2);

    let capture = LogCapture::start();
    let culled = cull_unlit_chunks(
        &chunks,
        section.clone(),
        &[(0, 1), (1, 1), (2, 0)],
        &layout,
        64,
    );

    capture.assert_logged_once(Level::Info, "culled 1 of 2 chunks with no lit texel");
    assert_eq!(culled.chunk_section.chunks.len(), 1);
    assert_eq!(culled.chunk_section.chunks[0].face_index, 1);
    assert_eq!(culled.chunk_section.chunks[0].index_offset, 0);
    assert_eq!(culled.chunk_section.light_indices, vec![1]);

    // Face 0 lost its only chunk, so it owns no block; face 1's block is
    // renumbered 0 and its identity page is the only one left.
    let rect = culled.weight_maps.chunk_rects[0];
    assert_eq!(
        rect,
        ChunkAtlasRect {
            texel_offset: 0,
            block: 0,
            ..section.chunk_rects[1]
        },
    );
    assert_eq!(
        culled.weight_maps.blocks,
        vec![AnimatedBlock {
            compact_layer: 0,
            ..section.blocks[1]
        }]
    );
    assert_eq!(culled.weight_maps.compact_layers, 1);
    assert_eq!(
        rect_entries(&culled.weight_maps, 0),
        rect_entries(&section, 1)
    );
    assert_eq!(culled.weight_maps.texel_lights, section.texel_lights);
    assert_eq!(culled.weight_maps.consistency_error(), None);

    // Leaf 0 lost its only chunk; leaf 1's chunk moved to index 0; the
    // trailing empty leaf stays empty at the end of the chunk array.
    assert_eq!(culled.leaf_chunk_ranges, vec![(0, 0), (0, 1), (1, 0)]);
}

#[test]
fn cull_drops_every_chunk_when_none_is_lit() {
    let (chunks, section, layout) = bake_real_multi_layer_fixture(glam::Vec3::NEG_Y);
    let only_unlit = AnimatedLightChunksSection {
        chunks: vec![chunks.chunks[0]],
        light_indices: chunks.light_indices.clone(),
    };
    let rect = section.chunk_rects[0];
    let unlit_section = AnimatedLightWeightMapsSection {
        page_size: 64,
        compact_layers: 1,
        blocks: vec![section.blocks[0]],
        chunk_rects: vec![rect],
        offset_counts: section.offset_counts[..(rect.width * rect.height) as usize].to_vec(),
        texel_lights: Vec::new(),
    };

    let culled = cull_unlit_chunks(&only_unlit, unlit_section, &[(0, 1)], &layout, 64);

    assert!(culled.chunk_section.chunks.is_empty());
    assert!(culled.chunk_section.light_indices.is_empty());
    assert!(culled.weight_maps.chunk_rects.is_empty());
    assert!(culled.weight_maps.offset_counts.is_empty());
    assert!(culled.weight_maps.blocks.is_empty());
    assert_eq!(culled.weight_maps.compact_layers, 0);
    assert_eq!(culled.leaf_chunk_ranges, vec![(0, 0)]);
}

/// One face with three chunks, the middle one unlit: the face keeps one
/// block spanning its whole placement, the surviving chunks keep their
/// offsets inside it, and no surviving rect covers the culled chunk's
/// texels, so compose never writes them and they stay zero.
#[test]
fn cull_keeps_one_block_for_a_face_with_some_culled_chunks() {
    let chunk = |index_offset: u32| AnimatedLightChunk {
        aabb_min: [0.0; 3],
        face_index: 0,
        aabb_max: [1.0; 3],
        index_offset,
        uv_min: [0.0; 2],
        uv_max: [1.0; 2],
        index_count: 1,
        _padding: 0,
    };
    let chunk_section = AnimatedLightChunksSection {
        chunks: vec![chunk(0), chunk(1), chunk(2)],
        light_indices: vec![0, 0, 0],
    };
    let rect = |x: u32, texel_offset: u32| ChunkAtlasRect {
        compact_x: x,
        compact_y: 12,
        width: 4,
        height: 2,
        texel_offset,
        block: 0,
    };
    let lit = TexelLightEntry {
        offset: 0,
        count: 1,
    };
    let unlit = TexelLightEntry {
        offset: 1,
        count: 0,
    };
    let mut offset_counts = vec![lit; 8];
    offset_counts.extend([unlit; 8]);
    offset_counts.extend([lit; 8]);
    let placement = AnimatedBlock {
        lightmap_block: 2,
        block_x: 8,
        block_y: 10,
        compact_x: 8,
        compact_y: 10,
        compact_layer: 0,
        width: 16,
        height: 6,
    };
    let section = AnimatedLightWeightMapsSection {
        page_size: 256,
        compact_layers: 1,
        blocks: vec![placement],
        chunk_rects: vec![rect(10, 0), rect(14, 8), rect(18, 16)],
        offset_counts,
        texel_lights: vec![TexelLight {
            light_index: 0,
            weight: 1.0,
            direction_oct: [0, 0],
        }],
    };
    assert_eq!(section.consistency_error(), None);

    let layout = BlockLayout {
        direction_texel_scale: 2,
        blocks: (0..3)
            .map(|layer| crate::lightmap_bake::CellBlock {
                cell_id: layer,
                width: 256,
                height: 256,
                layer,
                x: 0,
                y: 0,
            })
            .collect(),
        chart_blocks: vec![2],
    };
    let culled = cull_unlit_chunks(&chunk_section, section.clone(), &[(0, 3)], &layout, 256);
    let weight_maps = &culled.weight_maps;

    assert_eq!(
        weight_maps.blocks,
        vec![placement],
        "one block, whole placement"
    );
    assert_eq!(weight_maps.chunk_rects.len(), 2);
    for (rect, original) in weight_maps.chunk_rects.iter().zip([0, 2]) {
        let original = section.chunk_rects[original];
        assert_eq!(rect.block, 0);
        assert_eq!(
            (
                rect.compact_x - placement.compact_x,
                rect.compact_y - placement.compact_y
            ),
            (
                original.compact_x - placement.compact_x,
                original.compact_y - placement.compact_y
            ),
        );
        let culled_rect = section.chunk_rects[1];
        let overlap = rect.compact_x < culled_rect.compact_x + culled_rect.width
            && culled_rect.compact_x < rect.compact_x + rect.width;
        assert!(
            !overlap,
            "no surviving chunk writes the culled chunk's texels"
        );
    }
    assert_eq!(weight_maps.consistency_error(), None);
}

#[test]
fn animated_atlas_over_budget_names_budget_and_found_bytes() {
    let err = validate_animated_atlas_budget_with_limit(64, 2, 1)
        .expect_err("injected low budget must reject the atlas");
    let AnimatedWeightMapBakeError::AtlasOverBudget {
        budget_bytes,
        found_bytes,
    } = err
    else {
        panic!("expected an over-budget error, got {err}");
    };
    assert_eq!(budget_bytes, 1);
    assert_eq!(found_bytes, 64 * 64 * 2 * 12);
    let message = AnimatedWeightMapBakeError::AtlasOverBudget {
        budget_bytes,
        found_bytes,
    }
    .to_string();
    assert!(message.contains("budget 1 bytes"));
    assert!(message.contains(&format!("found {found_bytes} bytes")));
}

#[test]
#[should_panic(expected = "face 0")]
fn overlap_assert_rejects_cross_face_rects_on_one_layer() {
    let chunks = vec![
        AnimatedLightChunk {
            aabb_min: [0.0; 3],
            face_index: 0,
            aabb_max: [1.0; 3],
            index_offset: 0,
            uv_min: [0.0; 2],
            uv_max: [1.0; 2],
            index_count: 1,
            _padding: 0,
        },
        AnimatedLightChunk {
            aabb_min: [0.0; 3],
            face_index: 1,
            aabb_max: [1.0; 3],
            index_offset: 1,
            uv_min: [0.0; 2],
            uv_max: [1.0; 2],
            index_count: 1,
            _padding: 0,
        },
    ];
    let result = || ChunkBakeResult {
        rect: ChunkAtlasRect {
            compact_x: 3,
            compact_y: 4,
            width: 2,
            height: 2,
            texel_offset: 0,
            block: 0,
        },
        layer: 1,
        offset_counts: Vec::new(),
        texel_lights: Vec::new(),
    };
    let results = vec![result(), result()];

    assert_no_overlapping_rects_per_layer(&chunks, &results);
}

fn rect_result(layer: u32, x: u32, y: u32, width: u32, height: u32) -> ChunkBakeResult {
    ChunkBakeResult {
        rect: ChunkAtlasRect {
            compact_x: x,
            compact_y: y,
            width,
            height,
            texel_offset: 0,
            block: 0,
        },
        layer,
        offset_counts: Vec::new(),
        texel_lights: Vec::new(),
    }
}

fn zero_chunks(count: usize) -> Vec<AnimatedLightChunk> {
    vec![
        AnimatedLightChunk {
            aabb_min: [0.0; 3],
            face_index: 0,
            aabb_max: [0.0; 3],
            index_offset: 0,
            uv_min: [0.0; 2],
            uv_max: [0.0; 2],
            index_count: 0,
            _padding: 0,
        };
        count
    ]
}

/// The pairwise half-open test the overlap assert has always applied.
fn pairwise_overlap(results: &[ChunkBakeResult]) -> bool {
    results.iter().enumerate().any(|(i, a)| {
        results[i + 1..].iter().any(|b| {
            let (a, b) = (&a.rect, &b.rect);
            a.compact_x < b.compact_x + b.width
                && b.compact_x < a.compact_x + a.width
                && a.compact_y < b.compact_y + b.height
                && b.compact_y < a.compact_y + a.height
        })
    })
}

#[test]
fn overlap_bitmap_agrees_with_pairwise_scan_on_random_layers() {
    // xorshift64: deterministic, no dependency.
    let mut state = 0x9E37_79B9_7F4A_7C15_u64;
    let mut next = |bound: u32| {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state % u64::from(bound)) as u32
    };
    let mut occupancy = Vec::new();
    let (mut overlapping, mut clear) = (0, 0);
    for _ in 0..2_000 {
        let count = 1 + next(12) as usize;
        let results: Vec<ChunkBakeResult> = (0..count)
            .map(|_| rect_result(0, next(200), next(40), 1 + next(90), 1 + next(12)))
            .collect();
        let indices: Vec<usize> = (0..count).collect();
        let expected = pairwise_overlap(&results);
        assert_eq!(
            layer_rects_may_overlap(&results, &indices, &mut occupancy),
            expected,
            "rects {:?}",
            results.iter().map(|r| r.rect).collect::<Vec<_>>()
        );
        if expected {
            overlapping += 1;
        } else {
            clear += 1;
        }
    }
    assert!(
        overlapping > 100 && clear > 100,
        "{overlapping} overlapping, {clear} clear"
    );
}

#[test]
fn overlap_assert_accepts_edge_sharing_rects_across_word_boundaries() {
    // Abutting at x = 64 and x = 128, and rows touching at y = 3: no shared texel.
    let results = vec![
        rect_result(2, 0, 0, 64, 3),
        rect_result(2, 64, 0, 64, 3),
        rect_result(2, 128, 0, 1, 3),
        rect_result(2, 60, 3, 70, 2),
        rect_result(5, 60, 3, 70, 2),
    ];
    assert_no_overlapping_rects_per_layer(&zero_chunks(results.len()), &results);
}

#[test]
#[should_panic(expected = "chunks 0 (face 0) and 2 (face 0) on bake layer 2")]
fn overlap_assert_names_first_pair_when_one_texel_is_shared_across_a_word_boundary() {
    let results = vec![
        rect_result(2, 0, 0, 64, 3),
        rect_result(2, 65, 0, 10, 3),
        rect_result(2, 63, 2, 2, 1),
    ];
    assert_no_overlapping_rects_per_layer(&zero_chunks(results.len()), &results);
}

#[test]
#[should_panic(expected = "overlapping atlas rects")]
fn overlap_assert_keeps_rejecting_a_zero_width_rect_inside_another() {
    let results = vec![rect_result(1, 3, 4, 2, 2), rect_result(1, 4, 4, 0, 2)];
    assert_no_overlapping_rects_per_layer(&zero_chunks(results.len()), &results);
}

#[test]
fn single_chunk_single_light_emits_one_light_per_covered_texel() {
    let section = bake_with_geometry_and_chunks(
        unit_floor_geometry(),
        vec![animated_point_light_above()],
        |charts| full_face_chunk(charts, 0, vec![0]),
    );

    assert_eq!(section.chunk_rects.len(), 1);
    let rect = &section.chunk_rects[0];
    assert!(rect.width >= 1 && rect.height >= 1);
    assert!(section.is_consistent());

    let mut covered = 0;
    for entry in &section.offset_counts {
        if entry.count > 0 {
            covered += 1;
            assert_eq!(
                entry.count, 1,
                "expected exactly one light per covered texel"
            );
            let tl = &section.texel_lights[entry.offset as usize];
            assert_eq!(tl.light_index, 0);
            assert!(
                tl.weight > 0.0 && tl.weight <= 1.0 + 1.0e-4,
                "unexpected weight {}",
                tl.weight
            );
        }
    }
    assert!(covered > 0, "expected at least one covered texel");
}

#[test]
fn parallel_plate_occluder_zeros_shadowed_texels() {
    let section = bake_with_geometry_and_chunks(
        floor_plus_blocker_geometry(),
        vec![animated_point_light_above()],
        |charts| full_face_chunk(charts, 0, vec![0]),
    );

    for entry in &section.offset_counts {
        assert_eq!(
            entry.count, 0,
            "expected every floor texel to be fully occluded by the ceiling",
        );
    }
    assert!(
        section.texel_lights.is_empty(),
        "no weights should be emitted when every texel is shadowed",
    );
}

/// Task 4: a partially-occluded soft light produces *fractional* weights in
/// penumbra texels, not just the old binary 0/1 include. At least one covered
/// texel must carry a weight strictly between the noise floor and the fully-
/// lit value, and that fractional texel must be a covered (emitted) entry.
#[test]
fn soft_light_partial_occluder_emits_fractional_weight() {
    // Hard reference: a zero-size light over the same partial blocker bakes a
    // crisp 0/1 mask. Its maximum (fully-lit) weight bounds "fully lit".
    let hard = bake_with_geometry_and_chunks(
        floor_plus_partial_blocker_geometry(),
        vec![animated_point_light_above()],
        |charts| full_face_chunk(charts, 0, vec![0]),
    );
    let hard_max = hard
        .texel_lights
        .iter()
        .map(|tl| tl.weight)
        .fold(0.0_f32, f32::max);
    assert!(hard_max > 0.0, "hard reference produced no lit texels");

    let soft = bake_with_geometry_and_chunks(
        floor_plus_partial_blocker_geometry(),
        vec![soft_animated_point_light_above()],
        |charts| full_face_chunk(charts, 0, vec![0]),
    );
    assert!(soft.is_consistent());

    // A penumbra weight is one strictly below the fully-lit value at the same
    // geometric falloff (scaled down by soft visibility < 1) but above the
    // numerical noise floor — i.e. a continuous gradient, not 0/1.
    let mut penumbra_count = 0;
    for entry in &soft.offset_counts {
        for i in 0..entry.count {
            let tl = &soft.texel_lights[(entry.offset + i) as usize];
            assert!(
                tl.weight > WEIGHT_EPSILON,
                "emitted entries must clear the noise floor; got {}",
                tl.weight,
            );
            if tl.weight < hard_max * 0.95 {
                penumbra_count += 1;
            }
        }
    }
    assert!(
        penumbra_count > 0,
        "expected at least one penumbra texel with fractional (< fully-lit) \
             weight under soft visibility, found none",
    );
}

/// `floor_plus_partial_blocker_geometry` with a far 1 m quad prepended in
/// the same cell: the floor becomes face 1 and, packed after the equal-size
/// far quad, moves inside the cell block and so in the bake layer.
fn far_quad_then_floor_plus_partial_blocker_geometry() -> GeometryResult {
    let mut geo = floor_plus_partial_blocker_geometry();
    let section = &mut geo.geometry;
    let mut vertices = xz_quad_face(0.0, 1.0, 500.0);
    vertices.append(&mut section.vertices);
    section.vertices = vertices;
    let mut indices = vec![0, 1, 2, 0, 2, 3];
    indices.extend(section.indices.iter().map(|&index| index + 4));
    section.indices = indices;
    section.faces.insert(
        0,
        FaceMeta {
            leaf_index: 0,
            texture_index: 0,
        },
    );
    for range in &mut geo.face_index_ranges {
        range.index_offset += 6;
    }
    geo.face_index_ranges.insert(
        0,
        FaceIndexRange {
            index_offset: 0,
            index_count: 6,
        },
    );
    geo
}

/// Soft-visibility seeds key on the chart, not its bake-layer coordinates or
/// face index: the floor's penumbra weights are identical after an unrelated
/// quad renumbers it and moves its placement.
#[test]
fn moved_and_renumbered_chart_bakes_identical_animated_weights() {
    let alone = bake_with_geometry_and_chunks(
        floor_plus_partial_blocker_geometry(),
        vec![soft_animated_point_light_above()],
        |charts| full_face_chunk(charts, 0, vec![0]),
    );
    let shifted = bake_with_geometry_and_chunks(
        far_quad_then_floor_plus_partial_blocker_geometry(),
        vec![soft_animated_point_light_above()],
        |charts| full_face_chunk(charts, 1, vec![0]),
    );
    assert_ne!(
        alone.to_bytes(),
        shifted.to_bytes(),
        "the far quad must move the floor's block placement"
    );
    let hard_max = alone
        .texel_lights
        .iter()
        .map(|tl| tl.weight)
        .fold(0.0_f32, f32::max);
    assert!(
        alone
            .texel_lights
            .iter()
            .any(|tl| tl.weight > WEIGHT_EPSILON && tl.weight < hard_max * 0.95),
        "fixture must bake a penumbra"
    );
    assert_eq!(alone.offset_counts, shifted.offset_counts);
    assert_eq!(alone.texel_lights, shifted.texel_lights);
}

/// Task 4: fully-occluded texels under a soft light still emit *no* entry —
/// soft visibility of 0 means "drop", same sparsity as the old binary gate.
/// The full (large) blocker covers the whole floor, so every disk sample is
/// occluded for every texel.
#[test]
fn soft_light_full_occluder_emits_no_entry() {
    let section = bake_with_geometry_and_chunks(
        floor_plus_blocker_geometry(),
        vec![soft_animated_point_light_above()],
        |charts| full_face_chunk(charts, 0, vec![0]),
    );
    for entry in &section.offset_counts {
        assert_eq!(
            entry.count, 0,
            "a fully-occluded soft light must emit no entry (v <= 0)",
        );
    }
    assert!(section.texel_lights.is_empty());
}

/// Task 4: the soft-visibility multiply composes *after* the sdf-typed
/// `continue`, so `sdf`-typed lights remain fully skipped (no baked weight).
#[test]
fn sdf_typed_soft_light_is_skipped() {
    let mut light = soft_animated_point_light_above();
    light.shadow_type = crate::map_data::ShadowType::Sdf;
    let section = bake_with_geometry_and_chunks(unit_floor_geometry(), vec![light], |charts| {
        full_face_chunk(charts, 0, vec![0])
    });
    for entry in &section.offset_counts {
        assert_eq!(entry.count, 0, "sdf-typed lights emit no baked weight");
    }
    assert!(section.texel_lights.is_empty());
}

/// The pipeline folds `area_sample_count` into the
/// `animated_lm_weight_maps` cache key's input hash, so changing it produces
/// a cache miss and re-bake. This test
/// verifies the field actually reaches `soft_visibility` — raising it shifts
/// penumbra weights at the higher stratification resolution. The cache-miss
/// contract is covered separately by `stage_version_bump_misses_then_hits`.
#[test]
fn area_sample_count_field_changes_penumbra_weights() {
    let low = bake_with_sample_count(
        floor_plus_partial_blocker_geometry(),
        vec![soft_animated_point_light_above()],
        16,
        |charts| full_face_chunk(charts, 0, vec![0]),
    );
    let high = bake_with_sample_count(
        floor_plus_partial_blocker_geometry(),
        vec![soft_animated_point_light_above()],
        64,
        |charts| full_face_chunk(charts, 0, vec![0]),
    );
    assert!(low.is_consistent() && high.is_consistent());

    // Collect penumbra weights (those strictly below the per-build max) at
    // each sample count. The finer stratification at 64 quantizes the
    // fraction differently, so the multisets must differ.
    let low_weights: Vec<u32> = low
        .texel_lights
        .iter()
        .map(|t| t.weight.to_bits())
        .collect();
    let high_weights: Vec<u32> = high
        .texel_lights
        .iter()
        .map(|t| t.weight.to_bits())
        .collect();
    assert_ne!(
        low_weights, high_weights,
        "raising the area-sample-count knob must change baked penumbra weights",
    );
}

/// Task 4: the soft bake stays deterministic — the per-texel seed is a fixed
/// hash of `(x, y)`, no RNG / hash-order, so two builds are byte-identical.
#[test]
fn soft_light_determinism_two_builds_byte_identical() {
    let bytes_a = bake_with_geometry_and_chunks(
        floor_plus_partial_blocker_geometry(),
        vec![soft_animated_point_light_above()],
        |charts| full_face_chunk(charts, 0, vec![0]),
    )
    .to_bytes();
    let bytes_b = bake_with_geometry_and_chunks(
        floor_plus_partial_blocker_geometry(),
        vec![soft_animated_point_light_above()],
        |charts| full_face_chunk(charts, 0, vec![0]),
    )
    .to_bytes();
    assert_eq!(bytes_a, bytes_b);
}

#[test]
fn determinism_two_builds_byte_identical() {
    let bytes_a = bake_with_geometry_and_chunks(
        unit_floor_geometry(),
        vec![animated_point_light_above()],
        |charts| full_face_chunk(charts, 0, vec![0]),
    )
    .to_bytes();
    let bytes_b = bake_with_geometry_and_chunks(
        unit_floor_geometry(),
        vec![animated_point_light_above()],
        |charts| full_face_chunk(charts, 0, vec![0]),
    )
    .to_bytes();
    assert_eq!(bytes_a, bytes_b);
}

#[test]
fn empty_chunk_section_yields_empty_output() {
    let section = bake_with_geometry_and_chunks(
        unit_floor_geometry(),
        vec![animated_point_light_above()],
        |_charts| AnimatedLightChunksSection::empty(),
    );
    assert!(section.chunk_rects.is_empty());
    assert!(section.offset_counts.is_empty());
    assert!(section.texel_lights.is_empty());
}

/// Two chunks with a 1-texel UV gap (matching chart packer guarantee) so their
/// rounded rects don't overlap. Verifies prefix-sum texel_offset invariant.
#[test]
fn texel_offsets_form_prefix_sum_partition() {
    let section = bake_with_geometry_and_chunks(
        unit_floor_geometry(),
        vec![animated_point_light_above()],
        |charts| {
            let chart = &charts[0];
            let u0 = chart.uv_min[0];
            let u1 = chart.uv_min[0] + chart.uv_extent[0];
            let v0 = chart.uv_min[1];
            let v1 = chart.uv_min[1] + chart.uv_extent[1];
            let u_mid_lo = u0 + 0.4 * chart.uv_extent[0];
            let u_mid_hi = u0 + 0.6 * chart.uv_extent[0];
            AnimatedLightChunksSection {
                chunks: vec![
                    AnimatedLightChunk {
                        aabb_min: [0.0, 0.0, 0.0],
                        face_index: 0,
                        aabb_max: [0.5, 0.0, 1.0],
                        index_offset: 0,
                        uv_min: [u0, v0],
                        uv_max: [u_mid_lo, v1],
                        index_count: 1,
                        _padding: 0,
                    },
                    AnimatedLightChunk {
                        aabb_min: [0.5, 0.0, 0.0],
                        face_index: 0,
                        aabb_max: [1.0, 0.0, 1.0],
                        index_offset: 1,
                        uv_min: [u_mid_hi, v0],
                        uv_max: [u1, v1],
                        index_count: 1,
                        _padding: 0,
                    },
                ],
                light_indices: vec![0, 0],
            }
        },
    );
    assert!(section.is_consistent());
    let mut running = 0u32;
    for chunk in &section.chunk_rects {
        assert_eq!(chunk.texel_offset, running);
        running += chunk.width * chunk.height;
    }
    assert_eq!(section.offset_counts.len() as u32, running);
}

#[test]
fn byte_size_under_8_mib_budget() {
    let section = bake_with_geometry_and_chunks(
        unit_floor_geometry(),
        vec![animated_point_light_above()],
        |charts| full_face_chunk(charts, 0, vec![0]),
    );
    let bytes = section.to_bytes();
    assert!(
        bytes.len() < 8 * 1024 * 1024,
        "section exceeded 8 MiB budget ({} bytes)",
        bytes.len(),
    );
}

#[test]
fn mean_lights_per_covered_texel_under_2_5() {
    let section = bake_with_geometry_and_chunks(
        unit_floor_geometry(),
        vec![animated_point_light_above()],
        |charts| full_face_chunk(charts, 0, vec![0]),
    );
    let covered: usize = section.offset_counts.iter().filter(|e| e.count > 0).count();
    assert!(covered > 0, "expected at least one covered texel");
    let mean = section.texel_lights.len() as f64 / covered as f64;
    assert!(
        mean <= 2.5,
        "mean lights per covered texel {mean} exceeded 2.5 target",
    );
}

/// Task 2b: a static-geometry animated light's per-texel incoming
/// direction is bakeable (it never changes). This anchors that the
/// weight-map baker retains what `light_contribution_and_direction`
/// computes rather than discarding it.
#[test]
fn weight_map_retains_per_light_per_texel_incoming_direction() {
    let light = animated_point_light_above();
    let section =
        bake_with_geometry_and_chunks(unit_floor_geometry(), vec![light.clone()], |charts| {
            full_face_chunk(charts, 0, vec![0])
        });

    assert!(
        !section.texel_lights.is_empty(),
        "expected at least one covered texel — fixture is broken otherwise",
    );

    // Every covered entry must decode to a roughly-unit vector pointing
    // upward (the light sits above a floor with normal +Y).
    for tl in &section.texel_lights {
        let decoded = postretro_level_format::octahedral::decode(tl.direction_oct);
        let len =
            (decoded[0] * decoded[0] + decoded[1] * decoded[1] + decoded[2] * decoded[2]).sqrt();
        assert!(
            (len - 1.0).abs() < 1.0e-3,
            "decoded direction {:?} not unit-length (len={})",
            decoded,
            len,
        );
        assert!(
            decoded[1] > 0.0,
            "expected +Y dominant direction (light above floor), got {:?}",
            decoded,
        );
    }
}

/// Anchors the cache-bump contract: bumping `STAGE_VERSION` invalidates
/// the prior `animated_lm_weight_maps` cache entry. The companion round-trip
/// test below verifies that the new version then hits on a second read.
#[test]
fn stage_version_bump_changes_cache_key() {
    use crate::cache::CacheKey;

    let input_hash = b"fake-animated-weight-maps-input-fingerprint";
    let stale = CacheKey::new("animated_lm_weight_maps", STAGE_VERSION - 1, input_hash);
    let current = CacheKey::new("animated_lm_weight_maps", STAGE_VERSION, input_hash);

    assert_ne!(
        stale.as_filename(),
        current.as_filename(),
        "a STAGE_VERSION bump must change the cache key — a stale entry \
             must not be reachable under the bumped version",
    );
}

#[test]
fn stage_version_bump_misses_then_hits() {
    use crate::cache::{CacheKey, StageCache};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    // Unique temp dir so parallel test runs don't collide.
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nonce = COUNTER.fetch_add(1, Ordering::Relaxed);
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!(
        "postretro_anim_lm_stage_bump_{stamp}_{nonce}_{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let cache = StageCache::new(&dir).expect("create cache dir");

    let input_hash = b"anim-lm-weight-maps-input-fingerprint";

    // A pre-bump entry baked by the previous version (no direction field).
    let stale_key = CacheKey::new("animated_lm_weight_maps", STAGE_VERSION - 1, input_hash);
    cache.put(&stale_key, b"old-baked-weight-maps-without-direction");

    // Same inputs, current version: the version is folded into the key,
    // so the stale entry must not be reachable — first build is a miss.
    let current_key = CacheKey::new("animated_lm_weight_maps", STAGE_VERSION, input_hash);
    assert!(
        cache.get(&current_key).is_none(),
        "first build after STAGE_VERSION bump must miss and rebake",
    );

    // The rebake stores the direction-bearing section; the second build hits.
    let rebaked = b"weight-maps-with-direction".to_vec();
    cache.put(&current_key, &rebaked);
    let loaded = cache
        .get(&current_key)
        .expect("second build under the bumped version must hit the cache");
    assert_eq!(loaded, rebaked);

    let _ = std::fs::remove_dir_all(&dir);
}

/// Regression: with outward rounding (floor min, ceil max), two sibling
/// chunks sharing a UV boundary inflated outward into the same atlas texel
/// column/row, tripping `assert_no_overlapping_rects_per_layer`. Center-based
/// half-open ownership packs them adjacent with no overlap.
#[test]
fn sibling_chunks_with_shared_uv_edge_pack_without_overlap() {
    let chart = Chart {
        origin: glam::Vec3::ZERO,
        u_axis: glam::Vec3::X,
        v_axis: glam::Vec3::Z,
        uv_min: [0.0, 0.0],
        uv_extent: [1.0, 1.0],
        normal: glam::Vec3::Y,
        width_texels: 8,
        height_texels: 8,
        leaf_index: 0,
        window: None,
    };
    let placement = ChartPlacement {
        x: 0,
        y: 0,
        layer: 0,
    };
    let atlas_size = 64u32;

    // Splits the chart's U range at uv=0.5 — what `recurse` does on a
    // U-split. Pre-fix, both rects shared the same atlas texel column.
    let (ax_a, _ay_a, w_a, _h_a) = chunk_atlas_rect(
        &chart,
        placement,
        [0.0, 0.0],
        [0.5, 1.0],
        atlas_size,
        atlas_size,
    );
    let (ax_b, _ay_b, _w_b, _h_b) = chunk_atlas_rect(
        &chart,
        placement,
        [0.5, 0.0],
        [1.0, 1.0],
        atlas_size,
        atlas_size,
    );
    assert!(
        ax_a + w_a <= ax_b,
        "sibling chunks must not overlap: A ends at {} but B starts at {}",
        ax_a + w_a,
        ax_b,
    );

    // Non-integer-texel boundary at uv=0.4 (i.e. fx = 1.6, on a 4-texel
    // interior) — the original failure mode where outward rounding put A's
    // ax_max=2 and B's ax_min=1 into the same column.
    let (ax_a2, _, w_a2, _) = chunk_atlas_rect(
        &chart,
        placement,
        [0.0, 0.0],
        [0.4, 1.0],
        atlas_size,
        atlas_size,
    );
    let (ax_b2, _, _w_b2, _) = chunk_atlas_rect(
        &chart,
        placement,
        [0.4, 0.0],
        [1.0, 1.0],
        atlas_size,
        atlas_size,
    );
    assert!(
        ax_a2 + w_a2 <= ax_b2,
        "sibling chunks split at uv=0.4 must not overlap: A ends at {} but B starts at {}",
        ax_a2 + w_a2,
        ax_b2,
    );
}

/// Sibling chunks share a UV boundary that can drift by ~1e-7 in f32 due to
/// recursive halving of a non-dyadic chart extent. Without the boundary
/// snap in `chunk_atlas_rect`, the two sides straddle an integer in
/// interior-texel space, `ceil` disagrees by one, and the atlas rects
/// overlap by a single texel row.
#[test]
fn sibling_chunks_with_drifted_shared_uv_edge_pack_without_overlap() {
    // Chart geometry chosen so V has a fine interior pitch (~0.0254 m),
    // which is where the drift surfaces in practice.
    let chart = Chart {
        origin: glam::Vec3::ZERO,
        u_axis: glam::Vec3::X,
        v_axis: glam::Vec3::Z,
        uv_min: [0.0, 0.0],
        uv_extent: [0.0508, 8.128],
        normal: glam::Vec3::Y,
        width_texels: 3,
        height_texels: 322,
        leaf_index: 0,
        window: None,
    };
    let placement = ChartPlacement {
        x: 0,
        y: 0,
        layer: 0,
    };
    let atlas_size = 4096u32;

    // A.uv_max and B.uv_min are intended-equal but drift apart by ~2e-7;
    // this is the pattern recursive halving produces at sibling seams.
    let a_uv_min = [0.0, 2.3876];
    let a_uv_max = [0.0508, 2.4384];
    let b_uv_min = [0.0, 2.4383998];
    let b_uv_max = [0.0508, 2.4891999];

    let (ax_a, ay_a, w_a, h_a) = chunk_atlas_rect(
        &chart, placement, a_uv_min, a_uv_max, atlas_size, atlas_size,
    );
    let (ax_b, ay_b, w_b, h_b) = chunk_atlas_rect(
        &chart, placement, b_uv_min, b_uv_max, atlas_size, atlas_size,
    );

    let overlap_x = ax_a < ax_b + w_b && ax_b < ax_a + w_a;
    let overlap_y = ay_a < ay_b + h_b && ay_b < ay_a + h_a;
    assert!(
        !(overlap_x && overlap_y),
        "drifted-boundary siblings must not overlap: A={ax_a}+{ay_a}+{w_a}x{h_a} \
             vs B={ax_b}+{ay_b}+{w_b}x{h_b}",
    );
}

/// Regression: previously only ax_max/ay_max were clamped; a misplaced chart
/// could put ax_min >= atlas_width, underflowing the width, and textureStore
/// wrote past the atlas edge. Both corners are now clamped.
#[test]
fn chunk_atlas_rect_handles_placement_at_and_beyond_atlas_bound() {
    let chart = Chart {
        origin: glam::Vec3::ZERO,
        u_axis: glam::Vec3::X,
        v_axis: glam::Vec3::Z,
        uv_min: [0.0, 0.0],
        uv_extent: [1.0, 1.0],
        normal: glam::Vec3::Y,
        width_texels: 8,
        height_texels: 8,
        leaf_index: 0,
        window: None,
    };
    let atlas_size = 64u32;

    // Case 1: placement exactly at the atlas edge.
    let placement = ChartPlacement {
        x: atlas_size - 1,
        y: atlas_size - 1,
        layer: 0,
    };
    let (ax, ay, w, h) = chunk_atlas_rect(
        &chart,
        placement,
        [0.0, 0.0],
        [1.0, 1.0],
        atlas_size,
        atlas_size,
    );
    assert!(ax < atlas_size, "ax_min {ax} overflowed atlas {atlas_size}");
    assert!(ay < atlas_size, "ay_min {ay} overflowed atlas {atlas_size}");
    assert!(w >= 1 && h >= 1);
    assert!(
        ax + w <= atlas_size,
        "rect ends at {}; atlas size {atlas_size}",
        ax + w
    );
    assert!(
        ay + h <= atlas_size,
        "rect ends at {}; atlas size {atlas_size}",
        ay + h
    );

    // Case 2: placement past the atlas bound; both corners must be pinned inside.
    let placement_past = ChartPlacement {
        x: atlas_size + 100,
        y: atlas_size + 100,
        layer: 0,
    };
    let (ax2, ay2, w2, h2) = chunk_atlas_rect(
        &chart,
        placement_past,
        [0.0, 0.0],
        [1.0, 1.0],
        atlas_size,
        atlas_size,
    );
    assert!(
        ax2 < atlas_size,
        "ax_min {ax2} must be clamped inside atlas (was past-bound)",
    );
    assert!(
        ay2 < atlas_size,
        "ay_min {ay2} must be clamped inside atlas (was past-bound)",
    );
    assert!(ax2 + w2 <= atlas_size);
    assert!(ay2 + h2 <= atlas_size);
}

/// Both floors of `two_separate_floor_geometry` (cells 0 and 1) prepared with
/// one static light, so vertices carry block ids, plus one full-face animated
/// chunk per face and the baked identity section.
fn two_cell_block_fixture() -> (
    GeometryResult,
    crate::lightmap_bake::PreparedAtlas,
    AnimatedLightChunksSection,
    AnimatedLightWeightMapsSection,
) {
    let mut geo = two_separate_floor_geometry();
    let static_light = {
        let mut light = animated_point_light_above();
        light.animation = None;
        light
    };
    let statics = [static_light];
    let static_lights = crate::light_namespaces::StaticBakedLights::from_lights(&statics);
    let prepared = crate::lightmap_bake::prepare_atlas(&mut geo, &static_lights, 0.25, &[])
        .expect("fixture prepares");
    let (bvh, primitives, _) = build_bvh(&geo).expect("fixture BVH");
    let chunks = prepared
        .charts
        .iter()
        .enumerate()
        .map(|(face, chart)| AnimatedLightChunk {
            aabb_min: [0.0; 3],
            face_index: face as u32,
            aabb_max: [3.0, 0.0, 1.0],
            index_offset: 0,
            uv_min: chart.uv_min,
            uv_max: [
                chart.uv_min[0] + chart.uv_extent[0],
                chart.uv_min[1] + chart.uv_extent[1],
            ],
            index_count: 1,
            _padding: 0,
        })
        .collect();
    let chunk_section = AnimatedLightChunksSection {
        chunks,
        light_indices: vec![0],
    };
    let mut wide_light = animated_point_light_above();
    wide_light.falloff_range = 20.0;
    let lights = [wide_light];
    let section = bake_animated_light_weight_maps(&WeightMapInputs {
        bvh: &bvh,
        primitives: &primitives,
        geometry: &geo,
        chunk_section: &chunk_section,
        lights: &lights,
        face_charts: &prepared.charts,
        face_placements: &prepared.placements,
        layout: &prepared.layout,
        atlas_width: prepared.atlas_width,
        atlas_height: prepared.atlas_height,
        static_atlas_layer_count: prepared.layer_count,
        area_sample_count: 1,
    })
    .expect("charts stay inside their cell blocks");
    (geo, prepared, chunk_section, section)
}

/// Every animated block's key, resolved through its cell block's bake-layer
/// origin, lands on its face's chart placement, and every chunk's block-local
/// origin lands on the bake-layer rect the chunk was baked at; the compact
/// repack keeps both.
#[test]
fn animated_block_keys_address_the_same_chart_texels_as_their_bake_layer_rects() {
    let (geo, prepared, chunk_section, mut section) = two_cell_block_fixture();
    let layout = &prepared.layout;
    assert_eq!(section.blocks.len(), 2);
    assert!(
        layout.blocks.iter().any(|b| b.x > 0 || b.y > 0),
        "fixture must rebase through a nonzero cell-block origin"
    );

    let resolve = |lightmap_block: u32, x: u32, y: u32| {
        let cell = &layout.blocks[lightmap_block as usize];
        (cell.layer, cell.x + x, cell.y + y)
    };
    for (face, block) in section.blocks.iter().enumerate() {
        let placement = prepared.placements[face];
        assert_eq!(block.lightmap_block, layout.chart_blocks[face]);
        assert_eq!(
            resolve(
                block.lightmap_block,
                u32::from(block.block_x),
                u32::from(block.block_y)
            ),
            (placement.layer, placement.x, placement.y),
            "face {face}'s key must resolve to its chart placement"
        );
    }
    let bake_origins: Vec<_> = chunk_section
        .chunks
        .iter()
        .map(|chunk| {
            let face = chunk.face_index as usize;
            let placement = prepared.placements[face];
            let (x, y, _, _) = super::static_atlas_frame::chunk_atlas_rect(
                &prepared.charts[face],
                placement,
                chunk.uv_min,
                chunk.uv_max,
                prepared.atlas_width,
                prepared.atlas_height,
            );
            (placement.layer, x, y)
        })
        .collect();
    let block_origins = |section: &AnimatedLightWeightMapsSection| -> Vec<_> {
        (0..section.chunk_rects.len())
            .map(|index| {
                let (block, x, y) = section.chunk_block_origin(index).unwrap();
                resolve(block, x, y)
            })
            .collect()
    };
    assert_eq!(block_origins(&section), bake_origins);

    crate::animated_atlas_layout::choose_compact_layout(&mut section, layout, prepared.atlas_width);
    assert_eq!(
        block_origins(&section),
        bake_origins,
        "the compact repack must not move a chunk's cell-block key"
    );

    let face_blocks = crate::animated_block_ids::face_blocks(
        &chunk_section,
        &section,
        geo.face_index_ranges.len(),
    )
    .unwrap();
    assert_eq!(
        crate::animated_block_ids::validate_block_guards(
            &geo.geometry,
            &geo.face_index_ranges,
            &section,
            &face_blocks,
            layout,
        ),
        Ok(()),
        "vertex footprints resolve inside their placements in the block frame"
    );
}

/// A rect leaving its cell block cannot be keyed in it.
#[test]
fn animated_rebase_rejects_a_rect_outside_its_cell_block() {
    let (_, prepared, _, _) = two_cell_block_fixture();
    let layout = &prepared.layout;
    let face = 1;
    let chart = &prepared.charts[face];
    let placement = prepared.placements[face];
    let (width, height) = (chart.width_texels, chart.height_texels);
    assert!(rebase_to_cell_block(layout, face, placement, width, height).is_ok());

    let cell = layout.blocks[layout.chart_blocks[face] as usize];
    let past_right = ChartPlacement {
        x: cell.x + cell.width - width + 1,
        ..placement
    };
    let other_layer = ChartPlacement {
        layer: placement.layer + 1,
        ..placement
    };
    let before_origin = ChartPlacement {
        x: cell.x.wrapping_sub(1),
        ..placement
    };
    for outside in [past_right, other_layer, before_origin] {
        let error = rebase_to_cell_block(layout, face, outside, width, height)
            .expect_err("a rect outside its cell block must be rejected");
        assert!(
            matches!(
                error,
                AnimatedWeightMapBakeError::BlockOutsideCellBlock { face: 1, .. }
            ),
            "{error}"
        );
        assert!(error.to_string().contains("leaves its lightmap cell block"));
    }
}
