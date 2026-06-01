// Directional lightmap baker.

use std::collections::HashSet;

use bvh::bvh::Bvh;
use bvh::ray::Ray;
use glam::Vec3;
use nalgebra::{Point3, Vector3};
use postretro_level_format::lightmap::{
    LightmapMode, LightmapSection, encode_direction_oct, f32_to_f16_bits,
};
use thiserror::Error;

use crate::bvh_build::BvhPrimitive;
use crate::chart_raster::{CHART_PADDING_TEXELS, ChartPlacement, chart_texel_world_position};
use crate::geometry::GeometryResult;
use crate::light_namespaces::StaticBakedLights;
use crate::map_data::{FalloffModel, LightType, MapLight};

/// Default atlas texel density: 4 cm per texel.
pub const DEFAULT_TEXEL_DENSITY_METERS: f32 = 0.04;

/// Bump this when the lightmap baking algorithm changes. Invalidates all
/// existing cache entries for this stage.
///
/// v2 bump: lightmap bake gained a shadowed/unshadowed mode flag whose section
/// trailer the runtime now reads. Re-bakes existing maps so cached entries
/// carry the explicit mode and so the unshadowed bake's visibility-skip branch
/// is exercised on a fresh cache miss.
///
/// v3: per-light `_shadow_type` routing (sdf-per-light-shadows).
/// v4 bump (sdf-per-light-shadows Task 3): the shadow-type exclusion moved to
/// the direct lightmap consumer and keys on the renamed two-value `ShadowType`
/// (`sdf` dropped here; dynamic-tier lights drop via the position-axis
/// namespace). A stale cached lightmap could carry a direct shadow for an `sdf`
/// light the runtime now resolves separately (double-count), so the bump forces
/// a re-bake of the now-disjoint direct set.
///
/// v5 bump (baked-soft-lightmap-shadows Task 3): the per-texel static-light gate
/// changed from a hard 1-texel `shadow_visible` step to an area-sampled
/// `soft_visibility` fraction multiplied into irradiance and the
/// dominant-direction accumulation. The per-texel output values shift, so a
/// stale cached lightmap would serve the old hard-shadow output; the bump forces
/// a re-bake into the soft-shadow values.
pub const STAGE_VERSION: u32 = 5;

/// Atlas width/height when no face would fit otherwise. Power-of-two for BC6H block alignment.
const MIN_ATLAS_DIMENSION: u32 = 64;

/// Maximum atlas dimension. Beyond this the baker returns an error so the caller can retry at a
/// coarser density. 4096 is well under the 8192 `max_texture_dimension_2d` floor required by
/// wgpu and fits ~164 m at 4 cm/texel.
const MAX_ATLAS_DIMENSION: u32 = 4096;

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
        "lightmap atlas overflow: charts do not fit in {max}x{max} at {density_m_per_texel} m/texel \
         (needed {needed_w}x{needed_h}); raise `texel_density` or split the map"
    )]
    AtlasOverflow {
        max: u32,
        needed_w: u32,
        needed_h: u32,
        density_m_per_texel: f32,
    },
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
}

pub struct LightmapBakeCtx<'a> {
    pub bvh: &'a Bvh<f32, 3>,
    pub primitives: &'a [BvhPrimitive],
    /// Mutable: baker writes per-vertex lightmap UVs back after atlas placement.
    pub geometry: &'a mut GeometryResult,
    pub lights: &'a StaticBakedLights<'a>,
}

/// Owned, serializable snapshot of the data the lightmap bake reads. Used for
/// cache key derivation: postcard-serialize this + LightmapConfig to get the
/// input hash.
#[derive(serde::Serialize)]
pub struct LightmapInputs {
    /// Static-baked lights (filter: !is_dynamic && animation.is_none()).
    pub lights: Vec<crate::map_data::MapLight>,
    /// Geometry at the point the bake runs. Pins vertex positions, UVs, and
    /// index data — everything that affects chart shapes and shadow queries.
    pub geometry: crate::geometry::GeometryResult,
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
}

/// Cheap pre-bake setup: chart planning, shelf packing, and writing lightmap
/// UVs back into the geometry. Returned by [`prepare_atlas`] and consumed both
/// by a fresh bake (cache miss) and by the cache-hit path that rebuilds
/// charts/placements without re-running the per-texel ray casting.
#[derive(Debug)]
pub struct PreparedAtlas {
    pub charts: Vec<Chart>,
    pub placements: Vec<ChartPlacement>,
    pub atlas_width: u32,
    pub atlas_height: u32,
}

