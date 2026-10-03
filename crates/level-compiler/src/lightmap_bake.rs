// Directional lightmap baker.
// See: context/lib/build_pipeline.md §Compiler pipeline

use std::sync::Mutex;

use bvh::bvh::Bvh;
use bvh::ray::Ray;
use glam::Vec3;
use nalgebra::{Point3, Vector3};
use postretro_level_format::lightmap::{LIGHTMAP_POOL_LAYER_EDGE, LightmapSection};
use rayon::prelude::*;

use crate::bake_control::BakeControl;
use thiserror::Error;

mod atlas_layout;
mod atlas_pack;
mod block_layout;
mod cell_blocks;
mod charts;
mod encode;
mod face_cut;
mod reference;

#[cfg(test)]
pub(crate) use atlas_layout::prepare_atlas_within;
use atlas_layout::scatter_chart_into_atlas;
pub use atlas_layout::{
    CutCharts, PreparedAtlas, pack_cut_charts, plan_cut_charts, prepare_atlas,
    prepare_atlas_ordered,
};
#[cfg(test)]
pub(crate) use atlas_layout::{chart_texel_position, quantize_lightmap_uv};
pub(crate) use atlas_pack::MaxRects;
#[cfg(test)]
pub(crate) use atlas_pack::{pack_layers, pack_layers_with_layer_limit};

pub use block_layout::{BlockLayout, BlockOrdering, CellBlock};
#[cfg(test)]
pub(crate) use cell_blocks::{
    CANDIDATE_WIDTHS, PackedBlock, pack_cell_block, pack_cell_sub_blocks,
};
pub use charts::{Chart, ChartWindow};
pub(crate) use encode::{BlockSectionBuilder, copy_unit_rect, irradiance_format};
#[cfg(test)]
pub(crate) use reference::{bake_monolithic_atlas, bake_monolithic_atlas_controlled};

use crate::bvh_build::BvhPrimitive;
use crate::chart_raster::{CHART_PADDING_TEXELS, ChartPlacement};
use crate::geometry::GeometryResult;
use crate::light_namespaces::StaticBakedLights;
use crate::map_data::{FalloffModel, LightType, MapLight, MapLightmapScaleRegion};
use crate::ray_traversal::BoundedRay;

/// Default atlas texel density: 4 cm per texel.
pub const DEFAULT_TEXEL_DENSITY_METERS: f32 = 0.04;

/// Default per-axis reduction for the static dominant-direction atlas.
///
/// The irradiance atlas stays at the planned chart resolution. Direction is a
/// lower-frequency signal, so its post-composite representation can be stored
/// at half resolution per axis without moving chart UVs or atlas placement.
pub const DIRECTION_TEXEL_SCALE: u32 = 2;

/// Atlas width/height when no face would fit otherwise. Power-of-two for BC6H block alignment.
/// The 4-alignment BC6H requires is satisfied for free since dimensions are always power-of-two
/// ≥ 4 here, meaning `ceil(w/4)` is always exact (no partial trailing block).
pub(crate) const MIN_ATLAS_DIMENSION: u32 = 64;

/// Largest internal bake-layer edge. Bake layers are compiler working planes,
/// never shipped. The layer packer sizes a layer to the smallest power of two
/// that holds the largest group alone, and a cell block is at most one pool
/// layer (`LIGHTMAP_POOL_LAYER_EDGE`), so cell-block packing passes this cap
/// but never needs it. It binds the chart-level packer tests and the dry run
/// use, whose leaf groups no pool layer bounds, and the shadowmask width limit
/// derives from it.
pub(crate) const MAX_ATLAS_DIMENSION: u32 = 8192;

/// Most internal bake layers. Each layer is a unit of the per-light cache
/// partition and of the one-plane bake working set; past this the packer
/// errors so the caller can coarsen density or split the map.
pub(crate) const MAX_ATLAS_LAYERS: u32 = 256;

/// Shadow ray self-intersection offset. `pub(crate)` so the animated weight-map baker uses the
/// same value — both bakers must agree or chunk boundaries show seams.
pub(crate) const RAY_EPSILON: f32 = 1.0e-3;

/// Shadow ray length for directional lights (no position, so must exceed the world diagonal).
const DIRECTIONAL_LIGHT_RAY_LENGTH_METERS: f32 = 10_000.0;

/// Rotates the Fibonacci lattice off its axis so axis-aligned light directions
/// don't land on a degenerate sample, and lets the per-texel `seed` decorrelate
/// adjacent texels. Same "PHBAKER" convention as `sh_bake.rs` — the two bakers
/// share the constant value (not the symbol) so their soft-shadow sampling reads
/// identically. No RNG: identical input yields byte-identical output.
const SAMPLING_LATTICE_OFFSET: u64 = 0x5048_4542_414b_4552; // "PHBAKER"

/// Errors surfaced from the lightmap bake stage. Caller can retry at a coarser texel density.
#[derive(Debug, Error)]
pub enum LightmapBakeError {
    #[error(
        "lightmap atlas layer overflow: the charts need {layer_count} bake layers but the \
         compiler allows at most {max}; raise `--lightmap-density` (or `_lightmap_density`), \
         lower a region's `_lightmap_scale`, or split the map"
    )]
    LayerOverflow { layer_count: u32, max: u32 },
    #[error(
        "lightmap chart has an invalid resolved density: face {face_index} resolved to \
         {density_m_per_texel} m/texel; scale regions must yield a finite positive density"
    )]
    InvalidChartDensity {
        face_index: usize,
        density_m_per_texel: f32,
    },
    #[error(
        "lightmap chart dimension overflow: face {face_index} {axis} extent {extent_m} m at \
         {density_m_per_texel} m/texel requires {texels} texels; raise `--lightmap-density` or \
         reduce `_lightmap_scale`"
    )]
    ChartDimensionOverflow {
        face_index: usize,
        axis: &'static str,
        extent_m: f32,
        density_m_per_texel: f32,
        texels: f32,
    },
    #[error(
        "lightmap leaf too large: leaf {leaf_index}'s {chart_count} chart(s) can't fit a single \
         {max_dim}x{max_dim} atlas layer (an uncut chart past the layer, or a leaf whose charts \
         must share one). Raise `--lightmap-density` or split the map."
    )]
    LeafTooLarge {
        leaf_index: u32,
        chart_count: usize,
        max_dim: u32,
    },
    #[error(
        "lightmap block count {count} exceeds the vertex block-id limit {max}: vertices name a \
         block as a u16 `id + 1`, and a cell past one pool layer counts every one of its \
         blocks. Coarsen the lightmap density, or bake fewer lightmapped cells."
    )]
    BlockCountOverflow { count: usize, max: u32 },
}

pub struct LightmapBakeCtx<'a> {
    pub bvh: &'a Bvh<f32, 3>,
    pub primitives: &'a [BvhPrimitive],
    /// Mutable: baker writes per-vertex lightmap UVs back after atlas placement.
    pub geometry: &'a mut GeometryResult,
    pub lights: &'a StaticBakedLights<'a>,
    /// Ordered compiler-only brush regions that override per-chart lightmap
    /// density. Both warm and cold paths must pass the map's same ordered set.
    pub scale_regions: &'a [MapLightmapScaleRegion],
}

