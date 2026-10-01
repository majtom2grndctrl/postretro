// Per-light lightmap contribution layers: cache, then composite back into a
// byte-identical pre-BC6H atlas.
// See: context/lib/build_pipeline.md (Lightmap id 22)

use glam::Vec3;
use rayon::prelude::*;

use crate::affinity_grid::{AABB_PADDING_METERS, light_aabb};
use crate::bake_control::BakeControl;
use crate::bvh_build::BvhPrimitive;
use crate::chart_raster::{
    ChartPlacement, chart_interior_dims, chart_texel_seed, chart_texel_world_position,
};
use crate::geometry::GeometryResult;
#[cfg(test)]
use crate::lightmap_bake::light_texel_is_covered;
use crate::lightmap_bake::{
    BlockLayout, Chart, CompositedAtlas, light_contribution_and_direction,
    light_texel_contribution_and_visibility, segment_clear,
};
#[cfg(test)]
use crate::map_data::LightType;
use crate::map_data::MapLight;
use glam::DVec3;

mod cache_keys;

pub(crate) use cache_keys::atlas_layout_fingerprint;
pub use cache_keys::{
    layer_input_hash, section_input_hash, validate_cached_lightmap_section,
    validate_layer_partition,
};

/// Bump when the per-light layer payload format or the single-light bake math
/// changes. Folded into every `"lightmap_layer"` cache key, so a bump
/// invalidates all cached layers and forces a re-bake. Each cached stage owns
/// its own version constant and bumps independently — the layer codec evolves
/// separately from the per-group SH and animated-weight-map stages.
///
/// v7: layers are internal bake layers holding packed cell blocks.
///
/// v8: soft-visibility seeds key on the chart's frame and texel
/// (`chart_raster::chart_texel_seed`), not bake-layer coordinates.
pub const LAYER_FORMAT_VERSION: u32 = 8;

/// Bump when the composite/dilate/`encode_section` pipeline or
/// `LightmapSection::to_bytes` serialization changes. Folded into the
/// `"lightmap_section"` cache key (the second-level memo of the composited
/// section), so a bump invalidates every cached section and forces a recompose.
///
/// Scoped differently from [`LAYER_FORMAT_VERSION`]: that constant covers the
/// per-light layer payload and single-light bake math; this one covers how
/// those layers are composited and encoded into the shipped `Lightmap` (id 22)
/// bytes. The scopes are disjoint, but a `LAYER_FORMAT_VERSION` bump also
/// invalidates every section entry — `section_input_hash` folds
/// `LAYER_FORMAT_VERSION` directly, so the section key changes with it.
/// Bump `LIGHTMAP_SECTION_VERSION` only when the composite/dilate/encode
/// pipeline or `LightmapSection::to_bytes` format changes independently.
///
/// v4: id 22 v3, per-cell blocks sliced from each encoded bake layer.
pub const LIGHTMAP_SECTION_VERSION: u32 = 4;

/// One analytically reached atlas texel from a single light.
///
/// The target atlas layer is stored once in [`LightmapLayer`]. Presence records
/// the analytic-coverage predicate, including fully occluded and NaN samples;
/// absence means the light had no direct term. The unshadowed contribution and
/// direction are reconstructed from the prepared atlas when the partition is
/// folded, leaving only the raw visibility in the cache payload.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct LayerTexel {
    /// Within-layer linear atlas texel index (`y * atlas_width + x`). The atlas
    /// layer is carried by `LightmapLayer.target_layer`, so this index is NOT a
    /// global index across layers.
    pub idx: u32,
    /// Raw soft visibility for this light at this analytically covered texel.
    pub raw_visibility: f32,
}

fn sparse_layer_texel(idx: u32, raw_visibility: Option<f32>) -> Option<LayerTexel> {
    raw_visibility.map(|raw_visibility| LayerTexel {
        idx,
        raw_visibility,
    })
}

fn bake_sparse_layer_texel(
    idx: u32,
    light: &MapLight,
    world_p: Vec3,
    surface_normal: Vec3,
    seed: u64,
    area_sample_count: u32,
    trace: impl Fn(Vec3, Vec3) -> bool,
) -> Option<LayerTexel> {
    let (_, _, raw_visibility) = light_texel_contribution_and_visibility(
        light,
        world_p,
        surface_normal,
        seed,
        area_sample_count,
        trace,
    );
    sparse_layer_texel(idx, raw_visibility)
}