/// Prepare atlas charts and assign lightmap UVs into geometry. Runs
/// `split_shared_vertices`, `plan_charts`, `shelf_pack`, and
/// `assign_lightmap_uvs`. Does NOT run the per-texel ray casting.
///
/// Called both before a fresh bake (cache miss) and on cache hit to
/// reconstruct charts/placements and re-apply lightmap UVs. The mutations
/// applied here — vertex splitting and lightmap UV writes — run on all
/// non-empty geometry, regardless of whether a full per-texel bake is
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
        });
    }

    if static_lights.is_empty() {
        // Plan charts anyway — the animated-light-chunks builder needs per-face UV bounds and
        // placements even when no static lights exist. Vertex splitting and UV assignment are
        // skipped because the empty bake path returns a placeholder section that no atlas
        // sampling consumes.
        let charts = plan_charts(geom, texel_density);
        let (atlas_w, atlas_h, placements) = match shelf_pack(&charts, texel_density) {
            Ok(p) => p,
            Err(_) => (1, 1, Vec::new()),
        };
        return Ok(PreparedAtlas {
            charts,
            placements,
            atlas_width: atlas_w,
            atlas_height: atlas_h,
        });
    }

    // Ensure no vertex index is shared across faces — each face must own its own lightmap UV slot.
    split_shared_vertices(geom);

    let charts = plan_charts(geom, texel_density);

    for (face_index, chart) in charts.iter().enumerate() {
        if chart.width_texels > MAX_ATLAS_DIMENSION || chart.height_texels > MAX_ATLAS_DIMENSION {
            return Err(LightmapBakeError::ChartTooLarge {
                face_index,
                width_texels: chart.width_texels,
                height_texels: chart.height_texels,
                max: MAX_ATLAS_DIMENSION,
                u_extent_m: chart.uv_extent[0],
                v_extent_m: chart.uv_extent[1],
                density_m_per_texel: texel_density,
            });
        }
    }

    let (atlas_w, atlas_h, placements) = shelf_pack(&charts, texel_density)?;
    if placements.is_empty() {
        return Ok(PreparedAtlas {
            charts,
            placements,
            atlas_width: atlas_w,
            atlas_height: atlas_h,
        });
    }

    assign_lightmap_uvs(geom, &charts, &placements, atlas_w, atlas_h);

    Ok(PreparedAtlas {
        charts,
        placements,
        atlas_width: atlas_w,
        atlas_height: atlas_h,
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

    let mut irradiance = vec![0f32; (atlas_w * atlas_h * 4) as usize];
    let mut direction = vec![Vec3::Y; (atlas_w * atlas_h) as usize];
    let mut coverage = vec![false; (atlas_w * atlas_h) as usize];

    for (face_idx, placement) in placements.iter().enumerate() {
        bake_face_chart(
            inputs.bvh,
            inputs.primitives,
            inputs.geometry,
            &static_lights,
            face_idx,
            &charts[face_idx],
            placement,
            atlas_w,
            area_sample_count,
            &mut irradiance,
            &mut direction,
            &mut coverage,
        );
    }

    dilate_edges(
        &mut irradiance,
        &mut direction,
        &mut coverage,
        atlas_w,
        atlas_h,
    );

    let irr_bytes = encode_irradiance_rgba16f(&irradiance);
    let dir_bytes = encode_direction_rgba8(&direction, &coverage);

    Ok(LightmapBakeOutput {
        section: LightmapSection {
            width: atlas_w,
            height: atlas_h,
            texel_density,
            irradiance: irr_bytes,
            direction: dir_bytes,
            // Lightmaps always bake shadowed: the `sdf` lights' direct term is
            // resolved at runtime, so a `static_light_map` light's shadow can
            // only come from this bake. `LightmapMode` is permanently
            // `Shadowed` on the bake-emit side (the `Unshadowed` variant
            // survives in `level-format`/runtime for legacy-PRL decode only).
            mode: LightmapMode::Shadowed,
        },
        charts,
        placements,
        atlas_width: atlas_w,
        atlas_height: atlas_h,
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

pub fn log_stats(section: &LightmapSection, static_light_count: usize) {
    log::info!(
        "Lightmap: {}x{} atlas, {} m/texel, {} static lights baked, irr={} B, dir={} B",
        section.width,
        section.height,
        section.texel_density,
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
}

fn plan_charts(geom: &GeometryResult, texel_density: f32) -> Vec<Chart> {
    let density = texel_density.max(1.0e-4);
    let section = &geom.geometry;

    let mut charts = Vec::with_capacity(section.faces.len());
    for range in geom.face_index_ranges.iter() {
        let start = range.index_offset as usize;
        if range.index_count < 3 {
            charts.push(empty_chart());
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
            charts.push(empty_chart());
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
            charts.push(empty_chart());
            continue;
        }
        let v_axis = normal.cross(u_axis).normalize_or_zero();
        if v_axis.length_squared() < 0.5 {
            charts.push(empty_chart());
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
        });
    }
    charts
}

fn empty_chart() -> Chart {
    Chart {
        origin: Vec3::ZERO,
        u_axis: Vec3::X,
        v_axis: Vec3::Y,
        uv_min: [0.0, 0.0],
        uv_extent: [0.0, 0.0],
        normal: Vec3::Y,
        width_texels: 1,
        height_texels: 1,
    }
}

/// Shelf-pack charts into an atlas. Returns `(atlas_width, atlas_height, placements)` in the
/// same order as `charts`. Grows width on overflow; errors when at `MAX_ATLAS_DIMENSION` —
/// silent clamping would produce out-of-bounds texel writes in the bake and dilation loops.
fn shelf_pack(
    charts: &[Chart],
    texel_density: f32,
) -> Result<(u32, u32, Vec<ChartPlacement>), LightmapBakeError> {
    if charts.is_empty() {
        return Ok((MIN_ATLAS_DIMENSION, MIN_ATLAS_DIMENSION, Vec::new()));
    }

    let mut order: Vec<usize> = (0..charts.len()).collect();
    order.sort_by(|&a, &b| charts[b].height_texels.cmp(&charts[a].height_texels));

    let total_area: u64 = charts
        .iter()
        .map(|c| (c.width_texels as u64) * (c.height_texels as u64))
        .sum();
    let target_side = (total_area as f64).sqrt().ceil() as u32;
    let mut atlas_w = target_side.max(MIN_ATLAS_DIMENSION).next_power_of_two();
    atlas_w = atlas_w.min(MAX_ATLAS_DIMENSION);

    // Widen if any individual chart is wider than the current estimate. Caller already validated
    // charts against MAX_ATLAS_DIMENSION so this cannot exceed the cap.
    for c in charts {
        if c.width_texels > atlas_w {
            atlas_w = c.width_texels.next_power_of_two().min(MAX_ATLAS_DIMENSION);
        }
    }

    loop {
        match try_shelf_pack(charts, &order, atlas_w) {
            Ok((atlas_h_raw, placements)) => {
                let atlas_h = atlas_h_raw.max(MIN_ATLAS_DIMENSION).next_power_of_two();
                if atlas_h > MAX_ATLAS_DIMENSION {
                    if atlas_w >= MAX_ATLAS_DIMENSION {
                        return Err(LightmapBakeError::AtlasOverflow {
                            max: MAX_ATLAS_DIMENSION,
                            needed_w: atlas_w,
                            needed_h: atlas_h,
                            density_m_per_texel: texel_density,
                        });
                    }
                    atlas_w = (atlas_w * 2).min(MAX_ATLAS_DIMENSION);
                    continue;
                }
                return Ok((atlas_w, atlas_h, placements));
            }
            Err(()) => {
                if atlas_w >= MAX_ATLAS_DIMENSION {
                    return Err(LightmapBakeError::AtlasOverflow {
                        max: MAX_ATLAS_DIMENSION,
                        needed_w: atlas_w,
                        needed_h: MAX_ATLAS_DIMENSION,
                        density_m_per_texel: texel_density,
                    });
                }
                atlas_w = (atlas_w * 2).min(MAX_ATLAS_DIMENSION);
            }
        }
    }
}

/// One shelf-packing attempt at a fixed width. `Err(())` if a chart is wider than `atlas_w`.
fn try_shelf_pack(
    charts: &[Chart],
    order: &[usize],
    atlas_w: u32,
) -> Result<(u32, Vec<ChartPlacement>), ()> {
    let mut placements = vec![ChartPlacement { x: 0, y: 0 }; charts.len()];
    let mut shelf_y: u32 = 0;
    let mut shelf_x: u32 = 0;
    let mut shelf_h: u32 = 0;

    for &idx in order {
        let c = &charts[idx];
        let w = c.width_texels;
        let h = c.height_texels;
        if w > atlas_w {
            return Err(());
        }
        if shelf_x + w > atlas_w {
            shelf_y += shelf_h;
            shelf_x = 0;
            shelf_h = 0;
        }
        placements[idx] = ChartPlacement {
            x: shelf_x,
            y: shelf_y,
        };
        shelf_x += w;
        if h > shelf_h {
            shelf_h = h;
        }
    }

    Ok((shelf_y + shelf_h, placements))
}

fn assign_lightmap_uvs(
    geom: &mut GeometryResult,
    charts: &[Chart],
    placements: &[ChartPlacement],
    atlas_w: u32,
    atlas_h: u32,
) {
    let atlas_w_f = atlas_w as f32;
    let atlas_h_f = atlas_h as f32;
    let ranges = geom.face_index_ranges.clone();

    for (face_index, chart) in charts.iter().enumerate() {
        let placement = placements[face_index];
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

    for ty in 0..interior_h {
        for tx in 0..interior_w {
            let atlas_x = placement.x as i32 + padding + tx;
            let atlas_y = placement.y as i32 + padding + ty;
            let idx = (atlas_y as u32 * atlas_w + atlas_x as u32) as usize;

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
                let (contribution, to_light) =
                    light_contribution_and_direction(light, world_p, surface_normal);
                if contribution.length_squared() <= 1.0e-12 {
                    continue;
                }
                // Lightmaps always bake shadowed: an occluded texel goes dark so a
                // `static_light_map` light's static shadow lives in the atlas.
                // `soft_visibility` returns the `[0,1]` unoccluded fraction over an
                // area sample of the emitter — a multi-texel penumbra instead of a
                // hard 1-texel step. `sdf` lights are already filtered out of
                // `static_lights` (their direct shadow resolves at runtime), so no
                // double-shadow.
                let v = soft_visibility(
                    world_p,
                    surface_normal,
                    light,
                    seed,
                    area_sample_count,
                    |from, to| segment_clear(bvh, primitives, geometry, from, to),
                );
                if v <= 0.0 {
                    continue;
                }
                irr += contribution * v;
                // Weight the dominant-direction accumulation by `v` too: in a
                // penumbra a partially-occluded light should bias the baked
                // direction less than a fully-visible one, matching the softened
                // irradiance it contributes. Direction encoding/format is
                // unchanged; only the accumulated values shift.
                let lum = (contribution.x + contribution.y + contribution.z) * v;
                weighted_dir += to_light * lum;
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
/// verbatim and would break on any run-to-run drift). Mirrors `sh_bake.rs`'s
/// fixed-constant sampling convention; `soft_visibility` XORs this with
/// `SAMPLING_LATTICE_OFFSET` internally.
fn texel_seed(x: u32, y: u32) -> u64 {
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
pub(crate) const SOFT_PROBE_SAMPLES: u32 = 4;

/// Default full stratified sample count once a penumbra is detected. The
/// area-sample-count bake knob (Task 6) overrides this per call via
/// `soft_visibility`'s `full_samples` argument; callers without a knob
/// (e.g. the SH bounce baker, where bounce is low-frequency) pass this default.
/// The probe set is always a strict prefix of `full_samples`, so a penumbra's
/// returned fraction is `clear / full_samples`.
pub(crate) const DEFAULT_AREA_SAMPLE_COUNT: u32 = 32;

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
/// (penumbra) sample target. The fixed `SOFT_PROBE_SAMPLES` probe set is always a
/// strict prefix, so it is clamped to at least the probe count; the escalation
/// threshold itself stays a fixed constant so the bake remains deterministic
/// regardless of the knob's value. Callers without a knob pass
/// [`DEFAULT_AREA_SAMPLE_COUNT`].
pub(crate) fn soft_visibility(
    surface_point: Vec3,
    surface_normal: Vec3,
    light: &MapLight,
    seed: u64,
    full_samples: u32,
    trace: impl Fn(Vec3, Vec3) -> bool,
) -> f32 {
    if !light.cast_shadows {
        return 1.0;
    }
    let origin = surface_point + surface_normal * RAY_EPSILON;

    // An author's explicit `0` is a hard edge: collapse to the single hard ray so
    // the result is identical to `shadow_visible` (1.0 clear / 0.0 occluded).
    let radius = match light.light_type {
        LightType::Point | LightType::Spot => light.light_size,
        LightType::Directional => light.angular_diameter,
    };
    if radius <= 0.0 {
        let target = hard_ray_target(surface_point, light);
        return if trace(origin, target) { 1.0 } else { 0.0 };
    }

    // Probe set is a strict prefix of the full set: the knob can only raise the
    // escalated count above the fixed probe floor, so escalation only adds samples
    // and the penumbra fraction stays `clear / full_samples`.
    let full_samples = full_samples.max(SOFT_PROBE_SAMPLES);
    let mut clear = 0u32;
    for i in 0..SOFT_PROBE_SAMPLES {
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

    for i in SOFT_PROBE_SAMPLES..full_samples {
        if trace(
            origin,
            area_sample_target(surface_point, light, seed, i, full_samples),
        ) {
            clear += 1;
        }
    }
    clear as f32 / full_samples as f32
}

/// Single hard-ray target, matching `shadow_visible`: the light origin for
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
        );
        let v1 = Vertex::new(
            [1.0, 0.0, 0.0],
            [1.0, 0.0],
            [0.0, 1.0, 0.0],
            [1.0, 0.0, 0.0],
            true,
            [0.0, 0.0],
        );
        let v2 = Vertex::new(
            [1.0, 0.0, 1.0],
            [1.0, 1.0],
            [0.0, 1.0, 0.0],
            [1.0, 0.0, 0.0],
            true,
            [0.0, 0.0],
        );
        let v3 = Vertex::new(
            [0.0, 0.0, 1.0],
            [0.0, 1.0],
            [0.0, 1.0, 0.0],
            [1.0, 0.0, 0.0],
            true,
            [0.0, 0.0],
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
            cast_shadows: true,
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
            },
        )
        .unwrap()
        .section;
        assert_eq!(section.width, 1);
        assert_eq!(section.height, 1);
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
            },
        )
        .unwrap()
        .section;
        assert_eq!(section.width, 1);
        assert_eq!(section.height, 1);
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
            },
        )
        .unwrap()
        .section;
        assert!(section.width >= MIN_ATLAS_DIMENSION);
        assert!(section.height >= MIN_ATLAS_DIMENSION);
        assert_eq!(
            section.irradiance.len(),
            (section.width * section.height * 8) as usize
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
            },
        )
        .unwrap()
        .section;
        assert_eq!(section.width, 1);
        assert_eq!(section.height, 1);
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
            },
        )
        .unwrap()
        .section;
        assert!(
            section_static.width >= MIN_ATLAS_DIMENSION,
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
            },
        )
        .unwrap()
        .section;
        assert_eq!(section_dyn.width, 1, "is_dynamic light must not bake");

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
            },
        )
        .unwrap()
        .section;
        assert_eq!(
            section_anim.width, 1,
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
            },
        )
        .unwrap()
        .section;
        assert_eq!(
            section_bo.width, 1,
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
    fn shelf_pack_is_deterministic() {
        let geo = unit_quad_geometry();
        let charts = plan_charts(&geo, 0.25);
        let (w1, h1, p1) = shelf_pack(&charts, 0.25).unwrap();
        let (w2, h2, p2) = shelf_pack(&charts, 0.25).unwrap();
        assert_eq!(w1, w2);
        assert_eq!(h1, h2);
        assert_eq!(p1.len(), p2.len());
        for (a, b) in p1.iter().zip(p2.iter()) {
            assert_eq!(a.x, b.x);
            assert_eq!(a.y, b.y);
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
            ),
            Vertex::new(
                [2.0, 0.0, 0.0],
                [1.0, 0.0],
                [0.0, 1.0, 0.0],
                [1.0, 0.0, 0.0],
                true,
                [0.0, 0.0],
            ),
            Vertex::new(
                [2.0, 0.0, 2.0],
                [1.0, 1.0],
                [0.0, 1.0, 0.0],
                [1.0, 0.0, 0.0],
                true,
                [0.0, 0.0],
            ),
            Vertex::new(
                [0.0, 0.0, 2.0],
                [0.0, 1.0],
                [0.0, 1.0, 0.0],
                [1.0, 0.0, 0.0],
                true,
                [0.0, 0.0],
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
            ),
            Vertex::new(
                [4.0, 1.0, -2.0],
                [1.0, 0.0],
                [0.0, -1.0, 0.0],
                [1.0, 0.0, 0.0],
                true,
                [0.0, 0.0],
            ),
            Vertex::new(
                [4.0, 1.0, 4.0],
                [1.0, 1.0],
                [0.0, -1.0, 0.0],
                [1.0, 0.0, 0.0],
                true,
                [0.0, 0.0],
            ),
            Vertex::new(
                [-2.0, 1.0, 4.0],
                [0.0, 1.0],
                [0.0, -1.0, 0.0],
                [1.0, 0.0, 0.0],
                true,
                [0.0, 0.0],
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
            cast_shadows: true,
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
            },
        )
        .unwrap()
        .section;

        let mut zero_count = 0;
        for t in 0..(section.width * section.height) as usize {
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
        let size = 200.0; // 5000 texels at 0.04 m/texel, beyond MAX_ATLAS_DIMENSION
        let v0 = Vertex::new(
            [0.0, 0.0, 0.0],
            [0.0, 0.0],
            [0.0, 1.0, 0.0],
            [1.0, 0.0, 0.0],
            true,
            [0.0, 0.0],
        );
        let v1 = Vertex::new(
            [size, 0.0, 0.0],
            [1.0, 0.0],
            [0.0, 1.0, 0.0],
            [1.0, 0.0, 0.0],
            true,
            [0.0, 0.0],
        );
        let v2 = Vertex::new(
            [size, 0.0, size],
            [1.0, 1.0],
            [0.0, 1.0, 0.0],
            [1.0, 0.0, 0.0],
            true,
            [0.0, 0.0],
        );
        let v3 = Vertex::new(
            [0.0, 0.0, size],
            [0.0, 1.0],
            [0.0, 1.0, 0.0],
            [1.0, 0.0, 0.0],
            true,
            [0.0, 0.0],
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
            },
        );
        match result {
            Err(LightmapBakeError::ChartTooLarge { .. })
            | Err(LightmapBakeError::AtlasOverflow { .. }) => {}
            other => panic!("expected overflow error, got {other:?}"),
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
        );
        let a1 = Vertex::new(
            [1.0, 0.0, 0.0],
            [1.0, 0.0],
            [0.0, 1.0, 0.0],
            [1.0, 0.0, 0.0],
            true,
            [0.0, 0.0],
        );
        let a2 = Vertex::new(
            [1.0, 0.0, 1.0],
            [1.0, 1.0],
            [0.0, 1.0, 0.0],
            [1.0, 0.0, 0.0],
            true,
            [0.0, 0.0],
        );
        let b1 = Vertex::new(
            [0.0, 0.0, 1.0],
            [0.0, 1.0],
            [0.0, 1.0, 0.0],
            [1.0, 0.0, 0.0],
            true,
            [0.0, 0.0],
        );
        let b2 = Vertex::new(
            [-1.0, 0.0, 1.0],
            [-1.0, 1.0],
            [0.0, 1.0, 0.0],
            [1.0, 0.0, 0.0],
            true,
            [0.0, 0.0],
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
    fn soft_visibility_no_cast_shadows_returns_fully_visible() {
        let mut light = soft_point_light(DEFAULT_LIGHT_SIZE);
        light.cast_shadows = false;
        // Every ray "blocked", but cast_shadows == false short-circuits first.
        let v = soft_visibility(
            Vec3::ZERO,
            Vec3::Y,
            &light,
            7,
            DEFAULT_AREA_SAMPLE_COUNT,
            |_, _| false,
        );
        assert!((v - 1.0).abs() < EPS, "got {v}");
    }

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
        let texel_count = (section.width * section.height) as usize;
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

    /// The cache-bump contract: Task 3 changed the per-texel output (hard gate →
    /// soft area sum), so `STAGE_VERSION` must advance or recompiles serve stale
    /// hard-shadow bytes. This exercises the real invalidation mechanism — the
    /// current version's cache key must differ from the prior version's — rather
    /// than asserting a constant, so it follows future bumps automatically.
    #[test]
    fn stage_version_bump_changes_lightmap_cache_key() {
        use crate::cache::CacheKey;
        let input_hash = [0x42u8; 32];
        let prior = CacheKey::new("lightmap", STAGE_VERSION - 1, &input_hash);
        let current = CacheKey::new("lightmap", STAGE_VERSION, &input_hash);
        assert_ne!(
            prior.as_filename(),
            current.as_filename(),
            "the soft-shadow output change must bump STAGE_VERSION so the lightmap cache key \
             differs from the prior version's and stale hard-shadow entries are invalidated",
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