/// CLI-driven configuration for the lightmap bake.
///
/// The cold bake consumes this directly. The warm path owns its cache keys
/// separately, so encode-only fields must also be folded into
/// `lightmap_layer::section_input_hash` rather than relying on this serde
/// derivation alone.
#[derive(serde::Serialize)]
pub struct LightmapConfig {
    pub lightmap_density: f32,
    /// Area-sample count for soft-shadow penumbra visibility (Task 6 knob).
    /// Folded into this stage's `input_hash`, so raising it invalidates the
    /// cache and re-bakes at the higher quality. Default [`DEFAULT_AREA_SAMPLE_COUNT`].
    pub area_sample_count: u32,
    /// Debug bypass for the BC6H compression step. When `true` the irradiance
    /// blob is emitted as uncompressed `Rgba16Float` (the pre-Task-3 layout) so
    /// the bake side stays byte-identical to legacy output for owner-side A/B
    /// comparisons. Default is `false` — BC6H is the production storage. The
    /// field participates in `LightmapConfig`'s serde derivation, so flipping
    /// the bool re-keys the cache; flipping it never silently serves a stale
    /// bake from the wrong format.
    pub uncompressed_irradiance: bool,
    /// Per-axis scale for the post-composite static-direction atlas. The CLI
    /// admits only powers of two up to [`MIN_ATLAS_DIMENSION`]; direct callers
    /// are normalized before encoding so no zero-sized direction atlas can be
    /// emitted.
    pub direction_texel_scale: u32,
}

/// Output of a lightmap bake pass. The animated weight-map baker consumes
/// `charts` + `placements` + the bake-layer size to resolve chunk rects, and
/// `layout` to key them in cell-block texels.
#[derive(Debug)]
pub struct LightmapBakeOutput {
    pub section: LightmapSection,
    pub charts: Vec<Chart>,
    /// Bake-layer placements, parallel to `charts`. Empty when the bake
    /// short-circuits.
    pub placements: Vec<ChartPlacement>,
    pub atlas_width: u32,
    pub atlas_height: u32,
    /// Internal bake layers.
    pub layer_count: u32,
    pub layout: BlockLayout,
}

/// The pre-encode (pre-BC6H) lightmap atlas: the full per-texel `(irradiance,
/// direction, coverage)` buffers exactly as the per-texel bake leaves them after
/// edge dilation, before any irradiance/direction byte encoding.
///
/// This is the **byte-identity comparison seam** for the incremental-bake cache:
/// both the frozen monolithic oracle ([`reference::bake_monolithic_atlas`])
/// and the per-light layer compositor ([`crate::lightmap_layer::composite_layers`]) yield a
/// `CompositedAtlas`, and the cache's exactness gate asserts the two are equal
/// here — before the lossy BC6H encode. Equality at this seam means the warm
/// composite reproduces the cold bake bit-for-bit.
#[derive(Debug, Clone, PartialEq)]
pub struct CompositedAtlas {
    /// RGBA f32, 4 floats per texel (alpha is always 1.0 on covered texels).
    /// Layer-major: layer 0's texels, then layer 1's, and so on.
    pub irradiance: Vec<f32>,
    /// One dominant-direction unit vector per texel. Layer-major.
    pub direction: Vec<Vec3>,
    /// Per-texel coverage flag (logical OR across contributing charts/layers).
    /// Layer-major.
    pub coverage: Vec<bool>,
    pub atlas_width: u32,
    pub atlas_height: u32,
    /// Number of atlas array layers. The three buffers above are sized
    /// `layer_count × atlas_width × atlas_height`; a texel at layer `l`, `(x, y)`
    /// lives at `l × (atlas_width × atlas_height) + y × atlas_width + x`.
    pub layer_count: u32,
}

/// Total texel count for a `layer_count`-layer atlas of `atlas_w × atlas_h`.
///
/// Promotes each dimension to `usize` *before* multiplying: at the cap
/// (`MAX_ATLAS_DIMENSION² × MAX_ATLAS_LAYERS` ≈ 1.7e10) the product overflows
/// `u32`, and a `u32` multiply would wrap to a too-small value — under-sizing
/// the atlas buffers so later layer-major writes land out of bounds.
fn composited_atlas_texel_count(atlas_w: u32, atlas_h: u32, layer_count: u32) -> usize {
    atlas_w as usize * atlas_h as usize * layer_count as usize
}

impl CompositedAtlas {
    /// Allocate a zero-initialized atlas sized for
    /// `layer_count × atlas_w × atlas_h` texels. Matches the monolithic bake's
    /// initial state: irradiance `0.0`, direction `Vec3::Y`, coverage `false`.
    pub fn zeroed(atlas_w: u32, atlas_h: u32, layer_count: u32) -> Self {
        let texels = composited_atlas_texel_count(atlas_w, atlas_h, layer_count);
        Self {
            irradiance: vec![0f32; texels * 4],
            direction: vec![Vec3::Y; texels],
            coverage: vec![false; texels],
            atlas_width: atlas_w,
            atlas_height: atlas_h,
            layer_count,
        }
    }

    /// Run edge dilation in place — the same neighbourhood fill the monolithic
    /// bake applies after the per-chart pass. Composite and monolithic paths must
    /// both call this so the seam compares post-dilation buffers.
    ///
    /// Dilation runs per layer over each layer's own slice: a layer boundary is a
    /// hard edge in the array texture, so a neighbour fill that read across it
    /// would drag a different chart's irradiance into the gutter and corrupt
    /// bilinear samples near layer-adjacent charts.
    pub fn dilate(&mut self) {
        let plane = (self.atlas_width * self.atlas_height) as usize;
        for layer in 0..self.layer_count as usize {
            let texel_start = layer * plane;
            let texel_end = texel_start + plane;
            dilate_edges(
                &mut self.irradiance[texel_start * 4..texel_end * 4],
                &mut self.direction[texel_start..texel_end],
                &mut self.coverage[texel_start..texel_end],
                self.atlas_width,
                self.atlas_height,
            );
        }
    }
}

/// Bake a directional lightmap. Returns a section without blocks when there is nothing to bake.
pub fn bake_lightmap(
    inputs: &mut LightmapBakeCtx<'_>,
    config: &LightmapConfig,
) -> Result<LightmapBakeOutput, LightmapBakeError> {
    bake_lightmap_controlled(inputs, config, &BakeControl::unrestricted())
}

pub fn bake_lightmap_controlled(
    inputs: &mut LightmapBakeCtx<'_>,
    config: &LightmapConfig,
    control: &BakeControl,
) -> Result<LightmapBakeOutput, LightmapBakeError> {
    let cut = plan_cut_charts(
        inputs.geometry,
        inputs.lights,
        config.lightmap_density,
        inputs.scale_regions,
        LIGHTMAP_POOL_LAYER_EDGE,
    )?;
    // The caller's BVH indexes the geometry as it stood before this call; a
    // cut rewrites the index buffer under it. The compiler pipeline cuts in
    // its atlas stage and rebuilds the BVH there.
    assert!(
        cut.face_remap.is_none(),
        "bake_lightmap cannot cut an oversize face under a prebuilt BVH;          prepare the atlas and rebuild the BVH first"
    );
    let prepared = pack_cut_charts(
        inputs.geometry,
        inputs.lights,
        cut.charts,
        BlockOrdering::by_cell_id(DIRECTION_TEXEL_SCALE),
        LIGHTMAP_POOL_LAYER_EDGE,
        &BakeControl::unrestricted(),
    )?;

    bake_prepared_lightmap_controlled(inputs, config, prepared, control)
}

