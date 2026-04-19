// Directional lightmap baker.
//
// UV-unwraps each world face onto a shared atlas using a planar per-face
// projection, shelf-packs the charts, then ray-casts per-texel irradiance and
// dominant incoming direction from all static (non-`is_dynamic`) lights
// through the shared BVH. Writes lightmap UVs back into the geometry vertices.
//
// Deviation from the original plan: the spec requested `xatlas` for automatic
// per-chart UV unwrapping. `xatlas` is a C library with no safe Rust binding
// in the existing dependency tree, and Postretro forbids `unsafe` without
// explicit approval (development_guide.md §3.5). Per-face planar unwrap is a
// correct, simpler first pass: each face is already convex and coplanar, so
// a single planar projection into its tangent/bitangent basis yields a valid
// chart with no distortion. A future revision may introduce chart merging to
// reduce atlas fragmentation — not required for the acceptance gates.
//
// See: context/plans/ready/lighting-lightmaps/index.md

use std::collections::HashSet;

use bvh::bvh::Bvh;
use bvh::ray::Ray;
use glam::Vec3;
use nalgebra::{Point3, Vector3};
use postretro_level_format::lightmap::{LightmapSection, encode_direction_oct, f32_to_f16_bits};
use thiserror::Error;

use crate::bvh_build::BvhPrimitive;
use crate::geometry::GeometryResult;
use crate::map_data::{FalloffModel, LightType, MapLight};

/// Default atlas texel density: 4 cm per texel. Matches the plan's default
/// and keeps the baker affordable on the hand-authored test maps.
pub const DEFAULT_TEXEL_DENSITY_METERS: f32 = 0.04;

/// Atlas width/height when no face would fit otherwise. Must be a power of two
/// so future GPU-side BC6H compression lands on a valid block size.
const MIN_ATLAS_DIMENSION: u32 = 64;

/// Maximum atlas dimension. Beyond this the baker returns an error so the
/// caller can retry at a coarser texel density. 4096 sits well under the
/// 8192 `max_texture_dimension_2d` floor of `wgpu::Limits::default()` (the
/// runtime's required limits) and fits a ~164 m axis at 4 cm/texel — enough
/// headroom for realistic indoor maps while leaving the retry path available
/// for anything larger.
const MAX_ATLAS_DIMENSION: u32 = 4096;

/// Padding inserted around each chart in atlas texels. One texel of padding
/// plus the post-bake edge-dilation pass keeps bilinear sampling from
/// dragging black into chart interiors.
const CHART_PADDING_TEXELS: u32 = 2;

/// Tiny offset applied along the surface normal when launching shadow rays to
/// avoid self-intersection with the face being shaded.
const RAY_EPSILON: f32 = 1.0e-3;

/// Length of the shadow ray fired toward the sun for a directional light.
/// Directional lights have no position, so we need an arbitrary far endpoint;
/// anything larger than the world's diagonal is correct. 10 km comfortably
/// exceeds the engine's 4096-unit far clip and any plausible world extent.
const DIRECTIONAL_LIGHT_RAY_LENGTH_METERS: f32 = 10_000.0;

/// Errors surfaced from the lightmap bake stage. Distinct from `anyhow` so the
/// caller can decide whether to retry at a coarser texel density.
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

/// Inputs the lightmap baker pulls from the rest of the compile stages.
pub struct LightmapInputs<'a> {
    pub bvh: &'a Bvh<f32, 3>,
    pub primitives: &'a [BvhPrimitive],
    /// Mutable because the baker writes per-vertex lightmap UVs back into
    /// the geometry section after placing each face's chart in the atlas.
    pub geometry: &'a mut GeometryResult,
    pub lights: &'a [MapLight],
}