pub(crate) fn reconstruct_light_texel(
    light: &MapLight,
    world_p: Vec3,
    surface_normal: Vec3,
    visibility: f32,
) -> (Vec3, Vec3) {
    let (contribution, to_light) = light_contribution_and_direction(light, world_p, surface_normal);
    if visibility <= 0.0 {
        return (Vec3::ZERO, Vec3::ZERO);
    }
    let irradiance = contribution * visibility;
    let luminance = (contribution.x + contribution.y + contribution.z) * visibility;
    (irradiance, to_light * luminance)
}

/// One atlas location covered by a light before visibility is sampled.
#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CoveredChartTexel {
    pub idx: u32,
    pub layer: u32,
}

// Pins the fixed codec stride: `to_bytes`/`from_bytes` cast the blob directly
// via bytemuck; any field change that shifts the stride breaks the on-disk format.
const _: () = assert!(std::mem::size_of::<LayerTexel>() == 8);

/// One light's contribution across the shared atlas.
///
/// Sparse over analytic light reach for exactly one atlas array layer.
#[derive(Debug, Clone, PartialEq)]
pub struct LightmapLayer {
    pub atlas_width: u32,
    pub atlas_height: u32,
    /// Shared atlas array layer count. Every partition in this build bakes
    /// against the same atlas layout.
    pub layer_count: u32,
    /// Atlas array layer shared by every texel record in this partition.
    pub target_layer: u32,
    /// Analytically reached texels in strictly increasing within-layer order.
    pub texels: Vec<LayerTexel>,
}

/// Fixed-layout header preceding the texel block in a layer blob: atlas
/// dimensions, the array layer count, target layer, and texel count, five
/// native-endian `u32`s (20 bytes).
const LAYER_HEADER_BYTES: usize = 5 * std::mem::size_of::<u32>();

impl LightmapLayer {
    /// Serialize to a fixed-layout native-endian byte blob for the cache.
    ///
    /// A 20-byte header (`atlas_width`, `atlas_height`, `layer_count`,
    /// `target_layer`, texel `count`, native `u32`s) followed by the
    /// `[LayerTexel]` block copied
    /// verbatim via `bytemuck::cast_slice`. The blob is compiler-internal and
    /// dev-local (never shipped, never read across architectures, matching the
    /// layer cache), so native-endian is fine and lets the body be a bulk memory
    /// copy instead of a per-field encode.
    pub fn to_bytes(&self) -> Vec<u8> {
        let texel_bytes = bytemuck::cast_slice::<LayerTexel, u8>(&self.texels);
        let mut out = Vec::with_capacity(LAYER_HEADER_BYTES + texel_bytes.len());
        out.extend_from_slice(&self.atlas_width.to_ne_bytes());
        out.extend_from_slice(&self.atlas_height.to_ne_bytes());
        out.extend_from_slice(&self.layer_count.to_ne_bytes());
        out.extend_from_slice(&self.target_layer.to_ne_bytes());
        out.extend_from_slice(&(self.texels.len() as u32).to_ne_bytes());
        out.extend_from_slice(texel_bytes);
        out
    }

    /// Deserialize a layer blob. A decode failure (truncated or format-skewed
    /// payload that still passed the `StageCache` length/hash check) returns
    /// `None` so the caller treats it as a miss and re-bakes. The cache's own
    /// length/hash validation catches bit-rot; this catches format skew.
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < LAYER_HEADER_BYTES {
            log::warn!("[Compiler] corrupt lightmap layer (truncated header), re-baking");
            return None;
        }
        // Header is 5 native-endian u32s; the slice lengths are fixed above, so
        // the `try_into`s cannot fail.
        let atlas_width = u32::from_ne_bytes(bytes[0..4].try_into().unwrap());
        let atlas_height = u32::from_ne_bytes(bytes[4..8].try_into().unwrap());
        let layer_count = u32::from_ne_bytes(bytes[8..12].try_into().unwrap());
        let target_layer = u32::from_ne_bytes(bytes[12..16].try_into().unwrap());
        let count = u32::from_ne_bytes(bytes[16..20].try_into().unwrap()) as usize;