/// Bake a directional lightmap from an atlas prepared by an earlier pipeline
/// stage. This is the production seam used when SH work must observe geometry
/// before lightmap UV assignment while the lightmap bake consumes that exact
/// prepared layout afterward.
pub fn bake_prepared_lightmap_controlled(
    inputs: &mut LightmapBakeCtx<'_>,
    config: &LightmapConfig,
    prepared: PreparedAtlas,
    control: &BakeControl,
) -> Result<LightmapBakeOutput, LightmapBakeError> {
    let texel_density = config.lightmap_density;
    let area_sample_count = config.area_sample_count;

    // No static lights, or atlas prep produced no placements → emit a section with no blocks
    // but return the planned charts/placements so downstream animated-light passes still have
    // per-face UV bounds.
    if inputs.lights.is_empty() || prepared.placements.is_empty() {
        return Ok(LightmapBakeOutput {
            section: LightmapSection::empty(prepared.layout.direction_texel_scale),
            charts: prepared.charts,
            placements: prepared.placements,
            atlas_width: prepared.atlas_width,
            atlas_height: prepared.atlas_height,
            layer_count: prepared.layer_count,
            layout: prepared.layout,
        });
    }

    // Disjoint-direct exclusion lives HERE, at the direct lightmap consumer —
    // not in the `StaticBakedLights` namespace (which keys on position so it can
    // also feed the SH base bake with every baked-tier light). Drop `sdf`-typed
    // lights so `lm_irr` holds only `static_light_map` lights; the `sdf` lights'
    // direct term resolves at runtime via the per-light SDF trace. Their SH
    // bounce is unaffected (the namespace still carries them to SH).
    let static_lights: Vec<&MapLight> = inputs
        .lights
        .entries()
        .iter()
        .map(|e| e.light)
        .filter(|l| l.shadow_type != crate::map_data::ShadowType::Sdf)
        .collect();

    // Author hint: flag any light whose emitter is too small to soften at this
    // atlas density before baking (Task 6). Output-only — never alters the bake.
    warn_sub_texel_penumbra_lights(&static_lights, texel_density);

    let charts = prepared.charts;
    let placements = prepared.placements;
    let atlas_w = prepared.atlas_width;
    let atlas_h = prepared.atlas_height;
    let layer_count = prepared.layer_count;
    let layout = prepared.layout;

    control.publish_total(placements.len());
    let section = bake_layered_section_controlled(
        inputs.bvh,
        inputs.primitives,
        inputs.geometry,
        &static_lights,
        &charts,
        &placements,
        &layout,
        atlas_w,
        atlas_h,
        layer_count,
        area_sample_count,
        config.uncompressed_irradiance,
        control,
    );

    Ok(LightmapBakeOutput {
        section,
        charts,
        placements,
        atlas_width: atlas_w,
        atlas_height: atlas_h,
        layer_count,
        layout,
    })
}

/// Bake, dilate, encode, and discard one bake layer at a time, keeping only
/// the blocks sliced out of it.
///
/// This is the shipping cold path. The layer loop is deliberately serial and
/// ascending. Chart work remains parallel within one layer, but every worker
/// joins before that layer is dilated, encoded, and dropped. The whole-atlas
/// [`reference::bake_monolithic_atlas_controlled`] builder remains as the
/// reference kernel for byte-identity tests.
#[allow(clippy::too_many_arguments)]
fn bake_layered_section_controlled(
    bvh: &Bvh<f32, 3>,
    primitives: &[BvhPrimitive],
    geometry: &GeometryResult,
    static_lights: &[&MapLight],
    charts: &[Chart],
    placements: &[ChartPlacement],
    layout: &BlockLayout,
    atlas_w: u32,
    atlas_h: u32,
    layer_count: u32,
    area_sample_count: u32,
    uncompressed_irradiance: bool,
    control: &BakeControl,
) -> LightmapSection {
    debug_assert_eq!(charts.len(), placements.len());

    let mut builder = BlockSectionBuilder::new(layout, uncompressed_irradiance);
    for layer in 0..layer_count {
        // Keep the layer's full-resolution f32 buffers scoped to this block:
        // the plane drops once its blocks are encoded, bounding the
        // uncompressed working set to one bake layer.
        let atlas = bake_atlas_layer_controlled(
            bvh,
            primitives,
            geometry,
            static_lights,
            charts,
            placements,
            atlas_w,
            atlas_h,
            layer,
            area_sample_count,
            control,
        );
        builder.push_layer(layer, &atlas);
    }
    builder.finish()
}

/// Bake one global atlas layer into a one-layer composited buffer.
///
/// The placement only routes the scatter, its layer rebased to 0 because the
/// temporary atlas has exactly one layer; seeds come from the chart, not the
/// placement.
#[allow(clippy::too_many_arguments)]
fn bake_atlas_layer_controlled(
    bvh: &Bvh<f32, 3>,
    primitives: &[BvhPrimitive],
    geometry: &GeometryResult,
    static_lights: &[&MapLight],
    charts: &[Chart],
    placements: &[ChartPlacement],
    atlas_w: u32,
    atlas_h: u32,
    target_layer: u32,
    area_sample_count: u32,
    control: &BakeControl,
) -> CompositedAtlas {
    // Collect serially from the chart/leaf order emitted by the planner. The
    // vector is only face indices, not per-texel bake data, and makes the
    // layer selection deterministic before per-chart work fans out.
    let layer_faces: Vec<usize> = placements
        .iter()
        .enumerate()
        .filter_map(|(face_idx, placement)| (placement.layer == target_layer).then_some(face_idx))
        .collect();

    // `pack_layers` assigns every chart a disjoint rectangle, so a chart can
    // bake independently. Keep only one local chart buffer per admitted worker
    // and scatter it immediately; collecting every chart buffer would duplicate
    // the uncompressed layer working set.
    let atlas = Mutex::new(CompositedAtlas::zeroed(atlas_w, atlas_h, 1));

    layer_faces.par_iter().for_each(|&face_idx| {
        // Parallel work items must enter exactly once at their outermost
        // boundary. In particular, `checkpoint` alone would honor pause while
        // bypassing the compiler's `-j` concurrency cap.
        let _permit = control.governor().enter();
        let chart = &charts[face_idx];
        let placement = &placements[face_idx];
        let chart_atlas = reference::bake_face_chart(
            bvh,
            primitives,
            geometry,
            static_lights,
            chart,
            area_sample_count,
        );

        // This lock only protects finite row-wise memcpy operations. It never
        // waits on another permit or on stage completion, so holding the permit
        // while waiting for the lock cannot form a nested-wait deadlock. A
        // degenerate chart's local buffer is all-default and intentionally has
        // no scatter work.
        if chart.uv_extent[0] > 0.0 && chart.uv_extent[1] > 0.0 {
            let mut atlas = atlas
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let rebased_placement = ChartPlacement {
                x: placement.x,
                y: placement.y,
                layer: 0,
            };
            scatter_chart_into_atlas(&chart_atlas, &rebased_placement, &mut atlas);
        }
        // Progress means charts baked. Degenerate charts return an empty local
        // contribution, still scatter as a no-op, and still advance here.
        control.advance(1);
    });

    let mut atlas = atlas
        .into_inner()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    // Dilation deliberately remains sequential and un-gated after every chart
    // has scattered, preserving the serial bake's post-pass ordering.
    atlas.dilate();
    atlas
}