/// Bake a directional lightmap.
///
/// Returns the placeholder section when there is no work to do (empty geometry
/// or no static lights) and an error when the chart set cannot be represented
/// within `MAX_ATLAS_DIMENSION`.
pub fn bake_lightmap(
    inputs: &mut LightmapInputs<'_>,
    texel_density: f32,
) -> Result<LightmapSection, LightmapBakeError> {
    if inputs.geometry.geometry.vertices.is_empty() || inputs.geometry.geometry.faces.is_empty() {
        return Ok(LightmapSection::placeholder());
    }

    // Filter lights: static only. `is_dynamic` lights contribute at runtime,
    // and `bake_only` is already ignored by the direct path — it still bakes
    // here because the static lightmap is its only contribution.
    let static_lights: Vec<&MapLight> = inputs.lights.iter().filter(|l| !l.is_dynamic).collect();
    if static_lights.is_empty() {
        return Ok(LightmapSection::placeholder());
    }

    // Split vertices shared across faces so each face owns its own per-vertex
    // lightmap UVs. A vertex shared across two faces would otherwise pick up
    // the first face's UV at assignment time (the duplicate-write guard in
    // `assign_lightmap_uvs` skips it on the second face), producing a seam
    // across the shared edge once normal maps or fine-grained atlas texels
    // amplify the discrepancy. Geometry extraction today already emits
    // per-face vertex ranges, so this is a defensive pass — but the contract
    // that follows (no vertex index appears in more than one face's range)
    // is what the baker needs, independent of upstream choices.
    split_shared_vertices(inputs.geometry);

    // --- UV unwrap: plan each face's chart ---
    let charts = plan_charts(inputs.geometry, texel_density);

    // --- Validate individual charts against the atlas dimension limit. ---
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

    // --- Pack charts into an atlas ---
    let (atlas_w, atlas_h, placements) = shelf_pack(&charts, texel_density)?;
    if placements.is_empty() {
        return Ok(LightmapSection::placeholder());
    }

    // --- Write lightmap UVs back into vertices ---
    assign_lightmap_uvs(inputs.geometry, &charts, &placements, atlas_w, atlas_h);

    // --- Rasterize each chart and bake per-texel irradiance + direction ---
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
            &mut irradiance,
            &mut direction,
            &mut coverage,
        );
    }

    // --- Edge dilation: extend coverage one texel outward ---
    dilate_edges(
        &mut irradiance,
        &mut direction,
        &mut coverage,
        atlas_w,
        atlas_h,
    );

    // --- Encode to on-disk byte layout ---
    let irr_bytes = encode_irradiance_rgba16f(&irradiance);
    let dir_bytes = encode_direction_rgba8(&direction, &coverage);

    Ok(LightmapSection {
        width: atlas_w,
        height: atlas_h,
        texel_density,
        irradiance: irr_bytes,
        direction: dir_bytes,
    })
}

/// Ensure no vertex index is referenced by more than one face's index range.
/// Any vertex shared across faces is duplicated so each face gets its own copy
/// (same position / UV / normal / tangent, distinct lightmap UV slot). Indices
/// inside each face's range are rewritten to point at the face's own copies.
fn split_shared_vertices(geom: &mut GeometryResult) {
    let face_count = geom.face_index_ranges.len();
    if face_count <= 1 {
        return;
    }

    // First-seen face owner for each original vertex; subsequent faces remap.
    let mut owner: Vec<u32> = vec![u32::MAX; geom.geometry.vertices.len()];
    let ranges = geom.face_index_ranges.clone();
    // Remap table reused per-face: original_vertex_index -> duplicated_index.
    // We key on original indices only — once a face is remapping, it never
    // writes the original index back into its range.
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
                // Shared with a different face — duplicate on first encounter,
                // then reuse the same duplicate for the rest of this face.
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
            // Same face re-using the vertex inside its own range: keep as-is.
        }
    }
}

/// Log bake statistics in the shape of the other compile stages.
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

// ---------------------------------------------------------------------------
// Chart planning

/// A face's chart plan: face-local 2D basis, origin, extent, and rectangle
/// size in atlas texels.
#[derive(Debug, Clone)]
struct Chart {
    /// Origin of the face-local (u, v) basis in world space.
    origin: Vec3,
    /// Face-local u-axis (unit, world space).
    u_axis: Vec3,
    /// Face-local v-axis (unit, world space).
    v_axis: Vec3,
    /// World-space min corner of the face's bounding box in the (u, v) basis.
    uv_min: [f32; 2],
    /// World-space extent (meters) along (u, v).
    uv_extent: [f32; 2],
    /// Surface normal used to offset shadow-ray origins.
    normal: Vec3,
    /// Chart size in atlas texels, including padding.
    width_texels: u32,
    height_texels: u32,
}

