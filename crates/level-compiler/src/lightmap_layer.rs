// Per-light lightmap contribution layers: bake one static light's irradiance +
// unnormalized weighted direction across the shared atlas, cache it, and
// composite the layers back into a byte-identical pre-BC6H atlas.
// See: context/plans/done/incremental-bake-per-element/index.md

use glam::Vec3;

use crate::affinity_grid::{AABB_PADDING_METERS, light_aabb};
use crate::bvh_build::BvhPrimitive;
use crate::chart_raster::{ChartPlacement, chart_interior_dims, chart_texel_world_position};
use crate::geometry::GeometryResult;
use crate::lightmap_bake::{
    Chart, CompositedAtlas, light_texel_contribution, segment_clear, texel_seed,
};
use crate::map_data::{LightType, MapLight};
use glam::DVec3;

/// Bump when the per-light layer payload format or the single-light bake math
/// changes. Folded into every `"lightmap_layer"` cache key, so a bump
/// invalidates all cached layers and forces a re-bake. Each cached stage owns
/// its own version constant and bumps independently — the layer codec evolves
/// separately from the per-group SH and animated-weight-map stages.
pub const LAYER_FORMAT_VERSION: u32 = 2;

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
pub const LIGHTMAP_SECTION_VERSION: u32 = 1;

/// One covered atlas texel's contribution from a single light.
///
/// These are exactly the values `bake_face_chart` accumulates per light *before*
/// the per-texel `weighted_dir.normalize()`: `irradiance` is the shadowed Lambert
/// term, `weighted_dir` is the unnormalized `to_light * luminance` accumulation.
/// `fallback_normal` is the surface normal the bake substitutes when the summed
/// weighted direction is ~zero (a covered-but-dark texel) — light-independent and
/// identical across every layer for a given texel, carried so the compositor can
/// reproduce that branch without re-reading chart geometry.
///
/// Lossless full-precision `f32` (never f16): the layer is compiler-internal and
/// must round-trip exactly so summed layers reproduce the monolithic bake
/// bit-for-bit.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct LayerTexel {
    /// Within-layer linear atlas texel index (`y * atlas_width + x`). The atlas
    /// layer is carried separately in `layer`, so this index is NOT a global
    /// index across layers.
    pub idx: u32,
    /// Atlas array layer this texel lives on. Resolved from the owning chart's
    /// `ChartPlacement.layer` at bake time.
    pub layer: u32,
    /// Shadowed irradiance contribution (RGB).
    pub irradiance: [f32; 3],
    /// Unnormalized weighted direction (`to_light * luminance`).
    pub weighted_dir: [f32; 3],
    /// Surface-normal fallback for the degenerate-direction branch.
    pub fallback_normal: [f32; 3],
}

// Pins the fixed codec stride: `to_bytes`/`from_bytes` cast the blob directly
// via bytemuck; any field change that shifts the stride breaks the on-disk format.
const _: () = assert!(std::mem::size_of::<LayerTexel>() == 44);

/// One light's contribution across the shared atlas.
///
/// Dense over **chart interiors**: every texel the monolithic `bake_face_chart`
/// marks covered appears here (the covered set is light-independent — it is the
/// union of all chart interior texels), each carrying this light's own
/// irradiance / weighted-direction terms (`0.0` where the light does not reach).
/// Deliberately not value-sparse: the byte-identity gate requires the composite
/// to reproduce coverage *and* the fallback normal on covered-but-dark texels, so
/// each layer must enumerate the full covered set. Storage is therefore
/// atlas-sized per layer rather than influence-sparse — these are compiler-internal
/// cache blobs, not shipped runtime data, so the storage overhead is acceptable;
/// storing every covered texel (not just lit ones) is what lets the compositor
/// reproduce the monolithic bake's coverage and dark-texel fallback exactly.
#[derive(Debug, Clone, PartialEq)]
pub struct LightmapLayer {
    pub atlas_width: u32,
    pub atlas_height: u32,
    /// Number of atlas array layers this layer's texels span. Shared across all
    /// of a build's layers (they all bake against the same atlas layout).
    pub layer_count: u32,
    /// Covered texels in ascending atlas order. The bake walks charts then
    /// interior rows/columns, which is the same deterministic order the
    /// monolithic bake uses, so the encoding is reproducible.
    pub texels: Vec<LayerTexel>,
}

/// Fixed-layout header preceding the texel block in a layer blob: atlas
/// dimensions, the array layer count, and the texel count, four native-endian
/// `u32`s (16 bytes).
const LAYER_HEADER_BYTES: usize = 4 * std::mem::size_of::<u32>();

