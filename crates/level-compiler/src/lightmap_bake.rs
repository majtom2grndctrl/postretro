// Directional lightmap baker.

use std::collections::HashSet;

use bvh::bvh::Bvh;
use bvh::ray::Ray;
use glam::Vec3;
use nalgebra::{Point3, Vector3};
use postretro_level_format::lightmap::{
    IRRADIANCE_FORMAT_BC6H, IRRADIANCE_FORMAT_RGBA16F, LightmapMode, LightmapSection,
    encode_direction_oct, f32_to_f16_bits,
};

use crate::bc6h;
use thiserror::Error;

use crate::bvh_build::BvhPrimitive;
use crate::chart_raster::{CHART_PADDING_TEXELS, ChartPlacement, chart_texel_world_position};
use crate::geometry::GeometryResult;
use crate::light_namespaces::StaticBakedLights;
use crate::map_data::{FalloffModel, LightType, MapLight};

/// Default atlas texel density: 4 cm per texel.
pub const DEFAULT_TEXEL_DENSITY_METERS: f32 = 0.04;

/// Atlas width/height when no face would fit otherwise. Power-of-two for BC6H block alignment.
/// The 4-alignment BC6H requires is satisfied for free since dimensions are always power-of-two
/// ≥ 4 here, meaning `ceil(w/4)` is always exact (no partial trailing block).
const MIN_ATLAS_DIMENSION: u32 = 64;

/// Maximum atlas dimension. Beyond this the baker returns an error so the caller can retry at a
/// coarser density. 8192 matches the `max_texture_dimension_2d` floor the runtime requires
/// (see `crates/postretro/src/render/renderer_init_resources.rs`'s adapter pre-check) and fits ~328 m at 4 cm/texel.
const MAX_ATLAS_DIMENSION: u32 = 8192;

/// Maximum atlas array layers. The atlas is a `texture_2d_array`; the multi-bin
/// packer opens a new layer whenever a BVH leaf's charts don't fit the current
/// one. Beyond this the packer errors so the caller can coarsen density or split
/// the map. 256 is the `max_texture_array_layers` floor the runtime requires.
const MAX_ATLAS_LAYERS: u32 = 256;

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
        "lightmap atlas layer overflow: packing the charts needs {layer_count} array layers \
         but the atlas supports at most {max}; raise `texel_density` or split the map"
    )]
    LayerOverflow { layer_count: u32, max: u32 },
    #[error(
        "lightmap chart too large: face {face_index} needs {width_texels}x{height_texels} texels at \
         {density_m_per_texel} m/texel (limit {max}); face extent {u_extent_m} x {v_extent_m} m. \
         Raise `texel_density` or subdivide the face."
    )]
    ChartTooLarge {
        face_index: usize,
        width_texels: u32,
        height_texels: u32,
        max: u32,
        u_extent_m: f32,
        v_extent_m: f32,
        density_m_per_texel: f32,
    },
    #[error(
        "lightmap leaf too large: BVH leaf {leaf_index}'s {chart_count} charts can't fit a single \
         {max_dim}x{max_dim} atlas layer, and the leaf-cohesion invariant forbids splitting a leaf \
         across layers. Raise `texel_density` or split the map."
    )]
    LeafTooLarge {
        leaf_index: u32,
        chart_count: usize,
        max_dim: u32,
    },
}

pub struct LightmapBakeCtx<'a> {
    pub bvh: &'a Bvh<f32, 3>,
    pub primitives: &'a [BvhPrimitive],
    /// Mutable: baker writes per-vertex lightmap UVs back after atlas placement.
    pub geometry: &'a mut GeometryResult,
    pub lights: &'a StaticBakedLights<'a>,
}

/// CLI-driven configuration for the lightmap bake. Fields are included in the
/// cache key so adding a field here automatically invalidates stale entries.
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
}

/// Output of a lightmap bake pass. The animated weight-map baker consumes
/// `charts` + `placements` + `atlas_width` to resolve chunk atlas rects.
#[derive(Debug)]
pub struct LightmapBakeOutput {
    pub section: LightmapSection,
    pub charts: Vec<Chart>,
    /// Parallel to `charts`. Empty when the bake short-circuits.
    pub placements: Vec<ChartPlacement>,
    pub atlas_width: u32,
    pub atlas_height: u32,
    /// Number of atlas array layers. `1` until the multi-bin packer (Task 3b)
    /// spills charts onto higher layers.
    pub layer_count: u32,
}

/// The pre-encode (pre-BC6H) lightmap atlas: the full per-texel `(irradiance,
/// direction, coverage)` buffers exactly as the per-texel bake leaves them after
/// edge dilation, before any irradiance/direction byte encoding.
///
/// This is the **byte-identity comparison seam** for the incremental-bake cache:
/// both the monolithic bake ([`bake_lightmap`]) and the per-light layer
/// compositor ([`crate::lightmap_layer::composite_layers`]) yield a
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

impl CompositedAtlas {
    /// Allocate a zero-initialized atlas sized for
    /// `layer_count × atlas_w × atlas_h` texels. Matches the monolithic bake's
    /// initial state: irradiance `0.0`, direction `Vec3::Y`, coverage `false`.
    pub fn zeroed(atlas_w: u32, atlas_h: u32, layer_count: u32) -> Self {
        let texels = (atlas_w * atlas_h * layer_count) as usize;
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

    /// Encode this atlas into a [`LightmapSection`] via the shared BC6H (or
    /// uncompressed-debug) irradiance path. The single section-22 encoder, so a
    /// composited atlas and a monolithically-baked one emit byte-identical
    /// sections when their buffers are equal.
    pub fn encode_section(
        &self,
        texel_density: f32,
        uncompressed_irradiance: bool,
    ) -> LightmapSection {
        let (irr_bytes, irradiance_format) = if uncompressed_irradiance {
            // RGBA16F is already flat layer-major (`w·h·8` per layer concatenated),
            // so the whole buffer encodes in one pass.
            (
                encode_irradiance_rgba16f(&self.irradiance),
                IRRADIANCE_FORMAT_RGBA16F,
            )
        } else {
            // The BC6H encoder is single-image — it asserts its input length is
            // exactly `w·h·4` floats — so it must run once per layer over that
            // layer's slice; the per-layer block blobs concatenate into the
            // layer-major irradiance blob.
            let plane = (self.atlas_width * self.atlas_height) as usize;
            let mut blob = Vec::new();
            for layer in 0..self.layer_count as usize {
                let start = layer * plane * 4;
                let end = start + plane * 4;
                blob.extend_from_slice(&bc6h::encode_bc6h_rgb_from_f32_rgba(
                    &self.irradiance[start..end],
                    self.atlas_width,
                    self.atlas_height,
                ));
            }
            (blob, IRRADIANCE_FORMAT_BC6H)
        };
        // Direction is Rgba8Unorm and already flat layer-major, so it encodes
        // whole-buffer like the RGBA16F irradiance path.
        let dir_bytes = encode_direction_rgba8(&self.direction, &self.coverage);
        LightmapSection {
            layer_count: self.layer_count,
            irr_width: self.atlas_width,
            irr_height: self.atlas_height,
            irr_texel_density: texel_density,
            irradiance: irr_bytes,
            irradiance_format,
            dir_width: self.atlas_width,
            dir_height: self.atlas_height,
            dir_texel_density: texel_density,
            direction: dir_bytes,
            mode: LightmapMode::Shadowed,
        }
    }
}

/// Cheap pre-bake setup: chart planning, shelf packing, and writing lightmap
/// UVs back into the geometry. Returned by [`prepare_atlas`] and consumed by
/// both the warm per-light composite path and the cold whole-atlas bake —
/// called once before either branch, so the atlas layout is shared.
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
        let charts = plan_charts(geom, texel_density);
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

