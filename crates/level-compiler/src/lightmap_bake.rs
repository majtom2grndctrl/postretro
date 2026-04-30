// Directional lightmap baker.
// See: context/plans/ready/lighting-lightmaps/index.md

use std::collections::HashSet;

use bvh::bvh::Bvh;
use bvh::ray::Ray;
use glam::Vec3;
use nalgebra::{Point3, Vector3};
use postretro_level_format::lightmap::{LightmapSection, encode_direction_oct, f32_to_f16_bits};
use thiserror::Error;

use crate::bvh_build::BvhPrimitive;
use crate::chart_raster::{CHART_PADDING_TEXELS, ChartPlacement, chart_texel_world_position};
use crate::geometry::GeometryResult;
use crate::light_namespaces::StaticBakedLights;
use crate::map_data::{FalloffModel, LightType, MapLight};

/// Default atlas texel density: 4 cm per texel.
pub const DEFAULT_TEXEL_DENSITY_METERS: f32 = 0.04;

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

pub struct LightmapInputs<'a> {
    pub bvh: &'a Bvh<f32, 3>,
    pub primitives: &'a [BvhPrimitive],
    /// Mutable: baker writes per-vertex lightmap UVs back after atlas placement.
    pub geometry: &'a mut GeometryResult,
    pub lights: &'a StaticBakedLights<'a>,
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

/// Bake a directional lightmap. Returns a placeholder when there is nothing to bake.
pub fn bake_lightmap(
    inputs: &mut LightmapInputs<'_>,
    texel_density: f32,
) -> Result<LightmapBakeOutput, LightmapBakeError> {
    if inputs.geometry.geometry.vertices.is_empty() || inputs.geometry.geometry.faces.is_empty() {
        return Ok(LightmapBakeOutput {
            section: LightmapSection::placeholder(),
            charts: Vec::new(),
            placements: Vec::new(),
            atlas_width: 1,
            atlas_height: 1,
        });
    }

    let static_lights: Vec<&MapLight> = inputs.lights.entries().iter().map(|e| e.light).collect();
    if static_lights.is_empty() {
        // Plan charts anyway — the animated-light-chunks builder needs per-face UV bounds and
        // placements even when no static lights exist.
        let charts = plan_charts(inputs.geometry, texel_density);
        let (atlas_w, atlas_h, placements) = match shelf_pack(&charts, texel_density) {
            Ok(p) => p,
            Err(_) => (1, 1, Vec::new()),
        };
        return Ok(LightmapBakeOutput {
            section: LightmapSection::placeholder(),
            charts,
            placements,
            atlas_width: atlas_w,
            atlas_height: atlas_h,
        });
    }

    // Ensure no vertex index is shared across faces — each face must own its own lightmap UV slot.
    split_shared_vertices(inputs.geometry);

    let charts = plan_charts(inputs.geometry, texel_density);

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
        return Ok(LightmapBakeOutput {
            section: LightmapSection::placeholder(),
            charts,
            placements,
            atlas_width: atlas_w,
            atlas_height: atlas_h,
        });
    }

    assign_lightmap_uvs(inputs.geometry, &charts, &placements, atlas_w, atlas_h);

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