pub fn log_stats(section: &LightmapSection, texel_density: f32, static_light_count: usize) {
    let texels: u64 = section
        .blocks
        .iter()
        .map(|b| u64::from(b.width) * u64::from(b.height))
        .sum();
    let largest = section
        .blocks
        .iter()
        .map(|b| (b.width, b.height))
        .max_by_key(|&(w, h)| u32::from(w) * u32::from(h))
        .unwrap_or((0, 0));
    log::info!(
        "Lightmap: {} cell blocks ({} texels, largest {}x{}), {} m/texel, {} static lights baked, \
         irr={} B, dir={} B",
        section.blocks.len(),
        texels,
        largest.0,
        largest.1,
        texel_density,
        static_light_count,
        section
            .blocks
            .iter()
            .map(|b| b.irradiance.len())
            .sum::<usize>(),
        section
            .blocks
            .iter()
            .map(|b| b.direction.len())
            .sum::<usize>(),
    );
}

/// Lambert contribution from one light plus the unit vector toward the light.
/// `pub(crate)` so the animated weight-map baker produces identical irradiance —
/// chunk boundaries must agree with the static bake or seams appear.
pub(crate) fn light_contribution_and_direction(
    light: &MapLight,
    surface_point: Vec3,
    surface_normal: Vec3,
) -> (Vec3, Vec3) {
    match light.light_type {
        LightType::Point => {
            let to_light = Vec3::new(
                light.origin.x as f32 - surface_point.x,
                light.origin.y as f32 - surface_point.y,
                light.origin.z as f32 - surface_point.z,
            );
            let dist = to_light.length();
            if dist < 1.0e-4 {
                return (Vec3::ZERO, Vec3::Y);
            }
            let l = to_light / dist;
            let ndotl = surface_normal.dot(l).max(0.0);
            if ndotl <= 0.0 {
                return (Vec3::ZERO, l);
            }
            let atten = falloff(light, dist);
            (
                Vec3::from(light.color) * (light.intensity * ndotl * atten),
                l,
            )
        }
        LightType::Spot => {
            let to_light = Vec3::new(
                light.origin.x as f32 - surface_point.x,
                light.origin.y as f32 - surface_point.y,
                light.origin.z as f32 - surface_point.z,
            );
            let dist = to_light.length();
            if dist < 1.0e-4 {
                return (Vec3::ZERO, Vec3::Y);
            }
            let l = to_light / dist;
            let ndotl = surface_normal.dot(l).max(0.0);
            if ndotl <= 0.0 {
                return (Vec3::ZERO, l);
            }
            let atten = falloff(light, dist);
            let cone = spot_cone(light, -l);
            (
                Vec3::from(light.color) * (light.intensity * ndotl * atten * cone),
                l,
            )
        }
        LightType::Directional => {
            let aim = Vec3::from(light.cone_direction.unwrap_or([0.0, -1.0, 0.0]));
            let l = (-aim).normalize_or_zero();
            let ndotl = surface_normal.dot(l).max(0.0);
            if ndotl <= 0.0 {
                return (Vec3::ZERO, l);
            }
            (Vec3::from(light.color) * (light.intensity * ndotl), l)
        }
    }
}

/// The direct-contribution floor shared by the lightmap bake and analytic
/// shadowmask coverage. Keeping the comparison in one helper prevents graph
/// membership from drifting away from the bake's `Option` boundary.
pub(crate) const LIGHT_TEXEL_CONTRIBUTION_EPSILON_SQUARED: f32 = 1.0e-12;

pub(crate) fn contribution_covers_shadowmask(contribution_squared: f32) -> bool {
    contribution_squared > LIGHT_TEXEL_CONTRIBUTION_EPSILON_SQUARED
}

/// Whether a light geometrically covers a texel before any visibility ray.
pub(crate) fn light_texel_is_covered(
    light: &MapLight,
    world_p: Vec3,
    surface_normal: Vec3,
) -> bool {
    let (contribution, _) = light_contribution_and_direction(light, world_p, surface_normal);
    contribution_covers_shadowmask(contribution.length_squared())
}

/// One light's contribution to a single atlas texel: the shadowed irradiance
/// (RGB) and the unnormalized weighted direction (`to_light * luminance`),
/// the exact two terms the frozen monolithic reference accumulates per light
/// before the per-texel `irr` sum and `weighted_dir.normalize()`.
///
/// Factored out so the frozen reference and generic per-light consumers share
/// byte-identical math — the cache's bit-for-bit composite gate depends on both
/// paths producing the same float terms in the same order.
///
/// `trace(from, to)` is the segment-clear closure the caller wraps around its
/// own `segment_clear`; it returns `(Vec3::ZERO, Vec3::ZERO)` when the light
/// contributes nothing (below the irradiance floor or fully occluded), so an
/// out-of-range light adds exactly `0.0` — the property that makes per-light
/// layers sum back to the monolithic result.
pub(crate) fn light_texel_contribution(
    light: &MapLight,
    world_p: Vec3,
    surface_normal: Vec3,
    seed: u64,
    area_sample_count: u32,
    trace: impl Fn(Vec3, Vec3) -> bool,
) -> (Vec3, Vec3) {
    let (irr, weighted_dir, _) = light_texel_contribution_and_visibility(
        light,
        world_p,
        surface_normal,
        seed,
        area_sample_count,
        trace,
    );
    (irr, weighted_dir)
}

/// Single-light texel bake plus the raw soft visibility used by ShadowmaskAtlas.
///
/// The returned visibility is `None` when the light has no direct term at this
/// texel (out of range, backfacing, outside spot cone, etc.). Otherwise it is
/// exactly the raw `soft_visibility` value that the irradiance path multiplies
/// into the unshadowed direct term, including `Some(0.0)` for fully occluded
/// but otherwise contributing texels.
pub(crate) fn light_texel_contribution_and_visibility(
    light: &MapLight,
    world_p: Vec3,
    surface_normal: Vec3,
    seed: u64,
    area_sample_count: u32,
    trace: impl Fn(Vec3, Vec3) -> bool,
) -> (Vec3, Vec3, Option<f32>) {
    light_texel_contribution_and_visibility_with(
        light,
        world_p,
        surface_normal,
        seed,
        || SoftProbes::new(light, area_sample_count),
        trace,
    )
}

