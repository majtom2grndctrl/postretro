// Lightmap atlas preparation: vertex splitting, packing, and lightmap UV write-back.
// See: context/lib/build_pipeline.md §Compiler pipeline

use std::collections::HashSet;

use glam::Vec3;

use postretro_level_format::lightmap::LIGHTMAP_POOL_LAYER_EDGE;

use super::block_layout::{BlockLayout, BlockOrdering, pack_cell_blocks_within};
use super::charts::{Chart, check_chart_extents, plan_charts};
use super::{CompositedAtlas, LightmapBakeError};
use crate::bake_control::BakeControl;
use crate::chart_raster::{CHART_PADDING_TEXELS, ChartPlacement};
use crate::geometry::GeometryResult;
use crate::light_namespaces::StaticBakedLights;
use crate::map_data::MapLightmapScaleRegion;

/// Cheap pre-bake setup: chart planning, per-cell block packing, bake-layer
/// placement, and writing block ids and block-local lightmap UVs back into
/// geometry. Returned by [`prepare_atlas`] and consumed by both the warm
/// per-light composite path and the cold per-layer bake — called once before
/// either branch, so the layout is shared.
#[derive(Debug)]
pub struct PreparedAtlas {
    pub charts: Vec<Chart>,
    /// Bake-layer placement of each chart.
    pub placements: Vec<ChartPlacement>,
    /// Edge of every internal bake layer.
    pub atlas_width: u32,
    pub atlas_height: u32,
    /// Internal bake layers the blocks were packed into.
    pub layer_count: u32,
    /// Cell blocks in block-id order and each chart's block.
    pub layout: BlockLayout,
}

/// [`prepare_atlas_ordered`] with blocks in cell-id order at the default
/// direction scale, for callers without a cluster partition.
pub fn prepare_atlas(
    geom: &mut GeometryResult,
    static_lights: &StaticBakedLights<'_>,
    texel_density: f32,
    scale_regions: &[MapLightmapScaleRegion],
) -> Result<PreparedAtlas, LightmapBakeError> {
    prepare_atlas_ordered(
        geom,
        static_lights,
        texel_density,
        scale_regions,
        BlockOrdering::by_cell_id(super::DIRECTION_TEXEL_SCALE),
        &BakeControl::unrestricted(),
    )
}

/// Prepare charts and cell blocks, and assign each vertex its block id and
/// block-local lightmap UV. Runs `split_shared_vertices`, `plan_charts`,
/// `check_chart_extents`, `pack_cell_blocks`, and `assign_lightmap_uvs`. Does
/// NOT run the per-texel ray casting.
///
/// Called once before either bake branch, so the layout is shared. Vertex
/// splitting and UV writes run on all non-empty geometry with static lights.
/// Without static lights the section has no blocks, so vertices keep block 0,
/// but charts and placements are still returned for the animated-light
/// passes; when those blocks exceed the runtime limits, placements come back
/// empty instead of failing the build. Empty geometry returns an empty layout
/// without mutating anything.
pub fn prepare_atlas_ordered(
    geom: &mut GeometryResult,
    static_lights: &StaticBakedLights<'_>,
    texel_density: f32,
    scale_regions: &[MapLightmapScaleRegion],
    ordering: BlockOrdering<'_>,
    control: &BakeControl,
) -> Result<PreparedAtlas, LightmapBakeError> {
    prepare_atlas_within(
        geom,
        static_lights,
        texel_density,
        scale_regions,
        ordering,
        LIGHTMAP_POOL_LAYER_EDGE,
        control,
    )
}