        let payload = &bytes[LAYER_HEADER_BYTES..];
        // A count that overflows means the blob is malformed; the codec's contract
        // is "malformed blob → cache miss → re-bake, never panic."
        let Some(expected) = count.checked_mul(std::mem::size_of::<LayerTexel>()) else {
            log::warn!("[Compiler] corrupt lightmap layer (count {count} overflows), re-baking");
            return None;
        };
        if payload.len() != expected {
            log::warn!(
                "[Compiler] corrupt lightmap layer (body {} bytes, expected {}), re-baking",
                payload.len(),
                expected
            );
            return None;
        }

        // `pod_collect_to_vec` bulk-copies the payload into an aligned
        // `Vec<LayerTexel>`, tolerating the source `&[u8]`'s arbitrary
        // alignment — a borrowed `cast_slice::<u8, LayerTexel>` would panic on
        // a misaligned slice. Single memcpy — no per-field deserialization.
        let texels = bytemuck::pod_collect_to_vec::<u8, LayerTexel>(payload);

        Some(Self {
            atlas_width,
            atlas_height,
            layer_count,
            target_layer,
            texels,
        })
    }
}

/// The shared atlas a single-light layer bakes against. Produced once by
/// `lightmap_bake::prepare_atlas` with the full static-light set, then threaded
/// into every single-light bake so all layers share one chart layout.
///
/// `placements` and the dimensions describe the internal bake layers; `layout`
/// names the cell blocks the encoders slice out of them.
pub struct SharedAtlas<'a> {
    pub charts: &'a [Chart],
    pub placements: &'a [ChartPlacement],
    pub atlas_width: u32,
    pub atlas_height: u32,
    pub layout: &'a BlockLayout,
}

/// One global atlas layer's in-progress, ordered light fold.
///
/// The production warm path retains only this one atlas plane plus one
/// light/layer cache partition at a time. `weighted_dir` deliberately stays
/// separate from the finished direction buffer so normalization still occurs
/// once, after the complete global-light-order fold.
pub struct IncrementalLayerAccumulator {
    atlas: CompositedAtlas,
    weighted_dir: Vec<Vec3>,
    fallback_normal: Vec<Vec3>,
    chart_index: Vec<u32>,
    target_layer: u32,
}

impl IncrementalLayerAccumulator {
    pub fn zeroed(atlas_width: u32, atlas_height: u32) -> Self {
        let texel_count = atlas_width as usize * atlas_height as usize;
        Self {
            atlas: CompositedAtlas::zeroed(atlas_width, atlas_height, 1),
            weighted_dir: vec![Vec3::ZERO; texel_count],
            fallback_normal: vec![Vec3::Y; texel_count],
            chart_index: vec![u32::MAX; texel_count],
            target_layer: 0,
        }
    }

    /// Initialize the output coverage, fallback normals, and reconstruction
    /// lookup from the prepared atlas's light-independent chart walk.
    pub fn for_atlas_layer(atlas: &SharedAtlas<'_>, target_layer: u32) -> Self {
        let mut accumulator = Self::zeroed(atlas.atlas_width, atlas.atlas_height);
        accumulator.target_layer = target_layer;
        for face_idx in 0..atlas.placements.len() {
            if atlas.placements[face_idx].layer != target_layer {
                continue;
            }
            for_each_light_layer_chart_texel(atlas, face_idx, |sample| {
                let idx = sample.idx as usize;
                accumulator.atlas.coverage[idx] = true;
                accumulator.fallback_normal[idx] = sample.surface_normal;
                accumulator.chart_index[idx] = face_idx as u32;
            });
        }
        accumulator
    }