/// [`light_texel_contribution_and_visibility`] with the light's probe set
/// supplied by the caller, so a chart walk computes it once rather than per
/// texel. `probes` runs only for a soft (non-zero-radius) emitter.
pub(crate) fn light_texel_contribution_and_visibility_with(
    light: &MapLight,
    world_p: Vec3,
    surface_normal: Vec3,
    seed: u64,
    probes: impl FnOnce() -> SoftProbes,
    trace: impl Fn(Vec3, Vec3) -> bool,
) -> (Vec3, Vec3, Option<f32>) {
    let (contribution, to_light) = light_contribution_and_direction(light, world_p, surface_normal);
    if !contribution_covers_shadowmask(contribution.length_squared()) {
        return (Vec3::ZERO, Vec3::ZERO, None);
    }
    // Lightmaps always bake shadowed: an occluded texel goes dark so a
    // `static_light_map` light's static shadow lives in the atlas.
    // `soft_visibility` returns the `[0,1]` unoccluded fraction over an area
    // sample of the emitter — a multi-texel penumbra instead of a hard 1-texel
    // step. `sdf` lights are filtered out of the static set upstream (their
    // direct shadow resolves at runtime), so no double-shadow.
    let v = soft_visibility_with(world_p, surface_normal, light, seed, probes, trace);
    if v <= 0.0 {
        return (Vec3::ZERO, Vec3::ZERO, Some(0.0));
    }
    let irr = contribution * v;
    // Weight the dominant-direction accumulation by `v` too: in a penumbra a
    // partially-occluded light should bias the baked direction less than a
    // fully-visible one, matching the softened irradiance it contributes.
    let lum = (contribution.x + contribution.y + contribution.z) * v;
    let weighted_dir = to_light * lum;
    (irr, weighted_dir, Some(v))
}

pub(crate) fn falloff(light: &MapLight, distance: f32) -> f32 {
    let range = light.falloff_range.max(1.0e-4);
    match light.falloff_model {
        FalloffModel::Linear => (1.0 - distance / range).clamp(0.0, 1.0),
        FalloffModel::InverseDistance => {
            if distance > range {
                0.0
            } else {
                1.0 / distance.max(1.0e-4)
            }
        }
        FalloffModel::InverseSquared => {
            if distance > range {
                0.0
            } else {
                let d2 = (distance * distance).max(1.0e-4);
                1.0 / d2
            }
        }
    }
}

fn spot_cone(light: &MapLight, light_to_surface: Vec3) -> f32 {
    let aim = Vec3::from(light.cone_direction.unwrap_or([0.0, -1.0, 0.0])).normalize_or_zero();
    let inner = light.cone_angle_inner.unwrap_or(0.0);
    let outer = light.cone_angle_outer.unwrap_or(inner + 0.01);
    let cos_inner = inner.cos();
    let cos_outer = outer.cos();
    let cos_theta = aim.dot(light_to_surface.normalize_or_zero());
    smoothstep(cos_outer, cos_inner, cos_theta)
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0).max(1.0e-4)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

// `soft_visibility` and its sampling helpers are the Task-2 deliverable of
// `baked-soft-lightmap-shadows`. The static lightmap bake (Task 3) calls
// `soft_visibility` in the per-texel loop above; the animated weight-map and SH
// bounce bakers (Tasks 4/4b) wrap their own `segment_clear` as the trace closure.

/// Sub-texel-penumbra warning threshold, in atlas texels (Task 6). An emitter
/// whose estimated penumbra spans fewer than this many texels can't produce a
/// visibly-soft edge — the area samples collapse into one texel and the result
/// reads as a hard step. Diagnostic-only: this constant never feeds bake output,
/// so it is exempt from the determinism fixed-constant rule (it only gates a
/// `log::warn!`). One texel is the floor below which softening is wasted.
const SUB_TEXEL_PENUMBRA_THRESHOLD: f32 = 1.0;

/// Coarse estimate of an emitter's penumbra width in **atlas texels**, used by
/// the sub-texel-penumbra author hint (Task 6). Deliberately uses only the
/// emitter size (`_light_size` / `_angular_diameter`), its `_falloff_range`
/// reach, and the atlas texel world-size (`texel_density`) — **no
/// distance-to-occluder term**, matching the no-occluder-distance design of
/// `soft_visibility`. It is an author hint, not an exact penumbra width.
///
/// Point/Spot: the emitter subtends `2·atan(light_size / falloff_range)` from the
/// receiver at the falloff reach; projected back over that reach the world-space
/// penumbra is `angular · falloff_range` (≈ `2·light_size` at small angles).
/// Directional: the angular diameter is given directly; projected over the
/// falloff reach as the characteristic scale. Dividing by `texel_density` yields
/// the span in texels.
fn penumbra_span_texels(light: &MapLight, texel_density: f32) -> f32 {
    let texel = texel_density.max(1.0e-4);
    let reach = light.falloff_range.max(1.0e-4);
    let angular = match light.light_type {
        LightType::Point | LightType::Spot => 2.0 * (light.light_size.max(0.0) / reach).atan(),
        LightType::Directional => light.angular_diameter.max(0.0).to_radians(),
    };
    let world_width = angular * reach;
    world_width / texel
}

/// Whether `light`'s estimated penumbra is narrower than one atlas texel — the
/// pure predicate behind the sub-texel-penumbra warning. A hard-authored light
/// (`light_size`/`angular_diameter == 0`, i.e. zero span) is intentionally *not*
/// flagged: the author opted into a hard edge, so a "too soft to see" hint would
/// be noise. Factored out of the logging path so it is unit-testable without
/// capturing `log` output.
fn penumbra_below_one_texel(light: &MapLight, texel_density: f32) -> bool {
    let span = penumbra_span_texels(light, texel_density);
    span > 0.0 && span < SUB_TEXEL_PENUMBRA_THRESHOLD
}

/// Emit one `log::warn!` per `static_light_map` light whose estimated penumbra is
/// narrower than one atlas texel (Task 6 author hint). Names the light's index
/// and origin so the author can locate the offending emitter. Lights at or above
/// the threshold — and explicitly-hard lights (zero size) — warn not at all.
pub(crate) fn warn_sub_texel_penumbra_lights(lights: &[&MapLight], texel_density: f32) {
    for (index, light) in lights.iter().enumerate() {
        if penumbra_below_one_texel(light, texel_density) {
            log::warn!(
                "[Lightmap] static light #{index} at ({:.2}, {:.2}, {:.2}) subtends a sub-texel \
                 penumbra (~{:.2} texel at {texel_density} m/texel) — its soft shadow will read \
                 as a hard edge; raise `_light_size`/`_angular_diameter` or `_falloff_range`",
                light.origin.x,
                light.origin.y,
                light.origin.z,
                penumbra_span_texels(light, texel_density),
            );
        }
    }
}

/// Probe samples traced before escalating. Tracing this many first lets the
/// fully-lit / fully-shadowed common case (where every probe agrees) stay cheap;
/// only a penumbra (probes disagree) pays for the full set. Fixed constant — the
/// adaptive-escalation threshold must not vary, so the bake stays deterministic
/// regardless of the caller's `full_samples` knob (Task 6).
///
/// The probe positions are a spread **subset** of the full lattice (each probe
/// snapped to the full-lattice index nearest one of [`SOFT_PROBE_SAMPLES`]
/// well-separated ideal directions), not the first N contiguous indices — see
/// [`probe_indices`] for why.
pub(crate) const SOFT_PROBE_SAMPLES: u32 = 4;

/// Default full stratified sample count once a penumbra is detected. The
/// area-sample-count bake knob (Task 6) overrides this per call via
/// `soft_visibility`'s `full_samples` argument; callers without a knob
/// (e.g. the SH bounce baker, where bounce is low-frequency) pass this default.
/// The probe set is a spread **subset** of `full_samples` (not a prefix), so a
/// penumbra's returned fraction is still `clear / full_samples`.
pub(crate) const DEFAULT_AREA_SAMPLE_COUNT: u32 = 32;

