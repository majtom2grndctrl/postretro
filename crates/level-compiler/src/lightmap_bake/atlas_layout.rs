// Lightmap atlas preparation: vertex splitting, packing, and lightmap UV write-back.
// See: context/lib/build_pipeline.md §Compiler pipeline

use std::collections::HashSet;

use glam::Vec3;

use super::atlas_pack::{PackOutput, pack_layers};
use super::charts::{Chart, plan_charts};
use super::{CompositedAtlas, LightmapBakeError, MAX_ATLAS_DIMENSION};
use crate::chart_raster::{CHART_PADDING_TEXELS, ChartPlacement};
use crate::geometry::GeometryResult;
use crate::light_namespaces::StaticBakedLights;
use crate::map_data::MapLightmapScaleRegion;

/// Cheap pre-bake setup: chart planning, MaxRects packing across atlas layers,
/// and writing lightmap UVs back into geometry. Returned by [`prepare_atlas`]
/// and consumed by both the warm per-light composite path and the cold
/// whole-atlas bake — called once before either branch, so the atlas layout is
/// shared.
#[derive(Debug)]
pub struct PreparedAtlas {
    pub charts: Vec<Chart>,
    pub placements: Vec<ChartPlacement>,
    pub atlas_width: u32,
    pub atlas_height: u32,
    /// Number of atlas array layers the multi-bin packer produced — `1` when
    /// every chart fits a single layer, more when a leaf spills onto a new one.
    pub layer_count: u32,
}

/// Prepare atlas charts and assign lightmap UVs into geometry. Runs
/// `split_shared_vertices`, `plan_charts`, `pack_layers`, and
/// `assign_lightmap_uvs`. Does NOT run the per-texel ray casting.
///
/// Called once before either bake branch — the warm per-light composite path
/// and the cold whole-atlas bake — so the atlas layout is shared. The
/// mutations applied here — vertex splitting and lightmap UV writes — run on
/// all non-empty geometry, regardless of whether a full per-texel bake is
/// needed. Empty geometry returns a placeholder immediately without running
/// any mutations.
pub fn prepare_atlas(
    geom: &mut GeometryResult,
    static_lights: &StaticBakedLights<'_>,
    texel_density: f32,
    scale_regions: &[MapLightmapScaleRegion],
) -> Result<PreparedAtlas, LightmapBakeError> {
    if geom.geometry.vertices.is_empty() || geom.geometry.faces.is_empty() {
        return Ok(PreparedAtlas {
            charts: Vec::new(),
            placements: Vec::new(),
            atlas_width: 1,
            atlas_height: 1,
            layer_count: 1,
        });
    }

    if static_lights.is_empty() {
        // Plan charts anyway — the animated-light-chunks builder needs per-face UV bounds and
        // placements even when no static lights exist. Vertex splitting and UV assignment are
        // skipped because the empty bake path returns a placeholder section that no atlas
        // sampling consumes.
        let charts = plan_charts(geom, texel_density, scale_regions)?;
        let pack = match pack_layers(&charts, MAX_ATLAS_DIMENSION, texel_density) {
            Ok(p) => p,
            Err(_) => PackOutput {
                layer_count: 1,
                atlas_width: 1,
                atlas_height: 1,
                placements: Vec::new(),
            },
        };
        return Ok(PreparedAtlas {
            charts,
            placements: pack.placements,
            atlas_width: pack.atlas_width,
            atlas_height: pack.atlas_height,
            layer_count: pack.layer_count,
        });
    }

    // Ensure no vertex index is shared across faces — each face must own its own lightmap UV slot.
    split_shared_vertices(geom);

    let charts = plan_charts(geom, texel_density, scale_regions)?;

    // `pack_layers` owns the `ChartTooLarge` check against its `max_dim`, so the
    // pre-pack loop that duplicated it is gone — one source of truth.
    let pack = pack_layers(&charts, MAX_ATLAS_DIMENSION, texel_density)?;
    if pack.placements.is_empty() {
        return Ok(PreparedAtlas {
            charts,
            placements: pack.placements,
            atlas_width: pack.atlas_width,
            atlas_height: pack.atlas_height,
            layer_count: pack.layer_count,
        });
    }

    assign_lightmap_uvs(geom, &charts, &pack);

    Ok(PreparedAtlas {
        charts,
        placements: pack.placements,
        atlas_width: pack.atlas_width,
        atlas_height: pack.atlas_height,
        layer_count: pack.layer_count,
    })
}

