// Per-light lightmap contribution layers: cache, then composite back into a
// byte-identical pre-BC6H atlas.
// See: context/lib/build_pipeline.md (Lightmap id 22)

use glam::Vec3;
use rayon::prelude::*;

use crate::affinity_grid::{AABB_PADDING_METERS, light_aabb};
use crate::bake_control::BakeControl;
use crate::bvh_build::BvhPrimitive;
use crate::chart_raster::{ChartPlacement, chart_interior_dims, chart_texel_world_position};
use crate::geometry::GeometryResult;
#[cfg(test)]
use crate::lightmap_bake::light_texel_is_covered;
use crate::lightmap_bake::{
    Chart, CompositedAtlas, effective_direction_texel_scale, light_contribution_and_direction,
    light_texel_contribution_and_visibility, segment_clear, texel_seed,
};
#[cfg(test)]
use crate::map_data::LightType;
use crate::map_data::MapLight;
use glam::DVec3;

/// Bump when the per-light layer payload format or the single-light bake math
/// changes. Folded into every `"lightmap_layer"` cache key, so a bump
/// invalidates all cached layers and forces a re-bake. Each cached stage owns
/// its own version constant and bumps independently — the layer codec evolves
/// separately from the per-group SH and animated-weight-map stages.
pub const LAYER_FORMAT_VERSION: u32 = 6;

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
pub const LIGHTMAP_SECTION_VERSION: u32 = 3;

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
pub struct SharedAtlas<'a> {
    pub charts: &'a [Chart],
    pub placements: &'a [ChartPlacement],
    pub atlas_width: u32,
    pub atlas_height: u32,
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
            let (interior_w, interior_h) = chart_interior_dims(chart);
            let world_p = chart_texel_world_position(chart, tx, ty, interior_w, interior_h);
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
/// light: same chart interior walk, same `texel_seed`, same
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

            let world_p = chart_texel_world_position(chart, tx, ty, interior_w, interior_h);
            let surface_normal = chart.normal;
            let seed = texel_seed(atlas_x as u32, atlas_y as u32);

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

/// The atlas layout descriptor folded into a layer's cache key. Captures atlas
/// dimensions, resolved chart sampling extents/dimensions, and per-chart
/// placements so an atlas repack (which shifts every placement) or a
/// per-surface density override invalidates all layers by changing this
/// fingerprint.
///
/// `ChartPlacement` does not derive `Serialize`, so this folds its `x`/`y`/`layer`
/// fields directly into the digest — the deterministically-derived proxy-bytes
/// fingerprint the animated-weight-map stage uses for its non-`Serialize` atlas
/// types. Every resolved per-chart sampling input is folded explicitly because
/// a scale-region edit can change texel world positions or resize a lone chart
/// while its 64² atlas and `(0, 0, 0)` placement remain unchanged. Raw region
/// definitions are intentionally not folded: equivalent resolved chart
/// outcomes share cache identity.
fn atlas_layout_fingerprint(atlas: &SharedAtlas<'_>) -> Vec<u8> {
    let mut hasher = blake3::Hasher::new();
    hasher.update(&atlas.atlas_width.to_le_bytes());
    hasher.update(&atlas.atlas_height.to_le_bytes());
    hasher.update(&(atlas.charts.len() as u32).to_le_bytes());
    for chart in atlas.charts {
        for component in [chart.origin.x, chart.origin.y, chart.origin.z] {
            hasher.update(&component.to_le_bytes());
        }
        for component in [chart.u_axis.x, chart.u_axis.y, chart.u_axis.z] {
            hasher.update(&component.to_le_bytes());
        }
        for component in [chart.v_axis.x, chart.v_axis.y, chart.v_axis.z] {
            hasher.update(&component.to_le_bytes());
        }
        for component in chart.uv_min {
            hasher.update(&component.to_le_bytes());
        }
        for component in chart.uv_extent {
            hasher.update(&component.to_le_bytes());
        }
        for component in [chart.normal.x, chart.normal.y, chart.normal.z] {
            hasher.update(&component.to_le_bytes());
        }
        hasher.update(&chart.width_texels.to_le_bytes());
        hasher.update(&chart.height_texels.to_le_bytes());
    }
    hasher.update(&(atlas.placements.len() as u32).to_le_bytes());
    for p in atlas.placements {
        hasher.update(&p.x.to_le_bytes());
        hasher.update(&p.y.to_le_bytes());
        // Fold the atlas layer so a repack that moves a chart to a different
        // array layer (same x/y) still invalidates the per-light cache.
        hasher.update(&p.layer.to_le_bytes());
    }
    hasher.finalize().as_bytes().to_vec()
}

/// Hash the influence-bounded geometry slice for one light: the content of every
/// face whose AABB overlaps the light's influence AABB.
///
/// This is the whole-stage `GeometryResult` content hash *restricted* to the
/// influence-overlapping faces. Faces are gathered by AABB overlap against the
/// per-face `BvhPrimitive`s (the BVH is an accelerator only; iterating the
/// already-collected primitive slice yields the identical set), mapped back to
/// face identity, then taken in canonical `sort_key` order — NOT the post-`Bvh::
/// build` permutation — so the hash is decoupled from BVH build determinism.
/// Each face is hashed by its geometry (`index_offset..index_offset+index_count`
/// indices, plus the referenced vertices), the face content that actually
/// affects the bake's chart shapes and shadow queries.
///
/// Occlusion is local to the influence sphere (any occluder on a light→texel
/// segment is nearer than the texel, hence inside `falloff_range`), so the
/// falloff-AABB slice is a sound conservative dependency set.
fn geometry_slice_hash(
    light: &MapLight,
    primitives: &[BvhPrimitive],
    geometry: &GeometryResult,
    world_aabb: (DVec3, DVec3),
) -> Vec<u8> {
    let (inf_min, inf_max) = layer_influence_aabb(light, world_aabb);

    // Gather overlapping primitives, then sort by canonical `sort_key` so the
    // hash is independent of however the BVH permuted the slice.
    let mut overlapping: Vec<&BvhPrimitive> = primitives
        .iter()
        .filter(|p| aabb_overlaps(p, inf_min, inf_max))
        .collect();
    overlapping.sort_by_key(|p| p.sort_key);

    let section = &geometry.geometry;
    let mut hasher = blake3::Hasher::new();
    for prim in overlapping {
        // Map the primitive back to its face slice. `index_offset`/`index_count`
        // on the primitive are the same range stored in `face_index_ranges`.
        let start = prim.index_offset as usize;
        let end = start + prim.index_count as usize;
        hasher.update(&prim.sort_key.to_le_bytes());
        for i in start..end {
            let vi = section.indices[i];
            hasher.update(&vi.to_le_bytes());
            let v = &section.vertices[vi as usize];
            hasher.update(&bytemuck_f32x3(&v.position));
            hasher.update(&[v.uv[0].to_le_bytes(), v.uv[1].to_le_bytes()].concat());
            hasher.update(&v.normal_oct[0].to_le_bytes());
            hasher.update(&v.normal_oct[1].to_le_bytes());
        }
    }
    hasher.finalize().as_bytes().to_vec()
}

fn aabb_overlaps(prim: &BvhPrimitive, inf_min: DVec3, inf_max: DVec3) -> bool {
    let p_min = prim.aabb_min;
    let p_max = prim.aabb_max;
    (p_min[0] as f64) <= inf_max.x
        && (p_max[0] as f64) >= inf_min.x
        && (p_min[1] as f64) <= inf_max.y
        && (p_max[1] as f64) >= inf_min.y
        && (p_min[2] as f64) <= inf_max.z
        && (p_max[2] as f64) >= inf_min.z
}

fn bytemuck_f32x3(v: &[f32; 3]) -> Vec<u8> {
    let mut out = Vec::with_capacity(12);
    for c in v {
        out.extend_from_slice(&c.to_le_bytes());
    }
    out
}

/// The full input fingerprint for one light's lightmap layer cache key.
///
/// Folds, under a fixed byte layout:
/// - the light's params (whole `MapLight`, fixed `postcard` encoding for the
///   key hash — `postcard` is used here only; the layer blob itself uses the
///   bytemuck codec),
/// - the influence-bounded geometry slice hash,
/// - `lightmap_density` + `area_sample_count`,
/// - the atlas layout descriptor (dims + per-chart placements).
/// - `target_layer`, so partitions with the same light and layout cannot alias.
///
/// Consumers pass this digest to `CacheKey::new("lightmap_layer",
/// LAYER_FORMAT_VERSION, &hash)` so the layer stage owns invalidation.
pub fn layer_input_hash(
    light: &MapLight,
    atlas: &SharedAtlas<'_>,
    primitives: &[BvhPrimitive],
    geometry: &GeometryResult,
    lightmap_density: f32,
    area_sample_count: u32,
    target_layer: u32,
) -> [u8; 32] {
    let world_aabb = geometry_world_aabb(geometry);

    let mut hasher = blake3::Hasher::new();
    hasher
        .update(&postcard::to_allocvec(light).expect("postcard serialize MapLight for layer key"));
    hasher.update(&geometry_slice_hash(
        light, primitives, geometry, world_aabb,
    ));
    hasher.update(&lightmap_density.to_le_bytes());
    hasher.update(&area_sample_count.to_le_bytes());
    hasher.update(&atlas_layout_fingerprint(atlas));
    hasher.update(&target_layer.to_le_bytes());
    *hasher.finalize().as_bytes()
}

/// Validate a decoded cache partition against the current shared atlas. Cache
/// corruption and stale/mismatched payloads are callers' soft misses, never a
/// PRL-load error.
pub fn validate_layer_partition(
    partition: &LightmapLayer,
    atlas: &SharedAtlas<'_>,
    target_layer: u32,
) -> Result<(), String> {
    if partition.atlas_width != atlas.atlas_width || partition.atlas_height != atlas.atlas_height {
        return Err(format!(
            "dimensions {}x{} != {}x{}",
            partition.atlas_width, partition.atlas_height, atlas.atlas_width, atlas.atlas_height
        ));
    }
    let layer_count = atlas_layer_count(atlas);
    if partition.layer_count != layer_count {
        return Err(format!(
            "layer_count {} != {}",
            partition.layer_count, layer_count
        ));
    }
    if target_layer >= layer_count {
        return Err(format!(
            "target layer {target_layer} out of bounds for {layer_count} layers"
        ));
    }
    if partition.target_layer != target_layer {
        return Err(format!(
            "partition target layer {} != {target_layer}",
            partition.target_layer
        ));
    }
    let Some(plane) = (atlas.atlas_width as usize).checked_mul(atlas.atlas_height as usize) else {
        return Err("atlas dimensions overflow texel plane size".to_string());
    };
    if atlas.charts.len() != atlas.placements.len() {
        return Err(format!(
            "chart count {} != placement count {}",
            atlas.charts.len(),
            atlas.placements.len()
        ));
    }

    let mut covered = vec![false; plane];
    for (chart, placement) in atlas.charts.iter().zip(atlas.placements) {
        if placement.layer != target_layer || chart.uv_extent[0] <= 0.0 || chart.uv_extent[1] <= 0.0
        {
            continue;
        }
        let (interior_width, interior_height) = chart_interior_dims(chart);
        let padding = crate::chart_raster::CHART_PADDING_TEXELS as i32;
        for y in 0..interior_height {
            for x in 0..interior_width {
                let atlas_x = placement.x as i32 + padding + x;
                let atlas_y = placement.y as i32 + padding + y;
                let expected_idx = atlas_y as u32 * atlas.atlas_width + atlas_x as u32;
                covered[expected_idx as usize] = true;
            }
        }
    }
    let mut previous = None;
    for (record_index, texel) in partition.texels.iter().enumerate() {
        let idx = texel.idx as usize;
        if idx >= plane {
            return Err(format!(
                "texel {record_index} idx {} out of bounds for {plane} texels",
                texel.idx
            ));
        }
        if !covered[idx] {
            return Err(format!(
                "texel {record_index} idx {} is outside the covered chart interiors on layer {target_layer}",
                texel.idx
            ));
        }
        let visibility = texel.raw_visibility;
        if !visibility.is_nan() && (!visibility.is_finite() || !(0.0..=1.0).contains(&visibility)) {
            return Err(format!(
                "texel {record_index} raw visibility {visibility} is outside 0..=1 or infinite"
            ));
        }
        if previous.is_some_and(|previous| texel.idx <= previous) {
            return Err(format!(
                "texel {record_index} idx {} is not strictly greater than previous idx {}",
                texel.idx,
                previous.unwrap()
            ));
        }
        previous = Some(texel.idx);
    }
    Ok(())
}