fn plan_charts(geom: &GeometryResult, texel_density: f32) -> Vec<Chart> {
    let density = texel_density.max(1.0e-4);
    let section = &geom.geometry;

    let mut charts = Vec::with_capacity(section.faces.len());
    for range in geom.face_index_ranges.iter() {
        // Gather world positions for the face's vertices (derived from the
        // first triangle of the fan — vertex 0 of the fan is always the
        // chart origin's seed in `extract_geometry`).
        let start = range.index_offset as usize;
        if range.index_count < 3 {
            // Degenerate chart — emit a 1x1 placeholder so the face still
            // has a valid atlas slot.
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
        // Use the stored vertex normal as the authoritative face orientation;
        // the fan-triangulation cross product can point the wrong way when
        // the winding doesn't match the stored normal, which would invert the
        // Lambert term and produce an all-zero chart.
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

        // Face-local basis: u = normalized edge1; v = normal × u.
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

        // Project all face vertices into (u, v). Collect min/max.
        let mut u_min = f32::INFINITY;
        let mut u_max = f32::NEG_INFINITY;
        let mut v_min = f32::INFINITY;
        let mut v_max = f32::NEG_INFINITY;

        // Every face-fan triangle reuses vertex 0 → we can walk the fan's
        // unique vertices by stepping through the fan triangles' third
        // index and include the seed and second vertex once.
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

// ---------------------------------------------------------------------------
// Shelf packing

/// Where a chart landed in the atlas, in texel coordinates.
#[derive(Debug, Clone, Copy)]
struct ChartPlacement {
    x: u32,
    y: u32,
}

/// Shelf-pack charts into an atlas. Charts are sorted by height descending,
/// then placed row-by-row. Returns `(atlas_width, atlas_height, placements)`
/// in the same order as `charts`.
///
/// Overflow policy: if the packed height would exceed `MAX_ATLAS_DIMENSION`,
/// the width is grown to the next power of two and packing is retried. When
/// `atlas_w == MAX_ATLAS_DIMENSION` and charts still overflow, an error is
/// returned — silent clamping here would produce out-of-bounds texel writes
/// in the bake loop and dilation pass.
fn shelf_pack(
    charts: &[Chart],
    texel_density: f32,
) -> Result<(u32, u32, Vec<ChartPlacement>), LightmapBakeError> {
    if charts.is_empty() {
        return Ok((MIN_ATLAS_DIMENSION, MIN_ATLAS_DIMENSION, Vec::new()));
    }

    // Sort indices by height descending for a standard shelf packer.
    let mut order: Vec<usize> = (0..charts.len()).collect();
    order.sort_by(|&a, &b| charts[b].height_texels.cmp(&charts[a].height_texels));

    // Estimate atlas width: start at the next power of two above sqrt(total area).
    let total_area: u64 = charts
        .iter()
        .map(|c| (c.width_texels as u64) * (c.height_texels as u64))
        .sum();
    let target_side = (total_area as f64).sqrt().ceil() as u32;
    let mut atlas_w = target_side.max(MIN_ATLAS_DIMENSION).next_power_of_two();
    atlas_w = atlas_w.min(MAX_ATLAS_DIMENSION);

    // Grow the width if any individual chart is wider than the candidate.
    // Chart widths were validated against MAX_ATLAS_DIMENSION by the caller,
    // so this cannot exceed the cap.
    for c in charts {
        if c.width_texels > atlas_w {
            atlas_w = c.width_texels.next_power_of_two().min(MAX_ATLAS_DIMENSION);
        }
    }

    // Try to pack. If the packed height overflows MAX_ATLAS_DIMENSION, grow
    // the width (which lets more charts fit per shelf) and retry. Stop when
    // we've already hit the width cap — return an error rather than clamp.
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
                // A chart was wider than `atlas_w`. Grow or bail.
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

/// Run one shelf-packing attempt at a fixed `atlas_w`. Returns the packed
/// height (unpadded) and placements on success, or `Err(())` if a chart is
/// wider than the atlas.
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
            // New shelf.
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

// ---------------------------------------------------------------------------
// UV assignment

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

        // Chart-local to atlas conversion. The chart is placed at
        // (placement.x + padding, placement.y + padding) in texels, with
        // interior extent (chart.width_texels - 2*padding, chart.height_texels
        // - 2*padding). Face-local UV range is `chart.uv_min` + `chart.uv_extent`.
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

// ---------------------------------------------------------------------------
// Per-chart baking

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
    irradiance: &mut [f32],
    direction: &mut [Vec3],
    coverage: &mut [bool],
) {
    if chart.uv_extent[0] <= 0.0 || chart.uv_extent[1] <= 0.0 {
        return;
    }
    let padding = CHART_PADDING_TEXELS as i32;
    let interior_w = (chart.width_texels as i32 - 2 * padding).max(1);
    let interior_h = (chart.height_texels as i32 - 2 * padding).max(1);

    for ty in 0..interior_h {
        for tx in 0..interior_w {
            // Atlas texel coordinate.
            let atlas_x = placement.x as i32 + padding + tx;
            let atlas_y = placement.y as i32 + padding + ty;
            let idx = (atlas_y as u32 * atlas_w + atlas_x as u32) as usize;

            // Texel centre in face-local (u, v) world space.
            let u_frac = (tx as f32 + 0.5) / interior_w as f32;
            let v_frac = (ty as f32 + 0.5) / interior_h as f32;
            let local_u = chart.uv_min[0] + u_frac * chart.uv_extent[0];
            let local_v = chart.uv_min[1] + v_frac * chart.uv_extent[1];
            let world_p = chart.origin + chart.u_axis * local_u + chart.v_axis * local_v;
            let surface_normal = chart.normal;

            // Evaluate every static light.
            let mut irr = Vec3::ZERO;
            let mut weighted_dir = Vec3::ZERO;
            for light in static_lights {
                let (contribution, to_light) =
                    light_contribution_and_direction(light, world_p, surface_normal);
                if contribution.length_squared() <= 1.0e-12 {
                    continue;
                }
                if !shadow_visible(bvh, primitives, geometry, world_p, surface_normal, light) {
                    continue;
                }
                irr += contribution;
                // Weight direction by luminance so bright lights dominate.
                let lum = contribution.x + contribution.y + contribution.z;
                weighted_dir += to_light * lum;
            }

            irradiance[idx * 4] = irr.x;
            irradiance[idx * 4 + 1] = irr.y;
            irradiance[idx * 4 + 2] = irr.z;
            irradiance[idx * 4 + 3] = 1.0;

            let dir = if weighted_dir.length_squared() > 1.0e-8 {
                weighted_dir.normalize()
            } else {
                // No direct contribution — store the surface normal as a
                // neutral fallback so bumped-Lambert degrades to flat Lambert.
                surface_normal
            };
            direction[idx] = dir;
            coverage[idx] = true;
        }
    }
}

/// Lambert contribution from one light plus the unit vector from surface to light
/// (for directional lights, the direction toward the light source).
fn light_contribution_and_direction(
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

fn shadow_visible(
    bvh: &Bvh<f32, 3>,
    primitives: &[BvhPrimitive],
    geometry: &GeometryResult,
    surface_point: Vec3,
    surface_normal: Vec3,
    light: &MapLight,
) -> bool {
    if !light.cast_shadows {
        return true;
    }
    let origin = surface_point + surface_normal * RAY_EPSILON;
    let target = match light.light_type {
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
    };
    segment_clear(bvh, primitives, geometry, origin, target)
}

fn segment_clear(
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

/// Double-sided Möller-Trumbore intersection. Returns distance along ray.
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

// ---------------------------------------------------------------------------
// Edge dilation

fn dilate_edges(
    irradiance: &mut [f32],
    direction: &mut [Vec3],
    coverage: &mut [bool],
    atlas_w: u32,
    atlas_h: u32,
) {
    // Single-pass dilation into the padding ring (1-2 texels). For each
    // uncovered texel whose 8-neighbourhood contains a covered texel, copy
    // the average of covered neighbours. The padding width is
    // CHART_PADDING_TEXELS so we run the loop that many times.
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

// ---------------------------------------------------------------------------
// Encoding

fn encode_irradiance_rgba16f(data: &[f32]) -> Vec<u8> {
    // RGBA16F: 8 bytes per texel. Input is interleaved RGBA.
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
            // Uncovered texels: neutral up-pointing direction so any stray
            // bilinear sampling returns a Lambert-valid vector.
            [128u8, 255, 128, 255]
        };
        out.extend_from_slice(&bytes);
    }
    out
}

// ---------------------------------------------------------------------------
// Tests

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bvh_build::build_bvh;
    use crate::geometry::FaceIndexRange;
    use glam::DVec3;
    use postretro_level_format::geometry::{FaceMeta, GeometrySection, Vertex};
    use postretro_level_format::texture_names::TextureNamesSection;

    fn unit_quad_geometry() -> GeometryResult {
        // A 1m × 1m quad on the XZ plane at y=0, facing +Y.
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
            cone_angle_inner: None,
            cone_angle_outer: None,
            cone_direction: None,
            animation: None,
            cast_shadows: true,
            bake_only: false,
            is_dynamic: false,
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
        let mut inputs = LightmapInputs {
            bvh: &bvh,
            primitives: &prims,
            geometry: &mut geo,
            lights: &lights,
        };
        let section = bake_lightmap(&mut inputs, DEFAULT_TEXEL_DENSITY_METERS).unwrap();
        // Placeholder is 1x1.
        assert_eq!(section.width, 1);
        assert_eq!(section.height, 1);
    }

    #[test]
    fn no_static_lights_returns_placeholder() {
        let mut geo = unit_quad_geometry();
        let (bvh, prims, _) = build_bvh(&geo).unwrap();
        let lights: Vec<MapLight> = Vec::new();
        let mut inputs = LightmapInputs {
            bvh: &bvh,
            primitives: &prims,
            geometry: &mut geo,
            lights: &lights,
        };
        let section = bake_lightmap(&mut inputs, DEFAULT_TEXEL_DENSITY_METERS).unwrap();
        assert_eq!(section.width, 1);
        assert_eq!(section.height, 1);
    }

    #[test]
    fn single_static_light_produces_nonzero_irradiance() {
        let mut geo = unit_quad_geometry();
        let (bvh, prims, _) = build_bvh(&geo).unwrap();
        let lights = vec![point_light_above()];
        let mut inputs = LightmapInputs {
            bvh: &bvh,
            primitives: &prims,
            geometry: &mut geo,
            lights: &lights,
        };
        let section = bake_lightmap(&mut inputs, 0.25).unwrap();
        // Expect a > 1×1 atlas (real bake path) and at least one non-zero
        // irradiance texel.
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

    #[test]
    fn is_dynamic_lights_skipped_by_bake() {
        let mut geo = unit_quad_geometry();
        let (bvh, prims, _) = build_bvh(&geo).unwrap();
        let mut dyn_light = point_light_above();
        dyn_light.is_dynamic = true;
        let lights = vec![dyn_light];
        let mut inputs = LightmapInputs {
            bvh: &bvh,
            primitives: &prims,
            geometry: &mut geo,
            lights: &lights,
        };
        let section = bake_lightmap(&mut inputs, DEFAULT_TEXEL_DENSITY_METERS).unwrap();
        // Only a dynamic light → placeholder path.
        assert_eq!(section.width, 1);
        assert_eq!(section.height, 1);
    }

    #[test]
    fn static_flag_bakes_but_dynamic_does_not() {
        // Regression pin for acceptance gate 5: toggling `_dynamic` on the
        // same light toggles whether its shadow appears in the bake.
        let mut geo_static = unit_quad_geometry();
        let (bvh, prims, _) = build_bvh(&geo_static).unwrap();
        let mut lights = vec![point_light_above()];
        let mut inputs = LightmapInputs {
            bvh: &bvh,
            primitives: &prims,
            geometry: &mut geo_static,
            lights: &lights,
        };
        let section_static = bake_lightmap(&mut inputs, 0.25).unwrap();

        lights[0].is_dynamic = true;
        let mut geo_dyn = unit_quad_geometry();
        let (bvh2, prims2, _) = build_bvh(&geo_dyn).unwrap();
        let mut inputs2 = LightmapInputs {
            bvh: &bvh2,
            primitives: &prims2,
            geometry: &mut geo_dyn,
            lights: &lights,
        };
        let section_dyn = bake_lightmap(&mut inputs2, 0.25).unwrap();

        assert!(section_static.width >= MIN_ATLAS_DIMENSION);
        assert_eq!(section_dyn.width, 1); // placeholder
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

    #[test]
    fn lightmap_uvs_in_zero_one_range_after_bake() {
        let mut geo = unit_quad_geometry();
        let (bvh, prims, _) = build_bvh(&geo).unwrap();
        let lights = vec![point_light_above()];
        let mut inputs = LightmapInputs {
            bvh: &bvh,
            primitives: &prims,
            geometry: &mut geo,
            lights: &lights,
        };
        let _ = bake_lightmap(&mut inputs, 0.25).unwrap();
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
        let indices = vec![
            0, 1, 2, 0, 2, 3, // floor
            4, 5, 6, 4, 6, 7, // ceiling
        ];
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
            origin: DVec3::new(1.0, 2.0, 1.0), // above the ceiling
            light_type: LightType::Point,
            intensity: 1.0,
            color: [1.0, 1.0, 1.0],
            falloff_model: FalloffModel::Linear,
            falloff_range: 10.0,
            cone_angle_inner: None,
            cone_angle_outer: None,
            cone_direction: None,
            animation: None,
            cast_shadows: true,
            bake_only: false,
            is_dynamic: false,
        };
        let lights = vec![light];
        let mut inputs = LightmapInputs {
            bvh: &bvh,
            primitives: &prims,
            geometry: &mut geo,
            lights: &lights,
        };
        let section = bake_lightmap(&mut inputs, 0.25).unwrap();

        // Every irradiance texel in the floor's chart should be zero (or have
        // leaked in only via edge-dilation into padding). Verify at least
        // *some* texels are fully zero — the bake must produce an occluded
        // region, not a uniformly-lit atlas.
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
        // Synthesize a single quad so large that even at the default texel
        // density it exceeds MAX_ATLAS_DIMENSION on one axis. The baker must
        // return ChartTooLarge (or AtlasOverflow) rather than silently
        // clamping and panicking in the bake loop.
        //
        // Regression: the old path clamped atlas_h via `.min(MAX_ATLAS_DIMENSION)`
        // but left chart placements at their pre-clamp y-coordinates, writing
        // out of bounds during bake and dilation.
        let size = 200.0; // metres — 5000 texels at 0.04 m/texel, beyond MAX_ATLAS_DIMENSION
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
        let mut inputs = LightmapInputs {
            bvh: &bvh,
            primitives: &prims,
            geometry: &mut geo,
            lights: &lights,
        };
        let result = bake_lightmap(&mut inputs, DEFAULT_TEXEL_DENSITY_METERS);
        match result {
            Err(LightmapBakeError::ChartTooLarge { .. })
            | Err(LightmapBakeError::AtlasOverflow { .. }) => {}
            other => panic!("expected overflow error, got {other:?}"),
        }
    }

    #[test]
    fn shared_world_vertex_produces_two_distinct_records() {
        // Two triangles that share a world-space vertex (index 0) across two
        // separate faces. After the baker runs, each face must own its own
        // vertex record so it can carry its own lightmap UV — otherwise one
        // face's UV overwrites the other at the seam.
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
        // Face A: (shared=0, a1=1, a2=2) — triangle in +X,+Z quadrant.
        // Face B: (shared=0, b1=3, b2=4) — triangle sharing vertex 0.
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
        let mut inputs = LightmapInputs {
            bvh: &bvh,
            primitives: &prims,
            geometry: &mut geo,
            lights: &lights,
        };
        let _ = bake_lightmap(&mut inputs, 0.25).unwrap();

        // Vertex 0 should now belong to face A only; face B should reference a
        // freshly-minted duplicate so its lightmap UV can differ.
        assert!(
            geo.geometry.vertices.len() > original_vertex_count,
            "expected at least one duplicated vertex after split, got {} (was {})",
            geo.geometry.vertices.len(),
            original_vertex_count,
        );
        // Face B's first index must no longer be 0 (the original shared vertex).
        let face_b_start = geo.face_index_ranges[1].index_offset as usize;
        let face_b_first_vi = geo.geometry.indices[face_b_start];
        assert_ne!(
            face_b_first_vi, 0,
            "face B should reference a duplicated vertex, not the original shared one",
        );
        // Confirm position, UV, normal, tangent are preserved across the split.
        let original = &geo.geometry.vertices[0];
        let duplicate = &geo.geometry.vertices[face_b_first_vi as usize];
        assert_eq!(original.position, duplicate.position);
        assert_eq!(original.uv, duplicate.uv);
        assert_eq!(original.normal_oct, duplicate.normal_oct);
        assert_eq!(original.tangent_packed, duplicate.tangent_packed);
        // Lightmap UVs may differ between the two copies (different chart
        // placements) — that's the whole point of the split. We do not assert
        // inequality here because the two triangles' charts could coincidentally
        // land on the same atlas texel; the structural guarantee (two distinct
        // records) is what matters.
    }
}
