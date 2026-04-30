// Animated-light weight-map baker.
// See: context/plans/in-progress/animated-light-weight-maps/index.md

use bvh::bvh::Bvh;
use glam::Vec3;
use postretro_level_format::animated_light_chunks::AnimatedLightChunksSection;
use postretro_level_format::animated_light_weight_maps::{
    AnimatedLightWeightMapsSection, ChunkAtlasRect, TexelLight, TexelLightEntry,
};
use rayon::prelude::*;

use crate::bvh_build::BvhPrimitive;
use crate::chart_raster::{
    CHART_PADDING_TEXELS, ChartPlacement, chart_interior_dims, chart_texel_world_position,
};
use crate::geometry::GeometryResult;
use crate::lightmap_bake::{Chart, light_contribution_and_direction, shadow_visible};
use crate::map_data::MapLight;

/// Dropped as numerical noise; prevents per-texel lists from inflating on
/// contributions too dim to matter.
const WEIGHT_EPSILON: f32 = 1.0e-6;

pub struct WeightMapInputs<'a> {
    pub bvh: &'a Bvh<f32, 3>,
    pub primitives: &'a [BvhPrimitive],
    pub geometry: &'a GeometryResult,
    pub chunk_section: &'a AnimatedLightChunksSection,
    /// Filtered `!is_dynamic && animation.is_some()` — same filter as
    /// `sh_bake.rs` for `animation_descriptors`, so indices agree without remap.
    pub lights: &'a [MapLight],
    pub face_charts: &'a [Chart],
    pub face_placements: &'a [ChartPlacement],
    pub atlas_width: u32,
    pub atlas_height: u32,
}

struct ChunkBakeResult {
    rect: ChunkAtlasRect, // texel_offset filled by concatenation
    /// chunk-local offsets; concatenation pass rewrites to global offsets.
    offset_counts: Vec<TexelLightEntry>,
    texel_lights: Vec<TexelLight>,
}

pub fn bake_animated_light_weight_maps(
    inputs: &WeightMapInputs<'_>,
) -> AnimatedLightWeightMapsSection {
    if inputs.chunk_section.chunks.is_empty() {
        return AnimatedLightWeightMapsSection::empty();
    }

    let chunks = &inputs.chunk_section.chunks;
    let light_indices_pool = &inputs.chunk_section.light_indices;

    let per_chunk: Vec<ChunkBakeResult> = chunks
        .par_iter()
        .map(|chunk| bake_one_chunk(inputs, chunk, light_indices_pool))
        .collect();

    assert_no_overlapping_rects_per_face(chunks, &per_chunk);

    let mut chunk_rects: Vec<ChunkAtlasRect> = Vec::with_capacity(per_chunk.len());
    let mut offset_counts: Vec<TexelLightEntry> = Vec::new();
    let mut texel_lights: Vec<TexelLight> = Vec::new();

    let mut running_texel_offset: u32 = 0;
    for result in per_chunk {
        let ChunkBakeResult {
            mut rect,
            offset_counts: chunk_oc,
            texel_lights: chunk_tl,
        } = result;

        rect.texel_offset = running_texel_offset;
        running_texel_offset += rect.width * rect.height;

        let light_base = texel_lights.len() as u32;
        for entry in chunk_oc.into_iter() {
            offset_counts.push(TexelLightEntry {
                offset: entry.offset + light_base,
                count: entry.count,
            });
        }
        texel_lights.extend(chunk_tl);
        chunk_rects.push(rect);
    }

    // Byte formula mirrors the section encoder.
    const HEADER_SIZE: usize = 16;
    const CHUNK_RECT_SIZE: usize = 20;
    const OFFSET_ENTRY_SIZE: usize = 8;
    const TEXEL_LIGHT_SIZE: usize = 8;
    let byte_size = HEADER_SIZE
        + chunk_rects.len() * CHUNK_RECT_SIZE
        + offset_counts.len() * OFFSET_ENTRY_SIZE
        + texel_lights.len() * TEXEL_LIGHT_SIZE;

    let covered_texels: u32 = offset_counts.iter().filter(|e| e.count > 0).count() as u32;
    let mean_lights_per_covered = if covered_texels == 0 {
        0.0
    } else {
        texel_lights.len() as f64 / covered_texels as f64
    };
    let peak_texels_per_chunk = chunk_rects
        .iter()
        .map(|r| r.width * r.height)
        .max()
        .unwrap_or(0);

    log::info!(
        "[AnimatedLightWeightMaps] {} chunks, {} byte section, {} covered texels, \
         mean {:.2} lights / covered texel, peak {} texels / chunk",
        chunk_rects.len(),
        byte_size,
        covered_texels,
        mean_lights_per_covered,
        peak_texels_per_chunk,
    );

    AnimatedLightWeightMapsSection {
        chunk_rects,
        offset_counts,
        texel_lights,
    }
}