/// [`prepare_atlas_ordered`] packing blocks against `pool_edge` rather than
/// the runtime's, so tests can bake multi-block cells from small charts.
/// Charts are still checked against the runtime pool edge; callers keep them
/// within `pool_edge`.
pub(crate) fn prepare_atlas_within(
    geom: &mut GeometryResult,
    static_lights: &StaticBakedLights<'_>,
    texel_density: f32,
    scale_regions: &[MapLightmapScaleRegion],
    ordering: BlockOrdering<'_>,
    pool_edge: u32,
    control: &BakeControl,
) -> Result<PreparedAtlas, LightmapBakeError> {
    if geom.geometry.vertices.is_empty() || geom.geometry.faces.is_empty() {
        return Ok(PreparedAtlas {
            charts: Vec::new(),
            placements: Vec::new(),
            atlas_width: 1,
            atlas_height: 1,
            layer_count: 1,
            layout: empty_layout(ordering),
        });
    }

    if static_lights.is_empty() {
        // Plan charts anyway — the animated-light-chunks builder needs per-face
        // UV bounds and placements even when no static lights exist. Vertex
        // splitting and UV assignment are skipped because the section carries
        // no blocks for any vertex to name. A layout the runtime could not hold
        // is tolerated here for the same reason: placements come back empty,
        // and the animated passes then plan no chunks. The empty layout keeps
        // the direction scale, which the placeholder id-22 header carries.
        let charts = plan_charts(geom, texel_density, scale_regions)?;
        let pack = check_chart_extents(&charts, texel_density, scale_regions)
            .and_then(|()| pack_cell_blocks_within(&charts, ordering, pool_edge, control));
        return Ok(match pack {
            Ok(pack) => PreparedAtlas {
                charts,
                placements: pack.placements,
                atlas_width: pack.layer_dim,
                atlas_height: pack.layer_dim,
                layer_count: pack.layer_count,
                layout: pack.layout,
            },
            Err(_) => PreparedAtlas {
                charts,
                placements: Vec::new(),
                atlas_width: 1,
                atlas_height: 1,
                layer_count: 1,
                layout: empty_layout(ordering),
            },
        });
    }

    // Ensure no vertex index is shared across faces — each face must own its own lightmap UV slot.
    split_shared_vertices(geom);

    let charts = plan_charts(geom, texel_density, scale_regions)?;
    check_chart_extents(&charts, texel_density, scale_regions)?;
    let pack = pack_cell_blocks_within(&charts, ordering, pool_edge, control)?;
    if !pack.placements.is_empty() {
        assign_lightmap_uvs(geom, &charts, &pack.placements, &pack.layout);
    }

    Ok(PreparedAtlas {
        charts,
        placements: pack.placements,
        atlas_width: pack.layer_dim,
        atlas_height: pack.layer_dim,
        layer_count: pack.layer_count,
        layout: pack.layout,
    })
}

/// A layout with no blocks at the ordering's normalized direction scale.
fn empty_layout(ordering: BlockOrdering<'_>) -> BlockLayout {
    BlockLayout {
        direction_texel_scale: super::encode::normalized_direction_texel_scale(
            ordering.direction_texel_scale,
        ),
        ..BlockLayout::default()
    }
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

/// Continuous texel position of `world_p` inside `chart` placed with its
/// padded top-left at `(origin_x, origin_y)`: the interior starts one padding
/// in, and the chart's UV extent spans the interior. Shared by UV assignment
/// (block-local origin) and the rebase proof (bake-layer origin).
pub(crate) fn chart_texel_position(
    chart: &Chart,
    origin_x: u32,
    origin_y: u32,
    world_p: Vec3,
) -> (f32, f32) {
    let padding = CHART_PADDING_TEXELS as f32;
    let interior_w = ((chart.width_texels as f32) - 2.0 * padding).max(1.0);
    let interior_h = ((chart.height_texels as f32) - 2.0 * padding).max(1.0);
    let scale_u = interior_w / chart.uv_extent[0].max(1.0e-6);
    let scale_v = interior_h / chart.uv_extent[1].max(1.0e-6);
    let rel = world_p - chart.origin;
    let local_u = rel.dot(chart.u_axis) - chart.uv_min[0];
    let local_v = rel.dot(chart.v_axis) - chart.uv_min[1];
    (
        (origin_x as f32 + padding) + local_u * scale_u,
        (origin_y as f32 + padding) + local_v * scale_v,
    )
}

/// Quantize a texel position over `extent` texels to the vertex's unorm16 UV.
pub(crate) fn quantize_lightmap_uv(texel: f32, extent: u32) -> u16 {
    let uv = (texel / extent as f32).clamp(0.0, 1.0);
    (uv * 65535.0 + 0.5) as u16
}

/// Write each face vertex's block (`id + 1`) and its UV over that block's
/// extent, computed from the chart's block-local placement. Vertices no face
/// references keep block 0 and UV 0.
pub(super) fn assign_lightmap_uvs(
    geom: &mut GeometryResult,
    charts: &[Chart],
    placements: &[ChartPlacement],
    layout: &BlockLayout,
) {
    for vert in &mut geom.geometry.vertices {
        vert.lightmap_uv = [0, 0];
        vert.lightmap_block = 0;
    }
    let ranges = geom.face_index_ranges.clone();

    for (face_index, chart) in charts.iter().enumerate() {
        let block_id = layout.chart_blocks[face_index];
        let block = &layout.blocks[block_id as usize];
        let (local_x, local_y) = layout.local_placement(face_index, &placements[face_index]);
        // `check_block_limits` bounds the block count to `MAX_LIGHTMAP_BLOCKS`,
        // so `id + 1` fits the vertex's `u16`.
        let vertex_block = u16::try_from(block_id + 1).expect("block count limit keeps ids in u16");
        let range = ranges[face_index];
        let start = range.index_offset as usize;
        let end = start + range.index_count as usize;

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
                let (tx, ty) =
                    chart_texel_position(chart, local_x, local_y, Vec3::from(vert.position));
                vert.lightmap_uv = [
                    quantize_lightmap_uv(tx, block.width),
                    quantize_lightmap_uv(ty, block.height),
                ];
                vert.lightmap_block = vertex_block;
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