/// The `SOFT_PROBE_SAMPLES` full-lattice indices used as the cheap probe set,
/// chosen so the probes spread across the **whole** emitter (both poles and all
/// azimuths), while remaining a strict **subset** of the `full_samples` lattice.
///
/// Why not the first N contiguous indices (the old behaviour): on the Fibonacci
/// sphere/cone lattice the low indices `0,1,2,3` all cluster at the emitter's top
/// pole / hug the cone axis — they spread in azimuth but not in polar angle. An
/// occluder edge crossing the *lower* hemisphere of the emitter then leaves all
/// four contiguous probes in agreement, early-outs to a hard 0/1, and silently
/// drops the penumbra — re-introducing the hard 1-texel edge for occluders facing
/// away from the pole.
///
/// Why not a plain index stride (`k·full/N`): on this lattice azimuth is linear in
/// the index (`golden_angle·i`), so any arithmetic stride aliases the azimuth into
/// a narrow wedge — it fixes polar coverage but *clusters* azimuth, trading one
/// blind spot for another (an azimuthal occluder split would then be missed).
///
/// So each probe `k` is **snapped** to the full-lattice index whose direction is
/// nearest the `k`-th direction of the inherently well-separated
/// `SOFT_PROBE_SAMPLES`-point Fibonacci lattice (computed with `seed == 0`; the
/// per-texel seed only adds a constant azimuth rotation to every sample, so it
/// cannot collapse the relative spread). The result spreads in BOTH polar and
/// azimuth for any `full_samples >= SOFT_PROBE_SAMPLES`. Deterministic — fixed
/// constants, no RNG. The chosen indices are a strict subset of `full_samples`, so
/// escalation only *adds* the remaining samples (no double-count, and no value
/// discontinuity between an escalated and an early-out texel).
///
/// The snap uses the same per-light-type mapping the actual sampling uses
/// (`fibonacci_sphere_sample` for Point/Spot, `cone_jittered_direction` for
/// Directional) so probes land on real sample directions of that emitter.
///
/// The returned indices are always **distinct**: a very narrow emitter collapses
/// its ideal probe directions together, so the naive nearest-index snap can pick
/// the same full-lattice index for several probes. A duplicate would be
/// double-counted into `clear` while a never-probed index is dropped — and at the
/// minimum sample count (`full_samples == SOFT_PROBE_SAMPLES`) escalation has
/// nothing left to add, so a real sub-0.05° penumbra would early-out to a hard
/// 0/1. The equal-count case returns the identity index set; the general case
/// skips already-chosen indices when snapping.
fn probe_indices(light: &MapLight, full_samples: u32) -> [u32; SOFT_PROBE_SAMPLES as usize] {
    // Equal-count fast path: the probe set IS the full set, so no snapping is
    // needed — and snapping would be unsafe here. For a near-zero-span emitter
    // the `SOFT_PROBE_SAMPLES` ideal directions collapse together, so the nearest
    // full-lattice index can be the *same* index for several probes (e.g. all four
    // snap to `0` below ~0.03°). With `full_samples == SOFT_PROBE_SAMPLES` there is
    // then nothing left for escalation to add, and a real penumbra early-outs to a
    // hard 0/1. Returning the identity `[0, 1, …, N-1]` keeps all probes distinct.
    if full_samples == SOFT_PROBE_SAMPLES {
        let mut out = [0u32; SOFT_PROBE_SAMPLES as usize];
        for (k, slot) in out.iter_mut().enumerate() {
            *slot = k as u32;
        }
        return out;
    }

    let mut out = [0u32; SOFT_PROBE_SAMPLES as usize];
    for k in 0..SOFT_PROBE_SAMPLES as usize {
        // Ideal direction k of the small, well-separated probe-count lattice.
        let ideal = probe_sample_direction(light, k as u32, SOFT_PROBE_SAMPLES);
        // Snap to the nearest full-lattice index whose direction is closest, but
        // SKIP any index already chosen by an earlier probe so the four results
        // are always distinct. Without this, a narrow emitter (whose ideal probe
        // directions nearly coincide) can snap two probes to the same index — the
        // duplicate would be double-counted into `clear` while a never-probed index
        // is silently dropped, miscounting a penumbra. Ties resolve to the lowest
        // index (the `>` keeps the first-seen best), so the choice stays
        // deterministic.
        let mut best_index = 0u32;
        let mut best_dot = f32::NEG_INFINITY;
        for i in 0..full_samples {
            if out[..k].contains(&i) {
                continue;
            }
            let dir = probe_sample_direction(light, i, full_samples);
            let dot = dir.dot(ideal);
            if dot > best_dot {
                best_dot = dot;
                best_index = i;
            }
        }
        out[k] = best_index;
    }
    out
}

/// Un-rotated (seed == 0) sample direction for index `i` of `count`, using the
/// emitter's per-light-type lattice mapping. Used only to pick the probe subset
/// (`probe_indices`); the live sampling re-derives targets through
/// `area_sample_target` with the real seed.
fn probe_sample_direction(light: &MapLight, i: u32, count: u32) -> Vec3 {
    match light.light_type {
        LightType::Point | LightType::Spot => fibonacci_sphere_sample(i, count, 0),
        LightType::Directional => {
            let aim = Vec3::from(light.cone_direction.unwrap_or([0.0, -1.0, 0.0]));
            let to_light = (-aim).normalize_or_zero();
            let half_angle = light.angular_diameter.to_radians() * 0.5;
            cone_jittered_direction(to_light, half_angle, i, count, 0)
        }
    }
}

/// Soft area-light visibility for a `(surface_point, surface_normal, light)` pair:
/// the `[0, 1]` fraction of stratified area-samples whose shadow ray is unoccluded.
/// Contact hardening is emergent — near a contact every sample occludes together
/// (sharp); with receiver distance the sample cone subtends a wider region (soft) —
/// so no distance-to-occluder input is needed.
///
/// `trace(from, to)` returns true when the segment is clear; the max-distance clamp
/// lives inside the closure. This keeps the helper trace-context-agnostic so all three
/// callers (static lightmap, animated weight-map, SH bounce) wrap their own
/// `segment_clear` — each with a different signature — as the closure.
///
/// Determinism: the sample pattern is a fixed Fibonacci lattice (mirroring
/// `sh_bake.rs`'s convention) rotated by `seed`. No RNG, no hash-order dependence —
/// the caller supplies `seed` deterministically (`chart_raster::chart_texel_seed`
/// for texels, or probe/ray/light indices) so the same inputs yield
/// byte-identical output.
///
/// `full_samples` is the area-sample-count bake knob (Task 6): the escalated
/// (penumbra) sample target. The fixed `SOFT_PROBE_SAMPLES` probe set is a spread
/// **subset** of the full lattice (see [`probe_indices`]), so `full_samples` is
/// clamped to at least the probe count. The escalation DECISION threshold (the
/// probe-disagreement count that triggers the full trace) is a fixed constant;
/// only the escalated sample count `full_samples` is caller-supplied — so the bake
/// stays deterministic regardless of the knob's value. Callers without a knob pass
/// [`DEFAULT_AREA_SAMPLE_COUNT`].
pub(crate) fn soft_visibility(
    surface_point: Vec3,
    surface_normal: Vec3,
    light: &MapLight,
    seed: u64,
    full_samples: u32,
    trace: impl Fn(Vec3, Vec3) -> bool,
) -> f32 {
    soft_visibility_with(
        surface_point,
        surface_normal,
        light,
        seed,
        || SoftProbes::new(light, full_samples),
        trace,
    )
}