fn bake_one_chunk(
    inputs: &WeightMapInputs<'_>,
    chunk: &postretro_level_format::animated_light_chunks::AnimatedLightChunk,
    light_indices_pool: &[u32],
) -> ChunkBakeResult {
    let face_index = chunk.face_index as usize;
    let chart = &inputs.face_charts[face_index];
    let placement = inputs.face_placements[face_index];
    let (interior_w, interior_h) = chart_interior_dims(chart);

    let (atlas_x, atlas_y, width, height) = chunk_atlas_rect(
        chart,
        placement,
        chunk.uv_min,
        chunk.uv_max,
        inputs.atlas_width,
        inputs.atlas_height,
    );

    let list_start = chunk.index_offset as usize;
    let list_end = list_start + chunk.index_count as usize;
    let chunk_light_indices: &[u32] = &light_indices_pool[list_start..list_end];

    let rect = ChunkAtlasRect {
        atlas_x,
        atlas_y,
        width,
        height,
        texel_offset: 0, // filled by caller
    };

    let texel_count = (width * height) as usize;
    let mut offset_counts: Vec<TexelLightEntry> = Vec::with_capacity(texel_count);
    let mut texel_lights: Vec<TexelLight> = Vec::new();

    let padding = CHART_PADDING_TEXELS as i32;

    let chart_usable = chart.uv_extent[0] > 0.0 && chart.uv_extent[1] > 0.0;

    // Row-major to match section encoding: chunk_rect.texel_offset + ty * width + tx.
    for ty in 0..height {
        for tx in 0..width {
            let ax = atlas_x + tx;
            let ay = atlas_y + ty;
            // Map back into the chart's interior coordinate space.
            let tx_interior = ax as i32 - placement.x as i32 - padding;
            let ty_interior = ay as i32 - placement.y as i32 - padding;

            // Texels outside the chart (artifact of outward rounding): zero-count entry.
            if !chart_usable
                || tx_interior < 0
                || ty_interior < 0
                || tx_interior >= interior_w
                || ty_interior >= interior_h
            {
                offset_counts.push(TexelLightEntry {
                    offset: texel_lights.len() as u32,
                    count: 0,
                });
                continue;
            }

            let world_p =
                chart_texel_world_position(chart, tx_interior, ty_interior, interior_w, interior_h);
            let surface_normal = chart.normal;

            let offset_start = texel_lights.len() as u32;
            let mut count: u32 = 0;
            for &light_index in chunk_light_indices {
                let light = &inputs.lights[light_index as usize];
                let (contribution, _dir) =
                    light_contribution_and_direction(light, world_p, surface_normal);
                let weight = contribution_to_weight(contribution, light.color, light.intensity);
                if weight <= WEIGHT_EPSILON {
                    continue;
                }
                if !shadow_visible(
                    inputs.bvh,
                    inputs.primitives,
                    inputs.geometry,
                    world_p,
                    surface_normal,
                    light,
                ) {
                    continue;
                }
                texel_lights.push(TexelLight {
                    light_index,
                    weight,
                });
                count += 1;
            }

            offset_counts.push(TexelLightEntry {
                offset: offset_start,
                count,
            });
        }
    }

    ChunkBakeResult {
        rect,
        offset_counts,
        texel_lights,
    }
}

/// Strips base color and intensity so the weight is a neutral Lambert × falloff
/// × cone scalar; runtime compose re-applies color/intensity from the descriptor.
/// Picks the dominant color channel to avoid divide-by-near-zero on weak channels.
fn contribution_to_weight(contribution: Vec3, color: [f32; 3], intensity: f32) -> f32 {
    let (c_contrib, c_color) = if color[0] >= color[1] && color[0] >= color[2] {
        (contribution.x, color[0])
    } else if color[1] >= color[2] {
        (contribution.y, color[1])
    } else {
        (contribution.z, color[2])
    };
    let denom = c_color * intensity;
    if denom <= 1.0e-6 {
        return 0.0;
    }
    (c_contrib / denom).max(0.0)
}

