//! Atlas preparation stage: the oversize-face cut, the face-identity rebuild, the cell partition, block packing.
//! See: context/lib/build_pipeline.md §Compiler pipeline (atlas preparation)

use std::collections::HashSet;
use std::ops::Range;

use bvh::bvh::Bvh;
use postretro_level_format::animated_lightmap_atlas::ANIMATED_BLOCK_CAP;
use postretro_level_format::bsp::BspLeavesSection;
use postretro_level_format::bvh::BvhSection;
use postretro_level_format::cell_draw_index::CellDrawIndexSection;
use postretro_level_format::lightmap::LIGHTMAP_POOL_LAYER_EDGE;

use super::cell_partition::{CellPartitionInputs, CellPartitionPlan, plan_cell_partition};
use super::portals;
use crate::animated_block_ids::AnimatedBlockGuardError;
use crate::animated_light_chunks::animated_candidate_face_count;
use crate::bake_control::BakeControl;
use crate::bvh_build::{BvhPrimitive, build_bvh};
use crate::cell_draw_index_bake::bake_cell_draw_index;
use crate::geometry::GeometryResult;
use crate::light_namespaces::{AnimatedBakedLights, StaticBakedLights};
use crate::lightmap_bake::{self, BlockOrdering, LightmapConfig, PreparedAtlas};
use crate::map_data::MapData;

pub(super) struct AtlasStageInputs<'a> {
    pub(super) map_data: &'a MapData,
    pub(super) static_lights: &'a StaticBakedLights<'a>,
    pub(super) animated_lights: &'a AnimatedBakedLights<'a>,
    pub(super) config: &'a LightmapConfig,
    pub(super) generated_portals: &'a [portals::Portal],
    pub(super) exterior_leaves: &'a HashSet<usize>,
    pub(super) control: &'a BakeControl,
}

/// The pre-atlas face-identity set, rebuilt over cut geometry. Every stage
/// after atlas preparation that names faces, index ranges or BVH leaves reads
/// these, never the pre-cut ones.
pub(super) struct RebuiltFaceIdentity {
    pub(super) bvh: Bvh<f32, 3>,
    pub(super) primitives: Vec<BvhPrimitive>,
    pub(super) bvh_section: BvhSection,
    pub(super) cell_draw_index: Option<CellDrawIndexSection>,
}

pub(super) struct AtlasStageOutput {
    pub(super) prepared: PreparedAtlas,
    pub(super) partition: CellPartitionPlan,
    /// `Some` when a face was cut.
    pub(super) rebuilt: Option<RebuiltFaceIdentity>,
    /// `Some` when a face was cut: the geometry before the cut, which the SDF
    /// bakes so density edits that move a cut never re-key it.
    pub(super) pre_cut_geometry: Option<GeometryResult>,
}

/// Plan charts and cut oversize faces; when a face was cut, rebuild the
/// face-identity set (leaf face ranges, BVH, CellDrawIndex) over the cut
/// geometry; resolve the cell partition from the final leaves and BVH; check
/// the conservative animated-block bound; then pack blocks in partition
/// order. `leaves` is updated in place; `bvh_section` is the pre-atlas BVH.
pub(super) fn prepare_atlas_stage(
    inputs: AtlasStageInputs<'_>,
    geometry: &mut GeometryResult,
    leaves: &mut BspLeavesSection,
    bvh_section: &BvhSection,
) -> anyhow::Result<AtlasStageOutput> {
    let prepare_error = |e| anyhow::anyhow!("Lightmap atlas prepare failed: {e}");
    let cut = lightmap_bake::plan_cut_charts(
        geometry,
        inputs.static_lights,
        inputs.config.lightmap_density,
        &inputs.map_data.lightmap_scale_regions,
        LIGHTMAP_POOL_LAYER_EDGE,
    )
    .map_err(prepare_error)?;
    let rebuilt = match &cut.face_remap {
        Some(face_remap) => Some(rebuild_face_identity(geometry, leaves, face_remap)?),
        None => None,
    };

    let partition = plan_cell_partition(CellPartitionInputs {
        generated_portals: inputs.generated_portals,
        streaming_seam_regions: &inputs.map_data.streaming_seam_regions,
        stream_resident_regions: &inputs.map_data.stream_resident_regions,
        stream_priority_regions: &inputs.map_data.stream_priority_regions,
        leaves,
        exterior_leaves: inputs.exterior_leaves,
        bvh: rebuilt.as_ref().map_or(bvh_section, |r| &r.bvh_section),
    })?;

    let animated_faces = animated_candidate_face_count(inputs.animated_lights, &cut.charts);
    if animated_faces > ANIMATED_BLOCK_CAP as usize {
        let error = AnimatedBlockGuardError::BlockCountOverCap {
            count: animated_faces,
            cap: ANIMATED_BLOCK_CAP,
        };
        anyhow::bail!("Lightmap atlas prepare failed: {error}");
    }

    let prepared = lightmap_bake::pack_cut_charts(
        geometry,
        inputs.static_lights,
        cut.charts,
        BlockOrdering {
            direction_texel_scale: inputs.config.direction_texel_scale,
            cell_clusters: &partition.cell_clusters(),
        },
        LIGHTMAP_POOL_LAYER_EDGE,
        inputs.control,
    )
    .map_err(prepare_error)?;
    Ok(AtlasStageOutput {
        prepared,
        partition,
        rebuilt,
        pre_cut_geometry: cut.pre_cut,
    })
}

/// Rebuild the face-identity set over cut geometry: each leaf's face range
/// follows its faces' replacements, then the BVH and CellDrawIndex are built
/// as the pre-atlas stages built them.
pub(super) fn rebuild_face_identity(
    geometry: &GeometryResult,
    leaves: &mut BspLeavesSection,
    face_remap: &[Range<usize>],
) -> anyhow::Result<RebuiltFaceIdentity> {
    remap_leaf_face_ranges(leaves, face_remap);
    let (bvh, primitives, bvh_section) = build_bvh(geometry)
        .map_err(|e| anyhow::anyhow!("BVH rebuild after the face cut failed: {e}"))?;
    let cell_draw_index = bake_cell_draw_index(&bvh_section.leaves, &leaves.leaves);
    Ok(RebuiltFaceIdentity {
        bvh,
        primitives,
        bvh_section,
        cell_draw_index,
    })
}

/// A leaf's faces are contiguous, and each face's replacements take its
/// slot, so a leaf's new range runs from its first face's first replacement
/// to its last face's last.
fn remap_leaf_face_ranges(leaves: &mut BspLeavesSection, face_remap: &[Range<usize>]) {
    let total = face_remap.last().map_or(0, |range| range.end);
    for leaf in &mut leaves.leaves {
        let (start, count) = (leaf.face_start as usize, leaf.face_count as usize);
        let new_start = face_remap.get(start).map_or(total, |range| range.start);
        let new_end = if count == 0 {
            new_start
        } else {
            face_remap[start + count - 1].end
        };
        leaf.face_start = new_start as u32;
        leaf.face_count = (new_end - new_start) as u32;
    }
}

#[cfg(test)]
mod tests;