    let charts = plan_charts(geom, texel_density);

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

/// Bake a directional lightmap. Returns a placeholder when there is nothing to bake.
pub fn bake_lightmap(
    inputs: &mut LightmapBakeCtx<'_>,
    config: &LightmapConfig,
) -> Result<LightmapBakeOutput, LightmapBakeError> {
    let texel_density = config.lightmap_density;
    let area_sample_count = config.area_sample_count;

    // Short-circuit on empty geometry: nothing for the atlas prep to do and the per-texel pass
    // would allocate zero-sized buffers.
    if inputs.geometry.geometry.vertices.is_empty() || inputs.geometry.geometry.faces.is_empty() {
        return Ok(LightmapBakeOutput {
            section: LightmapSection::placeholder(),
            charts: Vec::new(),
            placements: Vec::new(),
            atlas_width: 1,
            atlas_height: 1,
            layer_count: 1,
        });
    }

    let static_lights_empty = inputs.lights.is_empty();
    let prepared = prepare_atlas(inputs.geometry, inputs.lights, texel_density)?;

    // No static lights, or atlas prep produced no placements → emit a placeholder section but
    // return the planned charts/placements so downstream animated-light passes still have
    // per-face UV bounds.
    if static_lights_empty || prepared.placements.is_empty() {
        return Ok(LightmapBakeOutput {
            section: LightmapSection::placeholder(),
            charts: prepared.charts,
            placements: prepared.placements,
            atlas_width: prepared.atlas_width,
            atlas_height: prepared.atlas_height,
            layer_count: prepared.layer_count,
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

    let atlas = bake_monolithic_atlas(
        inputs.bvh,
        inputs.primitives,
        inputs.geometry,
        &static_lights,
        &charts,
        &placements,
        atlas_w,
        atlas_h,
        layer_count,
        area_sample_count,
    );

    // Compress to BC6H by default — `Bc6hRgbUfloat` is ~8× smaller than RGBA16F
    // on disk and in VRAM, and the smooth low-frequency irradiance compresses
    // cleanly under the in-tree single-mode encoder. The debug bypass leaves
    // output byte-identical to the pre-BC6H path so owner-side A/B compares
    // and round-trip determinism tests can run against a known-stable baseline.
    // `encode_section` is the shared section-22 encoder both this monolithic
    // path and the per-light compositor route through.
    let section = atlas.encode_section(texel_density, config.uncompressed_irradiance);

    Ok(LightmapBakeOutput {
        section,
        charts,
        placements,
        atlas_width: atlas_w,
        atlas_height: atlas_h,
        layer_count,
    })
}

/// Bake the full static-light atlas and dilate — the monolithic side of the
/// byte-identity comparison seam. Returns the pre-BC6H [`CompositedAtlas`] that
/// the per-light compositor ([`crate::lightmap_layer::composite_layers`]) must
/// reproduce bit-for-bit. `static_lights` must be in the same global order the
/// layer compositor sums in, and the atlas/charts/placements must be the shared
/// layout from a single [`prepare_atlas`] call.
///
/// `pub(crate)` so the layer-cache exactness test can build the monolithic atlas
/// directly (rather than re-deriving it from the encoded section).
#[allow(clippy::too_many_arguments)]
pub(crate) fn bake_monolithic_atlas(
    bvh: &Bvh<f32, 3>,
    primitives: &[BvhPrimitive],
    geometry: &GeometryResult,
    static_lights: &[&MapLight],
    charts: &[Chart],
    placements: &[ChartPlacement],
    atlas_w: u32,
    atlas_h: u32,
    layer_count: u32,
    area_sample_count: u32,
) -> CompositedAtlas {
    let mut atlas = CompositedAtlas::zeroed(atlas_w, atlas_h, layer_count);

    for (face_idx, placement) in placements.iter().enumerate() {
        bake_face_chart(
            bvh,
            primitives,
            geometry,
            static_lights,
            face_idx,
            &charts[face_idx],
            placement,
            atlas_w,
            atlas_h,
            area_sample_count,
            &mut atlas.irradiance,
            &mut atlas.direction,
            &mut atlas.coverage,
        );
    }

    atlas.dilate();
    atlas
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

pub fn log_stats(section: &LightmapSection, static_light_count: usize) {
    log::info!(
        "Lightmap: {}x{}x{} atlas, {} m/texel, {} static lights baked, irr={} B, dir={} B",
        section.irr_width,
        section.irr_height,
        section.layer_count,
        section.irr_texel_density,
        static_light_count,
        section.irradiance.len(),
        section.direction.len(),
    );
}

/// Face-local 2D chart plan. `pub` so the animated-light-chunks builder can reuse the same
/// per-face projection without duplicating the unwrap logic.
#[derive(Debug, Clone)]
pub struct Chart {
    pub origin: Vec3,
    pub u_axis: Vec3,
    pub v_axis: Vec3,
    pub uv_min: [f32; 2],
    pub uv_extent: [f32; 2],
    pub normal: Vec3,
    /// Includes padding.
    pub width_texels: u32,
    pub height_texels: u32,
    /// BSP leaf this chart's face belongs to. The multi-bin packer keeps all of
    /// one leaf's charts on a single atlas array layer (a leaf is the runtime
    /// draw/visibility unit, so its charts must share a layer to avoid a
    /// per-face layer switch in the hot path).
    pub leaf_index: u32,
}

fn plan_charts(geom: &GeometryResult, texel_density: f32) -> Vec<Chart> {
    let density = texel_density.max(1.0e-4);
    let section = &geom.geometry;

    let mut charts = Vec::with_capacity(section.faces.len());
    for (face_index, range) in geom.face_index_ranges.iter().enumerate() {
        // Charts map 1:1 with faces, so the chart's leaf is its face's leaf.
        let leaf_index = section.faces[face_index].leaf_index;
        let start = range.index_offset as usize;
        // A degenerate face still belongs to its leaf, so its placeholder chart
        // carries the real `leaf_index` — keeping the leaf's charts cohesive in
        // the packer even when one face is degenerate.
        if range.index_count < 3 {
            charts.push(empty_chart_for_leaf(leaf_index));
            continue;
        }

        let i0 = section.indices[start] as usize;
        let i1 = section.indices[start + 1] as usize;
        let i2 = section.indices[start + 2] as usize;
        let p0 = Vec3::from(section.vertices[i0].position);
        let p1 = Vec3::from(section.vertices[i1].position);
        let p2 = Vec3::from(section.vertices[i2].position);

        let edge1 = p1 - p0;
        let edge2 = p2 - p0;
        let normal_raw = edge1.cross(edge2);
        if normal_raw.length_squared() < 1.0e-12 {
            charts.push(empty_chart_for_leaf(leaf_index));
            continue;
        }
        // Prefer the stored vertex normal: the cross-product direction depends on winding and can
        // invert the Lambert term if it disagrees with the stored normal.
        let stored_normal_raw = section.vertices[i0].decode_normal();
        let stored_normal = Vec3::new(
            stored_normal_raw[0],
            stored_normal_raw[1],
            stored_normal_raw[2],
        );
        let normal = if stored_normal.length_squared() > 0.5 {
            stored_normal.normalize()
        } else {
            normal_raw.normalize()
        };

        let u_axis = edge1.normalize_or_zero();
        if u_axis.length_squared() < 0.5 {
            charts.push(empty_chart_for_leaf(leaf_index));
            continue;
        }
        let v_axis = normal.cross(u_axis).normalize_or_zero();
        if v_axis.length_squared() < 0.5 {
            charts.push(empty_chart_for_leaf(leaf_index));
            continue;
        }

        let mut u_min = f32::INFINITY;
        let mut u_max = f32::NEG_INFINITY;
        let mut v_min = f32::INFINITY;
        let mut v_max = f32::NEG_INFINITY;

        let mut seen_verts: Vec<usize> = Vec::new();
        let mut seen_set: HashSet<usize> = HashSet::new();
        let end = start + range.index_count as usize;
        let mut tri = start;
        while tri + 3 <= end {
            for j in 0..3 {
                let vi = section.indices[tri + j] as usize;
                if seen_set.insert(vi) {
                    seen_verts.push(vi);
                }
            }
            tri += 3;
        }

        for &vi in &seen_verts {
            let p = Vec3::from(section.vertices[vi].position);
            let rel = p - p0;
            let u = rel.dot(u_axis);
            let v = rel.dot(v_axis);
            if u < u_min {
                u_min = u;
            }
            if u > u_max {
                u_max = u;
            }
            if v < v_min {
                v_min = v;
            }
            if v > v_max {
                v_max = v;
            }
        }

        let u_extent = (u_max - u_min).max(density);
        let v_extent = (v_max - v_min).max(density);

        let width_texels = ((u_extent / density).ceil() as u32 + 2 * CHART_PADDING_TEXELS).max(1);
        let height_texels = ((v_extent / density).ceil() as u32 + 2 * CHART_PADDING_TEXELS).max(1);

        charts.push(Chart {
            origin: p0,
            u_axis,
            v_axis,
            uv_min: [u_min, v_min],
            uv_extent: [u_extent, v_extent],
            normal,
            width_texels,
            height_texels,
            leaf_index,
        });
    }
    charts
}

/// Placeholder chart for a degenerate face. Carries the face's `leaf_index` so
/// the packer keeps the leaf's charts cohesive even when one face degenerates.
fn empty_chart_for_leaf(leaf_index: u32) -> Chart {
    Chart {
        origin: Vec3::ZERO,
        u_axis: Vec3::X,
        v_axis: Vec3::Y,
        uv_min: [0.0, 0.0],
        uv_extent: [0.0, 0.0],
        normal: Vec3::Y,
        width_texels: 1,
        height_texels: 1,
        leaf_index,
    }
}

/// Result of multi-bin atlas packing. All layers share one `(atlas_width,
/// atlas_height)` — a `texture_2d_array` has a single per-layer dimension across
/// every layer — and each placement carries the layer its chart landed on.
/// `placements` is parallel to the input `charts`.
#[derive(Debug)]
pub struct PackOutput {
    pub layer_count: u32,
    pub atlas_width: u32,
    pub atlas_height: u32,
    pub placements: Vec<ChartPlacement>,
}

/// Round a raw texel extent up to the packer's dimension contract: power-of-two,
/// a multiple of 4 (BC6H block alignment — a pow2 ≥ 4 is 4-aligned for free),
/// and within `[MIN_ATLAS_DIMENSION, max_dim]`.
fn round_atlas_dim(raw: u32, max_dim: u32) -> u32 {
    raw.max(MIN_ATLAS_DIMENSION)
        .next_power_of_two()
        .min(max_dim)
}

/// Pack charts into a multi-layer atlas with leaf-aware MaxRects binning.
///
/// `max_dim` bounds each layer's width and height (production passes
/// [`MAX_ATLAS_DIMENSION`]; tests pass a small value). Every chart's largest
/// side must be `≤ max_dim` or [`LightmapBakeError::ChartTooLarge`] is returned
/// (this is the single source of truth for that check). The number of layers is
/// capped at [`MAX_ATLAS_LAYERS`]; exceeding it yields
/// [`LightmapBakeError::LayerOverflow`].
///
/// Leaf cohesion is a hard invariant: all charts of one BVH leaf land on a
/// single layer (a leaf is the runtime draw/visibility unit, so straddling a
/// layer boundary would force a per-face layer switch in the hot path). A leaf
/// is packed as a unit into the current layer's free rectangles; if it doesn't
/// all fit, the WHOLE leaf rolls to a fresh layer — partial placements from the
/// failed attempt are discarded.
///
/// The shared `(atlas_width, atlas_height)` is sized to host the largest single
/// leaf in one layer (grown by doubling, capped at `max_dim`), so no leaf is
/// ever forced to split for want of room within a layer.
fn pack_layers(
    charts: &[Chart],
    max_dim: u32,
    density_m_per_texel: f32,
) -> Result<PackOutput, LightmapBakeError> {
    if charts.is_empty() {
        return Ok(PackOutput {
            layer_count: 1,
            atlas_width: MIN_ATLAS_DIMENSION,
            atlas_height: MIN_ATLAS_DIMENSION,
            placements: Vec::new(),
        });
    }

    // ChartTooLarge is now owned here: a single chart wider/taller than a layer
    // can never be placed, regardless of how many layers we open.
    for (face_index, chart) in charts.iter().enumerate() {
        if chart.width_texels > max_dim || chart.height_texels > max_dim {
            return Err(LightmapBakeError::ChartTooLarge {
                face_index,
                width_texels: chart.width_texels,
                height_texels: chart.height_texels,
                max: max_dim,
                u_extent_m: chart.uv_extent[0],
                v_extent_m: chart.uv_extent[1],
                density_m_per_texel,
            });
        }
    }

    // Group chart indices by leaf, preserving first-seen order so packing is
    // deterministic (no HashMap iteration leaking into placement). Each group is
    // a contiguous unit the packer places together.
    let leaves = group_charts_by_leaf(charts);

    // Size the shared per-layer dimension to fit the largest leaf in a single
    // layer. Start from a square that covers each leaf's total area and largest
    // chart side, then grow (doubling) until every leaf packs alone.
    let atlas_dim = choose_layer_dim(charts, &leaves, max_dim);

    // Pack leaves into layers. Each leaf tries the current layer's MaxRects free
    // list; a leaf that doesn't fit rolls whole to a new layer.
    let mut placements = vec![
        ChartPlacement {
            x: 0,
            y: 0,
            layer: 0
        };
        charts.len()
    ];
    let mut layer: u32 = 0;
    let mut packer = MaxRects::new(atlas_dim, atlas_dim);

    for leaf in &leaves {
        if !place_leaf(&mut packer, charts, leaf, layer, &mut placements) {
            // The leaf didn't fit in the current layer — open a fresh one and
            // place the whole leaf there. Sizing guarantees a leaf fits an empty
            // layer, so this single retry always succeeds.
            layer += 1;
            if layer >= MAX_ATLAS_LAYERS {
                return Err(LightmapBakeError::LayerOverflow {
                    layer_count: layer + 1,
                    max: MAX_ATLAS_LAYERS,
                });
            }
            packer = MaxRects::new(atlas_dim, atlas_dim);
            let fit = place_leaf(&mut packer, charts, leaf, layer, &mut placements);
            if !fit {
                // A single leaf too large to fit even an empty `max_dim²` layer
                // can never be placed — and leaf cohesion forbids splitting it.
                // Error rather than `debug_assert!` so release builds reject it
                // instead of silently leaving its charts at the default `{0,0,N}`.
                return Err(LightmapBakeError::LeafTooLarge {
                    leaf_index: leaf.first().map(|&i| charts[i].leaf_index).unwrap_or(0),
                    chart_count: leaf.len(),
                    max_dim,
                });
            }
        }
    }

    Ok(PackOutput {
        layer_count: layer + 1,
        atlas_width: atlas_dim,
        atlas_height: atlas_dim,
        placements,
    })
}

/// Group chart indices by `leaf_index`, preserving first-seen leaf order. Charts
/// arrive leaf-ordered from `extract_geometry`, so this is usually contiguous,
/// but the grouping does not rely on that.
fn group_charts_by_leaf(charts: &[Chart]) -> Vec<Vec<usize>> {
    let mut order: Vec<u32> = Vec::new();
    let mut groups: std::collections::HashMap<u32, Vec<usize>> = std::collections::HashMap::new();
    for (i, c) in charts.iter().enumerate() {
        groups.entry(c.leaf_index).or_insert_with(|| {
            order.push(c.leaf_index);
            Vec::new()
        });
        groups.get_mut(&c.leaf_index).unwrap().push(i);
    }
    order
        .into_iter()
        .map(|leaf| groups.remove(&leaf).unwrap())
        .collect()
}

/// Choose the shared per-layer square dimension. It must host the largest single
/// leaf in one layer, so we grow (doubling, 4-aligned pow2, capped at `max_dim`)
/// until each leaf packs alone via MaxRects.
fn choose_layer_dim(charts: &[Chart], leaves: &[Vec<usize>], max_dim: u32) -> u32 {
    // Lower bound from the densest leaf: its total area and its widest/tallest
    // chart both have to fit one layer.
    let mut min_side = MIN_ATLAS_DIMENSION;
    for leaf in leaves {
        let area: u64 = leaf
            .iter()
            .map(|&i| charts[i].width_texels as u64 * charts[i].height_texels as u64)
            .sum();
        let side_from_area = (area as f64).sqrt().ceil() as u32;
        let max_chart_side = leaf
            .iter()
            .map(|&i| charts[i].width_texels.max(charts[i].height_texels))
            .max()
            .unwrap_or(0);
        min_side = min_side.max(side_from_area).max(max_chart_side);
    }

    let mut dim = round_atlas_dim(min_side, max_dim);
    loop {
        // A leaf fits this dimension if MaxRects places all its charts in one
        // empty layer. The area lower bound is optimistic (ignores fragmentation),
        // so confirm with a real pack and grow if any leaf overflows.
        let all_fit = leaves.iter().all(|leaf| {
            let mut packer = MaxRects::new(dim, dim);
            leaf.iter().all(|&i| {
                packer
                    .insert(charts[i].width_texels, charts[i].height_texels)
                    .is_some()
            })
        });
        if all_fit || dim >= max_dim {
            return dim;
        }
        dim = round_atlas_dim(dim + 1, max_dim);
    }
}

/// Try to place all of `leaf`'s charts into `packer` (the current layer). On
/// success, writes each placement at `layer` and returns `true`. On the first
/// chart that doesn't fit, returns `false` WITHOUT mutating `placements` for the
/// charts it did place — the caller rolls the whole leaf to a fresh layer, so
/// any partial work in `packer` is discarded with the packer itself.
fn place_leaf(
    packer: &mut MaxRects,
    charts: &[Chart],
    leaf: &[usize],
    layer: u32,
    placements: &mut [ChartPlacement],
) -> bool {
    // Largest-first within the leaf packs big charts before the free list
    // fragments — the standard MaxRects ordering for density.
    let mut order: Vec<usize> = leaf.to_vec();
    order.sort_by(|&a, &b| {
        let area_a = charts[a].width_texels as u64 * charts[a].height_texels as u64;
        let area_b = charts[b].width_texels as u64 * charts[b].height_texels as u64;
        area_b
            .cmp(&area_a)
            // Tie-break on chart index so the order is fully deterministic.
            .then(a.cmp(&b))
    });

    let mut staged: Vec<(usize, u32, u32)> = Vec::with_capacity(order.len());
    for &i in &order {
        match packer.insert(charts[i].width_texels, charts[i].height_texels) {
            Some((x, y)) => staged.push((i, x, y)),
            None => return false,
        }
    }
    for (i, x, y) in staged {
        placements[i] = ChartPlacement { x, y, layer };
    }
    true
}

/// MaxRects free-rectangle bin packer for one atlas layer. Maintains a list of
/// maximal free rectangles; each insert picks the best-short-side-fit free rect,
/// splits it, and prunes any free rects now contained in another. This is the
/// genuine MaxRects algorithm (Jylänki 2010), not a shelf fallback, so it reaches
/// the 85–95% density target on mixed chart sets.
struct MaxRects {
    free: Vec<Rect>,
}

#[derive(Clone, Copy)]
struct Rect {
    x: u32,
    y: u32,
    w: u32,
    h: u32,
}

impl MaxRects {
    fn new(width: u32, height: u32) -> Self {
        MaxRects {
            free: vec![Rect {
                x: 0,
                y: 0,
                w: width,
                h: height,
            }],
        }
    }

    /// Place a `w × h` rectangle, returning its top-left `(x, y)` on success.
    /// Best-Short-Side-Fit: among free rects that can host it, pick the one
    /// leaving the smallest leftover short side (ties broken by long side, then
    /// position) for a deterministic, dense placement.
    fn insert(&mut self, w: u32, h: u32) -> Option<(u32, u32)> {
        let mut best: Option<(usize, u32, u32)> = None; // (free idx, short leftover, long leftover)
        for (idx, r) in self.free.iter().enumerate() {
            if r.w >= w && r.h >= h {
                let leftover_x = r.w - w;
                let leftover_y = r.h - h;
                let short = leftover_x.min(leftover_y);
                let long = leftover_x.max(leftover_y);
                let better = match best {
                    None => true,
                    Some((_, bs, bl)) => short < bs || (short == bs && long < bl),
                };
                if better {
                    best = Some((idx, short, long));
                }
            }
        }

        let (idx, _, _) = best?;
        let placed = Rect {
            x: self.free[idx].x,
            y: self.free[idx].y,
            w,
            h,
        };

        // Split every free rect that overlaps the placed rect into the maximal
        // sub-rects that don't, then prune any free rect contained in another.
        let mut i = 0;
        while i < self.free.len() {
            if let Some(splits) = split_free(&self.free[i], &placed) {
                self.free.swap_remove(i);
                self.free.extend(splits);
            } else {
                i += 1;
            }
        }
        self.prune();

        Some((placed.x, placed.y))
    }

    /// Drop any free rect fully contained in another — the maximality invariant
    /// MaxRects relies on (without it the free list grows unbounded with
    /// redundant rects).
    fn prune(&mut self) {
        let mut i = 0;
        while i < self.free.len() {
            let mut removed = false;
            let mut j = 0;
            while j < self.free.len() {
                if i != j && contains(&self.free[j], &self.free[i]) {
                    self.free.swap_remove(i);
                    removed = true;
                    break;
                }
                j += 1;
            }
            if !removed {
                i += 1;
            }
        }
    }
}

/// True if `outer` fully contains `inner`.
fn contains(outer: &Rect, inner: &Rect) -> bool {
    inner.x >= outer.x
        && inner.y >= outer.y
        && inner.x + inner.w <= outer.x + outer.w
        && inner.y + inner.h <= outer.y + outer.h
}

/// If `placed` overlaps `free`, return the maximal sub-rects of `free` that lie
/// outside `placed` (up to four: left, right, top, bottom slabs). Returns `None`
/// when they don't overlap — `free` is left untouched.
fn split_free(free: &Rect, placed: &Rect) -> Option<Vec<Rect>> {
    let no_overlap = placed.x >= free.x + free.w
        || placed.x + placed.w <= free.x
        || placed.y >= free.y + free.h
        || placed.y + placed.h <= free.y;
    if no_overlap {
        return None;
    }

    let mut out = Vec::with_capacity(4);
    // Left slab.
    if placed.x > free.x {
        out.push(Rect {
            x: free.x,
            y: free.y,
            w: placed.x - free.x,
            h: free.h,
        });
    }
    // Right slab.
    if placed.x + placed.w < free.x + free.w {
        out.push(Rect {
            x: placed.x + placed.w,
            y: free.y,
            w: (free.x + free.w) - (placed.x + placed.w),
            h: free.h,
        });
    }
    // Bottom slab.
    if placed.y > free.y {
        out.push(Rect {
            x: free.x,
            y: free.y,
            w: free.w,
            h: placed.y - free.y,
        });
    }
    // Top slab.
    if placed.y + placed.h < free.y + free.h {
        out.push(Rect {
            x: free.x,
            y: placed.y + placed.h,
            w: free.w,
            h: (free.y + free.h) - (placed.y + placed.h),
        });
    }
    Some(out)
}

fn assign_lightmap_uvs(geom: &mut GeometryResult, charts: &[Chart], pack: &PackOutput) {
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

#[allow(clippy::too_many_arguments)]
fn bake_face_chart(
    bvh: &Bvh<f32, 3>,
    primitives: &[BvhPrimitive],
    geometry: &GeometryResult,
    static_lights: &[&MapLight],
    _face_idx: usize,
    chart: &Chart,
    placement: &ChartPlacement,
    atlas_w: u32,
    atlas_h: u32,
    area_sample_count: u32,
    irradiance: &mut [f32],
    direction: &mut [Vec3],
    coverage: &mut [bool],
) {
    if chart.uv_extent[0] <= 0.0 || chart.uv_extent[1] <= 0.0 {
        return;
    }
    let padding = CHART_PADDING_TEXELS as i32;
    let (interior_w, interior_h) = crate::chart_raster::chart_interior_dims(chart);

    // Offset into this chart's atlas layer. The buffers are layer-major, so a
    // chart on layer `l` writes into the slice starting at `l × (w × h)`.
    let layer_offset = placement.layer as usize * (atlas_w * atlas_h) as usize;

    for ty in 0..interior_h {
        for tx in 0..interior_w {
            let atlas_x = placement.x as i32 + padding + tx;
            let atlas_y = placement.y as i32 + padding + ty;
            let idx = layer_offset + (atlas_y as u32 * atlas_w + atlas_x as u32) as usize;

            // Shared helper keeps static and animated-weight bakers aligned at chunk boundaries.
            let world_p = chart_texel_world_position(chart, tx, ty, interior_w, interior_h);
            let surface_normal = chart.normal;

            // Seed the area-sampling lattice from a fixed integer hash of the
            // atlas-space texel coords. Deterministic (no `RandomState`) so the
            // bake is byte-identical across processes — the cache requires it —
            // while still decorrelating adjacent texels so penumbra bands don't
            // share an identical sample rotation.
            let seed = texel_seed(atlas_x as u32, atlas_y as u32);

            let mut irr = Vec3::ZERO;
            let mut weighted_dir = Vec3::ZERO;
            for light in static_lights {
                let (irr_contrib, dir_contrib) = light_texel_contribution(
                    light,
                    world_p,
                    surface_normal,
                    seed,
                    area_sample_count,
                    |from, to| segment_clear(bvh, primitives, geometry, from, to),
                );
                irr += irr_contrib;
                weighted_dir += dir_contrib;
            }

            irradiance[idx * 4] = irr.x;
            irradiance[idx * 4 + 1] = irr.y;
            irradiance[idx * 4 + 2] = irr.z;
            irradiance[idx * 4 + 3] = 1.0;

            let dir = if weighted_dir.length_squared() > 1.0e-8 {
                weighted_dir.normalize()
            } else {
                // No contribution — surface normal degrades bumped-Lambert to flat Lambert.
                surface_normal
            };
            direction[idx] = dir;
            coverage[idx] = true;
        }
    }
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

/// One light's contribution to a single atlas texel: the shadowed irradiance
/// (RGB) and the unnormalized weighted direction (`to_light * luminance`),
/// the exact two terms `bake_face_chart` accumulates per light before the
/// per-texel `irr` sum and `weighted_dir.normalize()`.
///
/// Factored out so the monolithic bake (`bake_face_chart`, summing over all
/// lights) and the per-light layer bake (`crate::lightmap_layer`, calling it
/// once) share byte-identical math — the cache's bit-for-bit composite gate
/// depends on both paths producing the same float terms in the same order.
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
    let (contribution, to_light) = light_contribution_and_direction(light, world_p, surface_normal);
    if contribution.length_squared() <= 1.0e-12 {
        return (Vec3::ZERO, Vec3::ZERO);
    }
    // Lightmaps always bake shadowed: an occluded texel goes dark so a
    // `static_light_map` light's static shadow lives in the atlas.
    // `soft_visibility` returns the `[0,1]` unoccluded fraction over an area
    // sample of the emitter — a multi-texel penumbra instead of a hard 1-texel
    // step. `sdf` lights are filtered out of the static set upstream (their
    // direct shadow resolves at runtime), so no double-shadow.
    let v = soft_visibility(
        world_p,
        surface_normal,
        light,
        seed,
        area_sample_count,
        trace,
    );
    if v <= 0.0 {
        return (Vec3::ZERO, Vec3::ZERO);
    }
    let irr = contribution * v;
    // Weight the dominant-direction accumulation by `v` too: in a penumbra a
    // partially-occluded light should bias the baked direction less than a
    // fully-visible one, matching the softened irradiance it contributes.
    let lum = (contribution.x + contribution.y + contribution.z) * v;
    let weighted_dir = to_light * lum;
    (irr, weighted_dir)
}

fn falloff(light: &MapLight, distance: f32) -> f32 {
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

/// Deterministic per-texel seed for `soft_visibility`'s sample-lattice rotation.
/// An FNV-1a hash of the atlas-space `(x, y)` — a fixed integer mix, never a
/// `RandomState` or any hash whose seed varies between processes — so the bake is
/// byte-identical across separate runs (the build cache reuses stored bytes
/// verbatim and would break on any run-to-run drift); `soft_visibility` XORs this
/// with `SAMPLING_LATTICE_OFFSET` internally.
///
/// The animated weight-map stage derives its own per-texel seed with a different
/// mixer (SplitMix64). The two need not match: they bake into INDEPENDENT atlases,
/// so each only needs to be deterministic within its own stage — not byte-identical
/// to the other.
pub(crate) fn texel_seed(x: u32, y: u32) -> u64 {
    const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut h = FNV_OFFSET;
    h = (h ^ x as u64).wrapping_mul(FNV_PRIME);
    h = (h ^ y as u64).wrapping_mul(FNV_PRIME);
    h
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
/// the caller supplies `seed` deterministically (texel `(x, y)` hash, or
/// probe/ray/light indices) so the same inputs yield byte-identical output.
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
    let full_samples = full_samples.max(SOFT_PROBE_SAMPLES);
    let probes = probe_indices(light, full_samples);
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
    let candidates = bvh.traverse(&ray, primitives);
    let max_distance = length - RAY_EPSILON;
    let geom = &geometry.geometry;
    for prim in candidates {
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

fn encode_irradiance_rgba16f(data: &[f32]) -> Vec<u8> {
    let texel_count = data.len() / 4;
    let mut out = Vec::with_capacity(texel_count * 8);
    for t in 0..texel_count {
        let r = f32_to_f16_bits(data[t * 4]);
        let g = f32_to_f16_bits(data[t * 4 + 1]);
        let b = f32_to_f16_bits(data[t * 4 + 2]);
        let a = f32_to_f16_bits(data[t * 4 + 3]);
        out.extend_from_slice(&r.to_le_bytes());
        out.extend_from_slice(&g.to_le_bytes());
        out.extend_from_slice(&b.to_le_bytes());
        out.extend_from_slice(&a.to_le_bytes());
    }
    out
}

fn encode_direction_rgba8(direction: &[Vec3], coverage: &[bool]) -> Vec<u8> {
    let mut out = Vec::with_capacity(direction.len() * 4);
    for (i, d) in direction.iter().enumerate() {
        let bytes = if coverage[i] {
            encode_direction_oct([d.x, d.y, d.z])
        } else {
            // Neutral up-direction: stray bilinear samples return a Lambert-valid vector.
            [128u8, 255, 128, 255]
        };
        out.extend_from_slice(&bytes);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bvh_build::build_bvh;
    use crate::geometry::FaceIndexRange;
    use glam::DVec3;
    use postretro_level_format::geometry::{FaceMeta, GeometrySection, Vertex};
    use postretro_level_format::texture_names::TextureNamesSection;

    fn unit_quad_geometry() -> GeometryResult {
        let v0 = Vertex::new(
            [0.0, 0.0, 0.0],
            [0.0, 0.0],
            [0.0, 1.0, 0.0],
            [1.0, 0.0, 0.0],
            true,
            [0.0, 0.0],
            0,
        );
        let v1 = Vertex::new(
            [1.0, 0.0, 0.0],
            [1.0, 0.0],
            [0.0, 1.0, 0.0],
            [1.0, 0.0, 0.0],
            true,
            [0.0, 0.0],
            0,
        );
        let v2 = Vertex::new(
            [1.0, 0.0, 1.0],
            [1.0, 1.0],
            [0.0, 1.0, 0.0],
            [1.0, 0.0, 0.0],
            true,
            [0.0, 0.0],
            0,
        );
        let v3 = Vertex::new(
            [0.0, 0.0, 1.0],
            [0.0, 1.0],
            [0.0, 1.0, 0.0],
            [1.0, 0.0, 0.0],
            true,
            [0.0, 0.0],
            0,
        );
        GeometryResult {
            geometry: GeometrySection {
                vertices: vec![v0, v1, v2, v3],
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

    fn point_light_above() -> MapLight {
        MapLight {
            origin: DVec3::new(0.5, 1.0, 0.5),
            light_type: LightType::Point,
            intensity: 1.0,
            color: [1.0, 1.0, 1.0],
            falloff_model: FalloffModel::Linear,
            falloff_range: 5.0,
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
            shadow_type: crate::map_data::ShadowType::StaticLightMap,
        }
    }

    #[test]
    fn empty_geometry_returns_placeholder() {
        let mut geo = GeometryResult {
            geometry: GeometrySection {
                vertices: vec![],
                indices: vec![],
                faces: vec![],
            },
            texture_names: TextureNamesSection { names: Vec::new() },
            face_index_ranges: vec![],
        };
        let bvh = bvh::bvh::Bvh { nodes: Vec::new() };
        let prims: Vec<BvhPrimitive> = Vec::new();
        let lights = vec![point_light_above()];
        let static_lights = StaticBakedLights::from_lights(&lights);
        let mut inputs = LightmapBakeCtx {
            bvh: &bvh,
            primitives: &prims,
            geometry: &mut geo,
            lights: &static_lights,
        };
        let section = bake_lightmap(
            &mut inputs,
            &LightmapConfig {
                lightmap_density: DEFAULT_TEXEL_DENSITY_METERS,
                area_sample_count: DEFAULT_AREA_SAMPLE_COUNT,
                uncompressed_irradiance: false,
            },
        )
        .unwrap()
        .section;
        assert_eq!(section.irr_width, 1);
        assert_eq!(section.irr_height, 1);
    }

    #[test]
    fn no_static_lights_returns_placeholder() {
        let mut geo = unit_quad_geometry();
        let (bvh, prims, _) = build_bvh(&geo).unwrap();
        let lights: Vec<MapLight> = Vec::new();
        let static_lights = StaticBakedLights::from_lights(&lights);
        let mut inputs = LightmapBakeCtx {
            bvh: &bvh,
            primitives: &prims,
            geometry: &mut geo,
            lights: &static_lights,
        };
        let section = bake_lightmap(
            &mut inputs,
            &LightmapConfig {
                lightmap_density: DEFAULT_TEXEL_DENSITY_METERS,
                area_sample_count: DEFAULT_AREA_SAMPLE_COUNT,
                uncompressed_irradiance: false,
            },
        )
        .unwrap()
        .section;
        assert_eq!(section.irr_width, 1);
        assert_eq!(section.irr_height, 1);
    }

    #[test]
    fn single_static_light_produces_nonzero_irradiance() {
        let mut geo = unit_quad_geometry();
        let (bvh, prims, _) = build_bvh(&geo).unwrap();
        let lights = vec![point_light_above()];
        let static_lights = StaticBakedLights::from_lights(&lights);
        let mut inputs = LightmapBakeCtx {
            bvh: &bvh,
            primitives: &prims,
            geometry: &mut geo,
            lights: &static_lights,
        };
        let section = bake_lightmap(
            &mut inputs,
            &LightmapConfig {
                lightmap_density: 0.25,
                area_sample_count: DEFAULT_AREA_SAMPLE_COUNT,
                // Test reads per-texel bytes assuming the 8-byte RGBA16F stride;
                // request the uncompressed debug bypass so the byte layout stays
                // readable without re-implementing the GPU's BC6H decode here.
                uncompressed_irradiance: true,
            },
        )
        .unwrap()
        .section;
        assert!(section.irr_width >= MIN_ATLAS_DIMENSION);
        assert!(section.irr_height >= MIN_ATLAS_DIMENSION);
        assert_eq!(
            section.irradiance.len(),
            (section.irr_width * section.irr_height * 8) as usize
        );
        let mut has_nonzero = false;
        for chunk in section.irradiance.chunks_exact(2).step_by(4) {
            let bits = u16::from_le_bytes([chunk[0], chunk[1]]);
            if bits != 0 {
                has_nonzero = true;
                break;
            }
        }
        assert!(
            has_nonzero,
            "expected at least one non-zero irradiance texel"
        );
    }

    /// Disjoint-direct contract (sdf-per-light-shadows Task 3): an `sdf`-typed
    /// light stays in the `StaticBakedLights` namespace (it's `!is_dynamic`, so
    /// SH still bakes its bounce), but the direct lightmap consumer drops it —
    /// `lm_irr` carries no direct term for it (the runtime SDF trace resolves
    /// that). The atlas preps to a real size (the namespace is non-empty) yet
    /// every irradiance texel is zero, in contrast to the `static_light_map`
    /// case above which produces non-zero irradiance from the same geometry.
    #[test]
    fn sdf_typed_light_excluded_from_direct_lightmap() {
        let mut geo = unit_quad_geometry();
        let (bvh, prims, _) = build_bvh(&geo).unwrap();
        let mut sdf_light = point_light_above();
        sdf_light.shadow_type = crate::map_data::ShadowType::Sdf;
        let lights = vec![sdf_light];
        let static_lights = StaticBakedLights::from_lights(&lights);
        // The sdf light is in the namespace (feeds SH); only the direct bake drops it.
        assert_eq!(
            static_lights.len(),
            1,
            "sdf light must remain in StaticBakedLights (keys on position, not shadow type)",
        );
        let mut inputs = LightmapBakeCtx {
            bvh: &bvh,
            primitives: &prims,
            geometry: &mut geo,
            lights: &static_lights,
        };
        let section = bake_lightmap(
            &mut inputs,
            &LightmapConfig {
                lightmap_density: 0.25,
                area_sample_count: DEFAULT_AREA_SAMPLE_COUNT,
                // See `single_static_light_produces_nonzero_irradiance`: this
                // test reads per-texel bytes from the irradiance blob, so it
                // requests the RGBA16F debug bypass rather than the default
                // BC6H production layout.
                uncompressed_irradiance: true,
            },
        )
        .unwrap()
        .section;
        let mut has_nonzero = false;
        for chunk in section.irradiance.chunks_exact(2).step_by(4) {
            let bits = u16::from_le_bytes([chunk[0], chunk[1]]);
            if bits != 0 {
                has_nonzero = true;
                break;
            }
        }
        assert!(
            !has_nonzero,
            "an sdf-typed light must contribute no direct lightmap irradiance",
        );
    }

    #[test]
    fn is_dynamic_lights_skipped_by_bake() {
        let mut geo = unit_quad_geometry();
        let (bvh, prims, _) = build_bvh(&geo).unwrap();
        let mut dyn_light = point_light_above();
        dyn_light.is_dynamic = true;
        let lights = vec![dyn_light];
        let static_lights = StaticBakedLights::from_lights(&lights);
        let mut inputs = LightmapBakeCtx {
            bvh: &bvh,
            primitives: &prims,
            geometry: &mut geo,
            lights: &static_lights,
        };
        let section = bake_lightmap(
            &mut inputs,
            &LightmapConfig {
                lightmap_density: DEFAULT_TEXEL_DENSITY_METERS,
                area_sample_count: DEFAULT_AREA_SAMPLE_COUNT,
                uncompressed_irradiance: false,
            },
        )
        .unwrap()
        .section;
        assert_eq!(section.irr_width, 1);
        assert_eq!(section.irr_height, 1);
    }

    #[test]
    fn static_nonanimated_bakes_but_dynamic_and_animated_do_not() {
        // The static atlas carries only non-animated static lights. Dynamic and animated lights
        // are owned by the runtime direct-lighting and weight-map compose passes respectively;
        // baking them here would double-count.
        use crate::map_data::LightAnimation;

        let mut geo_static = unit_quad_geometry();
        let (bvh, prims, _) = build_bvh(&geo_static).unwrap();
        let base = point_light_above();
        let static_base = StaticBakedLights::from_lights(std::slice::from_ref(&base));
        let mut inputs = LightmapBakeCtx {
            bvh: &bvh,
            primitives: &prims,
            geometry: &mut geo_static,
            lights: &static_base,
        };
        let section_static = bake_lightmap(
            &mut inputs,
            &LightmapConfig {
                lightmap_density: 0.25,
                area_sample_count: DEFAULT_AREA_SAMPLE_COUNT,
                uncompressed_irradiance: false,
            },
        )
        .unwrap()
        .section;
        assert!(
            section_static.irr_width >= MIN_ATLAS_DIMENSION,
            "non-animated static light must bake into a real atlas",
        );

        let mut dyn_light = point_light_above();
        dyn_light.is_dynamic = true;
        let mut geo_dyn = unit_quad_geometry();
        let (bvh_d, prims_d, _) = build_bvh(&geo_dyn).unwrap();
        let static_dyn = StaticBakedLights::from_lights(std::slice::from_ref(&dyn_light));
        let mut inputs_d = LightmapBakeCtx {
            bvh: &bvh_d,
            primitives: &prims_d,
            geometry: &mut geo_dyn,
            lights: &static_dyn,
        };
        let section_dyn = bake_lightmap(
            &mut inputs_d,
            &LightmapConfig {
                lightmap_density: 0.25,
                area_sample_count: DEFAULT_AREA_SAMPLE_COUNT,
                uncompressed_irradiance: false,
            },
        )
        .unwrap()
        .section;
        assert_eq!(section_dyn.irr_width, 1, "is_dynamic light must not bake");

        let mut anim_light = point_light_above();
        anim_light.animation = Some(LightAnimation {
            period: 1.0,
            phase: 0.0,
            brightness: Some(vec![1.0, 0.5]),
            color: None,
            direction: None,
            start_active: true,
        });
        let mut geo_anim = unit_quad_geometry();
        let (bvh_a, prims_a, _) = build_bvh(&geo_anim).unwrap();
        let static_anim = StaticBakedLights::from_lights(std::slice::from_ref(&anim_light));
        let mut inputs_a = LightmapBakeCtx {
            bvh: &bvh_a,
            primitives: &prims_a,
            geometry: &mut geo_anim,
            lights: &static_anim,
        };
        let section_anim = bake_lightmap(
            &mut inputs_a,
            &LightmapConfig {
                lightmap_density: 0.25,
                area_sample_count: DEFAULT_AREA_SAMPLE_COUNT,
                uncompressed_irradiance: false,
            },
        )
        .unwrap()
        .section;
        assert_eq!(
            section_anim.irr_width, 1,
            "animated light must not contribute to the static atlas",
        );

        let mut bake_only_anim = anim_light.clone();
        bake_only_anim.bake_only = true;
        let mut geo_bo = unit_quad_geometry();
        let (bvh_b, prims_b, _) = build_bvh(&geo_bo).unwrap();
        let static_bo = StaticBakedLights::from_lights(std::slice::from_ref(&bake_only_anim));
        let mut inputs_b = LightmapBakeCtx {
            bvh: &bvh_b,
            primitives: &prims_b,
            geometry: &mut geo_bo,
            lights: &static_bo,
        };
        let section_bo = bake_lightmap(
            &mut inputs_b,
            &LightmapConfig {
                lightmap_density: 0.25,
                area_sample_count: DEFAULT_AREA_SAMPLE_COUNT,
                uncompressed_irradiance: false,
            },
        )
        .unwrap()
        .section;
        assert_eq!(
            section_bo.irr_width, 1,
            "bake_only animated light must not contribute to the static atlas",
        );
    }

    #[test]
    fn chart_planning_produces_positive_extents() {
        let geo = unit_quad_geometry();
        let charts = plan_charts(&geo, 0.25);
        assert_eq!(charts.len(), 1);
        assert!(charts[0].uv_extent[0] > 0.0);
        assert!(charts[0].uv_extent[1] > 0.0);
        assert!(charts[0].width_texels >= 1);
        assert!(charts[0].height_texels >= 1);
    }

    #[test]
    fn pack_layers_is_deterministic() {
        let geo = unit_quad_geometry();
        let charts = plan_charts(&geo, 0.25);
        let p1 = pack_layers(&charts, MAX_ATLAS_DIMENSION, 0.25).unwrap();
        let p2 = pack_layers(&charts, MAX_ATLAS_DIMENSION, 0.25).unwrap();
        assert_eq!(p1.atlas_width, p2.atlas_width);
        assert_eq!(p1.atlas_height, p2.atlas_height);
        assert_eq!(p1.layer_count, p2.layer_count);
        assert_eq!(p1.placements.len(), p2.placements.len());
        for (a, b) in p1.placements.iter().zip(p2.placements.iter()) {
            assert_eq!(a.x, b.x);
            assert_eq!(a.y, b.y);
            assert_eq!(a.layer, b.layer);
        }
    }

    fn synthetic_chart(w: u32, h: u32) -> Chart {
        synthetic_chart_leaf(w, h, 0)
    }

    fn synthetic_chart_leaf(w: u32, h: u32, leaf_index: u32) -> Chart {
        Chart {
            origin: Vec3::ZERO,
            u_axis: Vec3::X,
            v_axis: Vec3::Z,
            uv_min: [0.0, 0.0],
            uv_extent: [1.0, 1.0],
            normal: Vec3::Y,
            width_texels: w,
            height_texels: h,
            leaf_index,
        }
    }

    /// Leaf-aware multi-bin packing (Task 3b): when two BVH leaves' charts can't
    /// all share one layer, the packer opens a second layer and keeps each leaf
    /// whole on its own layer. Forces two layers by sizing each leaf to fill an
    /// entire small `max_dim` atlas, so the second leaf cannot share the first
    /// leaf's layer.
    #[test]
    fn pack_layers_opens_second_layer_keeping_each_leaf_cohesive() {
        let max_dim = 64;
        // Leaf 0: one 64×64 chart that fills the whole 64² layer. Leaf 1: two
        // 64×32 charts that together fill a second 64² layer. Neither leaf can
        // share a layer with the other.
        let charts = vec![
            synthetic_chart_leaf(64, 64, 0),
            synthetic_chart_leaf(64, 32, 1),
            synthetic_chart_leaf(64, 32, 1),
        ];

        let pack = pack_layers(&charts, max_dim, 0.25).expect("must pack into two layers");

        assert_eq!(pack.layer_count, 2, "two full leaves must span two layers");
        assert_eq!(pack.atlas_width, 64);
        assert_eq!(pack.atlas_height, 64);

        // Every chart of a given leaf must land on exactly one layer.
        let mut leaf_layers: std::collections::HashMap<u32, u32> = std::collections::HashMap::new();
        for (chart, placement) in charts.iter().zip(pack.placements.iter()) {
            assert!(
                placement.layer < pack.layer_count,
                "layer {} out of range (count {})",
                placement.layer,
                pack.layer_count,
            );
            match leaf_layers.get(&chart.leaf_index) {
                Some(&existing) => assert_eq!(
                    existing, placement.layer,
                    "leaf {} straddled layers {existing} and {}",
                    chart.leaf_index, placement.layer,
                ),
                None => {
                    leaf_layers.insert(chart.leaf_index, placement.layer);
                }
            }
        }
        // The two leaves landed on different layers.
        assert_ne!(
            leaf_layers[&0], leaf_layers[&1],
            "the two leaves must occupy distinct layers",
        );
    }

    /// Task 3b AC — per-vertex `lightmap_layer`. The sibling test above asserts
    /// `ChartPlacement.layer`; this one proves `assign_lightmap_uvs` actually
    /// writes that layer into `Vertex.lightmap_layer`. From a two-layer
    /// `PackOutput`, run `assign_lightmap_uvs` over a minimal geometry whose
    /// triangle faces map 1:1 to the charts, then assert each face's vertices
    /// carry `placement.layer as u16` and that `lightmap_uv` normalizes against
    /// the shared `(atlas_width, atlas_height)` (in `[0,1]`).
    #[test]
    fn assign_lightmap_uvs_writes_per_vertex_layer_across_two_layers() {
        // Same two-layer setup as the sibling test: leaf 0 fills a 64² layer,
        // leaf 1's two charts fill a second. Three charts → three faces.
        let charts = vec![
            synthetic_chart_leaf(64, 64, 0),
            synthetic_chart_leaf(64, 32, 1),
            synthetic_chart_leaf(64, 32, 1),
        ];
        let pack = pack_layers(&charts, 64, 0.25).expect("must pack into two layers");
        assert_eq!(pack.layer_count, 2, "fixture must exercise two layers");

        // Minimal geometry: one triangle per chart, each face owning three
        // vertices (no sharing), so face index `i` maps to chart `i`.
        let mut vertices = Vec::new();
        let mut indices = Vec::new();
        let mut faces = Vec::new();
        let mut face_index_ranges = Vec::new();
        for chart in &charts {
            let base = vertices.len() as u32;
            for p in [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]] {
                vertices.push(Vertex::new(
                    p,
                    [0.0, 0.0],
                    [0.0, 1.0, 0.0],
                    [1.0, 0.0, 0.0],
                    true,
                    [0.0, 0.0],
                    0,
                ));
            }
            let offset = indices.len() as u32;
            indices.extend_from_slice(&[base, base + 1, base + 2]);
            faces.push(FaceMeta {
                leaf_index: chart.leaf_index,
                texture_index: 0,
            });
            face_index_ranges.push(FaceIndexRange {
                index_offset: offset,
                index_count: 3,
            });
        }
        let mut geom = GeometryResult {
            geometry: GeometrySection {
                vertices,
                indices,
                faces,
            },
            texture_names: TextureNamesSection { names: Vec::new() },
            face_index_ranges,
        };

        assign_lightmap_uvs(&mut geom, &charts, &pack);

        for (face_index, range) in geom.face_index_ranges.iter().enumerate() {
            let expected_layer = pack.placements[face_index].layer as u16;
            let start = range.index_offset as usize;
            let end = start + range.index_count as usize;
            for &idx in &geom.geometry.indices[start..end] {
                let v = &geom.geometry.vertices[idx as usize];
                assert_eq!(
                    v.lightmap_layer, expected_layer,
                    "face {face_index} vertex {idx}: lightmap_layer must equal placement.layer",
                );
                let uv = v.decode_lightmap_uv();
                assert!(
                    (0.0..=1.0).contains(&uv[0]) && (0.0..=1.0).contains(&uv[1]),
                    "face {face_index} vertex {idx}: lightmap_uv out of [0,1]: {uv:?}",
                );
            }
        }

        // The two leaves must have produced distinct per-vertex layers, so the
        // test actually exercises a non-zero `lightmap_layer` write.
        let layer_face0 = {
            let r = geom.face_index_ranges[0];
            geom.geometry.vertices[geom.geometry.indices[r.index_offset as usize] as usize]
                .lightmap_layer
        };
        let layer_face1 = {
            let r = geom.face_index_ranges[1];
            geom.geometry.vertices[geom.geometry.indices[r.index_offset as usize] as usize]
                .lightmap_layer
        };
        assert_ne!(
            layer_face0, layer_face1,
            "the two leaves' faces must land on distinct lightmap layers",
        );
    }

    /// Fix 1 robustness — a single BVH leaf whose charts can't fit even an empty
    /// `max_dim²` layer must return `LeafTooLarge`, not silently corrupt
    /// placements. Leaf cohesion forbids splitting the leaf across layers, so the
    /// packer has no escape: several near-`max_dim` charts on one leaf overflow a
    /// single layer regardless of how many layers it opens.
    #[test]
    fn pack_layers_single_oversized_leaf_errors() {
        let max_dim = 64;
        // One leaf, four 64×64 charts. Each fills a whole 64² layer, so they
        // cannot coexist on one layer — and the leaf cannot be split.
        let charts = vec![
            synthetic_chart_leaf(64, 64, 7),
            synthetic_chart_leaf(64, 64, 7),
            synthetic_chart_leaf(64, 64, 7),
            synthetic_chart_leaf(64, 64, 7),
        ];
        let err = pack_layers(&charts, max_dim, 0.25)
            .expect_err("an oversized single leaf must error, not pack");
        match err {
            LightmapBakeError::LeafTooLarge {
                leaf_index,
                chart_count,
                max_dim: reported_max,
            } => {
                assert_eq!(leaf_index, 7, "error must carry the BVH leaf index");
                assert_eq!(chart_count, 4, "error must report the leaf's chart count");
                assert_eq!(reported_max, max_dim, "error must report the max dimension");
            }
            other => panic!("expected LeafTooLarge, got {other:?}"),
        }
    }

    /// Cache-determinism guard: the lightmap bake is consumed by the build-stage
    /// cache, which keys cache entries on input hash and reuses stored output
    /// verbatim. Any run-to-run drift in the encoded section (HashMap iteration
    /// order leaking into output, parallel-reduce sums with variable ordering,
    /// RNG without a fixed seed) would defeat the cache. This test fails fast
    /// if any such drift is reintroduced into the bake path.
    #[test]
    fn lightmap_bake_produces_byte_identical_output_on_repeated_runs() {
        fn run_bake() -> Vec<u8> {
            let mut geo = unit_quad_geometry();
            let (bvh, prims, _) = build_bvh(&geo).unwrap();
            let lights = vec![point_light_above()];
            let static_lights = StaticBakedLights::from_lights(&lights);
            let mut inputs = LightmapBakeCtx {
                bvh: &bvh,
                primitives: &prims,
                geometry: &mut geo,
                lights: &static_lights,
            };
            let out = bake_lightmap(
                &mut inputs,
                &LightmapConfig {
                    lightmap_density: 0.25,
                    area_sample_count: DEFAULT_AREA_SAMPLE_COUNT,
                    uncompressed_irradiance: false,
                },
            )
            .unwrap();
            // LightmapSection has a defined on-disk byte layout — comparing
            // those bytes is the same check the cache substrate applies.
            out.section.to_bytes()
        }

        let bytes_a = run_bake();
        let bytes_b = run_bake();
        assert_eq!(
            bytes_a, bytes_b,
            "lightmap bake output drifted between runs; the build-stage cache requires \
             byte-identical output for identical inputs",
        );
    }

    /// Task 4a — determinism via decode. The plan AC reads: "Two `--no-cache`
    /// bakes of the same input each decode within tolerance — byte-equality
    /// not required." This test models that contract directly: run the bake
    /// twice on identical inputs (the in-process equivalent of `--no-cache`,
    /// which would force the cache substrate to re-invoke the bake), decode
    /// each BC6H block back to f16 through the same hardware-matching
    /// reference decoder the round-trip test uses, and assert per-channel
    /// per-texel agreement within the same frozen `BC6H_ROUNDTRIP_TOLERANCE_REL`
    /// the round-trip AC gates against.
    ///
    /// This is *not* redundant with `lightmap_bake_produces_byte_identical_output_on_repeated_runs`:
    /// that sibling locks down byte-equality (a stricter cache-friendliness
    /// invariant). This one is the plan-stated decoded-tolerance gate, so a
    /// future encoder swap that produces non-byte-identical-but-still-correct
    /// output (e.g. a cluster-fit refinement) keeps the determinism AC green
    /// while the byte-identity test is updated alongside the encoder.
    #[test]
    fn two_bakes_decode_within_frozen_tolerance() {
        fn run_bake() -> LightmapSection {
            let mut geo = unit_quad_geometry();
            let (bvh, prims, _) = build_bvh(&geo).unwrap();
            let lights = vec![point_light_above()];
            let static_lights = StaticBakedLights::from_lights(&lights);
            let mut inputs = LightmapBakeCtx {
                bvh: &bvh,
                primitives: &prims,
                geometry: &mut geo,
                lights: &static_lights,
            };
            bake_lightmap(
                &mut inputs,
                &LightmapConfig {
                    lightmap_density: 0.25,
                    area_sample_count: DEFAULT_AREA_SAMPLE_COUNT,
                    uncompressed_irradiance: false,
                },
            )
            .unwrap()
            .section
        }

        let a = run_bake();
        let b = run_bake();

        // The bake's `unit_quad_geometry` exercises the BC6H path (default
        // config, `uncompressed_irradiance = false`). The two sections must
        // agree on dimensions and irradiance format before we can do block-
        // wise decode comparison.
        assert_eq!(a.irr_width, b.irr_width, "width drifted between runs");
        assert_eq!(a.irr_height, b.irr_height, "height drifted between runs");
        assert_eq!(
            a.irradiance_format,
            postretro_level_format::lightmap::IRRADIANCE_FORMAT_BC6H,
            "determinism gate is BC6H-specific; bake unexpectedly emitted a different format",
        );
        assert_eq!(
            a.irradiance_format, b.irradiance_format,
            "irradiance format drifted between runs",
        );
        assert_eq!(
            a.irradiance.len(),
            b.irradiance.len(),
            "irradiance blob length drifted; block math diverged across runs",
        );

        // Block-wise decode + relative-error comparison. The decoded f16 bits
        // are first lifted to f32 so the relative-error metric matches the
        // round-trip AC's gauge (a per-channel epsilon-floored ratio).
        let blocks = a.irradiance.len() / 16;
        for bi in 0..blocks {
            let off = bi * 16;
            let block_a: [u8; 16] = a.irradiance[off..off + 16].try_into().unwrap();
            let block_b: [u8; 16] = b.irradiance[off..off + 16].try_into().unwrap();
            let decoded_a = crate::bc6h::decode_bc6h_block_for_tests(&block_a);
            let decoded_b = crate::bc6h::decode_bc6h_block_for_tests(&block_b);
            for t in 0..16 {
                for c in 0..3 {
                    let va = crate::bc6h::f16_bits_to_f32(decoded_a[t][c]);
                    let vb = crate::bc6h::f16_bits_to_f32(decoded_b[t][c]);
                    let denom = va.abs().max(vb.abs()).max(1.0e-3);
                    let rel = (va - vb).abs() / denom;
                    assert!(
                        rel <= crate::bc6h::BC6H_ROUNDTRIP_TOLERANCE_REL,
                        "block {bi} texel {t} c{c}: two-bake relative drift {rel} > frozen \
                         tolerance {} (a={va}, b={vb})",
                        crate::bc6h::BC6H_ROUNDTRIP_TOLERANCE_REL,
                    );
                }
            }
        }
    }

    /// Task 4a — atlas sizing. Across a representative chart set, the packed
    /// atlas dimensions must be (a) power-of-two, (b) 4-aligned (BC6H block
    /// requirement — satisfied for free when dimensions are power-of-two ≥ 4,
    /// but pinned here as a contract because the bake's BC6H sizing math
    /// (`ceil(w/4)·ceil(h/4)·16`) silently misreports if either axis is < 4),
    /// and (c) ≤ `MAX_ATLAS_DIMENSION`. All charts here share one leaf, so the
    /// packer resolves them into a single layer; this table-driven test extends
    /// dimension coverage across a representative breadth so a regression that
    /// only triggers on, e.g., narrow-tall sets is still caught.
    #[test]
    fn pack_layers_dims_are_pow2_4aligned_under_cap_across_chart_sets() {
        // Table of chart sets, each producing an atlas that the packer should
        // resolve under the 8192 cap. Mixed shapes: square small, square
        // large, narrow-tall, wide-short, and a single-chart degenerate.
        let cases: &[(&str, Vec<Chart>)] = &[
            ("single small square", vec![synthetic_chart(64, 64)]),
            (
                "many small squares",
                (0..16).map(|_| synthetic_chart(128, 128)).collect(),
            ),
            (
                "narrow-tall column",
                (0..4).map(|_| synthetic_chart(256, 2048)).collect(),
            ),
            (
                "wide-short row",
                (0..4).map(|_| synthetic_chart(2048, 256)).collect(),
            ),
            (
                "mixed shapes",
                vec![
                    synthetic_chart(512, 256),
                    synthetic_chart(256, 512),
                    synthetic_chart(1024, 128),
                    synthetic_chart(128, 1024),
                    synthetic_chart(256, 256),
                ],
            ),
        ];
        for (label, charts) in cases {
            let pack = pack_layers(charts, MAX_ATLAS_DIMENSION, 0.25)
                .unwrap_or_else(|e| panic!("{label}: pack_layers failed: {e:?}"));
            let (w, h) = (pack.atlas_width, pack.atlas_height);
            assert_eq!(
                pack.layer_count, 1,
                "{label}: single-leaf charts must pack into one layer, got {}",
                pack.layer_count,
            );
            assert!(
                w.is_power_of_two(),
                "{label}: atlas width must be power-of-two, got {w}",
            );
            assert!(
                h.is_power_of_two(),
                "{label}: atlas height must be power-of-two, got {h}",
            );
            assert!(w >= 4, "{label}: width must be ≥4 (BC6H block), got {w}");
            assert!(h >= 4, "{label}: height must be ≥4 (BC6H block), got {h}");
            assert_eq!(
                w % 4,
                0,
                "{label}: width must be 4-aligned (BC6H block), got {w}",
            );
            assert_eq!(
                h % 4,
                0,
                "{label}: height must be 4-aligned (BC6H block), got {h}",
            );
            assert!(
                w <= MAX_ATLAS_DIMENSION,
                "{label}: width must be ≤cap ({MAX_ATLAS_DIMENSION}), got {w}",
            );
            assert!(
                h <= MAX_ATLAS_DIMENSION,
                "{label}: height must be ≤cap ({MAX_ATLAS_DIMENSION}), got {h}",
            );
        }
    }

    #[test]
    fn lightmap_uvs_in_zero_one_range_after_bake() {
        let mut geo = unit_quad_geometry();
        let (bvh, prims, _) = build_bvh(&geo).unwrap();
        let lights = vec![point_light_above()];
        let static_lights = StaticBakedLights::from_lights(&lights);
        let mut inputs = LightmapBakeCtx {
            bvh: &bvh,
            primitives: &prims,
            geometry: &mut geo,
            lights: &static_lights,
        };
        let _ = bake_lightmap(
            &mut inputs,
            &LightmapConfig {
                lightmap_density: 0.25,
                area_sample_count: DEFAULT_AREA_SAMPLE_COUNT,
                uncompressed_irradiance: false,
            },
        )
        .unwrap();
        for v in &geo.geometry.vertices {
            let uv = v.decode_lightmap_uv();
            assert!(
                uv[0] >= 0.0 && uv[0] <= 1.0,
                "lightmap u out of range: {}",
                uv[0]
            );
            assert!(
                uv[1] >= 0.0 && uv[1] <= 1.0,
                "lightmap v out of range: {}",
                uv[1]
            );
        }
    }

    #[test]
    fn occluder_produces_dark_texel() {
        // Build two parallel quads: a floor and a ceiling blocker.
        // Light is above the ceiling, so the floor should see zero irradiance.
        let floor = vec![
            Vertex::new(
                [0.0, 0.0, 0.0],
                [0.0, 0.0],
                [0.0, 1.0, 0.0],
                [1.0, 0.0, 0.0],
                true,
                [0.0, 0.0],
                0,
            ),
            Vertex::new(
                [2.0, 0.0, 0.0],
                [1.0, 0.0],
                [0.0, 1.0, 0.0],
                [1.0, 0.0, 0.0],
                true,
                [0.0, 0.0],
                0,
            ),
            Vertex::new(
                [2.0, 0.0, 2.0],
                [1.0, 1.0],
                [0.0, 1.0, 0.0],
                [1.0, 0.0, 0.0],
                true,
                [0.0, 0.0],
                0,
            ),
            Vertex::new(
                [0.0, 0.0, 2.0],
                [0.0, 1.0],
                [0.0, 1.0, 0.0],
                [1.0, 0.0, 0.0],
                true,
                [0.0, 0.0],
                0,
            ),
        ];
        let ceiling = vec![
            Vertex::new(
                [-2.0, 1.0, -2.0],
                [0.0, 0.0],
                [0.0, -1.0, 0.0],
                [1.0, 0.0, 0.0],
                true,
                [0.0, 0.0],
                0,
            ),
            Vertex::new(
                [4.0, 1.0, -2.0],
                [1.0, 0.0],
                [0.0, -1.0, 0.0],
                [1.0, 0.0, 0.0],
                true,
                [0.0, 0.0],
                0,
            ),
            Vertex::new(
                [4.0, 1.0, 4.0],
                [1.0, 1.0],
                [0.0, -1.0, 0.0],
                [1.0, 0.0, 0.0],
                true,
                [0.0, 0.0],
                0,
            ),
            Vertex::new(
                [-2.0, 1.0, 4.0],
                [0.0, 1.0],
                [0.0, -1.0, 0.0],
                [1.0, 0.0, 0.0],
                true,
                [0.0, 0.0],
                0,
            ),
        ];
        let mut vertices = floor;
        vertices.extend(ceiling);
        let indices = vec![0, 1, 2, 0, 2, 3, 4, 5, 6, 4, 6, 7];
        let faces = vec![
            FaceMeta {
                leaf_index: 0,
                texture_index: 0,
            },
            FaceMeta {
                leaf_index: 0,
                texture_index: 0,
            },
        ];
        let face_index_ranges = vec![
            FaceIndexRange {
                index_offset: 0,
                index_count: 6,
            },
            FaceIndexRange {
                index_offset: 6,
                index_count: 6,
            },
        ];
        let mut geo = GeometryResult {
            geometry: GeometrySection {
                vertices,
                indices,
                faces,
            },
            texture_names: TextureNamesSection { names: Vec::new() },
            face_index_ranges,
        };

        let (bvh, prims, _) = build_bvh(&geo).unwrap();
        let light = MapLight {
            origin: DVec3::new(1.0, 2.0, 1.0),
            light_type: LightType::Point,
            intensity: 1.0,
            color: [1.0, 1.0, 1.0],
            falloff_model: FalloffModel::Linear,
            falloff_range: 10.0,
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
            shadow_type: crate::map_data::ShadowType::StaticLightMap,
        };
        let lights = vec![light];
        let static_lights = StaticBakedLights::from_lights(&lights);
        let mut inputs = LightmapBakeCtx {
            bvh: &bvh,
            primitives: &prims,
            geometry: &mut geo,
            lights: &static_lights,
        };
        let section = bake_lightmap(
            &mut inputs,
            &LightmapConfig {
                lightmap_density: 0.25,
                area_sample_count: DEFAULT_AREA_SAMPLE_COUNT,
                // Reads per-texel bytes — request the RGBA16F debug bypass
                // so the 8-byte stride matches what this loop expects.
                uncompressed_irradiance: true,
            },
        )
        .unwrap()
        .section;

        let mut zero_count = 0;
        for t in 0..(section.irr_width * section.irr_height) as usize {
            let r_bits =
                u16::from_le_bytes([section.irradiance[t * 8], section.irradiance[t * 8 + 1]]);
            if r_bits == 0 {
                zero_count += 1;
            }
        }
        assert!(
            zero_count > 0,
            "expected at least one occluded (zero-irradiance) texel",
        );
    }

    #[test]
    fn oversize_face_returns_error_rather_than_panicking() {
        // Regression: the old path clamped atlas_h but left chart placements at pre-clamp
        // coordinates, causing out-of-bounds writes during bake and dilation.
        let size = 400.0; // 10000 texels at 0.04 m/texel, beyond MAX_ATLAS_DIMENSION (8192)
        let v0 = Vertex::new(
            [0.0, 0.0, 0.0],
            [0.0, 0.0],
            [0.0, 1.0, 0.0],
            [1.0, 0.0, 0.0],
            true,
            [0.0, 0.0],
            0,
        );
        let v1 = Vertex::new(
            [size, 0.0, 0.0],
            [1.0, 0.0],
            [0.0, 1.0, 0.0],
            [1.0, 0.0, 0.0],
            true,
            [0.0, 0.0],
            0,
        );
        let v2 = Vertex::new(
            [size, 0.0, size],
            [1.0, 1.0],
            [0.0, 1.0, 0.0],
            [1.0, 0.0, 0.0],
            true,
            [0.0, 0.0],
            0,
        );
        let v3 = Vertex::new(
            [0.0, 0.0, size],
            [0.0, 1.0],
            [0.0, 1.0, 0.0],
            [1.0, 0.0, 0.0],
            true,
            [0.0, 0.0],
            0,
        );
        let mut geo = GeometryResult {
            geometry: GeometrySection {
                vertices: vec![v0, v1, v2, v3],
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
        };
        let (bvh, prims, _) = build_bvh(&geo).unwrap();
        let lights = vec![point_light_above()];
        let static_lights = StaticBakedLights::from_lights(&lights);
        let mut inputs = LightmapBakeCtx {
            bvh: &bvh,
            primitives: &prims,
            geometry: &mut geo,
            lights: &static_lights,
        };
        let result = bake_lightmap(
            &mut inputs,
            &LightmapConfig {
                lightmap_density: DEFAULT_TEXEL_DENSITY_METERS,
                area_sample_count: DEFAULT_AREA_SAMPLE_COUNT,
                uncompressed_irradiance: false,
            },
        );
        // A 400 m face is 10000 texels at 0.04 m/texel — wider than a single
        // `MAX_ATLAS_DIMENSION` (8192) layer, so the packer reports `ChartTooLarge`
        // rather than placing it. (Atlas-area overflow no longer errors: the
        // multi-bin packer opens more layers instead.)
        match result {
            Err(LightmapBakeError::ChartTooLarge { .. }) => {}
            other => panic!("expected ChartTooLarge error, got {other:?}"),
        }
    }

    #[test]
    fn shared_world_vertex_produces_two_distinct_records() {
        let shared = Vertex::new(
            [0.0, 0.0, 0.0],
            [0.0, 0.0],
            [0.0, 1.0, 0.0],
            [1.0, 0.0, 0.0],
            true,
            [0.0, 0.0],
            0,
        );
        let a1 = Vertex::new(
            [1.0, 0.0, 0.0],
            [1.0, 0.0],
            [0.0, 1.0, 0.0],
            [1.0, 0.0, 0.0],
            true,
            [0.0, 0.0],
            0,
        );
        let a2 = Vertex::new(
            [1.0, 0.0, 1.0],
            [1.0, 1.0],
            [0.0, 1.0, 0.0],
            [1.0, 0.0, 0.0],
            true,
            [0.0, 0.0],
            0,
        );
        let b1 = Vertex::new(
            [0.0, 0.0, 1.0],
            [0.0, 1.0],
            [0.0, 1.0, 0.0],
            [1.0, 0.0, 0.0],
            true,
            [0.0, 0.0],
            0,
        );
        let b2 = Vertex::new(
            [-1.0, 0.0, 1.0],
            [-1.0, 1.0],
            [0.0, 1.0, 0.0],
            [1.0, 0.0, 0.0],
            true,
            [0.0, 0.0],
            0,
        );
        let mut geo = GeometryResult {
            geometry: GeometrySection {
                vertices: vec![shared, a1, a2, b1, b2],
                indices: vec![0, 1, 2, 0, 3, 4],
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
                    index_count: 3,
                },
                FaceIndexRange {
                    index_offset: 3,
                    index_count: 3,
                },
            ],
        };
        let original_vertex_count = geo.geometry.vertices.len();
        let (bvh, prims, _) = build_bvh(&geo).unwrap();
        let lights = vec![point_light_above()];
        let static_lights = StaticBakedLights::from_lights(&lights);
        let mut inputs = LightmapBakeCtx {
            bvh: &bvh,
            primitives: &prims,
            geometry: &mut geo,
            lights: &static_lights,
        };
        let _ = bake_lightmap(
            &mut inputs,
            &LightmapConfig {
                lightmap_density: 0.25,
                area_sample_count: DEFAULT_AREA_SAMPLE_COUNT,
                uncompressed_irradiance: false,
            },
        )
        .unwrap();

        assert!(
            geo.geometry.vertices.len() > original_vertex_count,
            "expected at least one duplicated vertex after split, got {} (was {})",
            geo.geometry.vertices.len(),
            original_vertex_count,
        );
        let face_b_start = geo.face_index_ranges[1].index_offset as usize;
        let face_b_first_vi = geo.geometry.indices[face_b_start];
        assert_ne!(
            face_b_first_vi, 0,
            "face B should reference a duplicated vertex, not the original shared one",
        );
        let original = &geo.geometry.vertices[0];
        let duplicate = &geo.geometry.vertices[face_b_first_vi as usize];
        assert_eq!(original.position, duplicate.position);
        assert_eq!(original.uv, duplicate.uv);
        assert_eq!(original.normal_oct, duplicate.normal_oct);
        assert_eq!(original.tangent_packed, duplicate.tangent_packed);
    }

    // --- soft_visibility -------------------------------------------------
    //
    // These exercise the trace-context-agnostic helper with mock `trace`
    // closures, so they need no BVH/geometry — the closure stands in for
    // each caller's `segment_clear`.

    use crate::map_data::{DEFAULT_ANGULAR_DIAMETER_DEG, DEFAULT_LIGHT_SIZE};

    fn soft_point_light(size: f32) -> MapLight {
        let mut l = point_light_above();
        l.light_size = size;
        l
    }

    fn soft_directional_light(angular_diameter: f32) -> MapLight {
        let mut l = point_light_above();
        l.light_type = LightType::Directional;
        l.cone_direction = Some([0.0, -1.0, 0.0]);
        l.angular_diameter = angular_diameter;
        l
    }

    const EPS: f32 = 1.0e-5;

    #[test]
    fn soft_visibility_zero_light_size_matches_hard_ray() {
        // An authored `0` must reproduce the single hard ray exactly: 1.0 clear,
        // 0.0 blocked — preserving an explicit hard edge.
        let light = soft_point_light(0.0);
        let clear = soft_visibility(
            Vec3::ZERO,
            Vec3::Y,
            &light,
            1,
            DEFAULT_AREA_SAMPLE_COUNT,
            |_, _| true,
        );
        let blocked = soft_visibility(
            Vec3::ZERO,
            Vec3::Y,
            &light,
            1,
            DEFAULT_AREA_SAMPLE_COUNT,
            |_, _| false,
        );
        assert!((clear - 1.0).abs() < EPS, "clear got {clear}");
        assert!(blocked.abs() < EPS, "blocked got {blocked}");
    }

    #[test]
    fn soft_visibility_zero_angular_diameter_matches_hard_ray() {
        let light = soft_directional_light(0.0);
        let clear = soft_visibility(
            Vec3::ZERO,
            Vec3::Y,
            &light,
            1,
            DEFAULT_AREA_SAMPLE_COUNT,
            |_, _| true,
        );
        let blocked = soft_visibility(
            Vec3::ZERO,
            Vec3::Y,
            &light,
            1,
            DEFAULT_AREA_SAMPLE_COUNT,
            |_, _| false,
        );
        assert!((clear - 1.0).abs() < EPS, "clear got {clear}");
        assert!(blocked.abs() < EPS, "blocked got {blocked}");
    }

    #[test]
    fn soft_visibility_zero_size_single_ray_equals_shadow_visible_target() {
        // The hard-ray short-circuit must hit the same target shadow_visible uses,
        // so a closure that only passes that exact target returns fully visible.
        let light = soft_point_light(0.0);
        let surface = Vec3::new(0.5, 0.0, 0.5);
        let expected_target = Vec3::new(
            light.origin.x as f32,
            light.origin.y as f32,
            light.origin.z as f32,
        );
        let v = soft_visibility(
            surface,
            Vec3::Y,
            &light,
            0,
            DEFAULT_AREA_SAMPLE_COUNT,
            |_, to| (to - expected_target).length() < EPS,
        );
        assert!((v - 1.0).abs() < EPS, "got {v}");
    }

    #[test]
    fn soft_visibility_full_clear_is_one() {
        let light = soft_point_light(DEFAULT_LIGHT_SIZE);
        let v = soft_visibility(
            Vec3::ZERO,
            Vec3::Y,
            &light,
            42,
            DEFAULT_AREA_SAMPLE_COUNT,
            |_, _| true,
        );
        assert!((v - 1.0).abs() < EPS, "got {v}");
    }

    #[test]
    fn soft_visibility_full_block_is_zero() {
        let light = soft_point_light(DEFAULT_LIGHT_SIZE);
        let v = soft_visibility(
            Vec3::ZERO,
            Vec3::Y,
            &light,
            42,
            DEFAULT_AREA_SAMPLE_COUNT,
            |_, _| false,
        );
        assert!(v.abs() < EPS, "got {v}");
    }

    #[test]
    fn soft_visibility_partial_occluder_is_fractional() {
        // Mock occluder: a half-plane through the light origin. Samples on the +X
        // half of the emitter sphere are blocked, the -X half clear — guaranteeing
        // a penumbra with the escalated sample set.
        let light = soft_point_light(0.5);
        let center = Vec3::new(
            light.origin.x as f32,
            light.origin.y as f32,
            light.origin.z as f32,
        );
        let v = soft_visibility(
            Vec3::ZERO,
            Vec3::Y,
            &light,
            9,
            DEFAULT_AREA_SAMPLE_COUNT,
            |_, to| (to - center).x <= 0.0,
        );
        assert!(v > 0.0 && v < 1.0, "expected penumbra fraction, got {v}");
    }

    #[test]
    fn soft_visibility_is_deterministic_across_repeated_calls() {
        // Same inputs incl. seed → identical output, no RNG, no hash-order
        // dependence. A position-dependent occluder forces the escalated path.
        let light = soft_point_light(0.5);
        let surface = Vec3::new(0.5, 0.0, 0.5);
        let occluder = |_from: Vec3, to: Vec3| to.x <= 0.5;
        let a = soft_visibility(
            surface,
            Vec3::Y,
            &light,
            0xABCD,
            DEFAULT_AREA_SAMPLE_COUNT,
            occluder,
        );
        let b = soft_visibility(
            surface,
            Vec3::Y,
            &light,
            0xABCD,
            DEFAULT_AREA_SAMPLE_COUNT,
            occluder,
        );
        let c = soft_visibility(
            surface,
            Vec3::Y,
            &light,
            0xABCD,
            DEFAULT_AREA_SAMPLE_COUNT,
            occluder,
        );
        assert_eq!(a.to_bits(), b.to_bits(), "repeat 1 diverged: {a} vs {b}");
        assert_eq!(a.to_bits(), c.to_bits(), "repeat 2 diverged: {a} vs {c}");
    }

    /// F1 regression: an occluder clipping the emitter's *lower* hemisphere — the
    /// orientation the old contiguous probe set (`0,1,2,3`, all clustered at the
    /// emitter's top cap) missed — must now yield a fractional `soft_visibility`,
    /// not a hard 0/1. The strided probe subset (`{0,8,16,24}` for full=32) places
    /// probes in both the upper and lower halves, so the split forces escalation.
    /// Block rays toward the lower half of the emitter sphere (sample `y` below the
    /// light center), pass the upper half.
    #[test]
    fn soft_visibility_lower_hemisphere_occluder_is_fractional() {
        let light = soft_point_light(0.5);
        let center = Vec3::new(
            light.origin.x as f32,
            light.origin.y as f32,
            light.origin.z as f32,
        );
        // Clear (true) for the upper half of the emitter, blocked for the lower.
        let v = soft_visibility(
            Vec3::ZERO,
            Vec3::Y,
            &light,
            13,
            DEFAULT_AREA_SAMPLE_COUNT,
            |_, to| (to - center).y >= 0.0,
        );
        assert!(
            v > 0.0 && v < 1.0,
            "lower-hemisphere occluder should be a penumbra, got {v} \
             (clustered top-cap probes would have early-out to a hard edge)"
        );
    }

    /// Regression: a sub-0.05° directional emitter baked at the minimum sample
    /// count (`full_samples == SOFT_PROBE_SAMPLES == 4`) used to collapse its four
    /// probe indices (`probe_indices` snapped multiple probes to index 0), so a
    /// partially-occluded texel double-counted a clear sample and early-out to a
    /// hard 1.0 instead of a penumbra. With distinct probe indices the genuine
    /// split now bakes a fraction. At 0.03° / full=4 the four cone samples have
    /// x-offsets `{0, 0, -3.4, +2.8}`, so `to.x < 0.0` blocks exactly one sample.
    #[test]
    fn soft_visibility_narrow_directional_at_min_samples_is_fractional() {
        let light = soft_directional_light(0.03);
        let full_samples = SOFT_PROBE_SAMPLES; // 4 — the minimum, where escalation
        // has no extra samples to recover a collapsed probe set.
        let v = soft_visibility(Vec3::ZERO, Vec3::Y, &light, 13, full_samples, |_, to| {
            to.x >= 0.0
        });
        assert!(
            v > 0.0 && v < 1.0,
            "narrow directional at min samples should be a penumbra, got {v} \
             (a duplicated probe index would have double-counted to a hard 0/1)"
        );
        // Exactly one of the four distinct samples is blocked → 3/4 clear. A
        // duplicated index would skew this off the 4-sample grid.
        assert!((v - 0.75).abs() < EPS, "expected 3/4 clear, got {v}");
    }

    #[test]
    fn soft_visibility_directional_partial_occluder_is_fractional() {
        // Wide cone so jittered directions spread; occluder splits the cone.
        let light = soft_directional_light(20.0);
        let v = soft_visibility(
            Vec3::ZERO,
            Vec3::Y,
            &light,
            3,
            DEFAULT_AREA_SAMPLE_COUNT,
            |_, to| to.x <= 0.0,
        );
        assert!(v > 0.0 && v < 1.0, "expected penumbra fraction, got {v}");
    }

    proptest::proptest! {
        // The unoccluded fraction is always a valid probability for any seed and
        // any deterministic occluder pattern keyed on the sample index.
        #[test]
        fn soft_visibility_always_in_unit_interval(
            seed in proptest::prelude::any::<u64>(),
            size in 0.0f32..2.0,
            pattern in proptest::prelude::any::<u32>(),
        ) {
            let light = soft_point_light(size);
            // Pseudo-arbitrary but deterministic clear/block decision per call:
            // `Cell` gives the `Fn` closure a per-sample counter without `FnMut`.
            let counter = std::cell::Cell::new(0u32);
            let occluder = |_from: Vec3, _to: Vec3| {
                let i = counter.get();
                counter.set(i + 1);
                (pattern >> (i % 32)) & 1 == 0
            };
            let v = soft_visibility(Vec3::ZERO, Vec3::Y, &light, seed, DEFAULT_AREA_SAMPLE_COUNT, occluder);
            proptest::prop_assert!((0.0..=1.0).contains(&v), "v out of range: {v}");
        }

        #[test]
        fn soft_visibility_directional_always_in_unit_interval(
            seed in proptest::prelude::any::<u64>(),
            angular in 0.0f32..45.0,
            all_clear in proptest::prelude::any::<bool>(),
        ) {
            let light = soft_directional_light(angular);
            let v = soft_visibility(Vec3::ZERO, Vec3::Y, &light, seed, DEFAULT_AREA_SAMPLE_COUNT, |_, _| all_clear);
            proptest::prop_assert!((0.0..=1.0).contains(&v), "v out of range: {v}");
        }
    }

    #[test]
    fn soft_visibility_default_sizes_softpath_disagreeing_probes_escalate() {
        // Sanity: documented nonzero defaults take the soft path (not the hard
        // short-circuit), so a split occluder yields a fraction, not 0/1.
        let point = soft_point_light(DEFAULT_LIGHT_SIZE);
        let center = Vec3::new(
            point.origin.x as f32,
            point.origin.y as f32,
            point.origin.z as f32,
        );
        let pv = soft_visibility(
            Vec3::ZERO,
            Vec3::Y,
            &point,
            11,
            DEFAULT_AREA_SAMPLE_COUNT,
            |_, to| (to - center).x <= 0.0,
        );
        assert!(pv > 0.0 && pv < 1.0, "point default not soft: {pv}");

        let dir = soft_directional_light(DEFAULT_ANGULAR_DIAMETER_DEG);
        // Directional default is a narrow 0.5°; a split still yields a fraction.
        let dv = soft_visibility(
            Vec3::ZERO,
            Vec3::Y,
            &dir,
            11,
            DEFAULT_AREA_SAMPLE_COUNT,
            |_, to| to.x <= 0.0,
        );
        assert!(
            (0.0..=1.0).contains(&dv),
            "directional default out of range: {dv}"
        );
    }

    // --- Task 6: sub-texel-penumbra author hint --------------------------
    //
    // Test the pure predicate `penumbra_below_one_texel` (and the
    // `warn_sub_texel_penumbra_lights` count via it), not the `log::warn!`
    // macro — the warning is gated entirely by this predicate, so its branch
    // is the testable seam.

    #[test]
    fn penumbra_below_one_texel_flags_tiny_point_emitter() {
        // A near-zero (but nonzero) emitter over a long reach subtends a
        // sub-texel penumbra at the default atlas density → flagged.
        let mut light = soft_point_light(0.001);
        light.falloff_range = 5.0;
        assert!(
            penumbra_below_one_texel(&light, DEFAULT_TEXEL_DENSITY_METERS),
            "tiny emitter should be flagged sub-texel"
        );
    }

    #[test]
    fn penumbra_below_one_texel_passes_default_sized_point_emitter() {
        // The documented default `_light_size` is sized to span ~multiple
        // texels at the default density → not flagged.
        let mut light = soft_point_light(DEFAULT_LIGHT_SIZE);
        light.falloff_range = 5.0;
        assert!(
            !penumbra_below_one_texel(&light, DEFAULT_TEXEL_DENSITY_METERS),
            "default-sized emitter must not be flagged"
        );
    }

    #[test]
    fn penumbra_below_one_texel_does_not_flag_explicit_hard_light() {
        // An explicitly-authored hard light (size 0) opted into a hard edge —
        // a "too soft" hint would be noise, so it is never flagged.
        let mut light = soft_point_light(0.0);
        light.falloff_range = 5.0;
        assert!(
            !penumbra_below_one_texel(&light, DEFAULT_TEXEL_DENSITY_METERS),
            "explicit hard light (size 0) must not be flagged"
        );
        let dir = soft_directional_light(0.0);
        assert!(
            !penumbra_below_one_texel(&dir, DEFAULT_TEXEL_DENSITY_METERS),
            "explicit hard directional (angular 0) must not be flagged"
        );
    }

    #[test]
    fn penumbra_below_one_texel_flags_narrow_directional() {
        // A 0.001° sun over a unit reach is well below one texel → flagged;
        // a wide 5° sun is not.
        let mut narrow = soft_directional_light(0.001);
        narrow.falloff_range = 1.0;
        assert!(
            penumbra_below_one_texel(&narrow, DEFAULT_TEXEL_DENSITY_METERS),
            "narrow directional should be flagged sub-texel"
        );
        let mut wide = soft_directional_light(5.0);
        wide.falloff_range = 1.0;
        assert!(
            !penumbra_below_one_texel(&wide, DEFAULT_TEXEL_DENSITY_METERS),
            "wide directional must not be flagged"
        );
    }

    #[test]
    fn warn_sub_texel_penumbra_counts_only_below_threshold_lights() {
        // Mixed set: one flagged (tiny), one not (default). The predicate that
        // gates the per-light `log::warn!` must fire for exactly one of them.
        let mut tiny = soft_point_light(0.001);
        tiny.falloff_range = 5.0;
        let mut ok = soft_point_light(DEFAULT_LIGHT_SIZE);
        ok.falloff_range = 5.0;
        let lights = [&tiny, &ok];
        let flagged = lights
            .iter()
            .filter(|l| penumbra_below_one_texel(l, DEFAULT_TEXEL_DENSITY_METERS))
            .count();
        assert_eq!(flagged, 1, "exactly one light should warn");
        // Smoke: the warning emitter runs without panicking on the same set.
        warn_sub_texel_penumbra_lights(&lights, DEFAULT_TEXEL_DENSITY_METERS);
    }

    // --- Task 6: area-sample-count knob ----------------------------------

    #[test]
    fn soft_visibility_knob_below_probe_floor_is_clamped() {
        // A knob below the fixed probe count must not panic or invert the
        // prefix invariant — it clamps up to the probe floor. Full-clear and
        // full-block still resolve to 1.0 / 0.0.
        let light = soft_point_light(0.5);
        let clear = soft_visibility(Vec3::ZERO, Vec3::Y, &light, 1, 1, |_, _| true);
        let blocked = soft_visibility(Vec3::ZERO, Vec3::Y, &light, 1, 1, |_, _| false);
        assert!((clear - 1.0).abs() < EPS, "clamped clear got {clear}");
        assert!(blocked.abs() < EPS, "clamped blocked got {blocked}");
    }

    #[test]
    fn soft_visibility_higher_knob_changes_penumbra_fraction_resolution() {
        // Raising the knob raises the stratification denominator, so a penumbra
        // fraction is quantized more finely. The two counts trace different
        // sample sets, so the returned fractions differ — proving the knob
        // reaches `soft_visibility`'s full-sample target.
        let light = soft_point_light(0.5);
        let center = Vec3::new(
            light.origin.x as f32,
            light.origin.y as f32,
            light.origin.z as f32,
        );
        let occluder = |_from: Vec3, to: Vec3| (to - center).x <= 0.0;
        let low = soft_visibility(Vec3::ZERO, Vec3::Y, &light, 9, 16, occluder);
        let high = soft_visibility(Vec3::ZERO, Vec3::Y, &light, 9, 64, occluder);
        assert!(low > 0.0 && low < 1.0, "low knob not a penumbra: {low}");
        assert!(high > 0.0 && high < 1.0, "high knob not a penumbra: {high}");
        assert_ne!(
            low.to_bits(),
            high.to_bits(),
            "knob did not change full-sample resolution: {low} vs {high}"
        );
    }

    // --- Task 3: static lightmap soft sum (full bake) --------------------
    //
    // These drive the real per-texel bake (BVH + `segment_clear`), unlike the
    // mock-closure tests above which exercise `soft_visibility` in isolation.

    /// A large floor at y=0 plus a small horizontal occluder quad floating above
    /// it, positioned so a light above the occluder casts a shadow with a
    /// penumbra band onto the floor. The occluder is small relative to the floor
    /// so the shadow edge falls on the floor's interior texels.
    fn box_on_floor_geometry() -> GeometryResult {
        // Floor: 4x4 m quad centered at origin, upward normal.
        let floor = [
            ([-2.0, 0.0, -2.0], [0.0, 0.0]),
            ([2.0, 0.0, -2.0], [1.0, 0.0]),
            ([2.0, 0.0, 2.0], [1.0, 1.0]),
            ([-2.0, 0.0, 2.0], [0.0, 1.0]),
        ];
        // Occluder: 1x1 m quad at y=1, downward normal (faces the floor). Sits
        // between the light (high above) and the floor center.
        let occluder = [
            ([-0.5, 1.0, -0.5], [0.0, 0.0]),
            ([0.5, 1.0, -0.5], [1.0, 0.0]),
            ([0.5, 1.0, 0.5], [1.0, 1.0]),
            ([-0.5, 1.0, 0.5], [0.0, 1.0]),
        ];
        let mut vertices = Vec::new();
        for (pos, uv) in floor {
            vertices.push(Vertex::new(
                pos,
                uv,
                [0.0, 1.0, 0.0],
                [1.0, 0.0, 0.0],
                true,
                [0.0, 0.0],
                0,
            ));
        }
        for (pos, uv) in occluder {
            vertices.push(Vertex::new(
                pos,
                uv,
                [0.0, -1.0, 0.0],
                [1.0, 0.0, 0.0],
                true,
                [0.0, 0.0],
                0,
            ));
        }
        let indices = vec![0, 1, 2, 0, 2, 3, 4, 5, 6, 4, 6, 7];
        let faces = vec![
            FaceMeta {
                leaf_index: 0,
                texture_index: 0,
            },
            FaceMeta {
                leaf_index: 0,
                texture_index: 0,
            },
        ];
        let face_index_ranges = vec![
            FaceIndexRange {
                index_offset: 0,
                index_count: 6,
            },
            FaceIndexRange {
                index_offset: 6,
                index_count: 6,
            },
        ];
        GeometryResult {
            geometry: GeometrySection {
                vertices,
                indices,
                faces,
            },
            texture_names: TextureNamesSection { names: Vec::new() },
            face_index_ranges,
        }
    }

    /// Point light high above the occluder center, default soft size.
    fn soft_overhead_light() -> MapLight {
        let mut l = point_light_above();
        l.origin = DVec3::new(0.0, 4.0, 0.0);
        l.falloff_range = 20.0;
        l.light_size = DEFAULT_LIGHT_SIZE;
        l
    }

    /// Decode the floor's irradiance texels (R channel) from the encoded section.
    fn floor_irradiance_r(section: &LightmapSection) -> Vec<f32> {
        let texel_count = (section.irr_width * section.irr_height) as usize;
        let mut out = Vec::with_capacity(texel_count);
        for t in 0..texel_count {
            let bits =
                u16::from_le_bytes([section.irradiance[t * 8], section.irradiance[t * 8 + 1]]);
            out.push(f16_bits_to_f32(bits));
        }
        out
    }

    /// IEEE-754 half → f32 decode for reading baked irradiance back in tests.
    /// Mirrors `sh_bake.rs`'s private decoder (the inverse of
    /// `level-format`'s `f32_to_f16_bits`); duplicated here because that one is
    /// a sibling-module private and level-format exposes no public decoder.
    fn f16_bits_to_f32(bits: u16) -> f32 {
        let sign = (bits >> 15) & 0x1;
        let exp = (bits >> 10) & 0x1f;
        let mant = bits & 0x3ff;
        let value = if exp == 0 {
            (mant as f32) * 2.0f32.powi(-24)
        } else if exp == 0x1f {
            if mant == 0 { f32::INFINITY } else { f32::NAN }
        } else {
            let m = 1.0 + (mant as f32) / 1024.0;
            m * 2.0f32.powi(exp as i32 - 15)
        };
        if sign == 1 { -value } else { value }
    }

    /// Soft-sum penumbra: a default-sized area light over a box-on-floor scene
    /// must bake a *gradient* of intermediate irradiance across the shadow
    /// boundary, not the binary lit/dark step a hard `shadow_visible` gate
    /// produces. We assert the floor has texels strictly between fully dark and
    /// fully lit — the fractional `v` band that defines a penumbra.
    #[test]
    fn soft_sum_bakes_penumbra_gradient_not_hard_step() {
        let mut geo = box_on_floor_geometry();
        let (bvh, prims, _) = build_bvh(&geo).unwrap();
        let lights = vec![soft_overhead_light()];
        let static_lights = StaticBakedLights::from_lights(&lights);
        let mut inputs = LightmapBakeCtx {
            bvh: &bvh,
            primitives: &prims,
            geometry: &mut geo,
            lights: &static_lights,
        };
        let section = bake_lightmap(
            &mut inputs,
            &LightmapConfig {
                lightmap_density: 0.05,
                area_sample_count: DEFAULT_AREA_SAMPLE_COUNT,
                // `floor_irradiance_r` reads per-texel bytes from the
                // irradiance blob — request the RGBA16F debug bypass so the
                // 8-byte stride stays valid.
                uncompressed_irradiance: true,
            },
        )
        .unwrap()
        .section;

        let r = floor_irradiance_r(&section);
        let max_lit = r.iter().cloned().fold(0.0f32, f32::max);
        assert!(max_lit > 0.0, "expected some lit floor texels");

        // A penumbra texel: partially occluded, so its irradiance lands strictly
        // between full dark and (near-)full light. Fully-lit and fully-shadowed
        // texels are excluded by the margins.
        let lit_threshold = max_lit * 0.95;
        let dark_threshold = max_lit * 0.05;
        let penumbra_count = r
            .iter()
            .filter(|&&v| v > dark_threshold && v < lit_threshold)
            .count();
        assert!(
            penumbra_count > 1,
            "expected a multi-texel penumbra gradient, found {penumbra_count} intermediate texels \
             (max_lit={max_lit}); a hard shadow step would yield ~0",
        );
    }

    /// Soft-shadow determinism: re-baking the identical box-on-floor + soft-light
    /// scene must produce byte-identical output. The per-texel seed is a fixed
    /// `(x, y)` hash, so escalated penumbra sampling stays reproducible across
    /// processes — the property the build cache relies on.
    #[test]
    fn soft_sum_bake_is_byte_identical_on_repeat() {
        fn run() -> Vec<u8> {
            let mut geo = box_on_floor_geometry();
            let (bvh, prims, _) = build_bvh(&geo).unwrap();
            let lights = vec![soft_overhead_light()];
            let static_lights = StaticBakedLights::from_lights(&lights);
            let mut inputs = LightmapBakeCtx {
                bvh: &bvh,
                primitives: &prims,
                geometry: &mut geo,
                lights: &static_lights,
            };
            bake_lightmap(
                &mut inputs,
                &LightmapConfig {
                    lightmap_density: 0.05,
                    area_sample_count: DEFAULT_AREA_SAMPLE_COUNT,
                    uncompressed_irradiance: false,
                },
            )
            .unwrap()
            .section
            .to_bytes()
        }
        let a = run();
        let b = run();
        assert_eq!(
            a, b,
            "soft-shadow bake drifted between runs; the area-sample seed must be a fixed \
             (x, y) hash with no RNG or hash-order dependence",
        );
    }

    /// `texel_seed` is a pure deterministic function of `(x, y)` — same coords
    /// give the same seed, and distinct coords decorrelate (so adjacent penumbra
    /// texels don't share a sample rotation). Guards against accidentally
    /// reintroducing process-varying hashing.
    #[test]
    fn texel_seed_is_deterministic_and_position_varying() {
        assert_eq!(texel_seed(3, 7), texel_seed(3, 7));
        assert_ne!(texel_seed(3, 7), texel_seed(7, 3));
        assert_ne!(texel_seed(0, 0), texel_seed(0, 1));
        assert_ne!(texel_seed(0, 0), texel_seed(1, 0));
    }
}