/// Outward-round (floor min, ceil max) so no covered texel is lost; clamp to atlas extent.
fn chunk_atlas_rect(
    chart: &Chart,
    placement: ChartPlacement,
    chunk_uv_min: [f32; 2],
    chunk_uv_max: [f32; 2],
    atlas_width: u32,
    atlas_height: u32,
) -> (u32, u32, u32, u32) {
    let (interior_w, interior_h) = chart_interior_dims(chart);
    let padding = CHART_PADDING_TEXELS as f32;

    // Degenerate chart — one-texel rect so downstream width × height ≥ 1.
    if chart.uv_extent[0] <= 0.0 || chart.uv_extent[1] <= 0.0 {
        return (placement.x, placement.y, 1, 1);
    }

    let scale_u = interior_w as f32 / chart.uv_extent[0];
    let scale_v = interior_h as f32 / chart.uv_extent[1];

    let fx_min_unclamped =
        placement.x as f32 + padding + (chunk_uv_min[0] - chart.uv_min[0]) * scale_u;
    let fx_max_unclamped =
        placement.x as f32 + padding + (chunk_uv_max[0] - chart.uv_min[0]) * scale_u;
    let fy_min_unclamped =
        placement.y as f32 + padding + (chunk_uv_min[1] - chart.uv_min[1]) * scale_v;
    let fy_max_unclamped =
        placement.y as f32 + padding + (chunk_uv_max[1] - chart.uv_min[1]) * scale_v;

    // Clamp before `f32 as u32`: a misplaced chart can put coordinates below 0,
    // and `(-n as u32)` saturates to 0 (wrong). Clamping pins rogue rects to
    // the atlas edge; the interior-check loop in `bake_one_chunk` skips out-of-range texels.
    let atlas_w_f = atlas_width as f32;
    let atlas_h_f = atlas_height as f32;
    let fx_min = fx_min_unclamped.clamp(0.0, atlas_w_f);
    let fx_max = fx_max_unclamped.clamp(0.0, atlas_w_f);
    let fy_min = fy_min_unclamped.clamp(0.0, atlas_h_f);
    let fy_max = fy_max_unclamped.clamp(0.0, atlas_h_f);

    let ax_min_raw = fx_min.floor() as u32;
    let ay_min_raw = fy_min.floor() as u32;
    let ax_max_raw = (fx_max.ceil() as u32).max(ax_min_raw + 1);
    let ay_max_raw = (fy_max.ceil() as u32).max(ay_min_raw + 1);

    // Clamp the min corner: without this, a chart past the atlas bound can make
    // `ax_max - ax_min` underflow, handing a 1-texel rect to `textureStore` at
    // an out-of-bounds coord. Pin to the last valid texel column/row.
    let ax_min = ax_min_raw.min(atlas_width.saturating_sub(1));
    let ay_min = ay_min_raw.min(atlas_height.saturating_sub(1));
    let ax_max = ax_max_raw.min(atlas_width).max(ax_min + 1);
    let ay_max = ay_max_raw.min(atlas_height).max(ay_min + 1);
    let width = ax_max - ax_min;
    let height = ay_max - ay_min;

    (ax_min, ay_min, width, height)
}