/// Validate a decoded composited-section memo against the current atlas and
/// encode configuration. A decodable but stale payload is a soft cache miss.
#[allow(clippy::too_many_arguments)]
pub fn validate_cached_lightmap_section(
    section: &postretro_level_format::lightmap::LightmapSection,
    atlas: &SharedAtlas<'_>,
    expected_layer_count: u32,
    texel_density: f32,
    uncompressed_irradiance: bool,
    direction_texel_scale: u32,
) -> Result<(), String> {
    use postretro_level_format::lightmap::{
        DIRECTION_FORMAT_OCT_RG8, IRRADIANCE_FORMAT_BC6H, IRRADIANCE_FORMAT_RGBA16F, LightmapMode,
    };

    if section.irr_width != atlas.atlas_width || section.irr_height != atlas.atlas_height {
        return Err(format!(
            "irradiance dimensions {}x{} != {}x{}",
            section.irr_width, section.irr_height, atlas.atlas_width, atlas.atlas_height
        ));
    }
    if section.layer_count != expected_layer_count {
        return Err(format!(
            "layer_count {} != {expected_layer_count}",
            section.layer_count
        ));
    }
    if section.irr_texel_density.to_bits() != texel_density.to_bits() {
        return Err(format!(
            "irradiance density {} != {texel_density}",
            section.irr_texel_density
        ));
    }
    let expected_irradiance_format = if uncompressed_irradiance {
        IRRADIANCE_FORMAT_RGBA16F
    } else {
        IRRADIANCE_FORMAT_BC6H
    };
    if section.irradiance_format != expected_irradiance_format {
        return Err(format!(
            "irradiance format {} != {expected_irradiance_format}",
            section.irradiance_format
        ));
    }

    let direction_texel_scale = effective_direction_texel_scale(
        direction_texel_scale,
        atlas.atlas_width,
        atlas.atlas_height,
    );
    let expected_dir_width = atlas.atlas_width / direction_texel_scale;
    let expected_dir_height = atlas.atlas_height / direction_texel_scale;
    if section.dir_width != expected_dir_width || section.dir_height != expected_dir_height {
        return Err(format!(
            "direction dimensions {}x{} != {expected_dir_width}x{expected_dir_height}",
            section.dir_width, section.dir_height
        ));
    }
    let expected_dir_density = texel_density * direction_texel_scale as f32;
    if section.dir_texel_density.to_bits() != expected_dir_density.to_bits() {
        return Err(format!(
            "direction density {} != {expected_dir_density}",
            section.dir_texel_density
        ));
    }
    if section.direction_format != DIRECTION_FORMAT_OCT_RG8 {
        return Err(format!(
            "direction format {} != {DIRECTION_FORMAT_OCT_RG8}",
            section.direction_format
        ));
    }
    if section.mode != LightmapMode::Shadowed {
        return Err(format!(
            "lightmap mode {:?} != {:?}",
            section.mode,
            LightmapMode::Shadowed
        ));
    }
    Ok(())
}

/// Build the cache key for the composited lightmap section — the second-level
/// memo that lets a no-edit rebuild skip the per-light layer reads, composite,
/// dilate, and BC6H encode and instead decode the section bytes directly.
///
/// The fold covers every input that determines the section bytes, under a fixed
/// unambiguous byte layout:
/// 1. `LAYER_FORMAT_VERSION` (u32 LE) — couples this key to the per-light layer
///    format the same way the private per-light `CacheKey.digest`s would; a
///    layer-format bump invalidates the section without reading the layer keys.
/// 2. light count (u32 LE) — so add/remove can never alias a reorder. The
///    fixed-width 32-byte hash records below already make a plain concatenation
///    injective, but folding the count is a cheap belt-and-suspenders guard.
/// 3. each light-layer `layer_input_hash` `[u8; 32]`, in the caller's exact
///    layer-major then global-static-light order (`ShadowType::Sdf` dropped).
///    Folding the input hashes mirrors folding the full cache keys: any
///    per-light input change (light params, geometry slice, density, atlas
///    layout, or target layer) flows through here.
/// 4. the complete prepared atlas layout fingerprint. This is required even
///    when the filtered light set is empty, because the all-Sdf fallback bytes
///    still depend on atlas dimensions.
/// 5. `texel_density` (f32 LE) — the `density` passed to `encode_section`.
///    Already folded into every `layer_input_hash` via `lightmap_density`, so
///    this is belt-and-suspenders (same rationale as the light-count fold).
/// 6. `uncompressed_irradiance` (1 byte, 0/1) — selects BC6H vs RGBA16F output.
/// 7. `direction_texel_scale` (u32 LE) — selects the post-composite direction
///    atlas resolution without invalidating any per-light layer cache entry.
///
/// `layer_input_hashes` must be supplied in the same filtered order the warm
/// composite loop uses; the helper does not re-derive or re-sort them.
pub fn section_input_hash(
    layer_input_hashes: &[[u8; 32]],
    atlas: &SharedAtlas<'_>,
    texel_density: f32,
    uncompressed_irradiance: bool,
    direction_texel_scale: u32,
) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(&LAYER_FORMAT_VERSION.to_le_bytes());
    hasher.update(&(layer_input_hashes.len() as u32).to_le_bytes());
    for hash in layer_input_hashes {
        hasher.update(hash);
    }
    hasher.update(&atlas_layout_fingerprint(atlas));
    hasher.update(&texel_density.to_le_bytes());
    hasher.update(&[u8::from(uncompressed_irradiance)]);
    hasher.update(&direction_texel_scale.to_le_bytes());
    *hasher.finalize().as_bytes()
}

