// Per-light lightmap contribution layers: cache, then composite back into a
// byte-identical pre-BC6H atlas.
// See: context/lib/build_pipeline.md (Lightmap id 22)

use glam::Vec3;
#[cfg(test)]
use rayon::prelude::*;

use crate::affinity_grid::{AABB_PADDING_METERS, light_aabb};
#[cfg(test)]
use crate::bake_control::BakeControl;
#[cfg(test)]
use crate::bvh_build::BvhPrimitive;
use crate::chart_raster::{ChartPlacement, chart_interior_dims, chart_texel_world_position};
use crate::geometry::GeometryResult;
#[cfg(test)]
use crate::lightmap_bake::light_texel_is_covered;
use crate::lightmap_bake::{
    BlockLayout, Chart, CompositedAtlas, light_contribution_and_direction,
    light_texel_contribution_and_visibility_with,
};
use crate::map_data::{LightType, MapLight};
use crate::texel_footprint::TexelSite;
#[cfg(test)]
use crate::texel_footprint::{ProbeEarlyOut, has_embedded_emitter};
use glam::DVec3;

mod cache_keys;
mod chart_cull;
mod chart_walk;
mod shaft_cull;
#[cfg(test)]
mod skip_stats;

pub(crate) use cache_keys::atlas_layout_fingerprint;
pub use cache_keys::{
    LayerKeyContext, layer_input_hash, section_input_hash, validate_cached_lightmap_section,
    validate_layer_partition,
};
pub(crate) use chart_cull::{ExactSkips, chart_may_cover};
#[cfg(test)]
use chart_walk::bake_light_layer_chart_clipped;
pub(crate) use chart_walk::{bake_light_layer_chart_controlled, for_each_light_layer_chart_texel};
#[cfg(test)]
pub(crate) use chart_walk::{
    bake_light_layer_chart_without_clip_prepass, visit_light_chart_coverage_controlled,
};
pub(crate) use shaft_cull::ShaftScene;
#[cfg(test)]
pub(crate) use shaft_cull::{SHAFT_TILE_TEXELS, ShaftCull};
#[cfg(test)]
pub(crate) use skip_stats::{ChartTally, SkipStats};

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
///
/// v9: shadow rays test the chart walk's last occluding triangle first, which
/// also catches a hit the box test misses by rounding where the triangle meets
/// its box's boundary.
///
/// v10: texel visibility is filtered over a one-texel receiver footprint
/// (`texel_footprint`) unless `--no-penumbra-floor`.
///
/// v11: soft-light footprint samples pair with emitter samples by crossing
/// direction; clear footprint rays test their skipped lead-in; clip segments
/// are tested from the texel centre itself; the probe ring sits at a fixed
/// 0.6 texel at every count.
///
/// v12: a point or spot light whose emitter sphere reaches into geometry
/// (`texel_footprint::embedded_emitter_fraction`) traces every footprint
/// sample, with no probe early-out; the key's geometry bound holds the
/// emitter sphere.
///
/// v13: such a light is flagged only when at least
/// `EMBEDDED_EMITTER_MIN_DIRECTIONS` of the test's directions are embedded,
/// and a flagged light keeps the all-blocked probe early-out (stores 0),
/// skipping only the all-clear one.
pub const LAYER_FORMAT_VERSION: u32 = 13;

/// Bump when the composite/dilate/`encode_section` pipeline, the memo entry's
/// layout or `LightmapSection::to_bytes` serialization changes. Folded into the
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
/// pipeline, the entry layout, `LightmapSection::to_bytes` format or the
/// texel-step detector's verdicts change independently (the entry stores its
/// report; only the threshold is keyed).
///
/// v4: id 22 v3, per-cell blocks sliced from each encoded bake layer.
///
/// v5: the entry leads with the texel-step report, so a hit repeats the
/// detector's warnings.
pub const LIGHTMAP_SECTION_VERSION: u32 = 5;

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
    /// Raw per-light visibility at this analytically covered texel, filtered
    /// over the receiver footprint unless `--no-penumbra-floor`.
    pub raw_visibility: f32,
}