/// A light's probe subset at one escalated sample count. It depends only on
/// the light and the count, so per-texel callers compute it once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SoftProbes {
    full_samples: u32,
    probes: [u32; SOFT_PROBE_SAMPLES as usize],
}

impl SoftProbes {
    /// `full_samples` is clamped to at least the probe count, as in
    /// [`soft_visibility`].
    pub(crate) fn new(light: &MapLight, full_samples: u32) -> Self {
        let full_samples = full_samples.max(SOFT_PROBE_SAMPLES);
        Self {
            full_samples,
            probes: probe_indices(light, full_samples),
        }
    }
}

/// [`soft_visibility`] with a caller-supplied probe set; `probes` runs only
/// for a soft (non-zero-radius) emitter.
pub(crate) fn soft_visibility_with(
    surface_point: Vec3,
    surface_normal: Vec3,
    light: &MapLight,
    seed: u64,
    probes: impl FnOnce() -> SoftProbes,
    trace: impl Fn(Vec3, Vec3) -> bool,
) -> f32 {
    let origin = surface_point + surface_normal * RAY_EPSILON;

    // An author's explicit `0` is a hard edge: collapse to the single hard ray so
    // the result is 1.0 clear / 0.0 occluded. (`shadow_visible` was the prior
    // hard-gate function, removed when soft visibility subsumed it; its behavior is
    // preserved here in the `radius <= 0` hard-ray branch.)
    let radius = match light.light_type {
        LightType::Point | LightType::Spot => light.light_size,
        LightType::Directional => light.angular_diameter,
    };
    if radius <= 0.0 {
        let target = hard_ray_target(surface_point, light);
        return if trace(origin, target) { 1.0 } else { 0.0 };
    }

    // Probe set is a spread subset of the full set (`probe_indices`): the knob can
    // only raise the escalated count above the fixed probe floor, so escalation
    // only adds the in-between samples and the penumbra fraction stays
    // `clear / full_samples`. The subset spreads across the whole emitter (both
    // poles and all azimuths), so a penumbra anywhere splits the probes.
    let SoftProbes {
        full_samples,
        probes,
    } = probes();
    let mut clear = 0u32;
    for &i in &probes {
        if trace(
            origin,
            area_sample_target(surface_point, light, seed, i, full_samples),
        ) {
            clear += 1;
        }
    }

    // Fully-lit or fully-shadowed probes agree — no penumbra, stay cheap.
    if clear == 0 || clear == SOFT_PROBE_SAMPLES {
        return clear as f32 / SOFT_PROBE_SAMPLES as f32;
    }

    // Escalate: trace every full-lattice index that was NOT already a probe, so
    // probe rays are counted exactly once (no double-count) and the fraction is
    // over the full set.
    for i in 0..full_samples {
        if probes.contains(&i) {
            continue;
        }
        if trace(
            origin,
            area_sample_target(surface_point, light, seed, i, full_samples),
        ) {
            clear += 1;
        }
    }
    clear as f32 / full_samples as f32
}

/// Single hard-ray target for the `radius <= 0` hard edge (see the note in
/// `soft_visibility` on the removed `shadow_visible`): the light origin for
/// Point/Spot, or a far point along `-cone_direction` for Directional.
fn hard_ray_target(surface_point: Vec3, light: &MapLight) -> Vec3 {
    match light.light_type {
        LightType::Point | LightType::Spot => Vec3::new(
            light.origin.x as f32,
            light.origin.y as f32,
            light.origin.z as f32,
        ),
        LightType::Directional => {
            let aim = Vec3::from(light.cone_direction.unwrap_or([0.0, -1.0, 0.0]));
            let to_light = (-aim).normalize_or_zero();
            surface_point + to_light * DIRECTIONAL_LIGHT_RAY_LENGTH_METERS
        }
    }
}

/// Stratified area-sample target for sample index `i` of `full_samples` (the
/// area-sample-count knob; the lattice's stratification denominator).
/// Point/Spot → a point on the emitter sphere of radius `light_size` at the light
/// origin. Directional → a far point along a direction jittered within the cone of
/// half-angle `angular_diameter/2` about `-cone_direction`, at the shared far
/// distance (`DIRECTIONAL_LIGHT_RAY_LENGTH_METERS`, same for every directional
/// sample). The lattice is rotated by `seed` so adjacent texels decorrelate.
fn area_sample_target(
    surface_point: Vec3,
    light: &MapLight,
    seed: u64,
    i: u32,
    full_samples: u32,
) -> Vec3 {
    match light.light_type {
        LightType::Point | LightType::Spot => {
            let center = Vec3::new(
                light.origin.x as f32,
                light.origin.y as f32,
                light.origin.z as f32,
            );
            center + fibonacci_sphere_sample(i, full_samples, seed) * light.light_size
        }
        LightType::Directional => {
            let aim = Vec3::from(light.cone_direction.unwrap_or([0.0, -1.0, 0.0]));
            let to_light = (-aim).normalize_or_zero();
            let half_angle = light.angular_diameter.to_radians() * 0.5;
            let dir = cone_jittered_direction(to_light, half_angle, i, full_samples, seed);
            surface_point + dir * DIRECTIONAL_LIGHT_RAY_LENGTH_METERS
        }
    }
}

/// Sample `i` of `count` on the unit sphere via the Fibonacci lattice, rotated by
/// `seed`. Mirrors `sh_bake.rs::sphere_directions` so both bakers share the same
/// low-discrepancy, RNG-free convention.
fn fibonacci_sphere_sample(i: u32, count: u32, seed: u64) -> Vec3 {
    let phi = std::f32::consts::PI * (3.0 - (5.0_f32).sqrt()); // golden angle
    let seed_offset = ((seed ^ SAMPLING_LATTICE_OFFSET) & 0xFFFF_FFFF) as f32 / u32::MAX as f32;
    let t = (i as f32 + 0.5) / count as f32;
    let y = 1.0 - 2.0 * t;
    let radius = (1.0 - y * y).max(0.0).sqrt();
    let theta = phi * i as f32 + seed_offset * std::f32::consts::TAU;
    Vec3::new(theta.cos() * radius, y, theta.sin() * radius).normalize_or_zero()
}