/// Padding applied by the per-light cache key's influence bound.
pub const LAYER_AABB_PADDING_METERS: f32 = AABB_PADDING_METERS;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bake_control::BakeControl;
    use crate::bvh_build::build_bvh;
    use crate::governor::Governor;
    use crate::lightmap_bake::{
        bake_monolithic_atlas, bake_monolithic_atlas_controlled, prepare_atlas,
    };
    use crate::map_data::{FalloffModel, ShadowType};
    use crate::reporter::StageProgress;
    use glam::DVec3;
    use postretro_level_format::geometry::{FaceMeta, GeometrySection, Vertex};
    use postretro_level_format::texture_names::TextureNamesSection;
    use rayon::ThreadPoolBuilder;
    use std::sync::Arc;
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

    const AREA_SAMPLES: u32 = 16;
    const DENSITY: f32 = 0.25;

    /// Two coplanar quads on the floor (Y=0), separate faces so the atlas packs
    /// more than one chart and `split_shared_vertices` has work to do.
    fn two_quad_geometry() -> GeometryResult {
        let mk = |x: f32, z: f32| {
            let n = [0.0, 1.0, 0.0];
            let t = [1.0, 0.0, 0.0];
            vec![
                Vertex::new([x, 0.0, z], [0.0, 0.0], n, t, true, [0.0, 0.0], 0),
                Vertex::new([x + 1.0, 0.0, z], [1.0, 0.0], n, t, true, [0.0, 0.0], 0),
                Vertex::new(
                    [x + 1.0, 0.0, z + 1.0],
                    [1.0, 1.0],
                    n,
                    t,
                    true,
                    [0.0, 0.0],
                    0,
                ),
                Vertex::new([x, 0.0, z + 1.0], [0.0, 1.0], n, t, true, [0.0, 0.0], 0),
            ]
        };
        let mut vertices = mk(0.0, 0.0);
        vertices.extend(mk(3.0, 0.0));
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
                crate::geometry::FaceIndexRange {
                    index_offset: 0,
                    index_count: 6,
                },
                crate::geometry::FaceIndexRange {
                    index_offset: 6,
                    index_count: 6,
                },
            ],
        }
    }

    fn point_light(origin: [f64; 3], range: f32) -> MapLight {
        MapLight {
            origin: DVec3::new(origin[0], origin[1], origin[2]),
            carrier: String::new(),
            light_type: LightType::Point,
            intensity: 1.0,
            color: [1.0, 1.0, 1.0],
            falloff_model: FalloffModel::Linear,
            falloff_range: range,
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
            tags: vec![],
            shadow_type: ShadowType::StaticLightMap,
        }
    }

    fn directional_light() -> MapLight {
        let mut l = point_light([0.0, 5.0, 0.0], 0.0);
        l.light_type = LightType::Directional;
        l.cone_direction = Some([0.0, -1.0, 0.0]);
        l
    }

    fn bake_layer_for_test(
        light: &MapLight,
        atlas: &SharedAtlas<'_>,
        bvh: &bvh::bvh::Bvh<f32, 3>,
        primitives: &[BvhPrimitive],
        geometry: &GeometryResult,
        area_sample_count: u32,
    ) -> LightmapLayer {
        let progress = StageProgress::indeterminate();
        let control = BakeControl::new(Arc::new(Governor::new(1, false)), &progress);
        bake_light_layer_controlled(
            light,
            atlas,
            bvh,
            primitives,
            geometry,
            0,
            area_sample_count,
            &control,
        )
    }

    fn expected_chart_texel_order(atlas: &SharedAtlas<'_>) -> Vec<(u32, u32)> {
        let padding = crate::chart_raster::CHART_PADDING_TEXELS as i32;
        let mut order = Vec::new();
        for (face_idx, placement) in atlas.placements.iter().enumerate() {
            let chart = &atlas.charts[face_idx];
            if chart.uv_extent[0] <= 0.0 || chart.uv_extent[1] <= 0.0 {
                continue;
            }
            let (interior_w, interior_h) = chart_interior_dims(chart);
            for ty in 0..interior_h {
                for tx in 0..interior_w {
                    let atlas_x = placement.x as i32 + padding + tx;
                    let atlas_y = placement.y as i32 + padding + ty;
                    order.push((
                        placement.layer,
                        atlas_y as u32 * atlas.atlas_width + atlas_x as u32,
                    ));
                }
            }
        }
        order.sort_unstable();
        order
    }

    #[test]
    fn analytic_coverage_matches_baked_coverage_fixture_matrix() {
        let light = point_light([0.5, 1.0, 0.5], 4.0);
        let samples = [
            ("visible", Vec3::new(0.5, 0.0, 0.5), Vec3::Y, true, true),
            (
                "fully occluded",
                Vec3::new(0.5, 0.0, 0.5),
                Vec3::Y,
                false,
                true,
            ),
            (
                "beyond falloff",
                Vec3::new(20.0, 0.0, 20.0),
                Vec3::Y,
                true,
                false,
            ),
            (
                "backfacing",
                Vec3::new(0.5, 0.0, 0.5),
                -Vec3::Y,
                true,
                false,
            ),
        ];

        for (name, world_p, normal, trace_is_clear, expected_coverage) in samples {
            let analytic = light_texel_is_covered(&light, world_p, normal);
            let (_, _, raw_visibility) =
                crate::lightmap_bake::light_texel_contribution_and_visibility(
                    &light,
                    world_p,
                    normal,
                    7,
                    AREA_SAMPLES,
                    |_, _| trace_is_clear,
                );
            assert_eq!(analytic, raw_visibility.is_some(), "{name}");
            assert_eq!(analytic, expected_coverage, "{name}");
        }

        let contributing_point = Vec3::new(0.5, 0.0, 0.5);
        assert!(light_texel_is_covered(&light, contributing_point, Vec3::Y));
    }

    #[test]
    fn analytic_and_baked_chart_walks_both_skip_non_positive_extent() {
        let geometry = two_quad_geometry();
        let charts = vec![Chart {
            origin: Vec3::ZERO,
            u_axis: Vec3::X,
            v_axis: Vec3::Z,
            uv_min: [0.0, 0.0],
            uv_extent: [0.0, 1.0],
            normal: Vec3::Y,
            width_texels: 5,
            height_texels: 5,
            leaf_index: 0,
        }];
        let placements = vec![ChartPlacement {
            x: 0,
            y: 0,
            layer: 0,
        }];
        let shared = SharedAtlas {
            charts: &charts,
            placements: &placements,
            atlas_width: 5,
            atlas_height: 5,
        };
        let (bvh, primitives, _) = build_bvh(&geometry).unwrap();
        let light = point_light([0.5, 1.0, 0.5], 4.0);
        let baked = bake_light_layer_chart_controlled(
            &light,
            &shared,
            0,
            &bvh,
            &primitives,
            &geometry,
            AREA_SAMPLES,
            &BakeControl::unrestricted(),
        );
        let mut analytic = Vec::new();
        visit_light_chart_coverage_controlled(
            &light,
            &shared,
            0,
            &BakeControl::unrestricted(),
            |texel| analytic.push(texel),
        );

        assert!(baked.is_empty());
        assert!(analytic.is_empty());
    }

    /// The production warm fold reproduces the cold monolith on a synthetic
    /// multi-light, multi-atlas-layer layout. Two point lights over different
    /// quads plus one directional light exercise sparse + dense partitions,
    /// ordered direction accumulation, and the covered-but-dark fallback.
    #[test]
    fn incremental_layer_fold_matches_monolithic_section_bytes() {
        // One geometry clone per path so each path's prepare_atlas mutates its own
        // copy identically; the BVH is built once from the shared pre-bake state.
        let mut mono_geo = two_quad_geometry();
        let mut layer_geo = two_quad_geometry();

        let lights = vec![
            point_light([0.5, 1.0, 0.5], 5.0),
            point_light([3.5, 1.0, 0.5], 5.0),
            directional_light(),
        ];
        let static_lights = crate::light_namespaces::StaticBakedLights::from_lights(&lights);
        let light_refs: Vec<&MapLight> = static_lights.entries().iter().map(|e| e.light).collect();

        // Monolithic path.
        let mut mono_prepared = prepare_atlas(&mut mono_geo, &static_lights, DENSITY, &[]).unwrap();
        mono_prepared.placements[1].layer = 1;
        mono_prepared.layer_count = 2;
        let (mono_bvh, mono_prims, _) = build_bvh(&mono_geo).unwrap();
        let mono_progress = StageProgress::with_total(mono_prepared.placements.len());
        let mono_control = BakeControl::new(Arc::new(Governor::new(1, false)), &mono_progress);
        let mono_atlas = bake_monolithic_atlas_controlled(
            &mono_bvh,
            &mono_prims,
            &mono_geo,
            &light_refs,
            &mono_prepared.charts,
            &mono_prepared.placements,
            mono_prepared.atlas_width,
            mono_prepared.atlas_height,
            mono_prepared.layer_count,
            AREA_SAMPLES,
            &mono_control,
        );
        assert_eq!(mono_progress.completed(), mono_prepared.placements.len());

        // Warm production fold, with the same forced two-plane layout.
        let mut layer_prepared =
            prepare_atlas(&mut layer_geo, &static_lights, DENSITY, &[]).unwrap();
        layer_prepared.placements[1].layer = 1;
        layer_prepared.layer_count = 2;
        let (layer_bvh, layer_prims, _) = build_bvh(&layer_geo).unwrap();
        let shared = SharedAtlas {
            charts: &layer_prepared.charts,
            placements: &layer_prepared.placements,
            atlas_width: layer_prepared.atlas_width,
            atlas_height: layer_prepared.atlas_height,
        };
        let warm_section =
            compose_section(&light_refs, &shared, &layer_bvh, &layer_prims, &layer_geo);
        assert_eq!(
            mono_atlas.encode_section(DENSITY, true, crate::lightmap_bake::DIRECTION_TEXEL_SCALE,),
            warm_section,
            "layer-major incremental warm fold must equal the cold monolith byte-for-byte"
        );
        assert_eq!(
            mono_atlas.encode_section(DENSITY, false, crate::lightmap_bake::DIRECTION_TEXEL_SCALE,),
            compose_section_with_format(
                &light_refs,
                &shared,
                &layer_bvh,
                &layer_prims,
                &layer_geo,
                false,
            ),
            "incremental layer assembly must preserve the cold BC6H bytes too"
        );
    }

    #[test]
    fn layer_bake_preserves_chart_order_and_bytes_across_thread_counts() {
        let mut geo = two_quad_geometry();
        let lights = vec![point_light([0.5, 1.0, 0.5], 5.0)];
        let static_lights = crate::light_namespaces::StaticBakedLights::from_lights(&lights);
        let prepared = prepare_atlas(&mut geo, &static_lights, DENSITY, &[]).unwrap();
        assert!(
            prepared.placements.len() > 1,
            "fixture must have enough charts to exercise ordered parallel collection"
        );
        let (bvh, primitives, _) = build_bvh(&geo).unwrap();
        let shared = SharedAtlas {
            charts: &prepared.charts,
            placements: &prepared.placements,
            atlas_width: prepared.atlas_width,
            atlas_height: prepared.atlas_height,
        };

        let single_progress = StageProgress::with_total(prepared.placements.len());
        let single_control = BakeControl::new(Arc::new(Governor::new(1, false)), &single_progress);
        let one_thread = ThreadPoolBuilder::new()
            .num_threads(1)
            .build()
            .expect("single-worker rayon pool");
        let single_thread_layer = one_thread.install(|| {
            bake_light_layer_controlled(
                &lights[0],
                &shared,
                &bvh,
                &primitives,
                &geo,
                0,
                AREA_SAMPLES,
                &single_control,
            )
        });

        let multi_progress = StageProgress::with_total(prepared.placements.len());
        let multi_control = BakeControl::new(Arc::new(Governor::new(2, false)), &multi_progress);
        let multi_thread = ThreadPoolBuilder::new()
            .num_threads(2)
            .build()
            .expect("multi-worker rayon pool");
        let multi_thread_layer = multi_thread.install(|| {
            bake_light_layer_controlled(
                &lights[0],
                &shared,
                &bvh,
                &primitives,
                &geo,
                0,
                AREA_SAMPLES,
                &multi_control,
            )
        });

        assert_eq!(
            single_progress.completed(),
            prepared.placements.len(),
            "one progress advance is required for every chart"
        );
        assert_eq!(
            single_progress.total(),
            Some(single_progress.completed()),
            "single-worker bake must advance its published chart total exactly"
        );
        assert_eq!(
            multi_progress.completed(),
            prepared.placements.len(),
            "parallel workers must advance every chart exactly once"
        );
        assert_eq!(
            multi_progress.total(),
            Some(multi_progress.completed()),
            "parallel bake must advance its published chart total exactly"
        );
        assert_eq!(
            single_thread_layer.to_bytes(),
            multi_thread_layer.to_bytes(),
            "layer cache bytes must not depend on Rayon worker count"
        );
        assert_eq!(
            multi_thread_layer
                .texels
                .iter()
                .map(|texel| (multi_thread_layer.target_layer, texel.idx))
                .collect::<Vec<_>>(),
            expected_chart_texel_order(&shared),
            "parallel chart buffers must concatenate in placement order"
        );
    }

    #[test]
    fn layer_bake_degenerate_chart_advances_progress_and_keeps_ordered_empty_slot() {
        let mut geo = two_quad_geometry();
        let lights = vec![point_light([0.5, 1.0, 0.5], 5.0)];
        let static_lights = crate::light_namespaces::StaticBakedLights::from_lights(&lights);
        let mut prepared = prepare_atlas(&mut geo, &static_lights, DENSITY, &[]).unwrap();
        assert!(
            prepared.placements.len() > 1,
            "fixture must have a non-degenerate chart after the forced skip"
        );
        prepared.charts[0].uv_extent[0] = 0.0;
        let (bvh, primitives, _) = build_bvh(&geo).unwrap();
        let shared = SharedAtlas {
            charts: &prepared.charts,
            placements: &prepared.placements,
            atlas_width: prepared.atlas_width,
            atlas_height: prepared.atlas_height,
        };
        let progress = StageProgress::with_total(prepared.placements.len());
        let control = BakeControl::new(Arc::new(Governor::new(2, false)), &progress);
        let pool = ThreadPoolBuilder::new()
            .num_threads(2)
            .build()
            .expect("multi-worker rayon pool");
        let layer = pool.install(|| {
            bake_light_layer_controlled(
                &lights[0],
                &shared,
                &bvh,
                &primitives,
                &geo,
                0,
                AREA_SAMPLES,
                &control,
            )
        });

        assert_eq!(progress.completed(), prepared.placements.len());
        assert_eq!(
            progress.total(),
            Some(progress.completed()),
            "the degenerate chart must still complete the published chart total"
        );
        assert_eq!(
            layer
                .texels
                .iter()
                .map(|texel| (layer.target_layer, texel.idx))
                .collect::<Vec<_>>(),
            expected_chart_texel_order(&shared),
            "the degenerate chart must contribute an empty ordered buffer"
        );
    }

    #[test]
    fn layer_bake_paused_before_permit_release_starts_no_chart() {
        let mut geo = two_quad_geometry();
        let lights = vec![point_light([0.5, 1.0, 0.5], 5.0)];
        let static_lights = crate::light_namespaces::StaticBakedLights::from_lights(&lights);
        let prepared = prepare_atlas(&mut geo, &static_lights, DENSITY, &[]).unwrap();
        let (bvh, primitives, _) = build_bvh(&geo).unwrap();
        let chart_count = prepared.placements.len();
        let expected_texel_count = expected_chart_texel_order(&SharedAtlas {
            charts: &prepared.charts,
            placements: &prepared.placements,
            atlas_width: prepared.atlas_width,
            atlas_height: prepared.atlas_height,
        })
        .len();
        let governor = Arc::new(Governor::new(1, false));
        let held_permit = governor.enter();
        let progress = StageProgress::with_total(chart_count);
        let control = BakeControl::new(Arc::clone(&governor), &progress);
        let (started_tx, started_rx) = mpsc::channel();
        let (finished_tx, finished_rx) = mpsc::channel();

        let worker = thread::spawn(move || {
            let shared = SharedAtlas {
                charts: &prepared.charts,
                placements: &prepared.placements,
                atlas_width: prepared.atlas_width,
                atlas_height: prepared.atlas_height,
            };
            started_tx.send(()).expect("test coordinator is waiting");
            let layer = ThreadPoolBuilder::new()
                .num_threads(2)
                .build()
                .expect("multi-worker rayon pool")
                .install(|| {
                    bake_light_layer_controlled(
                        &lights[0],
                        &shared,
                        &bvh,
                        &primitives,
                        &geo,
                        0,
                        AREA_SAMPLES,
                        &control,
                    )
                });
            finished_tx
                .send(layer)
                .expect("test coordinator is waiting");
        });

        started_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("bake worker did not start");
        // The held permit prevents an in-flight chart. Pausing before releasing
        // it makes every pending `enter` observe the pause, without a timing
        // window in which a new chart could begin.
        governor.set_paused(true);
        drop(held_permit);
        assert_eq!(
            progress.completed(),
            0,
            "no chart may advance after pause and before admission resumes"
        );

        governor.set_paused(false);
        let layer = finished_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("bake did not resume after unpausing");
        worker.join().expect("bake worker must not panic");
        assert_eq!(progress.completed(), chart_count);
        assert_eq!(
            progress.total(),
            Some(progress.completed()),
            "the resumed layer bake must advance its published chart total exactly"
        );
        assert_eq!(
            layer.texels.len(),
            expected_texel_count,
            "every placement must bake once after resume"
        );
    }

    #[test]
    fn layer_roundtrips_through_codec() {
        let mut geo = two_quad_geometry();
        let lights = vec![point_light([0.5, 1.0, 0.5], 5.0)];
        let static_lights = crate::light_namespaces::StaticBakedLights::from_lights(&lights);
        let prepared = prepare_atlas(&mut geo, &static_lights, DENSITY, &[]).unwrap();
        let (bvh, prims, _) = build_bvh(&geo).unwrap();
        let shared = SharedAtlas {
            charts: &prepared.charts,
            placements: &prepared.placements,
            atlas_width: prepared.atlas_width,
            atlas_height: prepared.atlas_height,
        };
        let layer = bake_layer_for_test(&lights[0], &shared, &bvh, &prims, &geo, AREA_SAMPLES);

        let bytes = layer.to_bytes();
        assert_eq!(
            bytes.len(),
            LAYER_HEADER_BYTES + layer.texels.len() * 8,
            "sparse cache payload must use one 8-byte record per reached texel"
        );
        let decoded = LightmapLayer::from_bytes(&bytes).expect("round-trip decode");
        assert_eq!(layer, decoded, "layer codec must round-trip exactly");
    }

    #[test]
    fn sparse_codec_preserves_zero_and_nan_visibility_bits() {
        let nan = f32::from_bits(0x7fc0_1234);
        let layer = LightmapLayer {
            atlas_width: 8,
            atlas_height: 8,
            layer_count: 2,
            target_layer: 1,
            texels: vec![
                LayerTexel {
                    idx: 7,
                    raw_visibility: 0.0,
                },
                LayerTexel {
                    idx: 19,
                    raw_visibility: nan,
                },
            ],
        };
        let bytes = layer.to_bytes();
        assert_eq!(bytes.len(), 20 + 2 * 8);
        let decoded = LightmapLayer::from_bytes(&bytes).unwrap();
        assert_eq!(decoded.target_layer, 1);
        assert_eq!(decoded.texels[0].raw_visibility.to_bits(), 0.0f32.to_bits());
        assert_eq!(decoded.texels[1].raw_visibility.to_bits(), nan.to_bits());
    }

    #[test]
    fn sparse_presence_and_reconstruction_preserve_dense_edge_semantics() {
        let light = point_light([0.5, 1.0, 0.5], 5.0);
        let world_p = Vec3::new(0.5, 0.0, 0.5);

        let unreached = bake_sparse_layer_texel(
            3,
            &light,
            Vec3::new(20.0, 0.0, 20.0),
            Vec3::Y,
            0,
            1,
            |_, _| true,
        );
        assert!(
            unreached.is_none(),
            "an unreached texel must have no record"
        );

        let occluded = bake_sparse_layer_texel(3, &light, world_p, Vec3::Y, 0, 1, |_, _| false)
            .expect("analytic coverage survives full occlusion");
        assert_eq!(occluded.raw_visibility.to_bits(), 0.0f32.to_bits());
        let (irradiance, weighted_dir) =
            reconstruct_light_texel(&light, world_p, Vec3::Y, occluded.raw_visibility);
        assert!(irradiance.to_array().iter().all(|v| v.to_bits() == 0));
        assert!(weighted_dir.to_array().iter().all(|v| v.to_bits() == 0));

        let nan = f32::from_bits(0x7fc0_1234);
        let (contribution, to_light) = light_contribution_and_direction(&light, world_p, Vec3::Y);
        let expected_irradiance = contribution * nan;
        let expected_direction =
            to_light * ((contribution.x + contribution.y + contribution.z) * nan);
        let (actual_irradiance, actual_direction) =
            reconstruct_light_texel(&light, world_p, Vec3::Y, nan);
        assert_eq!(
            actual_irradiance.to_array().map(f32::to_bits),
            expected_irradiance.to_array().map(f32::to_bits)
        );
        assert_eq!(
            actual_direction.to_array().map(f32::to_bits),
            expected_direction.to_array().map(f32::to_bits)
        );
    }

    #[test]
    fn sparse_skip_preserves_dense_signed_zero_fold_bits() {
        let light = directional_light();
        let (_, term) = reconstruct_light_texel(&light, Vec3::ZERO, Vec3::Y, 1.0);
        assert_eq!(term.x.to_bits(), (-0.0f32).to_bits());
        let sparse = Vec3::ZERO + term;
        let dense = Vec3::ZERO + term + Vec3::ZERO;
        assert_eq!(
            sparse.to_array().map(f32::to_bits),
            dense.to_array().map(f32::to_bits),
            "omitting a later unreached +0 term must preserve dense fold bits"
        );
    }

    #[test]
    fn sparse_writer_uses_adjacent_values_around_coverage_epsilon() {
        let mut light = directional_light();
        let threshold =
            (crate::lightmap_bake::LIGHT_TEXEL_CONTRIBUTION_EPSILON_SQUARED / 3.0).sqrt();
        let mut below = threshold;
        while crate::lightmap_bake::contribution_covers_shadowmask(3.0 * below * below) {
            below = f32::from_bits(below.to_bits() - 1);
        }
        let above = f32::from_bits(below.to_bits() + 1);
        assert!(!crate::lightmap_bake::contribution_covers_shadowmask(
            3.0 * below * below
        ));
        assert!(crate::lightmap_bake::contribution_covers_shadowmask(
            3.0 * above * above
        ));

        light.intensity = below;
        assert!(
            bake_sparse_layer_texel(0, &light, Vec3::ZERO, Vec3::Y, 0, 1, |_, _| true).is_none()
        );
        light.intensity = above;
        assert!(
            bake_sparse_layer_texel(0, &light, Vec3::ZERO, Vec3::Y, 0, 1, |_, _| true).is_some()
        );
    }

    #[test]
    fn sparse_cache_epochs_are_pinned() {
        assert_eq!(LAYER_FORMAT_VERSION, 6);
        assert_eq!(LIGHTMAP_SECTION_VERSION, 3);
    }

    #[test]
    fn pre_sparse_cache_epochs_are_unreadable() {
        let dir = fresh_cache_dir("pre_sparse_epochs");
        let cache = StageCache::new(&dir).unwrap();
        let digest = [0x5a; 32];
        let old_layer = CacheKey::new("lightmap_layer", 5, &digest);
        let new_layer = CacheKey::new("lightmap_layer", LAYER_FORMAT_VERSION, &digest);
        let old_section = CacheKey::new("lightmap_section", 2, &digest);
        let new_section = CacheKey::new("lightmap_section", LIGHTMAP_SECTION_VERSION, &digest);
        cache.put(&old_layer, b"old dense layer");
        cache.put(&old_section, b"old section");
        assert!(cache.get(&new_layer).is_none());
        assert!(cache.get(&new_section).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn empty_sparse_partition_roundtrips_and_has_no_fold_effect() {
        let mut geo = two_quad_geometry();
        let lights = vec![point_light([100.0, 1.0, 100.0], 1.0)];
        let static_lights = crate::light_namespaces::StaticBakedLights::from_lights(&lights);
        let prepared = prepare_atlas(&mut geo, &static_lights, DENSITY, &[]).unwrap();
        let (bvh, prims, _) = build_bvh(&geo).unwrap();
        let shared = SharedAtlas {
            charts: &prepared.charts,
            placements: &prepared.placements,
            atlas_width: prepared.atlas_width,
            atlas_height: prepared.atlas_height,
        };
        let partition = bake_light_layer_controlled(
            &lights[0],
            &shared,
            &bvh,
            &prims,
            &geo,
            0,
            AREA_SAMPLES,
            &BakeControl::unrestricted(),
        );
        assert!(partition.texels.is_empty());
        validate_layer_partition(&partition, &shared, 0).unwrap();
        assert_eq!(
            LightmapLayer::from_bytes(&partition.to_bytes()),
            Some(partition.clone())
        );

        let mut accumulator = IncrementalLayerAccumulator::for_atlas_layer(&shared, 0);
        accumulator.fold_partition(&lights[0], &partition, &shared);
        let folded = accumulator.finish();
        assert!(folded.coverage.iter().any(|covered| *covered));
        assert!(
            folded
                .irradiance
                .chunks_exact(4)
                .all(|rgba| rgba[..3].iter().all(|value| value.to_bits() == 0))
        );
    }

    #[test]
    fn sparse_multilayer_payload_is_below_one_tenth_dense_bytes() {
        let mut geo = two_quad_geometry();
        let lights = vec![point_light([0.5, 1.0, 0.5], 1.0)];
        let static_lights = crate::light_namespaces::StaticBakedLights::from_lights(&lights);
        let mut prepared = prepare_atlas(&mut geo, &static_lights, 0.1, &[]).unwrap();
        prepared.placements[0].layer = 0;
        prepared.placements[1].layer = 1;
        let (bvh, prims, _) = build_bvh(&geo).unwrap();
        let shared = SharedAtlas {
            charts: &prepared.charts,
            placements: &prepared.placements,
            atlas_width: prepared.atlas_width,
            atlas_height: prepared.atlas_height,
        };
        let partitions = bake_light_layer(
            &lights[0],
            &shared,
            &bvh,
            &prims,
            &geo,
            AREA_SAMPLES,
            &BakeControl::unrestricted(),
        );
        assert_eq!(partitions.len(), 2, "named fixture must remain multi-layer");
        assert!(
            partitions[1].texels.is_empty(),
            "remote layer must be empty"
        );
        let sparse_bytes: usize = partitions
            .iter()
            .map(|partition| partition.to_bytes().len())
            .sum();
        let former_dense_records = expected_chart_texel_order(&shared).len();
        let former_dense_bytes = former_dense_records * 48;
        assert!(
            sparse_bytes * 10 < former_dense_bytes,
            "sparse {sparse_bytes} bytes must be below one tenth of former dense {former_dense_bytes} bytes"
        );
    }

    #[test]
    fn corrupt_layer_blob_is_a_miss() {
        // `from_bytes` rejects a garbage blob and returns `None` (format-validation
        // path). This is independent of the `StageCache` byte-level length/hash
        // check — it calls `from_bytes` directly, exercising only the codec.
        assert!(LightmapLayer::from_bytes(b"not a valid header+body layer").is_none());
    }

    #[test]
    fn from_bytes_rejects_overflowing_texel_count() {
        // Pins the overflow-guard defense: a header with count = u32::MAX causes
        // count * size_of::<LayerTexel>() to overflow; from_bytes must return None,
        // never panic.
        let mut blob = Vec::new();
        blob.extend_from_slice(&1u32.to_ne_bytes()); // atlas_width
        blob.extend_from_slice(&1u32.to_ne_bytes()); // atlas_height
        blob.extend_from_slice(&1u32.to_ne_bytes()); // layer_count
        blob.extend_from_slice(&0u32.to_ne_bytes()); // target_layer
        blob.extend_from_slice(&u32::MAX.to_ne_bytes()); // count = u32::MAX
        // No body bytes — the implied body cannot match.
        assert!(LightmapLayer::from_bytes(&blob).is_none());
    }

    #[test]
    fn layer_input_hash_changes_when_light_moves() {
        let mut geo = two_quad_geometry();
        let lights = vec![point_light([0.5, 1.0, 0.5], 5.0)];
        let static_lights = crate::light_namespaces::StaticBakedLights::from_lights(&lights);
        let prepared = prepare_atlas(&mut geo, &static_lights, DENSITY, &[]).unwrap();
        let (_, prims, _) = build_bvh(&geo).unwrap();
        let shared = SharedAtlas {
            charts: &prepared.charts,
            placements: &prepared.placements,
            atlas_width: prepared.atlas_width,
            atlas_height: prepared.atlas_height,
        };

        let base = point_light([0.5, 1.0, 0.5], 5.0);
        let moved = point_light([10.0, 1.0, 0.5], 5.0);
        let h_base = layer_input_hash(&base, &shared, &prims, &geo, DENSITY, AREA_SAMPLES, 0);
        let h_moved = layer_input_hash(&moved, &shared, &prims, &geo, DENSITY, AREA_SAMPLES, 0);
        assert_ne!(
            h_base, h_moved,
            "moving the light must change its layer cache key"
        );
    }

    #[test]
    fn target_layer_partitions_are_disjoint_and_rekeyed() {
        let mut geo = two_quad_geometry();
        let lights = vec![point_light([0.5, 1.0, 0.5], 5.0)];
        let static_lights = crate::light_namespaces::StaticBakedLights::from_lights(&lights);
        let mut prepared = prepare_atlas(&mut geo, &static_lights, DENSITY, &[]).unwrap();
        assert_eq!(prepared.placements.len(), 2, "fixture needs two charts");
        // Force a two-layer layout while retaining chart coordinates and seeds.
        prepared.placements[0].layer = 0;
        prepared.placements[1].layer = 1;
        let (bvh, prims, _) = build_bvh(&geo).unwrap();
        let shared = SharedAtlas {
            charts: &prepared.charts,
            placements: &prepared.placements,
            atlas_width: prepared.atlas_width,
            atlas_height: prepared.atlas_height,
        };
        let control = BakeControl::unrestricted();
        let first = bake_light_layer_controlled(
            &lights[0],
            &shared,
            &bvh,
            &prims,
            &geo,
            0,
            AREA_SAMPLES,
            &control,
        );
        let second = bake_light_layer_controlled(
            &lights[0],
            &shared,
            &bvh,
            &prims,
            &geo,
            1,
            AREA_SAMPLES,
            &control,
        );

        assert!(!first.texels.is_empty() && !second.texels.is_empty());
        assert_eq!(first.target_layer, 0);
        assert_eq!(second.target_layer, 1);
        assert!(validate_layer_partition(&first, &shared, 0).is_ok());
        assert!(validate_layer_partition(&second, &shared, 1).is_ok());
        assert!(
            validate_layer_partition(&first, &shared, 1).is_err(),
            "a partition from another atlas layer must be a soft cache miss"
        );

        let key_for = |target_layer| {
            CacheKey::new(
                "lightmap_layer",
                LAYER_FORMAT_VERSION,
                &layer_input_hash(
                    &lights[0],
                    &shared,
                    &prims,
                    &geo,
                    DENSITY,
                    AREA_SAMPLES,
                    target_layer,
                ),
            )
            .as_filename()
        };
        assert_ne!(
            key_for(0),
            key_for(1),
            "one light's partitions must not share a cache key"
        );
    }

    #[test]
    fn validate_layer_partition_accepts_sparse_and_rejects_invalid_records() {
        let mut geo = two_quad_geometry();
        let lights = vec![point_light([0.5, 1.0, 0.5], 5.0)];
        let static_lights = crate::light_namespaces::StaticBakedLights::from_lights(&lights);
        let prepared = prepare_atlas(&mut geo, &static_lights, DENSITY, &[]).unwrap();
        let (bvh, prims, _) = build_bvh(&geo).unwrap();
        let shared = SharedAtlas {
            charts: &prepared.charts,
            placements: &prepared.placements,
            atlas_width: prepared.atlas_width,
            atlas_height: prepared.atlas_height,
        };
        let original = bake_light_layer_controlled(
            &lights[0],
            &shared,
            &bvh,
            &prims,
            &geo,
            0,
            AREA_SAMPLES,
            &BakeControl::unrestricted(),
        );
        assert!(original.texels.len() > 1, "fixture needs multiple texels");

        let mut missing = original.clone();
        missing.texels.remove(missing.texels.len() / 2);
        assert!(
            validate_layer_partition(&missing, &shared, 0).is_ok(),
            "absence is the sparse encoding of an analytically unreached texel"
        );

        let mut duplicate = original.clone();
        duplicate.texels[1] = duplicate.texels[0];
        assert!(
            validate_layer_partition(&duplicate, &shared, 0).is_err(),
            "a duplicate replacing another in-bounds texel must be rejected"
        );

        let mut out_of_bounds = original.clone();
        out_of_bounds.texels[0].idx = shared.atlas_width * shared.atlas_height;
        out_of_bounds.texels.sort_unstable_by_key(|texel| texel.idx);
        assert!(validate_layer_partition(&out_of_bounds, &shared, 0).is_err());

        let mut outside_chart = original;
        outside_chart.texels[0].idx = 0;
        outside_chart.texels.sort_unstable_by_key(|texel| texel.idx);
        assert!(validate_layer_partition(&outside_chart, &shared, 0).is_err());

        let mut nan_visibility = missing.clone();
        nan_visibility.texels[0].raw_visibility = f32::from_bits(0x7fc0_1234);
        assert!(
            validate_layer_partition(&nan_visibility, &shared, 0).is_ok(),
            "NaN visibility remains a valid sparse record"
        );

        for (visibility, name) in [(f32::INFINITY, "+infinity"), (2.0, "above one")] {
            let mut invalid_visibility = missing.clone();
            invalid_visibility.texels[0].raw_visibility = visibility;
            assert!(
                validate_layer_partition(&invalid_visibility, &shared, 0).is_err(),
                "{name} visibility must become a semantic cache miss"
            );
        }
    }

    #[test]
    fn all_sdf_fallback_remains_one_uncovered_plane() {
        let mut geo = two_quad_geometry();
        let lights = vec![point_light([0.5, 1.0, 0.5], 5.0)];
        let static_lights = crate::light_namespaces::StaticBakedLights::from_lights(&lights);
        let mut prepared = prepare_atlas(&mut geo, &static_lights, DENSITY, &[]).unwrap();
        prepared.placements[1].layer = 1;
        let (bvh, prims, _) = build_bvh(&geo).unwrap();
        let shared = SharedAtlas {
            charts: &prepared.charts,
            placements: &prepared.placements,
            atlas_width: prepared.atlas_width,
            atlas_height: prepared.atlas_height,
        };

        let warm_fallback = compose_section(&[], &shared, &bvh, &prims, &geo);
        let mut legacy_fallback = empty_composite(shared.atlas_width, shared.atlas_height);
        legacy_fallback.dilate();
        assert_eq!(
            warm_fallback,
            legacy_fallback.encode_section(
                DENSITY,
                true,
                crate::lightmap_bake::DIRECTION_TEXEL_SCALE,
            ),
            "all-Sdf warm fallback must remain the legacy single uncovered plane"
        );
        assert_eq!(warm_fallback.layer_count, 1);
    }

    fn lone_chart_layout_fingerprint(
        width_texels: u32,
        height_texels: u32,
        uv_extent: [f32; 2],
    ) -> Vec<u8> {
        let charts = [Chart {
            origin: Vec3::ZERO,
            u_axis: Vec3::X,
            v_axis: Vec3::Z,
            uv_min: [0.0, 0.0],
            uv_extent,
            normal: Vec3::Y,
            width_texels,
            height_texels,
            leaf_index: 0,
        }];
        // Both P2 variants remain at the packer's 64² minimum and at the same
        // sole-chart placement. Only the resolved chart dimensions may re-key.
        let placements = [ChartPlacement {
            x: 0,
            y: 0,
            layer: 0,
        }];
        atlas_layout_fingerprint(&SharedAtlas {
            charts: &charts,
            placements: &placements,
            atlas_width: 64,
            atlas_height: 64,
        })
    }

    #[test]
    fn atlas_layout_fingerprint_rekeys_lone_chart_when_scale_changes_dimensions() {
        let fine = lone_chart_layout_fingerprint(40, 40, [1.0, 1.0]);
        let coarse = lone_chart_layout_fingerprint(20, 20, [1.0, 1.0]);
        assert_ne!(
            fine, coarse,
            "P2: chart dimensions must re-key even when 64² placement is unchanged"
        );
    }

    #[test]
    fn atlas_layout_fingerprint_tracks_resolved_dimensions_not_region_definition_order() {
        // Two region orderings that resolve to the same chart dimensions reach
        // this boundary as identical chart/placement data and must share a key.
        let equivalent_a = lone_chart_layout_fingerprint(20, 20, [1.0, 1.0]);
        let equivalent_b = lone_chart_layout_fingerprint(20, 20, [1.0, 1.0]);
        let different = lone_chart_layout_fingerprint(40, 40, [1.0, 1.0]);
        assert_eq!(
            equivalent_a, equivalent_b,
            "P11: equivalent resolved scale outcomes must not spuriously miss"
        );
        assert_ne!(
            equivalent_a, different,
            "P11: a changed resolved chart dimension must miss"
        );
    }

    #[test]
    fn atlas_layout_fingerprint_rekeys_lone_chart_when_uv_extent_changes() {
        // A scale edit can move chart sample positions without changing a
        // small chart's rounded dimensions or its sole 64² placement.
        let baseline = lone_chart_layout_fingerprint(20, 20, [1.0, 1.0]);
        let rescaled = lone_chart_layout_fingerprint(20, 20, [1.1, 1.0]);
        assert_ne!(
            baseline, rescaled,
            "resolved sampling extent must re-key a warm layer"
        );
    }

    #[test]
    fn geometry_slice_hash_ignores_faces_outside_influence() {
        // A light tightly bounded near the first quad must not fold the distant
        // second quad's geometry into its slice hash: editing the far quad leaves
        // the near light's key unchanged.
        let geo = two_quad_geometry();
        let (_, prims, _) = build_bvh(&geo).unwrap();
        let world = geometry_world_aabb(&geo);

        // Tight range so the influence AABB covers only the first quad
        // (x in [0,1]) and not the second (x in [3,4]).
        let near = point_light([0.5, 0.5, 0.5], 1.0);
        let slice_a = geometry_slice_hash(&near, &prims, &geo, world);

        // Move the far quad's vertices; rebuild prims/geo.
        let mut geo2 = two_quad_geometry();
        for v in geo2.geometry.vertices[4..].iter_mut() {
            v.position[1] += 2.0;
        }
        let (_, prims2, _) = build_bvh(&geo2).unwrap();
        let world2 = geometry_world_aabb(&geo2);
        let slice_b = geometry_slice_hash(&near, &prims2, &geo2, world2);

        assert_eq!(
            slice_a, slice_b,
            "editing geometry outside the influence AABB must not change the slice hash"
        );
    }

    // -----------------------------------------------------------------------
    // Cache-key behaviors at the module API seam and the full-fixture
    // determinism gate (1).

    use crate::cache::{CacheKey, StageCache};
    use std::sync::atomic::{AtomicU64, Ordering};

    fn fresh_cache_dir(label: &str) -> std::path::PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let nonce = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "postretro_lmlayer_test_{label}_{nonce}_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    /// Build one light's layer cache key the same way the production pipeline
    /// does, so the locality/round-trip tests assert on its key derivation.
    fn layer_key(
        light: &MapLight,
        shared: &SharedAtlas<'_>,
        prims: &[BvhPrimitive],
        geo: &GeometryResult,
    ) -> CacheKey {
        let h = layer_input_hash(light, shared, prims, geo, DENSITY, AREA_SAMPLES, 0);
        CacheKey::new("lightmap_layer", LAYER_FORMAT_VERSION, &h)
    }

    /// Round-trip skip: with a real `StageCache`, the first per-light bake is a
    /// miss + put, the second build serves every layer from cache (hit) and
    /// re-bakes nothing. Proves the lightmap-layer half of the
    /// "build twice → all entries hit" AC (the sh_group half lives in
    /// `cache_round_trip_hits_on_second_build`).
    #[test]
    fn layer_cache_round_trip_skips_rebake() {
        let mut geo = two_quad_geometry();
        let lights = vec![
            point_light([0.5, 1.0, 0.5], 5.0),
            point_light([3.5, 1.0, 0.5], 5.0),
        ];
        let static_lights = crate::light_namespaces::StaticBakedLights::from_lights(&lights);
        let prepared = prepare_atlas(&mut geo, &static_lights, DENSITY, &[]).unwrap();
        let (bvh, prims, _) = build_bvh(&geo).unwrap();
        let shared = SharedAtlas {
            charts: &prepared.charts,
            placements: &prepared.placements,
            atlas_width: prepared.atlas_width,
            atlas_height: prepared.atlas_height,
        };

        let dir = fresh_cache_dir("roundtrip");
        let cache = StageCache::new(&dir).expect("cache dir");

        // First build: every light is a miss, so bake + put.
        for light in &lights {
            let key = layer_key(light, &shared, &prims, &geo);
            assert!(cache.get(&key).is_none(), "first build must miss");
            let layer = bake_layer_for_test(light, &shared, &bvh, &prims, &geo, AREA_SAMPLES);
            cache.put(&key, &layer.to_bytes());
        }

        // Second build: every light hits and decodes; nothing re-bakes.
        for light in &lights {
            let key = layer_key(light, &shared, &prims, &geo);
            let bytes = cache.get(&key).expect("second build must hit");
            assert!(
                LightmapLayer::from_bytes(&bytes).is_some(),
                "cached layer must decode on the round-trip"
            );
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Single point-light edit: only the edited light's layer key changes; every
    /// other light's key is untouched (so only its layer re-bakes, the rest hit).
    /// This is the lightmap side of the "edit one point/spot light" AC.
    #[test]
    fn single_light_edit_invalidates_only_its_own_layer() {
        let mut geo = two_quad_geometry();
        // Two well-separated lights so an edit to one is provably outside the
        // other's influence slice.
        let lights = vec![
            point_light([0.5, 1.0, 0.5], 1.0),
            point_light([3.5, 1.0, 0.5], 1.0),
        ];
        let static_lights = crate::light_namespaces::StaticBakedLights::from_lights(&lights);
        let prepared = prepare_atlas(&mut geo, &static_lights, DENSITY, &[]).unwrap();
        let (_, prims, _) = build_bvh(&geo).unwrap();
        let shared = SharedAtlas {
            charts: &prepared.charts,
            placements: &prepared.placements,
            atlas_width: prepared.atlas_width,
            atlas_height: prepared.atlas_height,
        };

        let base_keys: Vec<String> = lights
            .iter()
            .map(|l| layer_key(l, &shared, &prims, &geo).as_filename())
            .collect();

        // Edit the first light's color only.
        let mut edited = lights.clone();
        edited[0].color = [0.3, 0.3, 0.3];
        let edited_keys: Vec<String> = edited
            .iter()
            .map(|l| layer_key(l, &shared, &prims, &geo).as_filename())
            .collect();

        assert_ne!(
            base_keys[0], edited_keys[0],
            "edited light's layer key must change (it re-bakes)"
        );
        assert_eq!(
            base_keys[1], edited_keys[1],
            "an untouched light's layer key must stay identical (it hits)"
        );
    }

    /// Directional-light edit: the directional's own layer key changes, and no
    /// other (point) light's layer key is affected. (The "all SH groups re-bake"
    /// half of the directional-edit AC is covered in `sh_group.rs`; SH groups
    /// fold the whole-map geometry hash and a directional reaches every group.)
    #[test]
    fn directional_light_edit_does_not_disturb_point_layers() {
        let mut geo = two_quad_geometry();
        let lights = vec![directional_light(), point_light([0.5, 1.0, 0.5], 2.0)];
        let static_lights = crate::light_namespaces::StaticBakedLights::from_lights(&lights);
        let prepared = prepare_atlas(&mut geo, &static_lights, DENSITY, &[]).unwrap();
        let (_, prims, _) = build_bvh(&geo).unwrap();
        let shared = SharedAtlas {
            charts: &prepared.charts,
            placements: &prepared.placements,
            atlas_width: prepared.atlas_width,
            atlas_height: prepared.atlas_height,
        };

        let dir_base = layer_key(&lights[0], &shared, &prims, &geo).as_filename();
        let pt_base = layer_key(&lights[1], &shared, &prims, &geo).as_filename();

        let mut edited = lights.clone();
        edited[0].color = [0.5, 0.4, 0.3]; // edit the directional only
        let dir_edited = layer_key(&edited[0], &shared, &prims, &geo).as_filename();
        let pt_edited = layer_key(&edited[1], &shared, &prims, &geo).as_filename();

        assert_ne!(
            dir_base, dir_edited,
            "edited directional layer must re-bake"
        );
        assert_eq!(
            pt_base, pt_edited,
            "a directional edit must not disturb a point light's layer key"
        );
    }

    /// Localized geometry edit: moving the far quad changes the far light's
    /// influence-bounded slice (its key — and only its key — changes), while the
    /// near light's slice is provably unchanged (its key is identical, so it
    /// hits). This exercises lightmap geometry-slice locality non-vacuously — at
    /// least one layer misses and at least one hits on the same edit.
    #[test]
    fn localized_geometry_edit_invalidates_only_overlapping_layers() {
        let geo = two_quad_geometry();
        let mut prep_geo = two_quad_geometry();
        let near = point_light([0.5, 0.5, 0.5], 1.0); // bounds the first quad only
        let far = point_light([3.5, 0.5, 0.5], 1.0); // bounds the second quad only
        let lights = vec![near.clone(), far.clone()];
        let static_lights = crate::light_namespaces::StaticBakedLights::from_lights(&lights);
        let prepared = prepare_atlas(&mut prep_geo, &static_lights, DENSITY, &[]).unwrap();
        let (_, prims, _) = build_bvh(&geo).unwrap();
        let shared = SharedAtlas {
            charts: &prepared.charts,
            placements: &prepared.placements,
            atlas_width: prepared.atlas_width,
            atlas_height: prepared.atlas_height,
        };

        let near_base = layer_key(&near, &shared, &prims, &geo).as_filename();
        let far_base = layer_key(&far, &shared, &prims, &geo).as_filename();

        // Edit the FAR quad's geometry (vertices 4..8).
        let mut geo2 = two_quad_geometry();
        for v in geo2.geometry.vertices[4..].iter_mut() {
            v.position[1] += 0.5;
        }
        let (_, prims2, _) = build_bvh(&geo2).unwrap();

        let near_edited = layer_key(&near, &shared, &prims2, &geo2).as_filename();
        let far_edited = layer_key(&far, &shared, &prims2, &geo2).as_filename();

        assert_eq!(
            near_base, near_edited,
            "near light's slice is outside the edit — its layer must hit"
        );
        assert_ne!(
            far_base, far_edited,
            "far light's slice covers the edited quad — its layer must re-bake"
        );
    }

    /// Corruption recovery at the real `StageCache`: a stored layer overwritten
    /// with garbage is detected (length/hash fail → `get` returns `None`, or a
    /// format-skewed payload → `from_bytes` returns `None`), so the caller
    /// re-bakes and the rebuilt layer is byte-identical to the original.
    #[test]
    fn corrupt_layer_entry_is_discarded_and_rebaked() {
        let mut geo = two_quad_geometry();
        let lights = vec![point_light([0.5, 1.0, 0.5], 5.0)];
        let static_lights = crate::light_namespaces::StaticBakedLights::from_lights(&lights);
        let prepared = prepare_atlas(&mut geo, &static_lights, DENSITY, &[]).unwrap();
        let (bvh, prims, _) = build_bvh(&geo).unwrap();
        let shared = SharedAtlas {
            charts: &prepared.charts,
            placements: &prepared.placements,
            atlas_width: prepared.atlas_width,
            atlas_height: prepared.atlas_height,
        };

        let dir = fresh_cache_dir("corrupt");
        let cache = StageCache::new(&dir).expect("cache dir");
        let key = layer_key(&lights[0], &shared, &prims, &geo);

        let original = bake_layer_for_test(&lights[0], &shared, &bvh, &prims, &geo, AREA_SAMPLES);
        cache.put(&key, &original.to_bytes());

        // Corrupt every file in the cache dir.
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_file() {
                std::fs::write(&path, b"corrupt").unwrap();
            }
        }

        // The cache rejects it (returns None), so the pipeline re-bakes.
        let recovered = match cache
            .get(&key)
            .and_then(|bytes| LightmapLayer::from_bytes(&bytes))
        {
            Some(layer) => layer,
            None => bake_layer_for_test(&lights[0], &shared, &bvh, &prims, &geo, AREA_SAMPLES),
        };
        assert_eq!(
            original, recovered,
            "re-bake after corruption must reproduce the original layer"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn malformed_partition_cache_hits_soft_miss_before_warm_fold() {
        let mut geo = two_quad_geometry();
        let lights = vec![point_light([0.5, 1.0, 0.5], 5.0)];
        let static_lights = crate::light_namespaces::StaticBakedLights::from_lights(&lights);
        let prepared = prepare_atlas(&mut geo, &static_lights, DENSITY, &[]).unwrap();
        let (bvh, prims, _) = build_bvh(&geo).unwrap();
        let shared = SharedAtlas {
            charts: &prepared.charts,
            placements: &prepared.placements,
            atlas_width: prepared.atlas_width,
            atlas_height: prepared.atlas_height,
        };
        let control = BakeControl::unrestricted();
        let expected = bake_light_layer_controlled(
            &lights[0],
            &shared,
            &bvh,
            &prims,
            &geo,
            0,
            AREA_SAMPLES,
            &control,
        );
        assert!(expected.texels.len() > 1, "fixture needs multiple texels");
        let key = layer_key(&lights[0], &shared, &prims, &geo);
        let dir = fresh_cache_dir("malformed_partition_soft_miss");
        let cache = StageCache::new(&dir).expect("cache dir");

        for malformed in [
            {
                let mut layer = expected.clone();
                layer.texels[0].idx = 0;
                layer
            },
            {
                let mut layer = expected.clone();
                layer.texels[1] = layer.texels[0];
                layer
            },
            {
                let mut layer = expected.clone();
                layer.texels.last_mut().unwrap().idx = shared.atlas_width * shared.atlas_height;
                layer
            },
        ] {
            cache.put(&key, &malformed.to_bytes());
            let recovered = cache
                .get(&key)
                .and_then(|bytes| LightmapLayer::from_bytes(&bytes))
                .and_then(|partition| {
                    validate_layer_partition(&partition, &shared, 0)
                        .ok()
                        .map(|()| partition)
                })
                .unwrap_or_else(|| {
                    bake_light_layer_controlled(
                        &lights[0],
                        &shared,
                        &bvh,
                        &prims,
                        &geo,
                        0,
                        AREA_SAMPLES,
                        &control,
                    )
                });
            assert_eq!(
                recovered, expected,
                "malformed decoded payload must degrade to the exact re-bake"
            );
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `--cache-dir` redirect (module-level): a `StageCache` opened on an
    /// override directory writes its layer entries there, under that path —
    /// nothing lands in the default `.build-caches/prl-cache/`.
    #[test]
    fn cache_dir_override_places_entries_under_override() {
        let mut geo = two_quad_geometry();
        let lights = vec![point_light([0.5, 1.0, 0.5], 5.0)];
        let static_lights = crate::light_namespaces::StaticBakedLights::from_lights(&lights);
        let prepared = prepare_atlas(&mut geo, &static_lights, DENSITY, &[]).unwrap();
        let (bvh, prims, _) = build_bvh(&geo).unwrap();
        let shared = SharedAtlas {
            charts: &prepared.charts,
            placements: &prepared.placements,
            atlas_width: prepared.atlas_width,
            atlas_height: prepared.atlas_height,
        };

        let dir = fresh_cache_dir("override");
        let cache = StageCache::new(&dir).expect("cache dir");
        let key = layer_key(&lights[0], &shared, &prims, &geo);
        let layer = bake_layer_for_test(&lights[0], &shared, &bvh, &prims, &geo, AREA_SAMPLES);
        cache.put(&key, &layer.to_bytes());

        let entry = dir.join(key.as_filename());
        assert!(
            entry.is_file(),
            "layer entry must land under the override dir: {}",
            entry.display()
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Determinism GATE (1): the warm per-light composite is byte-identical to
    /// the monolithic `bake_face_chart` atlas (pre-BC6H `CompositedAtlas`) on
    /// every `GATE_FIXTURES` entry. The synthetic
    /// `composite_matches_monolithic_atlas_bit_for_bit` proves the mechanism;
    /// this runs the real fixtures through both paths.
    ///
    /// `#[ignore]` because each fixture bake casts the full per-texel ray load
    /// (roughly a minute or two across the gate fixture set). Run manually:
    ///   cargo test -p postretro-level-compiler -- --ignored --nocapture \
    ///       lightmap_composite_equals_monolithic_on_fixtures
    #[test]
    #[ignore = "full-fixture lightmap bake; run with --ignored"]
    fn lightmap_composite_equals_monolithic_on_fixtures() {
        use crate::fixture_pipeline::{GATE_FIXTURES, load_fixture};

        for &name in GATE_FIXTURES {
            let fx = load_fixture(name);
            // Match the pipeline's direct-lightmap light set: global static order,
            // Sdf-shadow lights dropped (their direct term resolves at runtime).
            let static_lights = crate::light_namespaces::StaticBakedLights::from_lights(&fx.lights);
            let layer_lights: Vec<&MapLight> = static_lights
                .entries()
                .iter()
                .map(|e| e.light)
                .filter(|l| l.shadow_type != crate::map_data::ShadowType::Sdf)
                .collect();
            if layer_lights.is_empty() {
                // Placeholder path; no atlas to compare. Skip (still meaningful
                // for the other fixtures).
                continue;
            }

            // Two geometry clones so each path's prepare_atlas mutates its own.
            let mut mono_geo = fx.geometry.clone();
            let mut layer_geo = fx.geometry.clone();
            let density = crate::lightmap_bake::DEFAULT_TEXEL_DENSITY_METERS;

            let mono_prepared = prepare_atlas(&mut mono_geo, &static_lights, density, &[]).unwrap();
            if mono_prepared.placements.is_empty() {
                continue;
            }
            let (mono_bvh, mono_prims, _) = build_bvh(&mono_geo).unwrap();
            let mono_atlas = bake_monolithic_atlas(
                &mono_bvh,
                &mono_prims,
                &mono_geo,
                &layer_lights,
                &mono_prepared.charts,
                &mono_prepared.placements,
                mono_prepared.atlas_width,
                mono_prepared.atlas_height,
                mono_prepared.layer_count,
                AREA_SAMPLES,
            );

            let layer_prepared =
                prepare_atlas(&mut layer_geo, &static_lights, density, &[]).unwrap();
            let (layer_bvh, layer_prims, _) = build_bvh(&layer_geo).unwrap();
            let shared = SharedAtlas {
                charts: &layer_prepared.charts,
                placements: &layer_prepared.placements,
                atlas_width: layer_prepared.atlas_width,
                atlas_height: layer_prepared.atlas_height,
            };
            let layers: Vec<Vec<LightmapLayer>> = layer_lights
                .iter()
                .map(|light| {
                    bake_light_layer(
                        light,
                        &shared,
                        &layer_bvh,
                        &layer_prims,
                        &layer_geo,
                        AREA_SAMPLES,
                        &BakeControl::unrestricted(),
                    )
                })
                .collect();
            let light_layers: Vec<_> = layer_lights
                .iter()
                .zip(&layers)
                .flat_map(|(&light, partitions)| {
                    partitions.iter().map(move |partition| (light, partition))
                })
                .collect();
            let mut composite = composite_layers(&light_layers, &shared);
            composite.dilate();

            assert_eq!(
                mono_atlas, composite,
                "fixture {name}: per-light composite must equal the monolithic atlas bit-for-bit"
            );
        }
    }

    // -----------------------------------------------------------------------
    // Second-level "lightmap_section" cache behaviors, mirroring the layer
    // suite above. These protect the warm no-edit rebuild (section hit → one
    // decode, no layer reads / composite / encode) and the section-key
    // invalidation coupling that the production pipeline relies on.

    use postretro_level_format::lightmap::LightmapSection;

    /// Build the section cache key the same way the `pipeline.rs` warm path does:
    /// fold the ordered per-light `layer_input_hash` set through
    /// `section_input_hash`, then `CacheKey::new("lightmap_section", ...)`.
    /// Mirrors the existing `layer_key` helper so the section tests assert on
    /// the production key derivation, not a test-only reimplementation.
    fn section_key(
        layer_input_hashes: &[[u8; 32]],
        shared: &SharedAtlas<'_>,
        density: f32,
        uncompressed_irradiance: bool,
    ) -> CacheKey {
        let h = section_input_hash(
            layer_input_hashes,
            shared,
            density,
            uncompressed_irradiance,
            crate::lightmap_bake::DIRECTION_TEXEL_SCALE,
        );
        CacheKey::new("lightmap_section", LIGHTMAP_SECTION_VERSION, &h)
    }

    #[test]
    fn direction_texel_scale_rekeys_section_without_rekeying_light_layers() {
        // The layer hash represents an already-baked full-resolution light
        // contribution. Direction coarsening occurs only during section encode,
        // so the two builds must retain this exact layer key while their
        // composited-section keys differ.
        let layer_input_hashes = [[0x5a; 32]];
        let charts = [Chart {
            origin: Vec3::ZERO,
            u_axis: Vec3::X,
            v_axis: Vec3::Z,
            uv_min: [0.0, 0.0],
            uv_extent: [1.0, 1.0],
            normal: Vec3::Y,
            width_texels: 5,
            height_texels: 5,
            leaf_index: 0,
        }];
        let placements = [ChartPlacement {
            x: 0,
            y: 0,
            layer: 0,
        }];
        let shared = SharedAtlas {
            charts: &charts,
            placements: &placements,
            atlas_width: 64,
            atlas_height: 64,
        };
        let at_scale_two = CacheKey::new(
            "lightmap_section",
            LIGHTMAP_SECTION_VERSION,
            &section_input_hash(&layer_input_hashes, &shared, DENSITY, true, 2),
        );
        let at_scale_one = CacheKey::new(
            "lightmap_section",
            LIGHTMAP_SECTION_VERSION,
            &section_input_hash(&layer_input_hashes, &shared, DENSITY, true, 1),
        );
        assert_ne!(
            at_scale_two.as_filename(),
            at_scale_one.as_filename(),
            "changing only direction scale must re-key the warm section memo"
        );

        let dir = fresh_cache_dir("section_direction_scale");
        let cache = StageCache::new(&dir).expect("cache dir");
        cache.put(&at_scale_two, b"scale-two-section");
        assert!(
            cache.get(&at_scale_one).is_none(),
            "a factor-1 rebuild must miss a factor-2 section memo"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn all_sdf_section_cache_rekeys_when_prepared_dimensions_change() {
        // Regression: the empty layer-hash list previously let an all-Sdf
        // fallback from prepared layout A alias layout B even though its bytes
        // are dimension-dependent.
        let charts = [Chart {
            origin: Vec3::ZERO,
            u_axis: Vec3::X,
            v_axis: Vec3::Z,
            uv_min: [0.0, 0.0],
            uv_extent: [1.0, 1.0],
            normal: Vec3::Y,
            width_texels: 5,
            height_texels: 5,
            leaf_index: 0,
        }];
        let placements = [ChartPlacement {
            x: 0,
            y: 0,
            layer: 0,
        }];
        let layout_a = SharedAtlas {
            charts: &charts,
            placements: &placements,
            atlas_width: 64,
            atlas_height: 64,
        };
        let layout_b = SharedAtlas {
            charts: &charts,
            placements: &placements,
            atlas_width: 128,
            atlas_height: 64,
        };
        let key_a = section_key(&[], &layout_a, DENSITY, true);
        let key_b = section_key(&[], &layout_b, DENSITY, true);
        assert_ne!(key_a.as_filename(), key_b.as_filename());

        let dir = fresh_cache_dir("all_sdf_layout_change");
        let cache = StageCache::new(&dir).expect("cache dir");
        cache.put(&key_a, b"layout-a-section");
        assert!(
            cache.get(&key_b).is_none(),
            "layout B must miss a fallback section memo written for layout A"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Exercise the production warm fold: for each atlas layer, bake/load one
    /// light partition at a time in global light order, fold it, then
    /// dilate/encode/append that plane before advancing to the next one.
    fn compose_section(
        lights: &[&MapLight],
        shared: &SharedAtlas<'_>,
        bvh: &bvh::bvh::Bvh<f32, 3>,
        prims: &[BvhPrimitive],
        geo: &GeometryResult,
    ) -> LightmapSection {
        compose_section_with_format(lights, shared, bvh, prims, geo, true)
    }

    #[allow(clippy::too_many_arguments)]
    fn compose_section_with_format(
        lights: &[&MapLight],
        shared: &SharedAtlas<'_>,
        bvh: &bvh::bvh::Bvh<f32, 3>,
        prims: &[BvhPrimitive],
        geo: &GeometryResult,
        uncompressed_irradiance: bool,
    ) -> LightmapSection {
        if lights.is_empty() {
            let mut fallback = empty_composite(shared.atlas_width, shared.atlas_height);
            fallback.dilate();
            return fallback.encode_section(
                DENSITY,
                uncompressed_irradiance,
                crate::lightmap_bake::DIRECTION_TEXEL_SCALE,
            );
        }

        let control = BakeControl::unrestricted();
        let mut irradiance = Vec::new();
        let mut direction = Vec::new();
        for target_layer in 0..atlas_layer_count(shared) {
            let mut accumulator =
                IncrementalLayerAccumulator::for_atlas_layer(shared, target_layer);
            for light in lights {
                let partition = bake_light_layer_controlled(
                    light,
                    shared,
                    bvh,
                    prims,
                    geo,
                    target_layer,
                    AREA_SAMPLES,
                    &control,
                );
                accumulator.fold_partition(light, &partition, shared);
            }
            let mut plane = accumulator.finish();
            plane.dilate();
            let (mut plane_irradiance, mut plane_direction) =
                crate::lightmap_bake::encode_atlas_layer(
                    &plane,
                    uncompressed_irradiance,
                    crate::lightmap_bake::DIRECTION_TEXEL_SCALE,
                );
            irradiance.append(&mut plane_irradiance);
            direction.append(&mut plane_direction);
        }
        crate::lightmap_bake::assemble_layered_section(
            shared.atlas_width,
            shared.atlas_height,
            atlas_layer_count(shared),
            DENSITY,
            uncompressed_irradiance,
            crate::lightmap_bake::DIRECTION_TEXEL_SCALE,
            irradiance,
            direction,
        )
    }

    /// Compute the filtered direct-lightmap light set + their ordered
    /// `layer_input_hash`es exactly as the warm path does (global static order,
    /// `Sdf` dropped). Returned alongside the light refs so a test can key the
    /// section and bake from the same ordering.
    fn layer_input_hashes(
        light_refs: &[&MapLight],
        shared: &SharedAtlas<'_>,
        prims: &[BvhPrimitive],
        geo: &GeometryResult,
    ) -> Vec<[u8; 32]> {
        (0..atlas_layer_count(shared))
            .flat_map(|target_layer| {
                light_refs.iter().map(move |light| {
                    layer_input_hash(
                        light,
                        shared,
                        prims,
                        geo,
                        DENSITY,
                        AREA_SAMPLES,
                        target_layer,
                    )
                })
            })
            .collect()
    }

    /// Round-trip skip (section level): with a real `StageCache`, the first
    /// build is a section miss → recompose → `put`; the second build serves the
    /// section from cache (`get` + `from_bytes`) and reads NO per-light layer
    /// blob. Asserts the cached section decodes, equals the original, and the
    /// section key is identical across builds. Section-level mirror of
    /// `layer_cache_round_trip_skips_rebake`.
    #[test]
    fn section_cache_round_trip_skips_recompose() {
        let mut geo = two_quad_geometry();
        let lights = vec![
            point_light([0.5, 1.0, 0.5], 5.0),
            point_light([3.5, 1.0, 0.5], 5.0),
            directional_light(),
        ];
        let static_lights = crate::light_namespaces::StaticBakedLights::from_lights(&lights);
        let light_refs: Vec<&MapLight> = static_lights.entries().iter().map(|e| e.light).collect();
        let prepared = prepare_atlas(&mut geo, &static_lights, DENSITY, &[]).unwrap();
        let (bvh, prims, _) = build_bvh(&geo).unwrap();
        let shared = SharedAtlas {
            charts: &prepared.charts,
            placements: &prepared.placements,
            atlas_width: prepared.atlas_width,
            atlas_height: prepared.atlas_height,
        };

        let hashes = layer_input_hashes(&light_refs, &shared, &prims, &geo);
        let key_build1 = section_key(&hashes, &shared, DENSITY, true);

        let dir = fresh_cache_dir("section_roundtrip");
        let cache = StageCache::new(&dir).expect("cache dir");

        // First build: section miss → recompose → put.
        assert!(
            cache.get(&key_build1).is_none(),
            "first build must miss the section key"
        );
        let composed = compose_section(&light_refs, &shared, &bvh, &prims, &geo);
        cache.put(&key_build1, &composed.to_bytes());

        // Second build: same key, section hits and decodes — no layer blob read.
        let key_build2 = section_key(&hashes, &shared, DENSITY, true);
        assert_eq!(
            key_build1.as_filename(),
            key_build2.as_filename(),
            "an unchanged build must derive the identical section key"
        );
        let bytes = cache
            .get(&key_build2)
            .expect("second build must hit the section");
        let decoded = LightmapSection::from_bytes(&bytes).expect("cached section must decode");
        assert_eq!(
            decoded, composed,
            "the hit-path section must equal the originally composited section"
        );

        // No layer blob was ever written: only the section entry exists on disk.
        let layer_key0 = CacheKey::new("lightmap_layer", LAYER_FORMAT_VERSION, &hashes[0]);
        assert!(
            cache.get(&layer_key0).is_none(),
            "the section round-trip must not read or write any per-light layer blob"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cached_section_validation_covers_current_atlas_and_encode_config() {
        let mut geo = two_quad_geometry();
        let lights = vec![point_light([0.5, 1.0, 0.5], 5.0)];
        let static_lights = crate::light_namespaces::StaticBakedLights::from_lights(&lights);
        let light_refs: Vec<&MapLight> = static_lights.entries().iter().map(|e| e.light).collect();
        let prepared = prepare_atlas(&mut geo, &static_lights, DENSITY, &[]).unwrap();
        let (bvh, prims, _) = build_bvh(&geo).unwrap();
        let shared = SharedAtlas {
            charts: &prepared.charts,
            placements: &prepared.placements,
            atlas_width: prepared.atlas_width,
            atlas_height: prepared.atlas_height,
        };
        let expected = compose_section(&light_refs, &shared, &bvh, &prims, &geo);
        let validate = |section: &LightmapSection, direction_scale| {
            validate_cached_lightmap_section(
                section,
                &shared,
                prepared.layer_count,
                DENSITY,
                true,
                direction_scale,
            )
        };
        validate(&expected, crate::lightmap_bake::DIRECTION_TEXEL_SCALE)
            .expect("freshly composed section matches current inputs");

        let mut stale = expected.clone();
        stale.irr_width *= 2;
        assert!(validate(&stale, crate::lightmap_bake::DIRECTION_TEXEL_SCALE).is_err());
        let mut stale = expected.clone();
        stale.layer_count += 1;
        assert!(validate(&stale, crate::lightmap_bake::DIRECTION_TEXEL_SCALE).is_err());
        let mut stale = expected.clone();
        stale.irr_texel_density *= 2.0;
        assert!(validate(&stale, crate::lightmap_bake::DIRECTION_TEXEL_SCALE).is_err());
        let mut stale = expected.clone();
        stale.irradiance_format = postretro_level_format::lightmap::IRRADIANCE_FORMAT_BC6H;
        assert!(validate(&stale, crate::lightmap_bake::DIRECTION_TEXEL_SCALE).is_err());
        assert!(validate(&expected, 1).is_err(), "direction scale is config");
        let mut stale = expected.clone();
        stale.dir_width *= 2;
        assert!(validate(&stale, crate::lightmap_bake::DIRECTION_TEXEL_SCALE).is_err());
        let mut stale = expected.clone();
        stale.dir_texel_density *= 2.0;
        assert!(validate(&stale, crate::lightmap_bake::DIRECTION_TEXEL_SCALE).is_err());
        let mut stale = expected.clone();
        stale.direction_format = postretro_level_format::lightmap::DIRECTION_FORMAT_OCT_RGBA8;
        assert!(validate(&stale, crate::lightmap_bake::DIRECTION_TEXEL_SCALE).is_err());
        let mut stale = expected.clone();
        stale.mode = postretro_level_format::lightmap::LightmapMode::Unshadowed;
        assert!(validate(&stale, crate::lightmap_bake::DIRECTION_TEXEL_SCALE).is_err());
    }

    #[test]
    fn mismatched_decoded_section_cache_hit_soft_misses_and_recomposes() {
        let mut geo = two_quad_geometry();
        let lights = vec![point_light([0.5, 1.0, 0.5], 5.0)];
        let static_lights = crate::light_namespaces::StaticBakedLights::from_lights(&lights);
        let light_refs: Vec<&MapLight> = static_lights.entries().iter().map(|e| e.light).collect();
        let prepared = prepare_atlas(&mut geo, &static_lights, DENSITY, &[]).unwrap();
        let (bvh, prims, _) = build_bvh(&geo).unwrap();
        let shared = SharedAtlas {
            charts: &prepared.charts,
            placements: &prepared.placements,
            atlas_width: prepared.atlas_width,
            atlas_height: prepared.atlas_height,
        };
        let hashes = layer_input_hashes(&light_refs, &shared, &prims, &geo);
        let key = section_key(&hashes, &shared, DENSITY, true);
        let expected = compose_section(&light_refs, &shared, &bvh, &prims, &geo);
        let mut stale = expected.clone();
        stale.irr_texel_density *= 2.0;

        let dir = fresh_cache_dir("section_metadata_soft_miss");
        let cache = StageCache::new(&dir).expect("cache dir");
        cache.put(&key, &stale.to_bytes());
        let recovered = cache
            .get(&key)
            .and_then(|bytes| LightmapSection::from_bytes(&bytes).ok())
            .and_then(|section| {
                validate_cached_lightmap_section(
                    &section,
                    &shared,
                    prepared.layer_count,
                    DENSITY,
                    true,
                    crate::lightmap_bake::DIRECTION_TEXEL_SCALE,
                )
                .ok()
                .map(|()| section)
            })
            .unwrap_or_else(|| compose_section(&light_refs, &shared, &bvh, &prims, &geo));
        assert_eq!(recovered, expected);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Single-light edit: editing one light changes its `layer_input_hash`,
    /// hence the section key (a miss → recompose), while every OTHER light's
    /// layer key is unchanged (those layers still hit). Section-level proxy for
    /// the plan's primary correctness criterion. Mirror of
    /// `single_light_edit_invalidates_only_its_own_layer`.
    #[test]
    fn single_light_edit_changes_section_key_but_not_unedited_layer_key() {
        let mut geo = two_quad_geometry();
        // Well-separated lights so the edit to one is outside the other's slice.
        let lights = vec![
            point_light([0.5, 1.0, 0.5], 1.0),
            point_light([3.5, 1.0, 0.5], 1.0),
        ];
        let static_lights = crate::light_namespaces::StaticBakedLights::from_lights(&lights);
        let light_refs: Vec<&MapLight> = static_lights.entries().iter().map(|e| e.light).collect();
        let prepared = prepare_atlas(&mut geo, &static_lights, DENSITY, &[]).unwrap();
        let (_, prims, _) = build_bvh(&geo).unwrap();
        let shared = SharedAtlas {
            charts: &prepared.charts,
            placements: &prepared.placements,
            atlas_width: prepared.atlas_width,
            atlas_height: prepared.atlas_height,
        };

        let base_hashes = layer_input_hashes(&light_refs, &shared, &prims, &geo);
        let base_section = section_key(&base_hashes, &shared, DENSITY, true).as_filename();
        let base_layer1 =
            CacheKey::new("lightmap_layer", LAYER_FORMAT_VERSION, &base_hashes[1]).as_filename();

        // Edit only the first light's color.
        let mut edited = lights.clone();
        edited[0].color = [0.3, 0.3, 0.3];
        let edited_static = crate::light_namespaces::StaticBakedLights::from_lights(&edited);
        let edited_refs: Vec<&MapLight> = edited_static.entries().iter().map(|e| e.light).collect();
        let edited_hashes = layer_input_hashes(&edited_refs, &shared, &prims, &geo);
        let edited_section = section_key(&edited_hashes, &shared, DENSITY, true).as_filename();
        let edited_layer1 =
            CacheKey::new("lightmap_layer", LAYER_FORMAT_VERSION, &edited_hashes[1]).as_filename();

        assert_ne!(
            base_section, edited_section,
            "editing a light must change the section key (section miss → recompose)"
        );
        assert_eq!(
            base_layer1, edited_layer1,
            "an unedited light's layer key must be unchanged (it still hits)"
        );
    }

    /// Corruption recovery (section level): a present-but-undecodable
    /// `lightmap_section` entry — written through the cache so its length/hash
    /// check passes, but whose bytes `LightmapSection::from_bytes` rejects — is
    /// treated as a miss, so the pipeline recomposes. Asserts the recovered
    /// section equals the originally-composited one. Mirror of
    /// `corrupt_layer_entry_is_discarded_and_rebaked`, exercising the
    /// `get`-succeeds-but-`from_bytes`-fails branch guarded by `pipeline.rs`.
    #[test]
    fn corrupt_section_entry_is_discarded_and_recomposed() {
        let mut geo = two_quad_geometry();
        let lights = vec![
            point_light([0.5, 1.0, 0.5], 5.0),
            point_light([3.5, 1.0, 0.5], 5.0),
        ];
        let static_lights = crate::light_namespaces::StaticBakedLights::from_lights(&lights);
        let light_refs: Vec<&MapLight> = static_lights.entries().iter().map(|e| e.light).collect();
        let prepared = prepare_atlas(&mut geo, &static_lights, DENSITY, &[]).unwrap();
        let (bvh, prims, _) = build_bvh(&geo).unwrap();
        let shared = SharedAtlas {
            charts: &prepared.charts,
            placements: &prepared.placements,
            atlas_width: prepared.atlas_width,
            atlas_height: prepared.atlas_height,
        };

        let hashes = layer_input_hashes(&light_refs, &shared, &prims, &geo);
        let key = section_key(&hashes, &shared, DENSITY, true);
        let original = compose_section(&light_refs, &shared, &bvh, &prims, &geo);

        let dir = fresh_cache_dir("section_corrupt");
        let cache = StageCache::new(&dir).expect("cache dir");
        // Store a blob that PASSES the cache's length/hash check (it is `put`
        // through the cache) but is too short for `LightmapSection::from_bytes`
        // to parse — the exact format skew the pipeline recovers from.
        cache.put(&key, b"not a valid lightmap section blob");

        let recovered = match cache
            .get(&key)
            .and_then(|bytes| LightmapSection::from_bytes(&bytes).ok())
        {
            Some(section) => section,
            None => compose_section(&light_refs, &shared, &bvh, &prims, &geo),
        };
        assert_eq!(
            original, recovered,
            "recompose after a corrupt section entry must reproduce the original section"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `--cache-dir` redirect (section level): a `StageCache` opened on an
    /// override directory writes the `lightmap_section` entry under that path.
    /// Mirror of `cache_dir_override_places_entries_under_override`.
    #[test]
    fn cache_dir_override_places_section_entry_under_override() {
        let mut geo = two_quad_geometry();
        let lights = vec![point_light([0.5, 1.0, 0.5], 5.0)];
        let static_lights = crate::light_namespaces::StaticBakedLights::from_lights(&lights);
        let light_refs: Vec<&MapLight> = static_lights.entries().iter().map(|e| e.light).collect();
        let prepared = prepare_atlas(&mut geo, &static_lights, DENSITY, &[]).unwrap();
        let (bvh, prims, _) = build_bvh(&geo).unwrap();
        let shared = SharedAtlas {
            charts: &prepared.charts,
            placements: &prepared.placements,
            atlas_width: prepared.atlas_width,
            atlas_height: prepared.atlas_height,
        };

        let hashes = layer_input_hashes(&light_refs, &shared, &prims, &geo);
        let key = section_key(&hashes, &shared, DENSITY, true);
        let section = compose_section(&light_refs, &shared, &bvh, &prims, &geo);

        let dir = fresh_cache_dir("section_override");
        let cache = StageCache::new(&dir).expect("cache dir");
        cache.put(&key, &section.to_bytes());

        let entry = dir.join(key.as_filename());
        assert!(
            entry.is_file(),
            "section entry must land under the override dir: {}",
            entry.display()
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `--no-cache` bypass (section level): the section key is only ever
    /// consulted when a `StageCache` exists. With no cache, the warm branch is
    /// not entered at all (`pipeline.rs` gates the section-cache block behind
    /// `if let Some(ref cache) = stage_cache`), so no section entry is created.
    /// A clean unit assertion for "no cache → no entry" is to open a cache dir,
    /// derive the key WITHOUT putting anything (modeling the no-cache control
    /// flow that never reaches `put`), and confirm the entry does not exist.
    #[test]
    fn no_cache_path_writes_no_section_entry() {
        let mut geo = two_quad_geometry();
        let lights = vec![point_light([0.5, 1.0, 0.5], 5.0)];
        let static_lights = crate::light_namespaces::StaticBakedLights::from_lights(&lights);
        let light_refs: Vec<&MapLight> = static_lights.entries().iter().map(|e| e.light).collect();
        let prepared = prepare_atlas(&mut geo, &static_lights, DENSITY, &[]).unwrap();
        let (_, prims, _) = build_bvh(&geo).unwrap();
        let shared = SharedAtlas {
            charts: &prepared.charts,
            placements: &prepared.placements,
            atlas_width: prepared.atlas_width,
            atlas_height: prepared.atlas_height,
        };

        let hashes = layer_input_hashes(&light_refs, &shared, &prims, &geo);
        let key = section_key(&hashes, &shared, DENSITY, true);

        // No-cache control flow: the key is derivable, but with the section-cache
        // block skipped nothing is ever `put`. Model that by never calling `put`.
        let dir = fresh_cache_dir("section_nocache");
        let cache = StageCache::new(&dir).expect("cache dir");
        let entry = dir.join(key.as_filename());
        assert!(
            !entry.is_file(),
            "no section entry may exist when the warm path never puts one"
        );
        assert!(
            cache.get(&key).is_none(),
            "a section key must miss when no entry was written"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Add / remove / reorder invalidation: each mutation changes the folded
    /// `layer_input_hash` set (its length and/or order), so the section key
    /// changes versus the two-light baseline. Reorder is the subtle case — the
    /// fold concatenates the hashes in order, so the same two hashes in swapped
    /// order must still yield a different key.
    #[test]
    fn section_key_changes_on_add_remove_and_reorder() {
        let mut geo = two_quad_geometry();
        let lights = vec![
            point_light([0.5, 1.0, 0.5], 5.0),
            point_light([3.5, 1.0, 0.5], 5.0),
        ];
        let static_lights = crate::light_namespaces::StaticBakedLights::from_lights(&lights);
        let light_refs: Vec<&MapLight> = static_lights.entries().iter().map(|e| e.light).collect();
        let prepared = prepare_atlas(&mut geo, &static_lights, DENSITY, &[]).unwrap();
        let (_, prims, _) = build_bvh(&geo).unwrap();
        let shared = SharedAtlas {
            charts: &prepared.charts,
            placements: &prepared.placements,
            atlas_width: prepared.atlas_width,
            atlas_height: prepared.atlas_height,
        };

        let base = layer_input_hashes(&light_refs, &shared, &prims, &geo);
        let base_key = section_key(&base, &shared, DENSITY, true).as_filename();

        // Add a light: a third hash extends the fold.
        let mut added = base.clone();
        added.push(layer_input_hash(
            &point_light([2.0, 1.0, 0.5], 5.0),
            &shared,
            &prims,
            &geo,
            DENSITY,
            AREA_SAMPLES,
            0,
        ));
        assert_ne!(
            base_key,
            section_key(&added, &shared, DENSITY, true).as_filename(),
            "adding a light must change the section key"
        );

        // Remove a light: drop the second hash.
        let removed = vec![base[0]];
        assert_ne!(
            base_key,
            section_key(&removed, &shared, DENSITY, true).as_filename(),
            "removing a light must change the section key"
        );

        // Reorder: swap the two hashes. Same set, different concatenation order.
        let reordered = vec![base[1], base[0]];
        assert_ne!(
            base_key,
            section_key(&reordered, &shared, DENSITY, true).as_filename(),
            "reordering lights must change the section key (the fold is ordered)"
        );
        // Guard against an accidental commutative fold: the swap must NOT be a
        // no-op even though the hash multiset is identical.
        assert_ne!(
            base[0], base[1],
            "the two lights must have distinct layer hashes for the reorder check to bite"
        );
    }

    /// `--soft-shadow-samples` change → section miss: the sample count folds
    /// into every `layer_input_hash` (as `area_sample_count`), hence into the
    /// section key. Two different sample counts must yield different keys.
    #[test]
    fn section_key_changes_when_soft_shadow_samples_change() {
        let mut geo = two_quad_geometry();
        let lights = vec![point_light([0.5, 1.0, 0.5], 5.0)];
        let static_lights = crate::light_namespaces::StaticBakedLights::from_lights(&lights);
        let light_refs: Vec<&MapLight> = static_lights.entries().iter().map(|e| e.light).collect();
        let prepared = prepare_atlas(&mut geo, &static_lights, DENSITY, &[]).unwrap();
        let (_, prims, _) = build_bvh(&geo).unwrap();
        let shared = SharedAtlas {
            charts: &prepared.charts,
            placements: &prepared.placements,
            atlas_width: prepared.atlas_width,
            atlas_height: prepared.atlas_height,
        };

        let samples_a = 16u32;
        let samples_b = 32u32;
        let hashes_a: Vec<[u8; 32]> = light_refs
            .iter()
            .map(|l| layer_input_hash(l, &shared, &prims, &geo, DENSITY, samples_a, 0))
            .collect();
        let hashes_b: Vec<[u8; 32]> = light_refs
            .iter()
            .map(|l| layer_input_hash(l, &shared, &prims, &geo, DENSITY, samples_b, 0))
            .collect();

        assert_ne!(
            section_key(&hashes_a, &shared, DENSITY, true).as_filename(),
            section_key(&hashes_b, &shared, DENSITY, true).as_filename(),
            "changing --soft-shadow-samples must change the section key (section miss)"
        );
    }
}