/// `pub(crate)` so the animated weight-map baker shares the same traversal and epsilon —
/// both bakers must agree on occlusion at chunk boundaries.
pub(crate) fn shadow_visible(
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
            cone_angle_inner: None,
            cone_angle_outer: None,
            cone_direction: None,
            animation: None,
            cast_shadows: true,
            bake_only: false,
            is_dynamic: false,
            tags: vec![],
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
        let mut inputs = LightmapInputs {
            bvh: &bvh,
            primitives: &prims,
            geometry: &mut geo,
            lights: &static_lights,
        };
        let section = bake_lightmap(&mut inputs, DEFAULT_TEXEL_DENSITY_METERS)
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
        let mut inputs = LightmapInputs {
            bvh: &bvh,
            primitives: &prims,
            geometry: &mut geo,
            lights: &static_lights,
        };
        let section = bake_lightmap(&mut inputs, DEFAULT_TEXEL_DENSITY_METERS)
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
        let mut inputs = LightmapInputs {
            bvh: &bvh,
            primitives: &prims,
            geometry: &mut geo,
            lights: &static_lights,
        };
        let section = bake_lightmap(&mut inputs, 0.25).unwrap().section;
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
        let static_lights = StaticBakedLights::from_lights(&lights);
        let mut inputs = LightmapInputs {
            bvh: &bvh,
            primitives: &prims,
            geometry: &mut geo,
            lights: &static_lights,
        };
        let section = bake_lightmap(&mut inputs, DEFAULT_TEXEL_DENSITY_METERS)
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
        let mut inputs = LightmapInputs {
            bvh: &bvh,
            primitives: &prims,
            geometry: &mut geo_static,
            lights: &static_base,
        };
        let section_static = bake_lightmap(&mut inputs, 0.25).unwrap().section;
        assert!(
            section_static.width >= MIN_ATLAS_DIMENSION,
            "non-animated static light must bake into a real atlas",
        );

        let mut dyn_light = point_light_above();
        dyn_light.is_dynamic = true;
        let mut geo_dyn = unit_quad_geometry();
        let (bvh_d, prims_d, _) = build_bvh(&geo_dyn).unwrap();
        let static_dyn = StaticBakedLights::from_lights(std::slice::from_ref(&dyn_light));
        let mut inputs_d = LightmapInputs {
            bvh: &bvh_d,
            primitives: &prims_d,
            geometry: &mut geo_dyn,
            lights: &static_dyn,
        };
        let section_dyn = bake_lightmap(&mut inputs_d, 0.25).unwrap().section;
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
        let mut inputs_a = LightmapInputs {
            bvh: &bvh_a,
            primitives: &prims_a,
            geometry: &mut geo_anim,
            lights: &static_anim,
        };
        let section_anim = bake_lightmap(&mut inputs_a, 0.25).unwrap().section;
        assert_eq!(
            section_anim.width, 1,
            "animated light must not contribute to the static atlas",
        );

        let mut bake_only_anim = anim_light.clone();
        bake_only_anim.bake_only = true;
        let mut geo_bo = unit_quad_geometry();
        let (bvh_b, prims_b, _) = build_bvh(&geo_bo).unwrap();
        let static_bo = StaticBakedLights::from_lights(std::slice::from_ref(&bake_only_anim));
        let mut inputs_b = LightmapInputs {
            bvh: &bvh_b,
            primitives: &prims_b,
            geometry: &mut geo_bo,
            lights: &static_bo,
        };
        let section_bo = bake_lightmap(&mut inputs_b, 0.25).unwrap().section;
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

    #[test]
    fn lightmap_uvs_in_zero_one_range_after_bake() {
        let mut geo = unit_quad_geometry();
        let (bvh, prims, _) = build_bvh(&geo).unwrap();
        let lights = vec![point_light_above()];
        let static_lights = StaticBakedLights::from_lights(&lights);
        let mut inputs = LightmapInputs {
            bvh: &bvh,
            primitives: &prims,
            geometry: &mut geo,
            lights: &static_lights,
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
            cone_angle_inner: None,
            cone_angle_outer: None,
            cone_direction: None,
            animation: None,
            cast_shadows: true,
            bake_only: false,
            is_dynamic: false,
            tags: vec![],
        };
        let lights = vec![light];
        let static_lights = StaticBakedLights::from_lights(&lights);
        let mut inputs = LightmapInputs {
            bvh: &bvh,
            primitives: &prims,
            geometry: &mut geo,
            lights: &static_lights,
        };
        let section = bake_lightmap(&mut inputs, 0.25).unwrap().section;

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
        let mut inputs = LightmapInputs {
            bvh: &bvh,
            primitives: &prims,
            geometry: &mut geo,
            lights: &static_lights,
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
        let mut inputs = LightmapInputs {
            bvh: &bvh,
            primitives: &prims,
            geometry: &mut geo,
            lights: &static_lights,
        };
        let _ = bake_lightmap(&mut inputs, 0.25).unwrap();

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
}