/// If this fires, the UV packer violated the required 1-atlas-texel gap between
/// adjacent chunk boundaries within a face — fix the packer, not this baker.
fn assert_no_overlapping_rects_per_face(
    chunks: &[postretro_level_format::animated_light_chunks::AnimatedLightChunk],
    per_chunk: &[ChunkBakeResult],
) {
    use std::collections::HashMap;
    let mut by_face: HashMap<u32, Vec<usize>> = HashMap::new();
    for (i, c) in chunks.iter().enumerate() {
        by_face.entry(c.face_index).or_default().push(i);
    }
    for (face_index, indices) in &by_face {
        for (i_idx, &i) in indices.iter().enumerate() {
            let a = &per_chunk[i].rect;
            for &j in &indices[i_idx + 1..] {
                let b = &per_chunk[j].rect;
                let overlap_x = a.atlas_x < b.atlas_x + b.width && b.atlas_x < a.atlas_x + a.width;
                let overlap_y =
                    a.atlas_y < b.atlas_y + b.height && b.atlas_y < a.atlas_y + a.height;
                assert!(
                    !(overlap_x && overlap_y),
                    "animated-light chunks {i} and {j} on face {face_index} produced \
                     overlapping atlas rects after outward rounding \
                     ({}x{}+{}+{} vs {}x{}+{}+{}). Fix the UV chunk packer — it must \
                     leave a 1-atlas-texel gap between adjacent chunk UV boundaries \
                     within a face.",
                    a.width,
                    a.height,
                    a.atlas_x,
                    a.atlas_y,
                    b.width,
                    b.height,
                    b.atlas_x,
                    b.atlas_y,
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bvh_build::build_bvh;
    use crate::geometry::FaceIndexRange;
    use crate::map_data::{FalloffModel, LightAnimation, LightType};
    use glam::DVec3;
    use postretro_level_format::animated_light_chunks::{
        AnimatedLightChunk, AnimatedLightChunksSection,
    };
    use postretro_level_format::geometry::{FaceMeta, GeometrySection, Vertex};
    use postretro_level_format::texture_names::TextureNamesSection;

    fn xz_quad_face(y: f32, normal_y: f32, vertex_base: f32) -> Vec<Vertex> {
        let n = [0.0, normal_y, 0.0];
        let t = [1.0, 0.0, 0.0];
        vec![
            Vertex::new([vertex_base, y, 0.0], [0.0, 0.0], n, t, true, [0.0, 0.0]),
            Vertex::new(
                [vertex_base + 1.0, y, 0.0],
                [1.0, 0.0],
                n,
                t,
                true,
                [0.0, 0.0],
            ),
            Vertex::new(
                [vertex_base + 1.0, y, 1.0],
                [1.0, 1.0],
                n,
                t,
                true,
                [0.0, 0.0],
            ),
            Vertex::new([vertex_base, y, 1.0], [0.0, 1.0], n, t, true, [0.0, 0.0]),
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
            ),
            Vertex::new(
                [2.0, 0.5, -1.0],
                [1.0, 0.0],
                [0.0, -1.0, 0.0],
                [1.0, 0.0, 0.0],
                true,
                [0.0, 0.0],
            ),
            Vertex::new(
                [2.0, 0.5, 2.0],
                [1.0, 1.0],
                [0.0, -1.0, 0.0],
                [1.0, 0.0, 0.0],
                true,
                [0.0, 0.0],
            ),
            Vertex::new(
                [-1.0, 0.5, 2.0],
                [0.0, 1.0],
                [0.0, -1.0, 0.0],
                [1.0, 0.0, 0.0],
                true,
                [0.0, 0.0],
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

    fn animated_point_light_above() -> MapLight {
        MapLight {
            origin: DVec3::new(0.5, 1.0, 0.5),
            light_type: LightType::Point,
            intensity: 1.0,
            color: [1.0, 1.0, 1.0],
            falloff_model: FalloffModel::Linear,
            falloff_range: 5.0,
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
            cast_shadows: true,
            bake_only: false,
            is_dynamic: false,
            tags: vec![],
        }
    }

    fn bake_with_geometry_and_chunks<F>(
        mut geo: GeometryResult,
        lights: Vec<MapLight>,
        build_chunks: F,
    ) -> AnimatedLightWeightMapsSection
    where
        F: FnOnce(&[Chart]) -> AnimatedLightChunksSection,
    {
        let (bvh, prims, _) = build_bvh(&geo).unwrap();
        let static_lights = crate::light_namespaces::StaticBakedLights::from_lights(&lights);
        let mut lm_inputs = crate::lightmap_bake::LightmapInputs {
            bvh: &bvh,
            primitives: &prims,
            geometry: &mut geo,
            lights: &static_lights,
        };
        let lm_output = crate::lightmap_bake::bake_lightmap(&mut lm_inputs, 0.25).unwrap();

        let chunk_section = build_chunks(&lm_output.charts);

        let inputs = WeightMapInputs {
            bvh: &bvh,
            primitives: &prims,
            geometry: &geo,
            chunk_section: &chunk_section,
            lights: &lights,
            face_charts: &lm_output.charts,
            face_placements: &lm_output.placements,
            atlas_width: lm_output.atlas_width,
            atlas_height: lm_output.atlas_height,
        };
        bake_animated_light_weight_maps(&inputs)
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
        };
        let atlas_size = 64u32;

        // Case 1: placement exactly at the atlas edge.
        let placement = ChartPlacement {
            x: atlas_size - 1,
            y: atlas_size - 1,
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
}