/// Direction jittered within a cone of half-angle `half_angle` about `axis`, using
/// the Fibonacci lattice mapped onto the spherical cap (equal-area in `cos(theta)`),
/// rotated by `seed`. Same RNG-free convention as `fibonacci_sphere_sample`.
fn cone_jittered_direction(axis: Vec3, half_angle: f32, i: u32, count: u32, seed: u64) -> Vec3 {
    let phi = std::f32::consts::PI * (3.0 - (5.0_f32).sqrt()); // golden angle
    let seed_offset = ((seed ^ SAMPLING_LATTICE_OFFSET) & 0xFFFF_FFFF) as f32 / u32::MAX as f32;
    let t = (i as f32 + 0.5) / count as f32;
    // Equal-area cap mapping: cos(theta) ramps linearly from the rim to the axis.
    let cos_theta = 1.0 - t * (1.0 - half_angle.cos());
    let sin_theta = (1.0 - cos_theta * cos_theta).max(0.0).sqrt();
    let azimuth = phi * i as f32 + seed_offset * std::f32::consts::TAU;
    // Local sample around +Z, then rotate the +Z basis onto `axis`.
    let local = Vec3::new(
        azimuth.cos() * sin_theta,
        azimuth.sin() * sin_theta,
        cos_theta,
    );
    let (tangent, bitangent) = orthonormal_basis(axis);
    (tangent * local.x + bitangent * local.y + axis * local.z).normalize_or_zero()
}

/// Right-handed orthonormal basis whose third axis is `n`. Deterministic branch on
/// `n.z` avoids a degenerate cross product when `n` is near the world Z axis.
fn orthonormal_basis(n: Vec3) -> (Vec3, Vec3) {
    let helper = if n.z.abs() < 0.999 { Vec3::Z } else { Vec3::X };
    let tangent = helper.cross(n).normalize_or_zero();
    let bitangent = n.cross(tangent);
    (tangent, bitangent)
}

pub(crate) fn segment_clear(
    bvh: &Bvh<f32, 3>,
    primitives: &[BvhPrimitive],
    geometry: &GeometryResult,
    from: Vec3,
    to: Vec3,
) -> bool {
    let delta = to - from;
    let length = delta.length();
    if length < RAY_EPSILON {
        return true;
    }
    let dir = delta / length;
    let origin = from + dir * RAY_EPSILON;
    let ray = Ray::new(
        Point3::new(origin.x, origin.y, origin.z),
        Vector3::new(dir.x, dir.y, dir.z),
    );
    let max_distance = length - RAY_EPSILON;
    let geom = &geometry.geometry;
    let query = BoundedRay::new(&ray, max_distance);
    for prim in bvh.traverse_iterator(&query, primitives) {
        let start = prim.index_offset as usize;
        let end = start + prim.index_count as usize;
        let mut tri = start;
        while tri + 3 <= end {
            let i0 = geom.indices[tri] as usize;
            let i1 = geom.indices[tri + 1] as usize;
            let i2 = geom.indices[tri + 2] as usize;
            tri += 3;
            let p0 = Vec3::from(geom.vertices[i0].position);
            let p1 = Vec3::from(geom.vertices[i1].position);
            let p2 = Vec3::from(geom.vertices[i2].position);
            if let Some(dist) = ray_triangle_hit(origin, dir, p0, p1, p2) {
                if dist > 0.0 && dist < max_distance {
                    return false;
                }
            }
        }
    }
    true
}

/// Today's unbounded occlusion scan: the reference for [`segment_clear`]
/// (bake-parallelism-large-maps A4).
#[cfg(test)]
pub(crate) fn segment_clear_full_scan(
    bvh: &Bvh<f32, 3>,
    primitives: &[BvhPrimitive],
    geometry: &GeometryResult,
    from: Vec3,
    to: Vec3,
) -> bool {
    let delta = to - from;
    let length = delta.length();
    if length < RAY_EPSILON {
        return true;
    }
    let dir = delta / length;
    let origin = from + dir * RAY_EPSILON;
    let ray = Ray::new(
        Point3::new(origin.x, origin.y, origin.z),
        Vector3::new(dir.x, dir.y, dir.z),
    );
    let max_distance = length - RAY_EPSILON;
    let geom = &geometry.geometry;
    for prim in bvh.traverse_iterator(&ray, primitives) {
        let start = prim.index_offset as usize;
        let end = start + prim.index_count as usize;
        let mut tri = start;
        while tri + 3 <= end {
            let i0 = geom.indices[tri] as usize;
            let i1 = geom.indices[tri + 1] as usize;
            let i2 = geom.indices[tri + 2] as usize;
            tri += 3;
            let p0 = Vec3::from(geom.vertices[i0].position);
            let p1 = Vec3::from(geom.vertices[i1].position);
            let p2 = Vec3::from(geom.vertices[i2].position);
            if let Some(dist) = ray_triangle_hit(origin, dir, p0, p1, p2) {
                if dist > 0.0 && dist < max_distance {
                    return false;
                }
            }
        }
    }
    true
}

/// Double-sided Möller-Trumbore intersection.
fn ray_triangle_hit(origin: Vec3, dir: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Option<f32> {
    let edge1 = b - a;
    let edge2 = c - a;
    let h = dir.cross(edge2);
    let det = edge1.dot(h);
    if det.abs() < 1.0e-8 {
        return None;
    }
    let inv_det = 1.0 / det;
    let s = origin - a;
    let u = inv_det * s.dot(h);
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = s.cross(edge1);
    let v = inv_det * dir.dot(q);
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let t = inv_det * edge2.dot(q);
    if t <= 0.0 { None } else { Some(t) }
}

fn dilate_edges(
    irradiance: &mut [f32],
    direction: &mut [Vec3],
    coverage: &mut [bool],
    atlas_w: u32,
    atlas_h: u32,
) {
    let w = atlas_w as i32;
    let h = atlas_h as i32;

    for _ in 0..CHART_PADDING_TEXELS {
        let prev_cov = coverage.to_vec();
        let prev_irr = irradiance.to_vec();
        let prev_dir = direction.to_vec();
        for y in 0..h {
            for x in 0..w {
                let idx = (y as u32 * atlas_w + x as u32) as usize;
                if prev_cov[idx] {
                    continue;
                }
                let mut sum_r = 0.0;
                let mut sum_g = 0.0;
                let mut sum_b = 0.0;
                let mut sum_dir = Vec3::ZERO;
                let mut count = 0u32;
                for dy in -1..=1 {
                    for dx in -1..=1 {
                        if dx == 0 && dy == 0 {
                            continue;
                        }
                        let nx = x + dx;
                        let ny = y + dy;
                        if nx < 0 || ny < 0 || nx >= w || ny >= h {
                            continue;
                        }
                        let nidx = (ny as u32 * atlas_w + nx as u32) as usize;
                        if prev_cov[nidx] {
                            sum_r += prev_irr[nidx * 4];
                            sum_g += prev_irr[nidx * 4 + 1];
                            sum_b += prev_irr[nidx * 4 + 2];
                            sum_dir += prev_dir[nidx];
                            count += 1;
                        }
                    }
                }
                if count > 0 {
                    let inv = 1.0 / count as f32;
                    irradiance[idx * 4] = sum_r * inv;
                    irradiance[idx * 4 + 1] = sum_g * inv;
                    irradiance[idx * 4 + 2] = sum_b * inv;
                    irradiance[idx * 4 + 3] = 1.0;
                    direction[idx] = if sum_dir.length_squared() > 1.0e-8 {
                        sum_dir.normalize()
                    } else {
                        Vec3::Y
                    };
                    coverage[idx] = true;
                }
            }
        }
    }
}

#[cfg(test)]
mod reseed_baseline_tests;
#[cfg(test)]
mod tests;