/// One texel's sparse record: `visibility` runs only where `light` has a
/// direct term at `site`, and its raw value is the record. The chart walk
/// records every texel through here, so the sparse-writer tests pin the
/// production step.
fn bake_sparse_layer_texel(
    idx: u32,
    light: &MapLight,
    site: &TexelSite,
    visibility: impl FnOnce() -> f32,
) -> Option<LayerTexel> {
    let (_, _, raw_visibility) =
        light_texel_contribution_and_visibility_with(light, site.world_p, site.normal, visibility);
    raw_visibility.map(|raw_visibility| LayerTexel {
        idx,
        raw_visibility,
    })
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
/// The fused walk retains only this one atlas plane plus the partitions its
/// light window holds (`lightmap_stage::window`). `weighted_dir` deliberately stays
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
/// key. Point/Spot → `falloff_range + AABB_PADDING_METERS` cube, widened by the
/// larger of `footprint_radius` and the emitter's overhang past that reach;
/// Directional → the whole-world AABB. Delegates to the authoritative
/// `affinity_grid::light_aabb` (the f32-falloff copy).
///
/// A reached texel lies within `falloff_range`, but a footprint sample's ray
/// leaves from up to one texel pitch beside it, and `_lightmap_scale` lets a
/// pitch exceed the padding. `footprint_radius` is the largest footprint
/// radius among the charts the light reaches; zero (floor off) leaves the
/// bound unchanged unless the emitter overhangs.
///
/// Shadow rays end on the emitter sphere, and the embedded-emitter test
/// (`texel_footprint::embedded_emitter_fraction`) traces segments that stay
/// within `light_size` of the origin (each starts `RAY_EPSILON` behind it), so
/// the bound also holds the sphere. Only an emitter larger than
/// `falloff_range + AABB_PADDING_METERS` widens it, so every other light keeps
/// its bound. The test is then a function of the keyed light and the keyed
/// geometry, and the flag needs no key of its own.
pub fn layer_influence_aabb(
    light: &MapLight,
    world_aabb: (DVec3, DVec3),
    footprint_radius: f32,
) -> (DVec3, DVec3) {
    let (min, max) = light_aabb(light, world_aabb);
    match light.light_type {
        LightType::Point | LightType::Spot => {
            let reach = f64::from((light.falloff_range + AABB_PADDING_METERS).max(0.01));
            let emitter_beyond_reach = f64::from(light.light_size) - reach;
            let pad = f64::from(footprint_radius).max(emitter_beyond_reach);
            if pad > 0.0 {
                let pad = DVec3::splat(pad);
                (min - pad, max + pad)
            } else {
                (min, max)
            }
        }
        LightType::Directional => (min, max),
    }
}

/// World bounds of a chart's texel centres, padded by
/// `AABB_PADDING_METERS` to absorb `f32` rounding in the texel walk. The pad
/// (0.5 m, borrowed from `affinity_grid`) is deliberately generous and only
/// makes a reach test more conservative; a smaller pad would cull more, and
/// must still exceed the `f32` rounding error of the texel positions.
pub(crate) fn chart_texel_bounds(chart: &Chart) -> Option<(Vec3, Vec3)> {
    if chart.uv_extent[0] <= 0.0 || chart.uv_extent[1] <= 0.0 {
        return None;
    }
    // Texel centres are affine in (tx, ty), so the corner texels bound them.
    let (width, height) = chart_interior_dims(chart);
    let corners = [
        (0, 0),
        (width - 1, 0),
        (0, height - 1),
        (width - 1, height - 1),
    ]
    .map(|(tx, ty)| chart_texel_world_position(chart, tx, ty));
    let pad = Vec3::splat(AABB_PADDING_METERS);
    let min = corners
        .iter()
        .copied()
        .fold(Vec3::splat(f32::INFINITY), Vec3::min);
    let max = corners
        .iter()
        .copied()
        .fold(Vec3::splat(f32::NEG_INFINITY), Vec3::max);
    Some((min - pad, max + pad))
}

/// Whether any texel inside `bounds` may receive a direct term from `light`.
/// Every falloff model is zero beyond `falloff_range`, so a point or spot
/// light reaches no texel farther than that; a directional light reaches all.
/// A chart with no texels is never reached.
///
/// It stays range-only on purpose. The layer key's footprint padding reads
/// it, so a tighter test would move cache keys; the texel-step detector and
/// the shadowmask cut-face estimate read it too, and tightening them saves
/// nothing, so neither output moves. The static bake's partition planning
/// adds [`chart_may_cover`] on top of it.
pub(crate) fn light_may_reach(light: &MapLight, bounds: Option<(Vec3, Vec3)>) -> bool {
    let Some((min, max)) = bounds else {
        return false;
    };
    match light.light_type {
        LightType::Directional => true,
        LightType::Point | LightType::Spot => {
            let origin = light.origin.as_vec3();
            let nearest = origin.clamp(min, max);
            origin.distance(nearest) <= light.falloff_range.max(1.0e-4)
        }
    }
}

/// Bake one light's contribution layer across the shared atlas, walking every
/// chart of each layer with `exact_skips`.
///
/// Mirrors `bake_face_chart`'s per-texel structure exactly but for a single
/// light: same chart interior walk, same `chart_texel_seed`, same Lambert +
/// visibility math (`light_texel_contribution_and_visibility_with`, fed a
/// `TexelSampler` built once per chart rather than per texel). Directional lights are
/// evaluated across every chart texel, but sparse records are emitted only for
/// analytically reached, contributing samples.
///
/// The sparse set is the subset of chart interiors reached by this light before
/// visibility. The compositor derives atlas coverage and fallback normals from
/// the shared chart walk rather than from each light's records.
///
/// Test-only: prl-build bakes through the partition window
/// (`lightmap_stage::partitions`). This face walk plans no charts, so the
/// chart coverage cull never applies to it whatever `exact_skips` says; the
/// shaft cull follows `exact_skips.shaft_cull`.
#[cfg(test)]
#[allow(clippy::too_many_arguments)]
pub(crate) fn bake_light_layer(
    light: &MapLight,
    atlas: &SharedAtlas<'_>,
    bvh: &bvh::bvh::Bvh<f32, 3>,
    primitives: &[BvhPrimitive],
    geometry: &GeometryResult,
    area_sample_count: u32,
    penumbra_floor: bool,
    exact_skips: ExactSkips,
    control: &BakeControl,
) -> Vec<LightmapLayer> {
    // Once for the light, before any layer, as the production stage tests it.
    let embedded_emitter = embedded_emitter_flag(light, bvh, primitives, geometry, penumbra_floor);
    (0..atlas_layer_count(atlas))
        .map(|target_layer| {
            bake_light_layer_on(
                light,
                atlas,
                bvh,
                primitives,
                geometry,
                target_layer,
                area_sample_count,
                penumbra_floor,
                ProbeEarlyOut::for_light(embedded_emitter),
                exact_skips,
                control,
            )
        })
        .collect()
}

/// The light's embedded-emitter flag as the production stage tests it: only
/// with the floor on, which alone reads it.
#[cfg(test)]
fn embedded_emitter_flag(
    light: &MapLight,
    bvh: &bvh::bvh::Bvh<f32, 3>,
    primitives: &[BvhPrimitive],
    geometry: &GeometryResult,
    penumbra_floor: bool,
) -> bool {
    penumbra_floor && has_embedded_emitter(light, bvh, primitives, geometry)
}

/// Bake one light's cacheable contribution for exactly one atlas array layer,
/// walking every chart on it with `exact_skips` ([`bake_light_layer`]'s face
/// walk). With [`ExactSkips::OFF`] it is free of both exact skips: the
/// reference a skipped bake is held against.
///
/// `target_layer` is deliberately part of both the returned partition's
/// containment and its cache key. A partition never carries texels from a
/// neighbouring atlas layer.
#[cfg(test)]
#[allow(clippy::too_many_arguments)]
pub(crate) fn bake_light_layer_controlled(
    light: &MapLight,
    atlas: &SharedAtlas<'_>,
    bvh: &bvh::bvh::Bvh<f32, 3>,
    primitives: &[BvhPrimitive],
    geometry: &GeometryResult,
    target_layer: u32,
    area_sample_count: u32,
    penumbra_floor: bool,
    exact_skips: ExactSkips,
    control: &BakeControl,
) -> LightmapLayer {
    // One light and one layer, so the light's emitter test runs here; the
    // production stage runs it once per light before any partition, and
    // [`bake_light_layer`] once before its layers.
    let embedded_emitter = embedded_emitter_flag(light, bvh, primitives, geometry, penumbra_floor);
    bake_light_layer_on(
        light,
        atlas,
        bvh,
        primitives,
        geometry,
        target_layer,
        area_sample_count,
        penumbra_floor,
        ProbeEarlyOut::for_light(embedded_emitter),
        exact_skips,
        control,
    )
}

/// [`bake_light_layer_controlled`] with the light's footprint early-out
/// already chosen from its emitter flag ([`embedded_emitter_flag`]).
#[cfg(test)]
#[allow(clippy::too_many_arguments)]
fn bake_light_layer_on(
    light: &MapLight,
    atlas: &SharedAtlas<'_>,
    bvh: &bvh::bvh::Bvh<f32, 3>,
    primitives: &[BvhPrimitive],
    geometry: &GeometryResult,
    target_layer: u32,
    area_sample_count: u32,
    penumbra_floor: bool,
    early_out: ProbeEarlyOut,
    exact_skips: ExactSkips,
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
        penumbra_floor,
        early_out,
        exact_skips,
        control,
    )
}