fn split_shared_vertices(geom: &mut GeometryResult) {
    let face_count = geom.face_index_ranges.len();
    if face_count <= 1 {
        return;
    }

    // First-seen face owns the original vertex; subsequent faces get duplicates.
    let mut owner: Vec<u32> = vec![u32::MAX; geom.geometry.vertices.len()];
    let ranges = geom.face_index_ranges.clone();
    let mut face_remap: std::collections::HashMap<u32, u32> = std::collections::HashMap::new();

    for (face_idx, range) in ranges.iter().enumerate() {
        let face_idx_u32 = face_idx as u32;
        let start = range.index_offset as usize;
        let end = start + range.index_count as usize;
        face_remap.clear();

        for i in start..end {
            let vi = geom.geometry.indices[i];
            let cur = owner[vi as usize];
            if cur == u32::MAX {
                owner[vi as usize] = face_idx_u32;
            } else if cur != face_idx_u32 {
                let new_index = if let Some(&dup) = face_remap.get(&vi) {
                    dup
                } else {
                    let dup_vertex = geom.geometry.vertices[vi as usize].clone();
                    let new_index = geom.geometry.vertices.len() as u32;
                    geom.geometry.vertices.push(dup_vertex);
                    owner.push(face_idx_u32);
                    face_remap.insert(vi, new_index);
                    new_index
                };
                geom.geometry.indices[i] = new_index;
            }
        }
    }
}

pub(super) fn assign_lightmap_uvs(geom: &mut GeometryResult, charts: &[Chart], pack: &PackOutput) {
    // All layers share one dimension; UVs normalize against that per-layer size.
    let atlas_w_f = pack.atlas_width as f32;
    let atlas_h_f = pack.atlas_height as f32;
    let ranges = geom.face_index_ranges.clone();

    for (face_index, chart) in charts.iter().enumerate() {
        let placement = pack.placements[face_index];
        let range = ranges[face_index];
        let start = range.index_offset as usize;
        let end = start + range.index_count as usize;
        let padding = CHART_PADDING_TEXELS as f32;
        let interior_w = (chart.width_texels as f32) - 2.0 * padding;
        let interior_h = (chart.height_texels as f32) - 2.0 * padding;
        let interior_w = interior_w.max(1.0);
        let interior_h = interior_h.max(1.0);
        let scale_u = interior_w / chart.uv_extent[0].max(1.0e-6);
        let scale_v = interior_h / chart.uv_extent[1].max(1.0e-6);

        let geom_section = &mut geom.geometry;
        let mut assigned: HashSet<usize> = HashSet::new();
        let mut tri = start;
        while tri + 3 <= end {
            for j in 0..3 {
                let vi = geom_section.indices[tri + j] as usize;
                if !assigned.insert(vi) {
                    continue;
                }
                let vert = &mut geom_section.vertices[vi];
                let world_p = Vec3::from(vert.position);
                let rel = world_p - chart.origin;
                let local_u = rel.dot(chart.u_axis) - chart.uv_min[0];
                let local_v = rel.dot(chart.v_axis) - chart.uv_min[1];
                let tx = (placement.x as f32 + padding) + local_u * scale_u;
                let ty = (placement.y as f32 + padding) + local_v * scale_v;
                let atlas_u = (tx / atlas_w_f).clamp(0.0, 1.0);
                let atlas_v = (ty / atlas_h_f).clamp(0.0, 1.0);
                vert.lightmap_uv = [
                    (atlas_u * 65535.0 + 0.5) as u16,
                    (atlas_v * 65535.0 + 0.5) as u16,
                ];
                // Select the atlas array slice. `layer` is capped at
                // `MAX_ATLAS_LAYERS` (256), well within the on-disk `u16`.
                vert.lightmap_layer = placement.layer as u16;
            }
            tri += 3;
        }
    }
}

/// Copy one chart-local bake buffer into its disjoint rectangle in the
/// layer-major atlas. `pack_layers` establishes the non-overlap contract that
/// makes the completion order of these copies irrelevant to output bytes.
pub(super) fn scatter_chart_into_atlas(
    chart_atlas: &CompositedAtlas,
    placement: &ChartPlacement,
    atlas: &mut CompositedAtlas,
) {
    debug_assert_eq!(chart_atlas.layer_count, 1);
    debug_assert!(chart_atlas.atlas_width + placement.x <= atlas.atlas_width);
    debug_assert!(chart_atlas.atlas_height + placement.y <= atlas.atlas_height);
    debug_assert!(placement.layer < atlas.layer_count);

    let chart_w = chart_atlas.atlas_width as usize;
    let chart_h = chart_atlas.atlas_height as usize;
    let atlas_w = atlas.atlas_width as usize;
    let layer_offset = placement.layer as usize * atlas_w * atlas.atlas_height as usize;

    for row in 0..chart_h {
        let source_start = row * chart_w;
        let source_end = source_start + chart_w;
        let dest_start =
            layer_offset + (placement.y as usize + row) * atlas_w + placement.x as usize;
        let dest_end = dest_start + chart_w;
        atlas.irradiance[dest_start * 4..dest_end * 4]
            .copy_from_slice(&chart_atlas.irradiance[source_start * 4..source_end * 4]);
        atlas.direction[dest_start..dest_end]
            .copy_from_slice(&chart_atlas.direction[source_start..source_end]);
        atlas.coverage[dest_start..dest_end]
            .copy_from_slice(&chart_atlas.coverage[source_start..source_end]);
    }
}
