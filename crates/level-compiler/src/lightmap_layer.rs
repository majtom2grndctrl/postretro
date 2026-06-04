// Per-light lightmap contribution layers: bake one static light's irradiance +
// unnormalized weighted direction across the shared atlas, cache it, and
// composite the layers back into a byte-identical pre-BC6H atlas.
// See: context/plans/in-progress/incremental-bake-per-element/index.md

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
/// changes. Folded into every `"lightmap_layer"` cache key (Task 7 wiring), so a
/// bump invalidates all cached layers and forces a re-bake. Independent of
/// `lightmap_bake::STAGE_VERSION` — the layer codec evolves separately from the
/// whole-stage bake.
pub const LAYER_FORMAT_VERSION: u32 = 1;

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
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct LayerTexel {
    /// Linear atlas texel index (`y * atlas_width + x`).
    pub idx: u32,
    /// Shadowed irradiance contribution (RGB).
    pub irradiance: [f32; 3],
    /// Unnormalized weighted direction (`to_light * luminance`).
    pub weighted_dir: [f32; 3],
    /// Surface-normal fallback for the degenerate-direction branch.
    pub fallback_normal: [f32; 3],
}

/// One light's contribution across the shared atlas.
///
/// Dense over **chart interiors**: every texel the monolithic `bake_face_chart`
/// marks covered appears here (the covered set is light-independent — it is the
/// union of all chart interior texels), each carrying this light's own
/// irradiance / weighted-direction terms (`0.0` where the light does not reach).
/// Deliberately not value-sparse: the byte-identity gate requires the composite
/// to reproduce coverage *and* the fallback normal on covered-but-dark texels, so
/// each layer must enumerate the full covered set. Storage is therefore
/// atlas-sized per layer rather than influence-sparse — accepted for a warm-only
/// compiler cache (see plan Task 2 / research §1).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct LightmapLayer {
    pub atlas_width: u32,
    pub atlas_height: u32,
    /// Covered texels in ascending atlas order. The bake walks charts then
    /// interior rows/columns, which is the same deterministic order the
    /// monolithic bake uses, so the encoding is reproducible.
    pub texels: Vec<LayerTexel>,
}

impl LightmapLayer {
    /// Serialize to a deterministic, fixed-layout byte blob for the cache.
    /// Postcard over the owned fields gives an exact round-trip and a stable
    /// encoding for hashing.
    pub fn to_bytes(&self) -> Vec<u8> {
        postcard::to_allocvec(self).expect("postcard serialize LightmapLayer")
    }

    /// Deserialize a layer blob. A decode failure (truncated or format-skewed
    /// payload that still passed the `StageCache` length/hash check) returns
    /// `None` so the caller treats it as a miss and re-bakes. The cache's own
    /// length/hash validation catches bit-rot; this catches format skew.
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        match postcard::from_bytes::<Self>(bytes) {
            Ok(layer) => Some(layer),
            Err(err) => {
                log::warn!("[Compiler] corrupt lightmap layer, re-baking: {err}");
                None
            }
        }
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
    let mut atlas = CompositedAtlas::zeroed(atlas_w, atlas_h);
    let texel_count = (atlas_w * atlas_h) as usize;

    // Accumulate the unnormalized weighted direction separately; `atlas.direction`
    // holds the final normalized result, so it cannot double as the accumulator.
    let mut weighted_dir = vec![Vec3::ZERO; texel_count];
    let mut fallback = vec![Vec3::Y; texel_count];

    for layer in layers {
        for t in &layer.texels {
            let idx = t.idx as usize;
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
/// `ChartPlacement` does not derive `Serialize`, so this folds its `x`/`y`
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
/// - the light's params (whole `MapLight`, fixed `postcard` encoding — the
///   same discipline the whole-stage key uses),
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

    const AREA_SAMPLES: u32 = 16;
    const DENSITY: f32 = 0.25;

    /// Two coplanar quads on the floor (Y=0), separate faces so the atlas packs
    /// more than one chart and `split_shared_vertices` has work to do.
    fn two_quad_geometry() -> GeometryResult {
        let mk = |x: f32, z: f32| {
            let n = [0.0, 1.0, 0.0];
            let t = [1.0, 0.0, 0.0];
            vec![
                Vertex::new([x, 0.0, z], [0.0, 0.0], n, t, true, [0.0, 0.0]),
                Vertex::new([x + 1.0, 0.0, z], [1.0, 0.0], n, t, true, [0.0, 0.0]),
                Vertex::new([x + 1.0, 0.0, z + 1.0], [1.0, 1.0], n, t, true, [0.0, 0.0]),
                Vertex::new([x, 0.0, z + 1.0], [0.0, 1.0], n, t, true, [0.0, 0.0]),
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
            cast_shadows: true,
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
        // A truncated/garbage blob that the StageCache length/hash check would
        // pass (it validates bytes, not format) must still be rejected by the
        // codec so the caller re-bakes.
        assert!(LightmapLayer::from_bytes(b"not a valid postcard layer").is_none());
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
}