    /// Fold one already-validated `(light, target_layer)` partition. Callers
    /// supply partitions in the original global light order; float addition is
    /// intentionally neither reordered nor reduced.
    pub fn fold_partition(
        &mut self,
        light: &MapLight,
        partition: &LightmapLayer,
        atlas: &SharedAtlas<'_>,
    ) {
        debug_assert_eq!(partition.atlas_width, self.atlas.atlas_width);
        debug_assert_eq!(partition.atlas_height, self.atlas.atlas_height);
        debug_assert_eq!(partition.target_layer, self.target_layer);
        for texel in &partition.texels {
            let idx = texel.idx as usize;
            let face_idx = self.chart_index[idx];
            debug_assert_ne!(face_idx, u32::MAX);
            let face_idx = face_idx as usize;
            let chart = &atlas.charts[face_idx];
            let placement = &atlas.placements[face_idx];
            let padding = crate::chart_raster::CHART_PADDING_TEXELS;
            let atlas_x = texel.idx % atlas.atlas_width;
            let atlas_y = texel.idx / atlas.atlas_width;
            let tx = (atlas_x - placement.x - padding) as i32;
            let ty = (atlas_y - placement.y - padding) as i32;
            let world_p = chart_texel_world_position(chart, tx, ty);
            let (irradiance, weighted_dir) =
                reconstruct_light_texel(light, world_p, chart.normal, texel.raw_visibility);
            self.atlas.irradiance[idx * 4] += irradiance.x;
            self.atlas.irradiance[idx * 4 + 1] += irradiance.y;
            self.atlas.irradiance[idx * 4 + 2] += irradiance.z;
            self.weighted_dir[idx] += weighted_dir;
        }
    }

    /// Finish the ordered fold. The returned plane is ready for the shared
    /// per-layer dilation and encode path.
    pub fn finish(mut self) -> CompositedAtlas {
        for (idx, covered) in self.atlas.coverage.iter().copied().enumerate() {
            if !covered {
                continue;
            }
            self.atlas.irradiance[idx * 4 + 3] = 1.0;
            let weighted_dir = self.weighted_dir[idx];
            self.atlas.direction[idx] = if weighted_dir.length_squared() > 1.0e-8 {
                weighted_dir.normalize()
            } else {
                self.fallback_normal[idx]
            };
        }
        self.atlas
    }
}

/// Influence AABB used to bound the geometry slice folded into a light's cache
/// key. Point/Spot → `falloff_range
/// + AABB_PADDING_METERS` cube; Directional → the whole-world AABB. Delegates to
/// the authoritative `affinity_grid::light_aabb` (the f32-falloff copy).
pub fn layer_influence_aabb(light: &MapLight, world_aabb: (DVec3, DVec3)) -> (DVec3, DVec3) {
    light_aabb(light, world_aabb)
}

/// Bake one light's contribution layer across the shared atlas.
///
/// Mirrors `bake_face_chart`'s per-texel structure exactly but for a single
/// light: same chart interior walk, same `chart_texel_seed`, same
/// `light_texel_contribution_and_visibility` helper (which shares the
/// monolithic Lambert + soft-visibility math). Directional lights are
/// evaluated across every chart texel, but sparse records are emitted only for
/// analytically reached, contributing samples.
///
/// The sparse set is the subset of chart interiors reached by this light before
/// visibility. The compositor derives atlas coverage and fallback normals from
/// the shared chart walk rather than from each light's records.
pub fn bake_light_layer(
    light: &MapLight,
    atlas: &SharedAtlas<'_>,
    bvh: &bvh::bvh::Bvh<f32, 3>,
    primitives: &[BvhPrimitive],
    geometry: &GeometryResult,
    area_sample_count: u32,
    control: &BakeControl,
) -> Vec<LightmapLayer> {
    (0..atlas_layer_count(atlas))
        .map(|target_layer| {
            bake_light_layer_controlled(
                light,
                atlas,
                bvh,
                primitives,
                geometry,
                target_layer,
                area_sample_count,
                control,
            )
        })
        .collect()
}

/// Bake one light's cacheable contribution for exactly one atlas array layer.
///
/// `target_layer` is deliberately part of both the returned partition's
/// containment and its cache key. A partition never carries texels from a
/// neighbouring atlas layer.
#[allow(clippy::too_many_arguments)]
pub fn bake_light_layer_controlled(
    light: &MapLight,
    atlas: &SharedAtlas<'_>,
    bvh: &bvh::bvh::Bvh<f32, 3>,
    primitives: &[BvhPrimitive],
    geometry: &GeometryResult,
    target_layer: u32,
    area_sample_count: u32,
    control: &BakeControl,
) -> LightmapLayer {
    let face_indices: Vec<usize> = atlas
        .placements
        .iter()
        .enumerate()
        .filter_map(|(face_idx, placement)| (placement.layer == target_layer).then_some(face_idx))
        .collect();
    bake_light_layer_for_faces(
        light,
        atlas,
        bvh,
        primitives,
        geometry,
        &face_indices,
        target_layer,
        area_sample_count,
        control,
    )
}