/// [`bake_light_layer`] floored with the probe early-out off
/// ([`ProbeEarlyOut::Off`]) and both exact skips off ([`ExactSkips::OFF`]):
/// the full-set reference a measurement holds the floored bake against, which
/// neither an early-out nor a skip can bias.
#[cfg(test)]
#[allow(clippy::too_many_arguments)]
pub(crate) fn bake_light_layer_full_sets(
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
            bake_light_layer_on(
                light,
                atlas,
                bvh,
                primitives,
                geometry,
                target_layer,
                area_sample_count,
                true,
                ProbeEarlyOut::Off,
                ExactSkips::OFF,
                control,
            )
        })
        .collect()
}

#[cfg(test)]
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
    penumbra_floor: bool,
    early_out: ProbeEarlyOut,
    exact_skips: ExactSkips,
    control: &BakeControl,
) -> LightmapLayer {
    let layer_count = atlas_layer_count(atlas);
    // One scene per call, built by the first chart walk that takes a shaft
    // test, inside its permit, as the stage's partitions build theirs.
    let shaft_scene = std::sync::OnceLock::new();
    // `par_iter().map().collect()` is indexed: the outer collection retains
    // planner order even while ray work runs concurrently. Flattening those
    // per-chart buffers therefore preserves the cache payload's established
    // chart/row/column order exactly.
    let per_chart_texels: Vec<Vec<LayerTexel>> = face_indices
        .par_iter()
        .map(|&face_idx| {
            bake_light_layer_chart_clipped(
                light,
                atlas,
                face_idx,
                bvh,
                primitives,
                geometry,
                area_sample_count,
                penumbra_floor,
                early_out,
                true,
                exact_skips,
                &shaft_scene,
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