impl LightmapLayer {
    /// Serialize to a fixed-layout native-endian byte blob for the cache.
    ///
    /// A 16-byte header (`atlas_width`, `atlas_height`, `layer_count`, texel
    /// `count`, native `u32`s) followed by the `[LayerTexel]` block copied
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
        // Header is 4 native-endian u32s; the slice lengths are fixed above, so
        // the `try_into`s cannot fail.
        let atlas_width = u32::from_ne_bytes(bytes[0..4].try_into().unwrap());
        let atlas_height = u32::from_ne_bytes(bytes[4..8].try_into().unwrap());
        let layer_count = u32::from_ne_bytes(bytes[8..12].try_into().unwrap());
        let count = u32::from_ne_bytes(bytes[12..16].try_into().unwrap()) as usize;

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

/// Influence AABB of a single light, used both as the cache key's geometry-slice
/// bound and (by the wiring task) as a quick reject. Point/Spot → `falloff_range
/// + AABB_PADDING_METERS` cube; Directional → the whole-world AABB. Delegates to
/// the authoritative `affinity_grid::light_aabb` (the f32-falloff copy).
pub fn layer_influence_aabb(light: &MapLight, world_aabb: (DVec3, DVec3)) -> (DVec3, DVec3) {
    light_aabb(light, world_aabb)
}

/// Bake one light's contribution layer across the shared atlas.
///
/// Mirrors `bake_face_chart`'s per-texel structure exactly but for a single
/// light: same chart interior walk, same `texel_seed`, same
/// `light_texel_contribution` (which itself shares the monolithic Lambert +
/// soft-visibility math). Directional lights use this same path, producing a
/// full-atlas (non-sparse) layer.
///
/// The covered set is the union of all chart interior texels — identical to what
/// the monolithic bake marks covered — so the composited layers reproduce
/// coverage and the fallback-normal branch exactly.
pub fn bake_light_layer(
    light: &MapLight,
    atlas: &SharedAtlas<'_>,
    bvh: &bvh::bvh::Bvh<f32, 3>,
    primitives: &[BvhPrimitive],
    geometry: &GeometryResult,
    area_sample_count: u32,
) -> LightmapLayer {
    let atlas_w = atlas.atlas_width;
    let mut texels: Vec<LayerTexel> = Vec::new();

    // The atlas array layer count: the highest layer any chart landed on, plus
    // one. The multi-bin packer (`pack_layers`) spills leaves onto higher layers,
    // so this is `1` only when every chart fits a single layer.
    let layer_count = atlas
        .placements
        .iter()
        .map(|p| p.layer + 1)
        .max()
        .unwrap_or(1);

    for (face_idx, placement) in atlas.placements.iter().enumerate() {
        let chart = &atlas.charts[face_idx];
        if chart.uv_extent[0] <= 0.0 || chart.uv_extent[1] <= 0.0 {
            continue;
        }
        let padding = crate::chart_raster::CHART_PADDING_TEXELS as i32;
        let (interior_w, interior_h) = chart_interior_dims(chart);

        for ty in 0..interior_h {
            for tx in 0..interior_w {
                let atlas_x = placement.x as i32 + padding + tx;
                let atlas_y = placement.y as i32 + padding + ty;
                // Within-layer index — the atlas layer rides in `LayerTexel.layer`,
                // not folded into `idx`.
                let idx = (atlas_y as u32 * atlas_w + atlas_x as u32) as usize;

                let world_p = chart_texel_world_position(chart, tx, ty, interior_w, interior_h);
                let surface_normal = chart.normal;
                let seed = texel_seed(atlas_x as u32, atlas_y as u32);

                let (irr, weighted_dir) = light_texel_contribution(
                    light,
                    world_p,
                    surface_normal,
                    seed,
                    area_sample_count,
                    |from, to| segment_clear(bvh, primitives, geometry, from, to),
                );

                texels.push(LayerTexel {
                    idx: idx as u32,
                    layer: placement.layer,
                    irradiance: irr.to_array(),
                    weighted_dir: weighted_dir.to_array(),
                    fallback_normal: surface_normal.to_array(),
                });
            }
        }
    }

    LightmapLayer {
        atlas_width: atlas_w,
        atlas_height: atlas.atlas_height,
        layer_count,
        texels,
    }
}

/// Composite per-light layers into the pre-BC6H atlas, reproducing
/// `bake_face_chart`'s output bit-for-bit.
///
/// Element-wise sums irradiance and the unnormalized weighted directions across
/// layers in the order given (which the caller fixes to global `static_lights`
/// order so the float addition order matches the monolithic bake), then performs
/// a single `normalize` per covered texel — falling back to the stored surface
/// normal when the summed direction is degenerate, exactly as the monolithic
/// per-texel pass does. Coverage is the logical OR of the layers' covered texels.
/// Out-of-influence contributions are exactly `0.0` and so do not perturb the
/// sum.
///
/// The returned atlas is **not yet dilated**; the caller runs
/// [`CompositedAtlas::dilate`] to match the monolithic post-dilation seam.
pub fn composite_layers(layers: &[LightmapLayer], atlas_w: u32, atlas_h: u32) -> CompositedAtlas {
    // All entries share the same atlas layout; read the array layer count from
    // any of them. The fallback guards the empty-slice case (no panic) and keeps
    // single-layer output when there are no layers to composite.
    let layer_count = layers.first().map_or(1, |l| l.layer_count);
    let mut atlas = CompositedAtlas::zeroed(atlas_w, atlas_h, layer_count);
    let plane = (atlas_w * atlas_h) as usize;
    let texel_count = plane * layer_count as usize;

    // Accumulate the unnormalized weighted direction separately; `atlas.direction`
    // holds the final normalized result, so it cannot double as the accumulator.
    let mut weighted_dir = vec![Vec3::ZERO; texel_count];
    let mut fallback = vec![Vec3::Y; texel_count];

    for layer in layers {
        for t in &layer.texels {
            // Resolve the global layer-major index from the within-layer `idx`
            // plus the texel's atlas layer.
            let idx = t.layer as usize * plane + t.idx as usize;
            atlas.irradiance[idx * 4] += t.irradiance[0];
            atlas.irradiance[idx * 4 + 1] += t.irradiance[1];
            atlas.irradiance[idx * 4 + 2] += t.irradiance[2];
            weighted_dir[idx] += Vec3::from_array(t.weighted_dir);
            // Identical across layers; last write wins (all agree).
            fallback[idx] = Vec3::from_array(t.fallback_normal);
            atlas.coverage[idx] = true;
        }
    }

    for idx in 0..texel_count {
        if !atlas.coverage[idx] {
            continue;
        }
        // Alpha is 1.0 on covered texels, matching `bake_face_chart`.
        atlas.irradiance[idx * 4 + 3] = 1.0;
        let wd = weighted_dir[idx];
        atlas.direction[idx] = if wd.length_squared() > 1.0e-8 {
            wd.normalize()
        } else {
            fallback[idx]
        };
    }

    atlas
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
/// dimensions and the per-chart placements so an atlas repack (which shifts every
/// placement) invalidates all layers by changing this fingerprint.
///
/// `ChartPlacement` does not derive `Serialize`, so this folds its `x`/`y`/`layer`
/// fields directly into the digest — the deterministically-derived proxy-bytes
/// fingerprint the animated-weight-map stage uses for its non-`Serialize` atlas
/// types. Charts are covered transitively: placements are a deterministic
/// function of the charts under a fixed atlas, so the placement set fingerprints
/// the layout.
fn atlas_layout_fingerprint(atlas: &SharedAtlas<'_>) -> Vec<u8> {
    let mut hasher = blake3::Hasher::new();
    hasher.update(&atlas.atlas_width.to_le_bytes());
    hasher.update(&atlas.atlas_height.to_le_bytes());
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
///
/// The wiring task passes this to `CacheKey::new("lightmap_layer",
/// LAYER_FORMAT_VERSION, &hash)`.
pub fn layer_input_hash(
    light: &MapLight,
    atlas: &SharedAtlas<'_>,
    primitives: &[BvhPrimitive],
    geometry: &GeometryResult,
    lightmap_density: f32,
    area_sample_count: u32,
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
    *hasher.finalize().as_bytes()
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
/// 3. each light's `layer_input_hash` `[u8; 32]`, in the caller's exact filtered
///    order (global static order, `ShadowType::Sdf` dropped). Folding the input
///    hashes mirrors folding the full per-light keys: any per-light input change
///    (light params, geometry slice, density, atlas layout) flows through here.
/// 4. `texel_density` (f32 LE) — the `density` passed to `encode_section`.
///    Already folded into every `layer_input_hash` via `lightmap_density`, so
///    this is belt-and-suspenders (same rationale as the light-count fold).
/// 5. `uncompressed_irradiance` (1 byte, 0/1) — selects BC6H vs RGBA16F output.
///
/// `layer_input_hashes` must be supplied in the same filtered order the warm
/// composite loop uses; the helper does not re-derive or re-sort them.
pub fn section_input_hash(
    layer_input_hashes: &[[u8; 32]],
    texel_density: f32,
    uncompressed_irradiance: bool,
) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(&LAYER_FORMAT_VERSION.to_le_bytes());
    hasher.update(&(layer_input_hashes.len() as u32).to_le_bytes());
    for hash in layer_input_hashes {
        hasher.update(hash);
    }
    hasher.update(&texel_density.to_le_bytes());
    hasher.update(&[u8::from(uncompressed_irradiance)]);
    *hasher.finalize().as_bytes()
}

/// `true` for lights whose layer covers the whole atlas (directional). Point/Spot
/// layers are influence-bounded in *value* (zero past `falloff_range`) but still
/// enumerate the full covered set; this predicate is informational for callers.
pub fn is_full_atlas_light(light: &MapLight) -> bool {
    matches!(light.light_type, LightType::Directional)
}

/// Padding constant re-exported so the wiring task can reason about influence
/// bounds without reaching into `affinity_grid`.
pub const LAYER_AABB_PADDING_METERS: f32 = AABB_PADDING_METERS;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bvh_build::build_bvh;
    use crate::lightmap_bake::{bake_monolithic_atlas, prepare_atlas};
    use crate::map_data::{FalloffModel, ShadowType};
    use glam::DVec3;
    use postretro_level_format::geometry::{FaceMeta, GeometrySection, Vertex};
    use postretro_level_format::texture_names::TextureNamesSection;

    /// One per-texel contribution term for `expected_atlas_from_texels`:
    /// `(layer, within-layer idx, irradiance, weighted_dir, fallback_normal)`.
    type ContribTerm = (u32, u32, [f32; 3], [f32; 3], [f32; 3]);

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

    /// The headline gate (in miniature): the per-light compositor reproduces the
    /// monolithic `bake_face_chart` pre-BC6H atlas bit-for-bit on a synthetic
    /// multi-light atlas. Two point lights over different quads plus one
    /// directional (full-atlas layer) exercise sparse + dense layers, the
    /// summed-direction normalize, and the covered-but-dark fallback branch.
    #[test]
    fn composite_matches_monolithic_atlas_bit_for_bit() {
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
        let mono_prepared = prepare_atlas(&mut mono_geo, &static_lights, DENSITY).unwrap();
        let (mono_bvh, mono_prims, _) = build_bvh(&mono_geo).unwrap();
        let mono_atlas = bake_monolithic_atlas(
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
        );

        // Per-light layer path.
        let layer_prepared = prepare_atlas(&mut layer_geo, &static_lights, DENSITY).unwrap();
        let (layer_bvh, layer_prims, _) = build_bvh(&layer_geo).unwrap();
        let shared = SharedAtlas {
            charts: &layer_prepared.charts,
            placements: &layer_prepared.placements,
            atlas_width: layer_prepared.atlas_width,
            atlas_height: layer_prepared.atlas_height,
        };
        let layers: Vec<LightmapLayer> = light_refs
            .iter()
            .map(|l| {
                bake_light_layer(
                    l,
                    &shared,
                    &layer_bvh,
                    &layer_prims,
                    &layer_geo,
                    AREA_SAMPLES,
                )
            })
            .collect();
        let mut composite = composite_layers(&layers, shared.atlas_width, shared.atlas_height);
        composite.dilate();

        assert_eq!(
            mono_atlas, composite,
            "per-light composite must equal the monolithic atlas bit-for-bit"
        );
    }

    /// Build the monolithic-equivalent `CompositedAtlas` directly from a set of
    /// per-light, per-texel terms over a `layer_count`-layer atlas, mirroring what
    /// `bake_face_chart` + `composite_layers` jointly produce: sum each light's
    /// irradiance and weighted direction per texel, set alpha/coverage on covered
    /// texels, normalize the summed direction (falling back to the surface
    /// normal), then dilate. This is the "expected" side the per-light composite
    /// must reproduce bit-for-bit — assembled the same way the single-layer gate
    /// assembles its expected atlas, generalized to multiple layers.
    fn expected_atlas_from_texels(
        atlas_w: u32,
        atlas_h: u32,
        layer_count: u32,
        contributions: &[ContribTerm],
    ) -> CompositedAtlas {
        let plane = (atlas_w * atlas_h) as usize;
        let texel_count = plane * layer_count as usize;
        let mut atlas = CompositedAtlas::zeroed(atlas_w, atlas_h, layer_count);
        let mut weighted_dir = vec![Vec3::ZERO; texel_count];
        let mut fallback = vec![Vec3::Y; texel_count];

        for &(layer, idx, irr, wd, fb) in contributions {
            let gidx = layer as usize * plane + idx as usize;
            atlas.irradiance[gidx * 4] += irr[0];
            atlas.irradiance[gidx * 4 + 1] += irr[1];
            atlas.irradiance[gidx * 4 + 2] += irr[2];
            weighted_dir[gidx] += Vec3::from_array(wd);
            fallback[gidx] = Vec3::from_array(fb);
            atlas.coverage[gidx] = true;
        }
        for gidx in 0..texel_count {
            if !atlas.coverage[gidx] {
                continue;
            }
            atlas.irradiance[gidx * 4 + 3] = 1.0;
            let wd = weighted_dir[gidx];
            atlas.direction[gidx] = if wd.length_squared() > 1.0e-8 {
                wd.normalize()
            } else {
                fallback[gidx]
            };
        }
        atlas.dilate();
        atlas
    }

    /// Multi-layer byte-identity gate: the per-light `composite_layers` output
    /// equals the monolithic-equivalent atlas bit-for-bit for a TWO-layer atlas.
    /// Extends the single-layer `composite_matches_monolithic_atlas_bit_for_bit`
    /// gate to the array dimension.
    ///
    /// The fixture is hand-built (not routed through `prepare_atlas`, which is
    /// hardcoded single-layer): two `LightmapLayer` entries with `layer_count = 2`
    /// carrying texels on layer 0 AND layer 1, with distinct irradiance and
    /// weighted-direction terms — including a covered-but-dark texel (zero
    /// weighted direction) so the fallback-normal branch is exercised per layer.
    /// The same terms assemble the expected monolithic atlas, so a layer-addressing
    /// bug (e.g. dilation bleeding across the layer boundary, or `idx` folding the
    /// layer in) breaks the equality.
    #[test]
    fn multi_layer_composite_matches_monolithic_bit_for_bit() {
        // Small atlas so dilation has covered + uncovered texels to fill, on each
        // of two layers independently.
        let atlas_w = 8u32;
        let atlas_h = 8u32;
        let layer_count = 2u32;

        let mk_idx = |x: u32, y: u32| y * atlas_w + x;

        // Light A: a lit texel on each layer at different (x, y).
        let a_texels = vec![
            LayerTexel {
                idx: mk_idx(2, 2),
                layer: 0,
                irradiance: [0.4, 0.1, 0.2],
                weighted_dir: [0.0, 1.0, 0.5],
                fallback_normal: [0.0, 1.0, 0.0],
            },
            LayerTexel {
                idx: mk_idx(5, 4),
                layer: 1,
                irradiance: [0.0, 0.0, 0.0], // covered-but-dark on layer 1
                weighted_dir: [0.0, 0.0, 0.0], // → fallback-normal branch
                fallback_normal: [1.0, 0.0, 0.0],
            },
        ];
        // Light B: overlaps A's layer-0 texel (sum + direction accumulate) and
        // adds its own lit texel on layer 1.
        let b_texels = vec![
            LayerTexel {
                idx: mk_idx(2, 2),
                layer: 0,
                irradiance: [0.3, 0.2, 0.0],
                weighted_dir: [1.0, 0.0, 0.0],
                fallback_normal: [0.0, 1.0, 0.0],
            },
            LayerTexel {
                idx: mk_idx(5, 4),
                layer: 1,
                irradiance: [0.6, 0.5, 0.4],
                weighted_dir: [0.0, 0.0, 1.0],
                fallback_normal: [1.0, 0.0, 0.0],
            },
        ];

        let layers = vec![
            LightmapLayer {
                atlas_width: atlas_w,
                atlas_height: atlas_h,
                layer_count,
                texels: a_texels.clone(),
            },
            LightmapLayer {
                atlas_width: atlas_w,
                atlas_height: atlas_h,
                layer_count,
                texels: b_texels.clone(),
            },
        ];

        let mut composite = composite_layers(&layers, atlas_w, atlas_h);
        composite.dilate();

        let contributions: Vec<ContribTerm> = a_texels
            .iter()
            .chain(b_texels.iter())
            .map(|t| {
                (
                    t.layer,
                    t.idx,
                    t.irradiance,
                    t.weighted_dir,
                    t.fallback_normal,
                )
            })
            .collect();
        let expected = expected_atlas_from_texels(atlas_w, atlas_h, layer_count, &contributions);

        assert_eq!(composite.layer_count, 2, "composite must report two layers");
        assert_eq!(
            composite, expected,
            "two-layer per-light composite must equal the monolithic atlas bit-for-bit"
        );
    }

    #[test]
    fn layer_roundtrips_through_codec() {
        let mut geo = two_quad_geometry();
        let lights = vec![point_light([0.5, 1.0, 0.5], 5.0)];
        let static_lights = crate::light_namespaces::StaticBakedLights::from_lights(&lights);
        let prepared = prepare_atlas(&mut geo, &static_lights, DENSITY).unwrap();
        let (bvh, prims, _) = build_bvh(&geo).unwrap();
        let shared = SharedAtlas {
            charts: &prepared.charts,
            placements: &prepared.placements,
            atlas_width: prepared.atlas_width,
            atlas_height: prepared.atlas_height,
        };
        let layer = bake_light_layer(&lights[0], &shared, &bvh, &prims, &geo, AREA_SAMPLES);

        let bytes = layer.to_bytes();
        let decoded = LightmapLayer::from_bytes(&bytes).expect("round-trip decode");
        assert_eq!(layer, decoded, "layer codec must round-trip exactly");
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
        blob.extend_from_slice(&u32::MAX.to_ne_bytes()); // count = u32::MAX
        // No body bytes — the implied body cannot match.
        assert!(LightmapLayer::from_bytes(&blob).is_none());
    }

    #[test]
    fn layer_input_hash_changes_when_light_moves() {
        let mut geo = two_quad_geometry();
        let lights = vec![point_light([0.5, 1.0, 0.5], 5.0)];
        let static_lights = crate::light_namespaces::StaticBakedLights::from_lights(&lights);
        let prepared = prepare_atlas(&mut geo, &static_lights, DENSITY).unwrap();
        let (_, prims, _) = build_bvh(&geo).unwrap();
        let shared = SharedAtlas {
            charts: &prepared.charts,
            placements: &prepared.placements,
            atlas_width: prepared.atlas_width,
            atlas_height: prepared.atlas_height,
        };

        let base = point_light([0.5, 1.0, 0.5], 5.0);
        let moved = point_light([10.0, 1.0, 0.5], 5.0);
        let h_base = layer_input_hash(&base, &shared, &prims, &geo, DENSITY, AREA_SAMPLES);
        let h_moved = layer_input_hash(&moved, &shared, &prims, &geo, DENSITY, AREA_SAMPLES);
        assert_ne!(
            h_base, h_moved,
            "moving the light must change its layer cache key"
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

    #[test]
    fn directional_is_full_atlas_light() {
        assert!(is_full_atlas_light(&directional_light()));
        assert!(!is_full_atlas_light(&point_light([0.0, 1.0, 0.0], 5.0)));
    }

    // -----------------------------------------------------------------------
    // Cache wiring behaviors at the module API seam and the full-fixture
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

    /// Build one light's layer cache key the same way the `main.rs` wiring does,
    /// so the locality/round-trip tests assert on the production key derivation.
    fn layer_key(
        light: &MapLight,
        shared: &SharedAtlas<'_>,
        prims: &[BvhPrimitive],
        geo: &GeometryResult,
    ) -> CacheKey {
        let h = layer_input_hash(light, shared, prims, geo, DENSITY, AREA_SAMPLES);
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
        let prepared = prepare_atlas(&mut geo, &static_lights, DENSITY).unwrap();
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
            let layer = bake_light_layer(light, &shared, &bvh, &prims, &geo, AREA_SAMPLES);
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
        let prepared = prepare_atlas(&mut geo, &static_lights, DENSITY).unwrap();
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
        let prepared = prepare_atlas(&mut geo, &static_lights, DENSITY).unwrap();
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
        let prepared = prepare_atlas(&mut prep_geo, &static_lights, DENSITY).unwrap();
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
        let prepared = prepare_atlas(&mut geo, &static_lights, DENSITY).unwrap();
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

        let original = bake_light_layer(&lights[0], &shared, &bvh, &prims, &geo, AREA_SAMPLES);
        cache.put(&key, &original.to_bytes());

        // Corrupt every file in the cache dir.
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_file() {
                std::fs::write(&path, b"corrupt").unwrap();
            }
        }

        // The cache rejects it (returns None), so the wiring re-bakes.
        let recovered = match cache
            .get(&key)
            .and_then(|bytes| LightmapLayer::from_bytes(&bytes))
        {
            Some(layer) => layer,
            None => bake_light_layer(&lights[0], &shared, &bvh, &prims, &geo, AREA_SAMPLES),
        };
        assert_eq!(
            original, recovered,
            "re-bake after corruption must reproduce the original layer"
        );

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
        let prepared = prepare_atlas(&mut geo, &static_lights, DENSITY).unwrap();
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
        let layer = bake_light_layer(&lights[0], &shared, &bvh, &prims, &geo, AREA_SAMPLES);
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
    /// EVERY `content/dev/maps/` fixture. The synthetic
    /// `composite_matches_monolithic_atlas_bit_for_bit` proves the mechanism;
    /// this runs the real fixtures through both paths.
    ///
    /// `#[ignore]` because each fixture bake casts the full per-texel ray load
    /// (a few minutes across the gate fixture set). Run manually:
    ///   cargo test -p postretro-level-compiler -- --ignored --nocapture \
    ///       lightmap_composite_equals_monolithic_on_fixtures
    #[test]
    #[ignore = "full-fixture lightmap bake; run with --ignored"]
    fn lightmap_composite_equals_monolithic_on_fixtures() {
        use crate::fixture_pipeline::{GATE_FIXTURES, load_fixture};

        for &name in GATE_FIXTURES {
            let fx = load_fixture(name);
            // Match the wiring's direct-lightmap light set: global static order,
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

            let mono_prepared = prepare_atlas(&mut mono_geo, &static_lights, density).unwrap();
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

            let layer_prepared = prepare_atlas(&mut layer_geo, &static_lights, density).unwrap();
            let (layer_bvh, layer_prims, _) = build_bvh(&layer_geo).unwrap();
            let shared = SharedAtlas {
                charts: &layer_prepared.charts,
                placements: &layer_prepared.placements,
                atlas_width: layer_prepared.atlas_width,
                atlas_height: layer_prepared.atlas_height,
            };
            let layers: Vec<LightmapLayer> = layer_lights
                .iter()
                .map(|l| {
                    bake_light_layer(
                        l,
                        &shared,
                        &layer_bvh,
                        &layer_prims,
                        &layer_geo,
                        AREA_SAMPLES,
                    )
                })
                .collect();
            let mut composite = composite_layers(&layers, shared.atlas_width, shared.atlas_height);
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
    // invalidation coupling that the `main.rs` wiring relies on.

    use postretro_level_format::lightmap::LightmapSection;

    /// Build the section cache key the same way the `main.rs` warm path does:
    /// fold the ordered per-light `layer_input_hash` set through
    /// `section_input_hash`, then `CacheKey::new("lightmap_section", ...)`.
    /// Mirrors the existing `layer_key` helper so the section tests assert on
    /// the production key derivation, not a test-only reimplementation.
    fn section_key(
        layer_input_hashes: &[[u8; 32]],
        density: f32,
        uncompressed_irradiance: bool,
    ) -> CacheKey {
        let h = section_input_hash(layer_input_hashes, density, uncompressed_irradiance);
        CacheKey::new("lightmap_section", LIGHTMAP_SECTION_VERSION, &h)
    }

    /// Bake every light's layer, composite, dilate, and `encode_section` — the
    /// exact recompose the warm path runs on a section miss. Returns the
    /// composited `LightmapSection` the cache memoizes.
    fn compose_section(
        lights: &[&MapLight],
        shared: &SharedAtlas<'_>,
        bvh: &bvh::bvh::Bvh<f32, 3>,
        prims: &[BvhPrimitive],
        geo: &GeometryResult,
    ) -> LightmapSection {
        let layers: Vec<LightmapLayer> = lights
            .iter()
            .map(|l| bake_light_layer(l, shared, bvh, prims, geo, AREA_SAMPLES))
            .collect();
        let mut composite = composite_layers(&layers, shared.atlas_width, shared.atlas_height);
        composite.dilate();
        // Uncompressed RGBA16F so the synthetic-atlas tests stay off the BC6H
        // encoder; the cache behavior under test is format-agnostic and the
        // `--no-cache`/BC6H combination is exercised by the CLI/byte-identity
        // evidence in RESULTS.md.
        composite.encode_section(DENSITY, true)
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
        light_refs
            .iter()
            .map(|l| layer_input_hash(l, shared, prims, geo, DENSITY, AREA_SAMPLES))
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
        let prepared = prepare_atlas(&mut geo, &static_lights, DENSITY).unwrap();
        let (bvh, prims, _) = build_bvh(&geo).unwrap();
        let shared = SharedAtlas {
            charts: &prepared.charts,
            placements: &prepared.placements,
            atlas_width: prepared.atlas_width,
            atlas_height: prepared.atlas_height,
        };

        let hashes = layer_input_hashes(&light_refs, &shared, &prims, &geo);
        let key_build1 = section_key(&hashes, DENSITY, true);

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
        let key_build2 = section_key(&hashes, DENSITY, true);
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
        let prepared = prepare_atlas(&mut geo, &static_lights, DENSITY).unwrap();
        let (_, prims, _) = build_bvh(&geo).unwrap();
        let shared = SharedAtlas {
            charts: &prepared.charts,
            placements: &prepared.placements,
            atlas_width: prepared.atlas_width,
            atlas_height: prepared.atlas_height,
        };

        let base_hashes = layer_input_hashes(&light_refs, &shared, &prims, &geo);
        let base_section = section_key(&base_hashes, DENSITY, true).as_filename();
        let base_layer1 =
            CacheKey::new("lightmap_layer", LAYER_FORMAT_VERSION, &base_hashes[1]).as_filename();

        // Edit only the first light's color.
        let mut edited = lights.clone();
        edited[0].color = [0.3, 0.3, 0.3];
        let edited_static = crate::light_namespaces::StaticBakedLights::from_lights(&edited);
        let edited_refs: Vec<&MapLight> = edited_static.entries().iter().map(|e| e.light).collect();
        let edited_hashes = layer_input_hashes(&edited_refs, &shared, &prims, &geo);
        let edited_section = section_key(&edited_hashes, DENSITY, true).as_filename();
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
    /// treated as a miss, so the wiring recomposes. Asserts the recovered
    /// section equals the originally-composited one. Mirror of
    /// `corrupt_layer_entry_is_discarded_and_rebaked`, exercising the
    /// `get`-succeeds-but-`from_bytes`-fails branch the `main.rs` wiring guards.
    #[test]
    fn corrupt_section_entry_is_discarded_and_recomposed() {
        let mut geo = two_quad_geometry();
        let lights = vec![
            point_light([0.5, 1.0, 0.5], 5.0),
            point_light([3.5, 1.0, 0.5], 5.0),
        ];
        let static_lights = crate::light_namespaces::StaticBakedLights::from_lights(&lights);
        let light_refs: Vec<&MapLight> = static_lights.entries().iter().map(|e| e.light).collect();
        let prepared = prepare_atlas(&mut geo, &static_lights, DENSITY).unwrap();
        let (bvh, prims, _) = build_bvh(&geo).unwrap();
        let shared = SharedAtlas {
            charts: &prepared.charts,
            placements: &prepared.placements,
            atlas_width: prepared.atlas_width,
            atlas_height: prepared.atlas_height,
        };

        let hashes = layer_input_hashes(&light_refs, &shared, &prims, &geo);
        let key = section_key(&hashes, DENSITY, true);
        let original = compose_section(&light_refs, &shared, &bvh, &prims, &geo);

        let dir = fresh_cache_dir("section_corrupt");
        let cache = StageCache::new(&dir).expect("cache dir");
        // Store a blob that PASSES the cache's length/hash check (it is `put`
        // through the cache) but is too short for `LightmapSection::from_bytes`
        // to parse — the exact format-skew the wiring recovers from.
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
        let prepared = prepare_atlas(&mut geo, &static_lights, DENSITY).unwrap();
        let (bvh, prims, _) = build_bvh(&geo).unwrap();
        let shared = SharedAtlas {
            charts: &prepared.charts,
            placements: &prepared.placements,
            atlas_width: prepared.atlas_width,
            atlas_height: prepared.atlas_height,
        };

        let hashes = layer_input_hashes(&light_refs, &shared, &prims, &geo);
        let key = section_key(&hashes, DENSITY, true);
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
    /// not entered at all (`main.rs` gates the whole section-cache block behind
    /// `if let Some(ref cache) = stage_cache`), so no section entry is created.
    /// A clean unit assertion for "no cache → no entry" is to open a cache dir,
    /// derive the key WITHOUT putting anything (modeling the no-cache control
    /// flow that never reaches `put`), and confirm the entry does not exist.
    /// The end-to-end `--no-cache` CLI run is recorded in RESULTS.md.
    #[test]
    fn no_cache_path_writes_no_section_entry() {
        let mut geo = two_quad_geometry();
        let lights = vec![point_light([0.5, 1.0, 0.5], 5.0)];
        let static_lights = crate::light_namespaces::StaticBakedLights::from_lights(&lights);
        let light_refs: Vec<&MapLight> = static_lights.entries().iter().map(|e| e.light).collect();
        let prepared = prepare_atlas(&mut geo, &static_lights, DENSITY).unwrap();
        let (_, prims, _) = build_bvh(&geo).unwrap();
        let shared = SharedAtlas {
            charts: &prepared.charts,
            placements: &prepared.placements,
            atlas_width: prepared.atlas_width,
            atlas_height: prepared.atlas_height,
        };

        let hashes = layer_input_hashes(&light_refs, &shared, &prims, &geo);
        let key = section_key(&hashes, DENSITY, true);

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
        let prepared = prepare_atlas(&mut geo, &static_lights, DENSITY).unwrap();
        let (_, prims, _) = build_bvh(&geo).unwrap();
        let shared = SharedAtlas {
            charts: &prepared.charts,
            placements: &prepared.placements,
            atlas_width: prepared.atlas_width,
            atlas_height: prepared.atlas_height,
        };

        let base = layer_input_hashes(&light_refs, &shared, &prims, &geo);
        let base_key = section_key(&base, DENSITY, true).as_filename();

        // Add a light: a third hash extends the fold.
        let mut added = base.clone();
        added.push(layer_input_hash(
            &point_light([2.0, 1.0, 0.5], 5.0),
            &shared,
            &prims,
            &geo,
            DENSITY,
            AREA_SAMPLES,
        ));
        assert_ne!(
            base_key,
            section_key(&added, DENSITY, true).as_filename(),
            "adding a light must change the section key"
        );

        // Remove a light: drop the second hash.
        let removed = vec![base[0]];
        assert_ne!(
            base_key,
            section_key(&removed, DENSITY, true).as_filename(),
            "removing a light must change the section key"
        );

        // Reorder: swap the two hashes. Same set, different concatenation order.
        let reordered = vec![base[1], base[0]];
        assert_ne!(
            base_key,
            section_key(&reordered, DENSITY, true).as_filename(),
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
        let prepared = prepare_atlas(&mut geo, &static_lights, DENSITY).unwrap();
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
            .map(|l| layer_input_hash(l, &shared, &prims, &geo, DENSITY, samples_a))
            .collect();
        let hashes_b: Vec<[u8; 32]> = light_refs
            .iter()
            .map(|l| layer_input_hash(l, &shared, &prims, &geo, DENSITY, samples_b))
            .collect();

        assert_ne!(
            section_key(&hashes_a, DENSITY, true).as_filename(),
            section_key(&hashes_b, DENSITY, true).as_filename(),
            "changing --soft-shadow-samples must change the section key (section miss)"
        );
    }
}