#[allow(clippy::too_many_arguments)]
fn bake_light_layer_for_faces(
    light: &MapLight,
    atlas: &SharedAtlas<'_>,
    bvh: &bvh::bvh::Bvh<f32, 3>,
    primitives: &[BvhPrimitive],
    geometry: &GeometryResult,
    face_indices: &[usize],
    target_layer: u32,
    area_sample_count: u32,
    control: &BakeControl,
) -> LightmapLayer {
    let layer_count = atlas_layer_count(atlas);
    // `par_iter().map().collect()` is indexed: the outer collection retains
    // planner order even while ray work runs concurrently. Flattening those
    // per-chart buffers therefore preserves the cache payload's established
    // chart/row/column order exactly.
    let per_chart_texels: Vec<Vec<LayerTexel>> = face_indices
        .par_iter()
        .map(|&face_idx| {
            bake_light_layer_chart_controlled(
                light,
                atlas,
                face_idx,
                bvh,
                primitives,
                geometry,
                area_sample_count,
                control,
            )
        })
        .collect();

    let mut texels: Vec<LayerTexel> = per_chart_texels.into_iter().flatten().collect();
    texels.sort_unstable_by_key(|texel| texel.idx);
    LightmapLayer {
        atlas_width: atlas.atlas_width,
        atlas_height: atlas.atlas_height,
        layer_count,
        target_layer,
        texels,
    }
}

pub fn atlas_layer_count(atlas: &SharedAtlas<'_>) -> u32 {
    atlas
        .placements
        .iter()
        .map(|placement| placement.layer + 1)
        .max()
        .unwrap_or(1)
}

/// Bake one `(light, chart)` contribution in placement order.
///
/// This is the outermost governed unit for every light-layer bake path. Keeping
/// admission here lets callers choose a single Rayon level without nesting a
/// governor entry around the established per-chart work.
#[allow(clippy::too_many_arguments)]
pub(crate) fn bake_light_layer_chart_controlled(
    light: &MapLight,
    atlas: &SharedAtlas<'_>,
    face_idx: usize,
    bvh: &bvh::bvh::Bvh<f32, 3>,
    primitives: &[BvhPrimitive],
    geometry: &GeometryResult,
    area_sample_count: u32,
    control: &BakeControl,
) -> Vec<LayerTexel> {
    let chart = &atlas.charts[face_idx];
    let capacity = if chart.uv_extent[0] > 0.0 && chart.uv_extent[1] > 0.0 {
        let (width, height) = chart_interior_dims(chart);
        (width * height) as usize
    } else {
        0
    };
    let mut texels = Vec::with_capacity(capacity);
    for_each_light_layer_chart_texel_controlled(atlas, face_idx, control, |sample| {
        if let Some(texel) = bake_sparse_layer_texel(
            sample.idx,
            light,
            sample.world_p,
            sample.surface_normal,
            sample.seed,
            area_sample_count,
            |from, to| segment_clear(bvh, primitives, geometry, from, to),
        ) {
            texels.push(texel);
        }
    });
    texels
}

/// Walk one `(light, chart)` using the exact lightmap raster path, retaining
/// only texels with a direct contribution before visibility is sampled.
#[cfg(test)]
pub(crate) fn visit_light_chart_coverage_controlled(
    light: &MapLight,
    atlas: &SharedAtlas<'_>,
    face_idx: usize,
    control: &BakeControl,
    mut visit_covered: impl FnMut(CoveredChartTexel),
) {
    for_each_light_layer_chart_texel_controlled(atlas, face_idx, control, |sample| {
        if light_texel_is_covered(light, sample.world_p, sample.surface_normal) {
            visit_covered(CoveredChartTexel {
                idx: sample.idx,
                layer: atlas.placements[face_idx].layer,
            });
        }
    });
}

#[derive(Clone, Copy)]
pub(crate) struct ChartWalkSample {
    pub idx: u32,
    pub world_p: Vec3,
    pub surface_normal: Vec3,
    pub seed: u64,
}

pub(crate) fn for_each_light_layer_chart_texel_controlled(
    atlas: &SharedAtlas<'_>,
    face_idx: usize,
    control: &BakeControl,
    sample_texel: impl FnMut(ChartWalkSample),
) {
    // Parallel bake work must enter once at its outermost boundary so pause
    // and the shared concurrency cap apply to every chart.
    let _permit = control.governor().enter();
    for_each_light_layer_chart_texel(atlas, face_idx, sample_texel);
    control.advance(1);
}

/// Walk one chart without acquiring a governor permit. Callers that include
/// prune/setup work in the same parallel item acquire the permit outside this
/// helper, then use this exact raster loop.
pub(crate) fn for_each_light_layer_chart_texel(
    atlas: &SharedAtlas<'_>,
    face_idx: usize,
    mut sample_texel: impl FnMut(ChartWalkSample),
) {
    let placement = &atlas.placements[face_idx];
    let chart = &atlas.charts[face_idx];
    if chart.uv_extent[0] <= 0.0 || chart.uv_extent[1] <= 0.0 {
        return;
    }

    let padding = crate::chart_raster::CHART_PADDING_TEXELS as i32;
    let (interior_w, interior_h) = chart_interior_dims(chart);
    for ty in 0..interior_h {
        for tx in 0..interior_w {
            let atlas_x = placement.x as i32 + padding + tx;
            let atlas_y = placement.y as i32 + padding + ty;
            // Within-layer index — the atlas layer lives in the enclosing
            // `LightmapLayer.target_layer`, not folded into `idx`.
            let idx = atlas_y as u32 * atlas.atlas_width + atlas_x as u32;

            let world_p = chart_texel_world_position(chart, tx, ty);
            let surface_normal = chart.normal;
            let seed = chart_texel_seed(chart, tx, ty);

            let sample = ChartWalkSample {
                idx,
                world_p,
                surface_normal,
                seed,
            };
            sample_texel(sample);
        }
    }
}

/// Composite per-light layers into the pre-BC6H atlas, reproducing
/// `bake_face_chart`'s output bit-for-bit.
///
/// Reconstructs each sparse partition in caller-supplied global light order,
/// then performs a single normalization after the ordered fold. Atlas coverage
/// and fallback normals come from the light-independent chart walk.
///
/// The returned atlas is **not yet dilated**; the caller runs
/// [`CompositedAtlas::dilate`] to match the monolithic post-dilation seam.
pub fn composite_layers(
    light_layers: &[(&MapLight, &LightmapLayer)],
    shared: &SharedAtlas<'_>,
) -> CompositedAtlas {
    let layer_count = atlas_layer_count(shared);
    let mut atlas = CompositedAtlas::zeroed(shared.atlas_width, shared.atlas_height, layer_count);
    let plane = shared.atlas_width as usize * shared.atlas_height as usize;
    for target_layer in 0..layer_count {
        let mut accumulator = IncrementalLayerAccumulator::for_atlas_layer(shared, target_layer);
        for &(light, partition) in light_layers {
            if partition.target_layer == target_layer {
                accumulator.fold_partition(light, partition, shared);
            }
        }
        let layer = accumulator.finish();
        let offset = target_layer as usize * plane;
        atlas.irradiance[offset * 4..(offset + plane) * 4].copy_from_slice(&layer.irradiance);
        atlas.direction[offset..offset + plane].copy_from_slice(&layer.direction);
        atlas.coverage[offset..offset + plane].copy_from_slice(&layer.coverage);
    }
    atlas
}

/// The established one-plane fallback for a prepared atlas with no direct
/// layer-bearing lights.
pub fn empty_composite(atlas_width: u32, atlas_height: u32) -> CompositedAtlas {
    CompositedAtlas::zeroed(atlas_width, atlas_height, 1)
}

/// Whole-world AABB over the geometry's vertices, used as the directional-light
/// influence bound. Mirrors `affinity_grid`'s world-AABB computation in f64 so
/// the directional layer's influence matches the authoritative `light_aabb`.
pub fn geometry_world_aabb(geometry: &GeometryResult) -> (DVec3, DVec3) {
    let mut min = DVec3::splat(f64::INFINITY);
    let mut max = DVec3::splat(f64::NEG_INFINITY);
    for v in &geometry.geometry.vertices {
        let p = DVec3::new(
            v.position[0] as f64,
            v.position[1] as f64,
            v.position[2] as f64,
        );
        min = min.min(p);
        max = max.max(p);
    }
    (min, max)
}

/// Padding applied by the per-light cache key's influence bound.
pub const LAYER_AABB_PADDING_METERS: f32 = AABB_PADDING_METERS;

#[cfg(test)]
mod tests;
