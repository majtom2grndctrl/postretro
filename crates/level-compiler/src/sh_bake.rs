// Octahedral irradiance atlas baker. Uses L2 SH as an intermediate projection.
// See: context/lib/build_pipeline.md

use std::collections::HashSet;

use bvh::bvh::Bvh;
use bvh::ray::Ray;
use glam::{DVec3, Vec3};
use nalgebra::{Point3, Vector3};
use postretro_level_format::lightmap::f32_to_f16_bits;
use postretro_level_format::octahedral::{
    DEFAULT_IRRADIANCE_TILE_BORDER, DEFAULT_IRRADIANCE_TILE_DIMENSION, irradiance_atlas_dimensions,
    irradiance_atlas_tiles_per_row, irradiance_interior_texel_direction, irradiance_tile_origin,
    irradiance_tile_source_texel,
};
use postretro_level_format::sh_volume::{
    ANIMATED_SLOT_NONE, AnimationDescriptor, OCTAHEDRAL_PROBE_STRIDE, OctahedralAtlasTexel,
    OctahedralShProbe, OctahedralShVolumeSection,
};
use rayon::prelude::*;

use crate::bvh_build::BvhPrimitive;
use crate::geometry::GeometryResult;
use crate::light_namespaces::{AnimatedBakedLights, StaticBakedLights};
use crate::map_data::{FalloffModel, LightAnimation, LightType, MapLight};
use crate::partition::{BspTree, find_leaf_for_point};

/// Default grid cell size in meters. Overridden by `--probe-spacing`.
pub const DEFAULT_PROBE_SPACING: f32 = 1.0;

/// Bump this when the SH baking algorithm changes. Invalidates all existing
/// cache entries for this stage.
pub const STAGE_VERSION: u32 = 4;

const RAYS_PER_PROBE: u32 = 256;

/// Indirect-only: lightmap carries the direct term; folding direct into SH would double-count it at runtime.
const BOUNCE_ALBEDO: f32 = 0.45;

const SKY_COLOR: [f32; 3] = [0.0, 0.0, 0.0];

/// Offset to avoid self-intersections at the probe origin and shadow-ray hit points.
const RAY_EPSILON: f32 = 1.0e-3;

/// Rotates the Fibonacci lattice off the (0,0,1) axis so axis-aligned light directions
/// don't land on a degenerate sample. No RNG — identical input yields byte-identical output.
const SAMPLING_LATTICE_OFFSET: u64 = 0x5048_4542_414b_4552; // "PHBAKER"

/// Shared between the base SH baker and per-light delta SH baker (`delta_sh_bake.rs`).
pub(crate) struct RaytracingCtx<'a> {
    pub bvh: &'a Bvh<f32, 3>,
    pub primitives: &'a [BvhPrimitive],
    pub geometry: &'a GeometryResult,
}

pub struct ShBakeCtx<'a> {
    pub bvh: &'a Bvh<f32, 3>,
    pub primitives: &'a [BvhPrimitive],
    pub geometry: &'a GeometryResult,
    pub tree: &'a BspTree,
    /// Probes in these leaves are flagged invalid — out-of-map radiance pollutes
    /// trilinear interpolation at the playable edge.
    pub exterior_leaves: &'a HashSet<usize>,
    pub static_lights: &'a StaticBakedLights<'a>,
    pub animated_lights: &'a AnimatedBakedLights<'a>,
    /// Total `MapLight` count before static/animated filtering. Sizes the
    /// `slot_for_map_light` table emitted with `OctahedralShVolumeSection`.
    pub total_light_count: usize,
}

impl<'a> ShBakeCtx<'a> {
    fn ray_ctx(&self) -> RaytracingCtx<'a> {
        RaytracingCtx {
            bvh: self.bvh,
            primitives: self.primitives,
            geometry: self.geometry,
        }
    }
}

#[derive(Clone, Copy)]
struct BakedProbe {
    coefficients: [f32; 27],
    metadata: OctahedralShProbe,
}

impl Default for BakedProbe {
    fn default() -> Self {
        Self {
            coefficients: [0.0; 27],
            metadata: OctahedralShProbe::default(),
        }
    }
}

/// Owned, serializable snapshot of the data the SH volume bake reads. Used for
/// cache key derivation: postcard-serialize this + ShConfig to get the input hash.
#[derive(serde::Serialize)]
pub struct ShInputs {
    pub static_lights: Vec<crate::map_data::MapLight>,
    pub animated_lights: Vec<crate::map_data::MapLight>,
    pub geometry: crate::geometry::GeometryResult,
    /// Sorted list of exterior BSP leaf indices. Probes in these leaves are
    /// flagged invalid. Included so the hash catches changes that affect probe
    /// validity even when geometry is otherwise unchanged.
    pub exterior_leaves: Vec<usize>,
}

/// CLI-driven configuration for the SH volume bake.
#[derive(serde::Serialize)]
pub struct ShConfig {
    pub probe_spacing: f32,
}

/// Returns an empty section (`grid_dimensions == [0,0,0]`) for empty geometry,
/// matching the "no SH section" degradation path at runtime.
pub fn bake_sh_volume(inputs: &ShBakeCtx<'_>, config: &ShConfig) -> OctahedralShVolumeSection {
    let probe_spacing_meters = config.probe_spacing;
    let geom = &inputs.geometry.geometry;
    if geom.vertices.is_empty() {
        return OctahedralShVolumeSection {
            grid_origin: [0.0, 0.0, 0.0],
            cell_size: [probe_spacing_meters; 3],
            grid_dimensions: [0, 0, 0],
            probe_stride: OCTAHEDRAL_PROBE_STRIDE,
            tile_dimension: DEFAULT_IRRADIANCE_TILE_DIMENSION,
            tile_border: DEFAULT_IRRADIANCE_TILE_BORDER,
            atlas_dimensions: [0, 0],
            atlas_tiles_per_row: 0,
            probes: Vec::new(),
            atlas_texels: Vec::new(),
            animation_descriptors: Vec::new(),
            slot_for_map_light: vec![ANIMATED_SLOT_NONE; inputs.total_light_count],
        };
    }

    let (world_min, world_max) = world_aabb(inputs);
    let dims = grid_dimensions(world_min, world_max, probe_spacing_meters);
    let total = dims[0] as usize * dims[1] as usize * dims[2] as usize;
    let cell_size = [probe_spacing_meters; 3];

    let static_lights: Vec<&MapLight> = inputs
        .static_lights
        .entries()
        .iter()
        .map(|e| e.light)
        .collect();
    let animated_lights: Vec<&MapLight> = inputs
        .animated_lights
        .entries()
        .iter()
        .map(|e| e.light)
        .collect();

    let probe_positions: Vec<DVec3> = (0..total)
        .map(|i| probe_position(i, dims, world_min, probe_spacing_meters))
        .collect();

    let validity: Vec<u8> = probe_positions
        .iter()
        .map(|&p| {
            if probe_is_valid(inputs.tree, inputs.exterior_leaves, p) {
                1
            } else {
                0
            }
        })
        .collect();

    // Sky-miss rays contribute this distance to the depth moments. Cell-relative
    // (4× the full 3D cell diagonal) so it reads as "fully open" at the
    // probe-spacing scale the runtime Chebyshev interpolant operates in.
    let far_sentinel = 4.0 * Vec3::from(cell_size).length();

    let baked_probes: Vec<BakedProbe> = (0..total)
        .into_par_iter()
        .map(|i| {
            if validity[i] == 0 {
                return BakedProbe::default();
            }
            let pos = vec3_from(probe_positions[i]);
            let (coeffs, sum_d, sum_d2) =
                bake_probe_rgb_with_moments(inputs, pos, &static_lights, far_sentinel);
            // All RAYS_PER_PROBE rays accumulate (sky misses via the sentinel),
            // so these are exact divisions by the constant.
            let mean_distance = sum_d / RAYS_PER_PROBE as f32;
            let mean_sq_distance = sum_d2 / RAYS_PER_PROBE as f32;
            BakedProbe {
                coefficients: coeffs,
                metadata: OctahedralShProbe {
                    validity: 1,
                    mean_distance: f32_to_f16_bits(mean_distance),
                    mean_sq_distance: f32_to_f16_bits(mean_sq_distance),
                },
            }
        })
        .collect();
    let base_probes: Vec<OctahedralShProbe> = baked_probes.iter().map(|p| p.metadata).collect();
    let atlas_dimensions = irradiance_atlas_dimensions(dims, DEFAULT_IRRADIANCE_TILE_DIMENSION);
    let atlas_tiles_per_row = irradiance_atlas_tiles_per_row(dims)
        .expect("non-empty SH probe grid should have a valid atlas tile row count");
    let atlas_texels = pack_octahedral_irradiance_atlas(
        &baked_probes,
        dims,
        DEFAULT_IRRADIANCE_TILE_DIMENSION,
        DEFAULT_IRRADIANCE_TILE_BORDER,
        atlas_dimensions,
        atlas_tiles_per_row,
    );

    // Per-light monochrome SH layers removed; animated indirect is handled by the SH compose pass via delta SH volumes.
    let animation_descriptors: Vec<AnimationDescriptor> = animated_lights
        .iter()
        .map(|l| animation_descriptor_for(l))
        .collect();

    // Emit the map-light-index to animated-slot table consumed by runtime
    // animation lookup. This is the inverse of `AnimatedBakedLights` slotting.
    let mut slot_for_map_light = vec![ANIMATED_SLOT_NONE; inputs.total_light_count];
    for (slot, entry) in inputs.animated_lights.entries().iter().enumerate() {
        if entry.source_index < slot_for_map_light.len() {
            slot_for_map_light[entry.source_index] = slot as u32;
        }
    }

    OctahedralShVolumeSection {
        grid_origin: [world_min.x as f32, world_min.y as f32, world_min.z as f32],
        cell_size,
        grid_dimensions: dims,
        probe_stride: OCTAHEDRAL_PROBE_STRIDE,
        tile_dimension: DEFAULT_IRRADIANCE_TILE_DIMENSION,
        tile_border: DEFAULT_IRRADIANCE_TILE_BORDER,
        atlas_dimensions,
        atlas_tiles_per_row,
        probes: base_probes,
        atlas_texels,
        animation_descriptors,
        slot_for_map_light,
    }
}

pub fn log_stats(section: &OctahedralShVolumeSection) {
    let dims = section.grid_dimensions;
    let total = section.total_probes();
    let valid = section.probes.iter().filter(|p| p.validity == 1).count();
    let invalid = total - valid;

    // Coarse depth-moment aggregate over valid probes: mean and max E[d].
    // Decoded from the stored f16 bits — diagnostic only, so f16 precision is
    // fine and avoids threading the pre-rounding f32 moments through the bake.
    let mut sum_mean_d = 0.0f64;
    let mut max_mean_d = 0.0f32;
    for probe in section.probes.iter().filter(|p| p.validity == 1) {
        let mean_d = f16_bits_to_f32(probe.mean_distance);
        sum_mean_d += mean_d as f64;
        max_mean_d = max_mean_d.max(mean_d);
    }
    let avg_mean_d = if valid > 0 {
        (sum_mean_d / valid as f64) as f32
    } else {
        0.0
    };

    log::info!(
        "OctahedralShVolume: grid {}x{}x{} = {total} probes ({valid} valid, \
         {invalid} invalid), tile {} (border {}), atlas {}x{} ({} tile(s)/row), cell {}m, \
         depth E[d] mean {avg_mean_d:.2}m / max {max_mean_d:.2}m, \
         {} animated light(s)",
        dims[0],
        dims[1],
        dims[2],
        section.tile_dimension,
        section.tile_border,
        section.atlas_dimensions[0],
        section.atlas_dimensions[1],
        section.atlas_tiles_per_row,
        section.cell_size[0],
        section.animation_descriptors.len(),
    );
}

/// Decode IEEE 754 binary16 bits → f32. The inverse of
/// `lightmap::f32_to_f16_bits` for the depth moments; used only for the
/// diagnostic stats aggregate, so it covers finite non-negative magnitudes.
fn f16_bits_to_f32(bits: u16) -> f32 {
    let sign = (bits >> 15) & 0x1;
    let exp = (bits >> 10) & 0x1f;
    let mant = bits & 0x3ff;
    let value = if exp == 0 {
        // Subnormal: no implicit leading 1.
        (mant as f32) * 2.0f32.powi(-24)
    } else if exp == 0x1f {
        if mant == 0 { f32::INFINITY } else { f32::NAN }
    } else {
        let m = 1.0 + (mant as f32) / 1024.0;
        m * 2.0f32.powi(exp as i32 - 15)
    };
    if sign == 1 { -value } else { value }
}

fn world_aabb(inputs: &ShBakeCtx<'_>) -> (DVec3, DVec3) {
    let mut min = DVec3::splat(f64::INFINITY);
    let mut max = DVec3::splat(f64::NEG_INFINITY);
    for v in &inputs.geometry.geometry.vertices {
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

fn grid_dimensions(min: DVec3, max: DVec3, spacing: f32) -> [u32; 3] {
    let extents = (max - min).max(DVec3::splat(0.0));
    let spacing = spacing.max(1.0e-4) as f64;
    [
        ((extents.x / spacing).ceil() as u32 + 1).max(1),
        ((extents.y / spacing).ceil() as u32 + 1).max(1),
        ((extents.z / spacing).ceil() as u32 + 1).max(1),
    ]
}

/// z-major, then y, then x — matches the format crate's probe iteration order.
fn probe_index_to_xyz(linear: usize, dims: [u32; 3]) -> (u32, u32, u32) {
    let nx = dims[0] as usize;
    let ny = dims[1] as usize;
    let z = linear / (nx * ny);
    let rem = linear - z * nx * ny;
    let y = rem / nx;
    let x = rem - y * nx;
    (x as u32, y as u32, z as u32)
}

fn probe_position(linear: usize, dims: [u32; 3], origin: DVec3, spacing: f32) -> DVec3 {
    let (x, y, z) = probe_index_to_xyz(linear, dims);
    DVec3::new(
        origin.x + x as f64 * spacing as f64,
        origin.y + y as f64 * spacing as f64,
        origin.z + z as f64 * spacing as f64,
    )
}

fn probe_is_valid(tree: &BspTree, exterior: &HashSet<usize>, pos: DVec3) -> bool {
    if tree.leaves.is_empty() {
        return true;
    }
    let leaf = find_leaf_for_point(tree, pos);
    if tree.leaves[leaf].is_solid {
        return false;
    }
    // Exterior leaves pour out-of-map radiance into trilinear neighbors — see sub-plan 10 §"Fix D".
    !exterior.contains(&leaf)
}

fn vec3_from(v: DVec3) -> Vec3 {
    Vec3::new(v.x as f32, v.y as f32, v.z as f32)
}

/// Fibonacci-sphere directions: evenly-spaced, no RNG, identical input → identical output.
fn sphere_directions(count: u32, seed: u64) -> Vec<Vec3> {
    let mut out = Vec::with_capacity(count as usize);
    let phi = std::f32::consts::PI * (3.0 - (5.0_f32).sqrt()); // golden angle
    let seed_offset = ((seed & 0xFFFF_FFFF) as f32) / (u32::MAX as f32);
    for i in 0..count {
        let t = (i as f32 + 0.5) / count as f32;
        let y = 1.0 - 2.0 * t;
        let radius = (1.0 - y * y).max(0.0).sqrt();
        let theta = phi * i as f32 + seed_offset * std::f32::consts::TAU;
        let x = theta.cos() * radius;
        let z = theta.sin() * radius;
        out.push(Vec3::new(x, y, z).normalize());
    }
    out
}

struct Hit {
    point: Vec3,
    normal: Vec3,
    distance: f32,
}

fn closest_hit(
    ctx: &RaytracingCtx<'_>,
    ray_origin: Vec3,
    ray_dir: Vec3,
    max_distance: f32,
) -> Option<Hit> {
    let ray = Ray::new(
        Point3::new(ray_origin.x, ray_origin.y, ray_origin.z),
        Vector3::new(ray_dir.x, ray_dir.y, ray_dir.z),
    );
    let candidates = ctx.bvh.traverse(&ray, ctx.primitives);
    if candidates.is_empty() {
        return None;
    }

    let geom = &ctx.geometry.geometry;
    let mut best: Option<Hit> = None;

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

            if let Some((dist, normal)) = ray_triangle_hit(ray_origin, ray_dir, p0, p1, p2) {
                if dist > RAY_EPSILON && dist < max_distance {
                    let update = best.as_ref().map(|b| dist < b.distance).unwrap_or(true);
                    if update {
                        best = Some(Hit {
                            point: ray_origin + ray_dir * dist,
                            normal,
                            distance: dist,
                        });
                    }
                }
            }
        }
    }

    if let Some(h) = best.as_mut() {
        if h.distance >= max_distance {
            return None;
        }
    }
    best
}

/// Double-sided Möller-Trumbore. Normal is flipped toward the incoming ray so
/// indirect illumination does not vanish at back-facing walls.
fn ray_triangle_hit(origin: Vec3, dir: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Option<(f32, Vec3)> {
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
    if t <= 0.0 {
        return None;
    }
    let mut normal = edge1.cross(edge2).normalize_or_zero();
    if normal.dot(dir) > 0.0 {
        normal = -normal;
    }
    Some((t, normal))
}

fn segment_clear(ctx: &RaytracingCtx<'_>, from: Vec3, to: Vec3) -> bool {
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
    let candidates = ctx.bvh.traverse(&ray, ctx.primitives);
    let max_distance = length - RAY_EPSILON;
    let geom = &ctx.geometry.geometry;
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
            if let Some((dist, _)) = ray_triangle_hit(origin, dir, p0, p1, p2) {
                if dist > 0.0 && dist < max_distance {
                    return false;
                }
            }
        }
    }
    true
}

/// Lambert contribution without visibility — caller handles shadow testing.
fn light_contribution_lambert(light: &MapLight, surface_point: Vec3, surface_normal: Vec3) -> Vec3 {
    match light.light_type {
        LightType::Point => {
            let to_light = Vec3::new(
                light.origin.x as f32 - surface_point.x,
                light.origin.y as f32 - surface_point.y,
                light.origin.z as f32 - surface_point.z,
            );
            let dist = to_light.length();
            if dist < 1.0e-4 {
                return Vec3::ZERO;
            }
            let l = to_light / dist;
            let n_dot_l = surface_normal.dot(l).max(0.0);
            if n_dot_l <= 0.0 {
                return Vec3::ZERO;
            }
            let attenuation = falloff(light, dist);
            Vec3::from(light.color) * (light.intensity * n_dot_l * attenuation)
        }
        LightType::Spot => {
            let to_light = Vec3::new(
                light.origin.x as f32 - surface_point.x,
                light.origin.y as f32 - surface_point.y,
                light.origin.z as f32 - surface_point.z,
            );
            let dist = to_light.length();
            if dist < 1.0e-4 {
                return Vec3::ZERO;
            }
            let l = to_light / dist;
            let n_dot_l = surface_normal.dot(l).max(0.0);
            if n_dot_l <= 0.0 {
                return Vec3::ZERO;
            }
            let attenuation = falloff(light, dist);
            let cone = spot_cone_attenuation(light, -l);
            Vec3::from(light.color) * (light.intensity * n_dot_l * attenuation * cone)
        }
        LightType::Directional => {
            let dir = Vec3::from(light.cone_direction.unwrap_or([0.0, -1.0, 0.0]));
            // cone_direction is the photon travel vector; negate to get the surface-to-light vector.
            let l = (-dir).normalize_or_zero();
            let n_dot_l = surface_normal.dot(l).max(0.0);
            if n_dot_l <= 0.0 {
                return Vec3::ZERO;
            }
            Vec3::from(light.color) * (light.intensity * n_dot_l)
        }
    }
}

fn light_shadow_origin(light: &MapLight, surface_point: Vec3) -> Vec3 {
    match light.light_type {
        LightType::Point | LightType::Spot => Vec3::new(
            light.origin.x as f32,
            light.origin.y as f32,
            light.origin.z as f32,
        ),
        LightType::Directional => {
            let dir = Vec3::from(light.cone_direction.unwrap_or([0.0, -1.0, 0.0]));
            let to_light = (-dir).normalize_or_zero();
            surface_point + to_light * 10_000.0 // beyond any indoor map AABB
        }
    }
}

/// Must match `falloff` in `forward.wgsl` exactly — divergence produces "ghost glow"
/// or missing bounce light. InverseDistance/InverseSquared have no upper clamp past 1.0,
/// matching the shader convention.
fn falloff(light: &MapLight, distance: f32) -> f32 {
    let range = light.falloff_range.max(1.0e-4);
    match light.falloff_model {
        FalloffModel::Linear => (1.0 - distance / range).clamp(0.0, 1.0),
        FalloffModel::InverseDistance => {
            if distance > range {
                return 0.0;
            }
            1.0 / distance.max(1.0e-4)
        }
        FalloffModel::InverseSquared => {
            if distance > range {
                return 0.0;
            }
            let d2 = (distance * distance).max(1.0e-4);
            1.0 / d2
        }
    }
}

/// Must match `cone_attenuation` in `forward.wgsl` — Hermite cubic smoothstep
/// so direct and indirect agree along the cone fringe.
fn spot_cone_attenuation(light: &MapLight, light_to_surface: Vec3) -> f32 {
    let dir = Vec3::from(light.cone_direction.unwrap_or([0.0, -1.0, 0.0])).normalize_or_zero();
    let inner = light.cone_angle_inner.unwrap_or(0.0);
    let outer = light.cone_angle_outer.unwrap_or(inner + 0.01);
    let cos_outer = outer.cos();
    let cos_inner = inner.cos();
    let cos_theta = dir.dot(light_to_surface.normalize_or_zero());
    smoothstep(cos_outer, cos_inner, cos_theta)
}

/// `t² × (3 - 2t)` — matches WGSL's `smoothstep(edge0, edge1, x)`.
fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0).max(1.0e-4)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Real-valued L2 SH basis — Ramamoorthi-Hanrahan convention, Condon-Shortley phase omitted.
fn sh_basis_l2(dir: Vec3) -> [f32; 9] {
    let x = dir.x;
    let y = dir.y;
    let z = dir.z;
    let mut b = [0f32; 9];
    b[0] = 0.282_094_8; // 0.5 * sqrt(1/pi)
    b[1] = -0.488_602_5 * y;
    b[2] = 0.488_602_5 * z;
    b[3] = -0.488_602_5 * x;
    b[4] = 1.092_548_4 * x * y;
    b[5] = -1.092_548_4 * y * z;
    b[6] = 0.315_391_6 * (3.0 * z * z - 1.0);
    b[7] = -1.092_548_4 * x * z;
    b[8] = 0.546_274_2 * (x * x - y * y);
    b
}

fn accumulate_sh_rgb(acc: &mut [f32; 27], dir: Vec3, value: Vec3, weight: f32) {
    let basis = sh_basis_l2(dir);
    for (band, b) in basis.iter().enumerate() {
        let base = band * 3;
        acc[base] += *b * value.x * weight;
        acc[base + 1] += *b * value.y * weight;
        acc[base + 2] += *b * value.z * weight;
    }
}

fn evaluate_sh_rgb(coefficients: &[f32; 27], dir: Vec3) -> Vec3 {
    let basis = sh_basis_l2(dir);
    let mut out = Vec3::ZERO;
    for (band, b) in basis.iter().enumerate() {
        let base = band * 3;
        out.x += coefficients[base] * *b;
        out.y += coefficients[base + 1] * *b;
        out.z += coefficients[base + 2] * *b;
    }
    out.max(Vec3::ZERO)
}

fn pack_octahedral_irradiance_atlas(
    probes: &[BakedProbe],
    grid_dimensions: [u32; 3],
    tile_dimension: u32,
    border: u32,
    atlas_dimensions: [u32; 2],
    atlas_tiles_per_row: u32,
) -> Vec<OctahedralAtlasTexel> {
    let total = (grid_dimensions[0] as usize)
        * (grid_dimensions[1] as usize)
        * (grid_dimensions[2] as usize);
    debug_assert_eq!(probes.len(), total);
    let atlas_texel_count = atlas_dimensions[0] as usize * atlas_dimensions[1] as usize;
    let mut atlas = vec![OctahedralAtlasTexel::default(); atlas_texel_count];
    if total == 0 {
        return atlas;
    }

    for (probe_index, probe) in probes.iter().enumerate() {
        let origin = irradiance_tile_origin(probe_index, tile_dimension, atlas_tiles_per_row);
        let tile = pack_octahedral_irradiance_tile(
            &probe.coefficients,
            probe.metadata.validity != 0,
            tile_dimension,
            border,
        );

        for tile_y in 0..tile_dimension {
            for tile_x in 0..tile_dimension {
                let texel = tile[(tile_y * tile_dimension + tile_x) as usize];
                let atlas_x = origin[0] + tile_x;
                let atlas_y = origin[1] + tile_y;
                let atlas_off = (atlas_y * atlas_dimensions[0] + atlas_x) as usize;
                atlas[atlas_off] = texel;
            }
        }
    }
    atlas
}

pub(crate) fn pack_octahedral_irradiance_tile(
    coefficients: &[f32; 27],
    valid: bool,
    tile_dimension: u32,
    border: u32,
) -> Vec<OctahedralAtlasTexel> {
    let interior = tile_dimension - 2 * border;
    let mut interior_texels = vec![OctahedralAtlasTexel::default(); (interior * interior) as usize];

    if valid {
        let valid_alpha = f32_to_f16_bits(1.0);
        for iy in 0..interior {
            for ix in 0..interior {
                let dir = Vec3::from(irradiance_interior_texel_direction(
                    ix,
                    iy,
                    tile_dimension,
                    border,
                ));
                let irradiance = evaluate_sh_rgb(coefficients, dir);
                let off = (iy * interior + ix) as usize;
                interior_texels[off] = OctahedralAtlasTexel {
                    rgba: [
                        f32_to_f16_bits(irradiance.x),
                        f32_to_f16_bits(irradiance.y),
                        f32_to_f16_bits(irradiance.z),
                        valid_alpha,
                    ],
                };
            }
        }
    }

    let mut tile =
        vec![OctahedralAtlasTexel::default(); (tile_dimension * tile_dimension) as usize];
    for tile_y in 0..tile_dimension {
        for tile_x in 0..tile_dimension {
            let [src_x, src_y] =
                irradiance_tile_source_texel(tile_x, tile_y, tile_dimension, border);
            tile[(tile_y * tile_dimension + tile_x) as usize] =
                interior_texels[(src_y * interior + src_x) as usize];
        }
    }
    tile
}

// Ramamoorthi & Hanrahan 2001, eq. 11 — zonal-harmonic cosine-lobe factors.
// After convolution the coefficients reconstruct irradiance directly; the runtime shader
// needs no per-fragment A_l multiply.
const COSINE_LOBE_L0: f32 = std::f32::consts::PI;
const COSINE_LOBE_L1: f32 = 2.0 * std::f32::consts::PI / 3.0;
const COSINE_LOBE_L2: f32 = std::f32::consts::PI * 0.25;

fn apply_cosine_lobe_rgb(acc: &mut [f32; 27]) {
    for band in 0..9 {
        let factor = cosine_lobe_factor(band);
        let base = band * 3;
        acc[base] *= factor;
        acc[base + 1] *= factor;
        acc[base + 2] *= factor;
    }
}

fn cosine_lobe_factor(band: usize) -> f32 {
    match band {
        0 => COSINE_LOBE_L0,
        1..=3 => COSINE_LOBE_L1,
        4..=8 => COSINE_LOBE_L2,
        _ => 0.0,
    }
}

/// Indirect-only SH L2 RGB for the per-light delta SH baker (`delta_sh_bake.rs`),
/// which carries no depth moments. Shares the per-ray sampler with the SH-volume
/// path (`bake_probe_rgb_with_moments`); only the ray-accumulation loop is
/// duplicated, so the delta path projects radiance without ever touching the
/// `Σd` / `Σd²` moments. This loop discards the distance the shared sampler
/// returns.
pub(crate) fn bake_probe_indirect_rgb(
    ctx: &RaytracingCtx<'_>,
    probe_pos: Vec3,
    lights: &[&MapLight],
) -> [f32; 27] {
    let directions = sphere_directions(RAYS_PER_PROBE, SAMPLING_LATTICE_OFFSET);
    let mc_weight = (4.0 * std::f32::consts::PI) / RAYS_PER_PROBE as f32;
    let mut acc = [0f32; 27];
    for dir in &directions {
        // Pass f32::INFINITY as an unreachable sentinel; the delta path discards
        // the returned distance, so this value is never read.
        let (radiance, _) = sample_radiance_rgb(ctx, probe_pos, *dir, lights, f32::INFINITY);
        accumulate_sh_rgb(&mut acc, *dir, radiance, mc_weight);
    }
    apply_cosine_lobe_rgb(&mut acc);
    acc
}

/// SH-volume-path twin of `bake_probe_indirect_rgb` that also accumulates the
/// per-probe depth moments `Σd` / `Σd²` over the same 256-direction loop. The
/// shared per-ray sampler keeps the bounce math identical across both paths;
/// the ray-accumulation loop is duplicated so the delta path stays moment-free.
/// Sky-miss rays contribute `far_sentinel` to both sums (via
/// `sample_radiance_rgb`), so all `RAYS_PER_PROBE` rays accumulate and the
/// caller's division by the constant is exact. Accumulation is a sequential sum
/// over the fixed direction list so the bake stays byte-identical across runs.
fn bake_probe_rgb_with_moments(
    inputs: &ShBakeCtx<'_>,
    probe_pos: Vec3,
    static_lights: &[&MapLight],
    far_sentinel: f32,
) -> ([f32; 27], f32, f32) {
    let ctx = inputs.ray_ctx();
    let directions = sphere_directions(RAYS_PER_PROBE, SAMPLING_LATTICE_OFFSET);
    let mc_weight = (4.0 * std::f32::consts::PI) / RAYS_PER_PROBE as f32;
    let mut acc = [0f32; 27];
    let mut sum_d = 0.0f32;
    let mut sum_d2 = 0.0f32;
    for dir in &directions {
        let (radiance, distance) =
            sample_radiance_rgb(&ctx, probe_pos, *dir, static_lights, far_sentinel);
        accumulate_sh_rgb(&mut acc, *dir, radiance, mc_weight);
        sum_d += distance;
        sum_d2 += distance * distance;
    }
    apply_cosine_lobe_rgb(&mut acc);
    (acc, sum_d, sum_d2)
}

/// Indirect (bounced) radiance along one ray plus the ray's hit distance —
/// direct term is the lightmap's responsibility. Shared by both bake paths: the
/// SH-volume path consumes the distance for its depth moments, the delta path
/// discards it. A ray that misses all geometry returns `SKY_COLOR` and
/// `far_sentinel` as its distance — the depth-moment bake reads "no hit" as
/// "fully open" at the probe-spacing scale.
fn sample_radiance_rgb(
    ctx: &RaytracingCtx<'_>,
    origin: Vec3,
    dir: Vec3,
    lights: &[&MapLight],
    far_sentinel: f32,
) -> (Vec3, f32) {
    match closest_hit(ctx, origin + dir * RAY_EPSILON, dir, f32::INFINITY) {
        None => (Vec3::from(SKY_COLOR), far_sentinel),
        Some(hit) => {
            let mut radiance = Vec3::ZERO;
            for light in lights {
                if !shadow_visible(ctx, hit.point, hit.normal, light) {
                    continue;
                }
                radiance += light_contribution_lambert(light, hit.point, hit.normal);
            }
            (
                radiance * BOUNCE_ALBEDO / std::f32::consts::PI,
                hit.distance,
            )
        }
    }
}

fn shadow_visible(
    ctx: &RaytracingCtx<'_>,
    surface_point: Vec3,
    surface_normal: Vec3,
    light: &MapLight,
) -> bool {
    if !light.cast_shadows {
        return true;
    }
    let shadow_origin = light_shadow_origin(light, surface_point);
    // Nudge along the normal to avoid self-intersection with the hit face.
    let probe_end = surface_point + surface_normal * RAY_EPSILON;
    segment_clear(ctx, probe_end, shadow_origin)
}

pub(crate) fn probe_is_valid_pub(tree: &BspTree, exterior: &HashSet<usize>, pos: DVec3) -> bool {
    probe_is_valid(tree, exterior, pos)
}

/// Color animation is only valid on `bake_only` or `is_dynamic` lights — a color-animated
/// baked light would produce direct/indirect mismatch since the SH indirect was baked at a fixed color.
pub fn validate_light_animations(lights: &[MapLight]) -> Result<(), String> {
    for light in lights {
        let Some(anim) = light.animation.as_ref() else {
            continue;
        };
        if anim.color.is_some() && !light.bake_only && !light.is_dynamic {
            return Err(format!(
                "light at origin [{:.3}, {:.3}, {:.3}] has `color` animation \
                 but is neither `bake_only` nor `is_dynamic`; animated color on \
                 a baked light would mismatch the SH indirect bake. Either mark \
                 the light `_dynamic 1`, set `_bake_only 1`, or remove the \
                 color curve.",
                light.origin.x, light.origin.y, light.origin.z,
            ));
        }
    }
    Ok(())
}

fn animation_descriptor_for(light: &MapLight) -> AnimationDescriptor {
    let anim: &LightAnimation = light
        .animation
        .as_ref()
        .expect("animation_descriptor_for called on a non-animated light");
    AnimationDescriptor {
        period: anim.period.max(1.0e-6),
        phase: anim.phase,
        // Bake color × intensity into base_color: weight maps are baked at unit intensity/white,
        // so this is the only place the authored intensity re-enters the pipeline.
        base_color: [
            light.color[0] * light.intensity,
            light.color[1] * light.intensity,
            light.color[2] * light.intensity,
        ],
        brightness: anim.brightness.clone().unwrap_or_default(),
        color: anim.color.clone().unwrap_or_default(),
        direction: anim.direction.clone().unwrap_or_default(),
        start_active: if anim.start_active { 1 } else { 0 },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bvh_build::{build_bvh, collect_primitives};
    use crate::geometry::FaceIndexRange;
    use crate::map_data::{FalloffModel, LightType};
    use crate::partition::{Aabb as CompilerAabb, BspLeaf, BspTree};
    use postretro_level_format::geometry::{FaceMeta, GeometrySection, Vertex};
    use postretro_level_format::texture_names::TextureNamesSection;

    fn apply_cosine_lobe_mono(acc: &mut [f32; 9]) {
        for (band, v) in acc.iter_mut().enumerate() {
            *v *= cosine_lobe_factor(band);
        }
    }

    fn accumulate_sh(acc: &mut [f32; 9], dir: Vec3, value: f32, weight: f32) {
        let basis = sh_basis_l2(dir);
        for (a, b) in acc.iter_mut().zip(basis.iter()) {
            *a += *b * value * weight;
        }
    }

    fn point_light_with_falloff(model: FalloffModel, range: f32) -> MapLight {
        MapLight {
            origin: glam::DVec3::ZERO,
            light_type: LightType::Point,
            intensity: 1.0,
            color: [1.0, 1.0, 1.0],
            falloff_model: model,
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
            shadow_type: crate::map_data::ShadowType::StaticLightMap,
        }
    }

    // Pins the contract that `sh_bake::falloff` mirrors `falloff` in `forward.wgsl`.
    // Drift here produces "ghost glow" (indirect picked up where direct is culled).

    #[test]
    fn linear_falloff_matches_shader_curve() {
        let light = point_light_with_falloff(FalloffModel::Linear, 10.0);
        assert!((falloff(&light, 0.0) - 1.0).abs() < 1e-6);
        assert!((falloff(&light, 5.0) - 0.5).abs() < 1e-6);
        assert!(falloff(&light, 10.0).abs() < 1e-6);
        assert_eq!(falloff(&light, 15.0), 0.0);
    }

    #[test]
    fn inverse_distance_zeroes_past_range() {
        let light = point_light_with_falloff(FalloffModel::InverseDistance, 10.0);
        assert!((falloff(&light, 1.0) - 1.0).abs() < 1e-6);
        assert!((falloff(&light, 5.0) - 0.2).abs() < 1e-6);
        assert_eq!(falloff(&light, 10.001), 0.0);
        assert_eq!(falloff(&light, 50.0), 0.0);
    }

    #[test]
    fn inverse_squared_zeroes_past_range() {
        let light = point_light_with_falloff(FalloffModel::InverseSquared, 10.0);
        assert!((falloff(&light, 1.0) - 1.0).abs() < 1e-6);
        assert!((falloff(&light, 0.5) - 4.0).abs() < 1e-6); // close-range exceeds 1.0 deliberately
        assert_eq!(falloff(&light, 10.001), 0.0);
        assert_eq!(falloff(&light, 100.0), 0.0);
    }

    #[test]
    fn smoothstep_matches_wgsl_definition() {
        assert_eq!(smoothstep(0.0, 1.0, -0.5), 0.0);
        assert_eq!(smoothstep(0.0, 1.0, 1.5), 1.0);
        assert!((smoothstep(0.0, 1.0, 0.5) - 0.5).abs() < 1e-6);
        let lower = smoothstep(0.0, 1.0, 0.25);
        let upper = smoothstep(0.0, 1.0, 0.75);
        assert!((lower + upper - 1.0).abs() < 1e-6); // symmetric
    }

    fn tri_vertex(pos: [f32; 3]) -> Vertex {
        Vertex::new(
            pos,
            [0.0, 0.0],
            [0.0, 1.0, 0.0],
            [1.0, 0.0, 0.0],
            true,
            [0.0, 0.0],
        )
    }

    fn one_triangle_geometry(positions: [[f32; 3]; 3]) -> GeometryResult {
        GeometryResult {
            geometry: GeometrySection {
                vertices: positions.iter().map(|&p| tri_vertex(p)).collect(),
                indices: vec![0, 1, 2],
                faces: vec![FaceMeta {
                    leaf_index: 0,
                    texture_index: 0,
                }],
            },
            texture_names: TextureNamesSection { names: Vec::new() },
            face_index_ranges: vec![FaceIndexRange {
                index_offset: 0,
                index_count: 3,
            }],
        }
    }

    /// Geometry from a list of triangles, one BVH face/primitive per triangle.
    /// A single floor triangle gives near-uniform ray distances (most rays sky-
    /// miss to the sentinel, the rest graze one plane); multiple triangles at
    /// different depths give the depth-moment accumulator a real spread of
    /// `closest_hit` distances to integrate.
    fn multi_triangle_geometry(triangles: &[[[f32; 3]; 3]]) -> GeometryResult {
        let mut vertices = Vec::with_capacity(triangles.len() * 3);
        let mut indices = Vec::with_capacity(triangles.len() * 3);
        let mut faces = Vec::with_capacity(triangles.len());
        let mut face_index_ranges = Vec::with_capacity(triangles.len());
        for (i, tri) in triangles.iter().enumerate() {
            let base = (i * 3) as u32;
            for &p in tri {
                vertices.push(tri_vertex(p));
            }
            indices.extend_from_slice(&[base, base + 1, base + 2]);
            faces.push(FaceMeta {
                leaf_index: 0,
                texture_index: 0,
            });
            face_index_ranges.push(FaceIndexRange {
                index_offset: base,
                index_count: 3,
            });
        }
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

    /// A 4 m floor plus three walls forming an open-topped box. Probes in the
    /// box see a real spread of `closest_hit` distances: rays toward a near wall
    /// hit close, rays toward a far wall hit far, rays out the open top sky-miss
    /// to the sentinel. Exercises the depth-moment accumulation that a single
    /// floor triangle leaves near-degenerate.
    fn floor_and_walls_geometry() -> GeometryResult {
        // Floor (two triangles) spanning x,z in [0, 4] at y = 0.
        let floor_a = [[0.0, 0.0, 0.0], [4.0, 0.0, 0.0], [4.0, 0.0, 4.0]];
        let floor_b = [[0.0, 0.0, 0.0], [4.0, 0.0, 4.0], [0.0, 0.0, 4.0]];
        // Near wall at x = 0 and far wall at x = 4, plus a side wall at z = 4,
        // each a single triangle rising to y = 3. Their differing distances from
        // any interior probe spread the per-ray hit distances.
        let wall_near = [[0.0, 0.0, 0.0], [0.0, 0.0, 4.0], [0.0, 3.0, 0.0]];
        let wall_far = [[4.0, 0.0, 0.0], [4.0, 0.0, 4.0], [4.0, 3.0, 0.0]];
        let wall_side = [[0.0, 0.0, 4.0], [4.0, 0.0, 4.0], [0.0, 3.0, 4.0]];
        multi_triangle_geometry(&[floor_a, floor_b, wall_near, wall_far, wall_side])
    }

    fn tree_all_empty() -> BspTree {
        BspTree {
            nodes: Vec::new(),
            leaves: vec![BspLeaf {
                face_indices: Vec::new(),
                bounds: CompilerAabb {
                    min: DVec3::splat(-1000.0),
                    max: DVec3::splat(1000.0),
                },
                is_solid: false,
                defining_planes: Vec::new(),
            }],
        }
    }

    #[test]
    fn empty_geometry_returns_empty_section() {
        let geo = GeometryResult {
            geometry: GeometrySection {
                vertices: Vec::new(),
                indices: Vec::new(),
                faces: Vec::new(),
            },
            texture_names: TextureNamesSection { names: Vec::new() },
            face_index_ranges: Vec::new(),
        };
        let tree = tree_all_empty();
        let bvh = bvh::bvh::Bvh { nodes: Vec::new() };
        let primitives: Vec<BvhPrimitive> = Vec::new();
        let exterior: HashSet<usize> = HashSet::new();
        let lights: &[MapLight] = &[];
        let static_lights = StaticBakedLights::from_lights(lights);
        let animated_lights = AnimatedBakedLights::from_lights(lights);
        let inputs = ShBakeCtx {
            bvh: &bvh,
            primitives: &primitives,
            geometry: &geo,
            tree: &tree,
            exterior_leaves: &exterior,
            static_lights: &static_lights,
            animated_lights: &animated_lights,
            total_light_count: 0,
        };
        let section = bake_sh_volume(&inputs, &ShConfig { probe_spacing: 1.0 });
        assert_eq!(section.grid_dimensions, [0, 0, 0]);
        assert_eq!(section.atlas_dimensions, [0, 0]);
        assert_eq!(section.atlas_tiles_per_row, 0);
        assert!(section.probes.is_empty());
        assert!(section.atlas_texels.is_empty());
    }

    /// Empty geometry still emits a `slot_for_map_light` table sized to
    /// `total_light_count`. Every entry is `ANIMATED_SLOT_NONE`.
    #[test]
    fn empty_geometry_emits_sized_slot_table_when_lights_present() {
        use postretro_level_format::sh_volume::ANIMATED_SLOT_NONE;
        let geo = GeometryResult {
            geometry: GeometrySection {
                vertices: Vec::new(),
                indices: Vec::new(),
                faces: Vec::new(),
            },
            texture_names: TextureNamesSection { names: Vec::new() },
            face_index_ranges: Vec::new(),
        };
        let tree = tree_all_empty();
        let bvh = bvh::bvh::Bvh { nodes: Vec::new() };
        let primitives: Vec<BvhPrimitive> = Vec::new();
        let exterior: HashSet<usize> = HashSet::new();
        let lights: &[MapLight] = &[];
        let static_lights = StaticBakedLights::from_lights(lights);
        let animated_lights = AnimatedBakedLights::from_lights(lights);
        let inputs = ShBakeCtx {
            bvh: &bvh,
            primitives: &primitives,
            geometry: &geo,
            tree: &tree,
            exterior_leaves: &exterior,
            static_lights: &static_lights,
            animated_lights: &animated_lights,
            total_light_count: 7,
        };
        let section = bake_sh_volume(&inputs, &ShConfig { probe_spacing: 1.0 });
        assert_eq!(section.slot_for_map_light.len(), 7);
        assert!(
            section
                .slot_for_map_light
                .iter()
                .all(|&s| s == ANIMATED_SLOT_NONE)
        );
    }

    #[test]
    fn grid_dimensions_cover_world_aabb() {
        let min = DVec3::new(0.0, 0.0, 0.0);
        let max = DVec3::new(3.0, 0.0, 2.0);
        let dims = grid_dimensions(min, max, 1.0);
        assert_eq!(dims, [4, 1, 3]);
    }

    #[test]
    fn probe_iteration_matches_z_major_convention() {
        let dims = [2, 3, 4];
        let total = 2 * 3 * 4;
        assert_eq!(probe_index_to_xyz(0, dims), (0, 0, 0));
        assert_eq!(probe_index_to_xyz(total - 1, dims), (1, 2, 3));
        assert_eq!(probe_index_to_xyz(1, dims), (1, 0, 0)); // x advances first
        assert_eq!(probe_index_to_xyz(2, dims), (0, 1, 0)); // then y
        assert_eq!(probe_index_to_xyz(2 * 3, dims), (0, 0, 1)); // then z
    }

    #[test]
    fn sphere_directions_are_unit_and_stable() {
        let dirs = sphere_directions(32, 1);
        assert_eq!(dirs.len(), 32);
        for d in &dirs {
            let length = d.length();
            assert!(
                (length - 1.0).abs() < 1.0e-4,
                "direction not unit length: {}",
                length
            );
        }
        let again = sphere_directions(32, 1);
        assert_eq!(dirs, again);
    }

    #[test]
    fn cosine_lobe_makes_constant_radiance_reconstruct_to_pi() {
        // For constant L=1, diffuse irradiance = π. Verify the cosine-lobe convolution
        // produces this for any normal direction.
        let samples = 8192usize;
        let weight = 4.0 * std::f32::consts::PI / samples as f32;
        let mut coeffs = [0f32; 9];
        let phi = std::f32::consts::PI * (3.0 - 5.0_f32.sqrt()); // golden angle
        for i in 0..samples {
            let t = (i as f32 + 0.5) / samples as f32;
            let z = 1.0 - 2.0 * t;
            let r = (1.0 - z * z).max(0.0).sqrt();
            let theta = phi * i as f32;
            let dir = Vec3::new(r * theta.cos(), r * theta.sin(), z);
            accumulate_sh(&mut coeffs, dir, 1.0, weight);
        }
        apply_cosine_lobe_mono(&mut coeffs);

        let reconstruct = |n: Vec3| -> f32 {
            let b = sh_basis_l2(n);
            (0..9).map(|i| coeffs[i] * b[i]).sum()
        };

        let expected = std::f32::consts::PI;
        for n in [
            Vec3::X,
            Vec3::Y,
            Vec3::Z,
            -Vec3::X,
            -Vec3::Y,
            -Vec3::Z,
            Vec3::new(0.577, 0.577, 0.577).normalize(),
        ] {
            let got = reconstruct(n);
            assert!(
                (got - expected).abs() < 0.05,
                "constant radiance L=1 should reconstruct to π irradiance; \
                 got {got} for normal {n:?}, expected {expected}",
            );
        }
    }

    #[test]
    fn sh_basis_band0_is_constant() {
        let b0 = sh_basis_l2(Vec3::X)[0];
        let b1 = sh_basis_l2(Vec3::Y)[0];
        let b2 = sh_basis_l2(Vec3::new(0.3, 0.4, 0.5).normalize())[0];
        assert!((b0 - b1).abs() < 1.0e-6);
        assert!((b0 - b2).abs() < 1.0e-6);
    }

    #[test]
    fn bake_is_deterministic() {
        let geo = one_triangle_geometry([[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]]);
        let (bvh, prims, _) = build_bvh(&geo).unwrap();
        let tree = tree_all_empty();
        let light = MapLight {
            origin: DVec3::new(0.3, 1.0, 0.3),
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
        };
        let exterior: HashSet<usize> = HashSet::new();
        let lights = std::slice::from_ref(&light);
        let static_lights = StaticBakedLights::from_lights(lights);
        let animated_lights = AnimatedBakedLights::from_lights(lights);
        let inputs = ShBakeCtx {
            bvh: &bvh,
            primitives: &prims,
            geometry: &geo,
            tree: &tree,
            exterior_leaves: &exterior,
            static_lights: &static_lights,
            animated_lights: &animated_lights,
            total_light_count: 0,
        };
        let a = bake_sh_volume(&inputs, &ShConfig { probe_spacing: 1.0 });
        let b = bake_sh_volume(&inputs, &ShConfig { probe_spacing: 1.0 });
        assert_eq!(a.to_bytes(), b.to_bytes());
    }

    /// Cache-determinism guard: the SH volume bake feeds the build-stage cache,
    /// which keys cache entries on input hash and serves stored output bytes
    /// verbatim. The bake fans probes across rayon worker threads via
    /// `into_par_iter().map().collect()`; this is order-preserving, but
    /// regressions (e.g. swapping in `par_iter().reduce()` over floats, or
    /// iterating a `HashMap` to assemble probe output) would silently break
    /// the cache. This test fans out enough probes to exercise multiple worker
    /// chunks and asserts byte-for-byte equality on the encoded section.
    #[test]
    fn sh_volume_bake_produces_byte_identical_output_on_repeated_runs() {
        // Open-topped box (floor + three walls) spanning ~4 m → a 5×4×5 probe
        // grid at 1 m spacing, enough work for rayon to schedule across several
        // threads. The walls give probes a real spread of `closest_hit`
        // distances (near wall vs. far wall vs. open-top sky-miss), so this
        // genuinely exercises the depth-moment accumulation — a single floor
        // triangle would leave the moments near-degenerate and let the bake pass
        // without ever stressing the per-probe `Σd`/`Σd²` sums.
        let geo = floor_and_walls_geometry();
        let (bvh, prims, _) = build_bvh(&geo).unwrap();
        let tree = tree_all_empty();

        // Two static lights — verifies the per-probe sequential light sum is
        // stable across runs (would catch any future float-reduce swap).
        let lights = vec![
            MapLight {
                origin: DVec3::new(0.5, 1.0, 0.5),
                light_type: LightType::Point,
                intensity: 1.0,
                color: [1.0, 0.5, 0.25],
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
            },
            MapLight {
                origin: DVec3::new(3.0, 2.0, 3.0),
                light_type: LightType::Point,
                intensity: 2.0,
                color: [0.25, 0.5, 1.0],
                falloff_model: FalloffModel::InverseSquared,
                falloff_range: 8.0,
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
            },
        ];
        let exterior: HashSet<usize> = HashSet::new();
        let static_lights = StaticBakedLights::from_lights(&lights);
        let animated_lights = AnimatedBakedLights::from_lights(&lights);
        let inputs = ShBakeCtx {
            bvh: &bvh,
            primitives: &prims,
            geometry: &geo,
            tree: &tree,
            exterior_leaves: &exterior,
            static_lights: &static_lights,
            animated_lights: &animated_lights,
            total_light_count: 0,
        };
        let config = ShConfig { probe_spacing: 1.0 };

        let bytes_a = bake_sh_volume(&inputs, &config).to_bytes();
        let bytes_b = bake_sh_volume(&inputs, &config).to_bytes();
        assert_eq!(
            bytes_a, bytes_b,
            "SH volume bake output drifted between runs; the build-stage cache requires \
             byte-identical output for identical inputs",
        );
    }

    // Regression: guards variance non-negativity (`E[d²] >= E[d]²`) for a valid,
    // non-degenerate probe. Compares the pre-rounding f32 moments — two
    // independently-rounded f16 values can violate a naive `>=` and flake.
    #[test]
    fn probe_depth_moments_keep_squared_distance_at_least_mean_squared() {
        // Open-topped box gives this probe a genuine spread of ray distances:
        // some rays hit a near wall, some a far wall, some sky-miss to the
        // sentinel. A non-degenerate spread is what makes the variance check
        // meaningful rather than vacuously satisfied by equal distances.
        let geo = floor_and_walls_geometry();
        let (bvh, prims, _) = build_bvh(&geo).unwrap();
        let tree = tree_all_empty();
        let exterior: HashSet<usize> = HashSet::new();
        let lights: &[MapLight] = &[];
        let static_lights = StaticBakedLights::from_lights(lights);
        let animated_lights = AnimatedBakedLights::from_lights(lights);
        let inputs = ShBakeCtx {
            bvh: &bvh,
            primitives: &prims,
            geometry: &geo,
            tree: &tree,
            exterior_leaves: &exterior,
            static_lights: &static_lights,
            animated_lights: &animated_lights,
            total_light_count: 0,
        };
        let static_light_refs: Vec<&MapLight> = Vec::new();

        // Probe off-center so the four bounding walls sit at distinct distances.
        let probe_pos = Vec3::new(1.0, 1.0, 1.5);
        // 1 m cell → far sentinel matches the bake's `4 * length(cell_size)`.
        let far_sentinel = 4.0 * Vec3::splat(1.0).length();

        // Pre-rounding f32 moments: divide the raw sums by RAYS_PER_PROBE before
        // any f16 encoding (the f16 store happens later in `bake_sh_volume`).
        let (_coeffs, sum_d, sum_d2) =
            bake_probe_rgb_with_moments(&inputs, probe_pos, &static_light_refs, far_sentinel);
        let mean_d = sum_d / RAYS_PER_PROBE as f32;
        let mean_sq_d = sum_d2 / RAYS_PER_PROBE as f32;

        // Variance = E[d²] - E[d]² is non-negative by construction; allow a tiny
        // epsilon on the boundary for f32 summation rounding.
        assert!(
            mean_sq_d >= mean_d * mean_d - 1.0e-3,
            "variance must be non-negative: E[d²]={mean_sq_d} < E[d]²={}",
            mean_d * mean_d,
        );
        // Guard against a vacuous pass: a degenerate probe (all rays the same
        // distance) would make variance ~0. This fixture must produce spread.
        assert!(
            mean_sq_d - mean_d * mean_d > 1.0e-2,
            "fixture is degenerate — no distance spread to exercise the moments \
             (E[d²]={mean_sq_d}, E[d]²={})",
            mean_d * mean_d,
        );
    }

    // Anchors AC#2: the depth moments encode local occlusion, so an open-space
    // probe must bake a meaningfully larger mean distance than a cornered one.
    #[test]
    fn open_probe_mean_distance_exceeds_cornered_probe() {
        let geo = floor_and_walls_geometry();
        let (bvh, prims, _) = build_bvh(&geo).unwrap();
        let tree = tree_all_empty();
        let exterior: HashSet<usize> = HashSet::new();
        let lights: &[MapLight] = &[];
        let static_lights = StaticBakedLights::from_lights(lights);
        let animated_lights = AnimatedBakedLights::from_lights(lights);
        let inputs = ShBakeCtx {
            bvh: &bvh,
            primitives: &prims,
            geometry: &geo,
            tree: &tree,
            exterior_leaves: &exterior,
            static_lights: &static_lights,
            animated_lights: &animated_lights,
            total_light_count: 0,
        };
        let section = bake_sh_volume(&inputs, &ShConfig { probe_spacing: 1.0 });

        // Fixture grid: 5×4×5 probes at 1 m spacing over the [0,4] box, origin
        // at (0,0,0). Walls sit at x=0, x=4, z=4; the z=0 face and the top are
        // open. Flat index is z-major then y then x (see `probe_index_to_xyz`).
        let dims = section.grid_dimensions;
        assert_eq!(dims, [5, 4, 5], "fixture grid changed; revisit probe picks");
        let nx = dims[0] as usize;
        let ny = dims[1] as usize;
        let flat = |x: usize, y: usize, z: usize| z * nx * ny + y * nx + x;

        // Open probe: x-center (x=2) on the open z=0 face, on the floor — its
        // rays escape through the open side and open top, so it sees far.
        let open = &section.probes[flat(2, 0, 0)];
        // Cornered probe: tucked against the x=0 and z=4 walls, one step off the
        // floor — three nearby occluders pull its mean ray distance down.
        let corner = &section.probes[flat(1, 1, 3)];

        assert_eq!(
            open.validity, 1,
            "open probe must be a valid interior probe"
        );
        assert_eq!(
            corner.validity, 1,
            "corner probe must be a valid interior probe"
        );

        let open_mean = f16_bits_to_f32(open.mean_distance);
        let corner_mean = f16_bits_to_f32(corner.mean_distance);

        // Observed gap is ~2.5 m (open ≈6.4 m vs corner ≈3.9 m); require ≥1.5 m
        // so the assertion is non-vacuous yet has ample headroom against f16
        // rounding and any minor sampler tweak.
        assert!(
            open_mean - corner_mean >= 1.5,
            "open-space probe should bake a meaningfully larger mean distance \
             than a cornered probe (open={open_mean}, corner={corner_mean})",
        );
    }

    #[test]
    fn animated_light_produces_descriptor() {
        let geo = one_triangle_geometry([[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]]);
        let (bvh, prims, _) = build_bvh(&geo).unwrap();
        let tree = tree_all_empty();
        let animated = MapLight {
            origin: DVec3::new(0.5, 2.0, 0.5),
            light_type: LightType::Point,
            intensity: 1.0,
            color: [1.0, 0.5, 0.25],
            falloff_model: FalloffModel::Linear,
            falloff_range: 5.0,
            light_size: 0.0,
            angular_diameter: 0.0,
            cone_angle_inner: None,
            cone_angle_outer: None,
            cone_direction: None,
            animation: Some(LightAnimation {
                period: 1.0,
                phase: 0.0,
                brightness: Some(vec![0.0, 1.0, 0.0]),
                color: None,
                direction: None,
                start_active: true,
            }),
            cast_shadows: true,
            bake_only: false,
            is_dynamic: false,
            casts_entity_shadows: false,
            is_animated: false,
            tags: vec![],
            shadow_type: crate::map_data::ShadowType::StaticLightMap,
        };
        let exterior: HashSet<usize> = HashSet::new();
        let lights = std::slice::from_ref(&animated);
        let static_lights = StaticBakedLights::from_lights(lights);
        let animated_lights = AnimatedBakedLights::from_lights(lights);
        let inputs = ShBakeCtx {
            bvh: &bvh,
            primitives: &prims,
            geometry: &geo,
            tree: &tree,
            exterior_leaves: &exterior,
            static_lights: &static_lights,
            animated_lights: &animated_lights,
            total_light_count: 0,
        };
        let section = bake_sh_volume(&inputs, &ShConfig { probe_spacing: 1.0 });
        assert_eq!(section.animation_descriptors.len(), 1);
        assert_eq!(section.animation_descriptors[0].period, 1.0);
        assert_eq!(
            section.animation_descriptors[0].base_color,
            [1.0, 0.5, 0.25]
        );
    }

    /// Regression: `base_color` in the animation descriptor must encode
    /// `color × intensity` so the runtime compose and SH shaders see the
    /// correct peak irradiance. A light with `intensity = 3.0` and
    /// `color = [0.5, 1.0, 0.8]` must produce `base_color = [1.5, 3.0, 2.4]`,
    /// not just `[0.5, 1.0, 0.8]`.
    #[test]
    fn animation_descriptor_base_color_includes_intensity() {
        let geo = one_triangle_geometry([[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]]);
        let (bvh, prims, _) = build_bvh(&geo).unwrap();
        let tree = tree_all_empty();
        let light = MapLight {
            origin: DVec3::new(0.5, 2.0, 0.5),
            light_type: LightType::Point,
            intensity: 3.0,
            color: [0.5, 1.0, 0.8],
            falloff_model: FalloffModel::Linear,
            falloff_range: 5.0,
            light_size: 0.0,
            angular_diameter: 0.0,
            cone_angle_inner: None,
            cone_angle_outer: None,
            cone_direction: None,
            animation: Some(LightAnimation {
                period: 1.0,
                phase: 0.0,
                brightness: Some(vec![0.0, 1.0]),
                color: None,
                direction: None,
                start_active: true,
            }),
            cast_shadows: true,
            bake_only: false,
            is_dynamic: false,
            casts_entity_shadows: false,
            is_animated: false,
            tags: vec![],
            shadow_type: crate::map_data::ShadowType::StaticLightMap,
        };
        let exterior: HashSet<usize> = HashSet::new();
        let lights = std::slice::from_ref(&light);
        let static_lights = StaticBakedLights::from_lights(lights);
        let animated_lights = AnimatedBakedLights::from_lights(lights);
        let inputs = ShBakeCtx {
            bvh: &bvh,
            primitives: &prims,
            geometry: &geo,
            tree: &tree,
            exterior_leaves: &exterior,
            static_lights: &static_lights,
            animated_lights: &animated_lights,
            total_light_count: 0,
        };
        let section = bake_sh_volume(&inputs, &ShConfig { probe_spacing: 1.0 });
        assert_eq!(section.animation_descriptors.len(), 1);
        let base_color = section.animation_descriptors[0].base_color;
        let expected = [0.5 * 3.0, 1.0 * 3.0, 0.8 * 3.0];
        for (got, exp) in base_color.iter().zip(expected.iter()) {
            assert!(
                (got - exp).abs() < 1.0e-5,
                "base_color channel {got} != expected {exp} — \
                 intensity was not folded into base_color",
            );
        }
    }

    #[test]
    fn no_animated_lights_emits_static_only_layout() {
        let geo = one_triangle_geometry([[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]]);
        let (bvh, prims, _) = build_bvh(&geo).unwrap();
        let tree = tree_all_empty();
        let exterior: HashSet<usize> = HashSet::new();
        let lights: &[MapLight] = &[];
        let static_lights = StaticBakedLights::from_lights(lights);
        let animated_lights = AnimatedBakedLights::from_lights(lights);
        let inputs = ShBakeCtx {
            bvh: &bvh,
            primitives: &prims,
            geometry: &geo,
            tree: &tree,
            exterior_leaves: &exterior,
            static_lights: &static_lights,
            animated_lights: &animated_lights,
            total_light_count: 0,
        };
        let section = bake_sh_volume(&inputs, &ShConfig { probe_spacing: 1.0 });
        assert!(section.animation_descriptors.is_empty());
        // animated_light_count keeps the legacy header offset (bytes 44..48)
        // before the octahedral tile metadata.
        let bytes = section.to_bytes();
        assert_eq!(&bytes[44..48], &0u32.to_le_bytes());
    }

    #[test]
    fn solid_probes_are_flagged_invalid() {
        let tree = BspTree {
            nodes: Vec::new(),
            leaves: vec![BspLeaf {
                face_indices: Vec::new(),
                bounds: CompilerAabb {
                    min: DVec3::splat(-1000.0),
                    max: DVec3::splat(1000.0),
                },
                is_solid: true,
                defining_planes: Vec::new(),
            }],
        };
        let geo = one_triangle_geometry([[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]]);
        let (bvh, prims, _) = build_bvh(&geo).unwrap();
        let exterior: HashSet<usize> = HashSet::new();
        let lights: &[MapLight] = &[];
        let static_lights = StaticBakedLights::from_lights(lights);
        let animated_lights = AnimatedBakedLights::from_lights(lights);
        let inputs = ShBakeCtx {
            bvh: &bvh,
            primitives: &prims,
            geometry: &geo,
            tree: &tree,
            exterior_leaves: &exterior,
            static_lights: &static_lights,
            animated_lights: &animated_lights,
            total_light_count: 0,
        };
        let section = bake_sh_volume(&inputs, &ShConfig { probe_spacing: 1.0 });
        for probe in &section.probes {
            assert_eq!(probe.validity, 0);
        }
    }

    #[test]
    fn exterior_leaf_probes_are_flagged_invalid() {
        // Regression: sub-plan 10 Fix D — exercises probe_is_valid directly for
        // solid, interior-empty, and exterior-empty leaves (no BSP nodes, so leaf 0 catches all).
        let solid_leaf = BspLeaf {
            face_indices: Vec::new(),
            bounds: CompilerAabb {
                min: DVec3::splat(-1.0),
                max: DVec3::splat(1.0),
            },
            is_solid: true,
            defining_planes: Vec::new(),
        };
        let interior_leaf = BspLeaf {
            face_indices: Vec::new(),
            bounds: CompilerAabb {
                min: DVec3::splat(-1.0),
                max: DVec3::splat(1.0),
            },
            is_solid: false,
            defining_planes: Vec::new(),
        };
        let exterior_leaf = BspLeaf {
            face_indices: Vec::new(),
            bounds: CompilerAabb {
                min: DVec3::splat(-1.0),
                max: DVec3::splat(1.0),
            },
            is_solid: false,
            defining_planes: Vec::new(),
        };

        let tree_interior = BspTree {
            nodes: Vec::new(),
            leaves: vec![interior_leaf.clone()],
        };
        let mut exterior_set_empty: HashSet<usize> = HashSet::new();
        assert!(probe_is_valid(
            &tree_interior,
            &exterior_set_empty,
            DVec3::ZERO
        ));

        let tree_solid = BspTree {
            nodes: Vec::new(),
            leaves: vec![solid_leaf],
        };
        assert!(!probe_is_valid(
            &tree_solid,
            &exterior_set_empty,
            DVec3::ZERO
        ));

        let tree_exterior = BspTree {
            nodes: Vec::new(),
            leaves: vec![exterior_leaf],
        };
        exterior_set_empty.insert(0);
        assert!(!probe_is_valid(
            &tree_exterior,
            &exterior_set_empty,
            DVec3::ZERO
        ));
    }

    #[test]
    fn bake_sh_volume_marks_exterior_leaf_probes_invalid() {
        // Integration: verifies ShBakeCtx correctly wires exterior_leaves through the full bake.
        let geo = one_triangle_geometry([[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]]);
        let (bvh, prims, _) = build_bvh(&geo).unwrap();
        let tree = tree_all_empty();
        let mut exterior: HashSet<usize> = HashSet::new();
        exterior.insert(0);
        let lights: &[MapLight] = &[];
        let static_lights = StaticBakedLights::from_lights(lights);
        let animated_lights = AnimatedBakedLights::from_lights(lights);
        let inputs = ShBakeCtx {
            bvh: &bvh,
            primitives: &prims,
            geometry: &geo,
            tree: &tree,
            exterior_leaves: &exterior,
            static_lights: &static_lights,
            animated_lights: &animated_lights,
            total_light_count: 0,
        };
        let section = bake_sh_volume(&inputs, &ShConfig { probe_spacing: 1.0 });
        assert!(
            !section.probes.is_empty(),
            "expected at least one probe in the bake output",
        );
        for probe in &section.probes {
            assert_eq!(
                probe.validity, 0,
                "probes in an exterior-flagged leaf must be invalid after bake",
            );
            assert_eq!(
                probe.mean_distance, 0,
                "invalid probes must carry zeroed depth moments (AC#3)"
            );
            assert_eq!(
                probe.mean_sq_distance, 0,
                "invalid probes must carry zeroed depth moments (AC#3)"
            );
        }
    }

    #[test]
    fn is_dynamic_lights_skipped_by_bake() {
        // Regression: is_dynamic lights must be excluded so the runtime doesn't double-count them.
        let geo = one_triangle_geometry([[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]]);
        let (bvh, prims, _) = build_bvh(&geo).unwrap();
        let tree = tree_all_empty();
        let exterior: HashSet<usize> = HashSet::new();

        let baseline = {
            let lights: &[MapLight] = &[];
            let static_lights = StaticBakedLights::from_lights(lights);
            let animated_lights = AnimatedBakedLights::from_lights(lights);
            let inputs = ShBakeCtx {
                bvh: &bvh,
                primitives: &prims,
                geometry: &geo,
                tree: &tree,
                exterior_leaves: &exterior,
                static_lights: &static_lights,
                animated_lights: &animated_lights,
                total_light_count: 0,
            };
            bake_sh_volume(&inputs, &ShConfig { probe_spacing: 1.0 })
        };

        let mut dyn_light = MapLight {
            origin: DVec3::new(0.3, 1.0, 0.3),
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
        };
        // Dynamic-tier lights are selected by classname → `is_dynamic`; the
        // namespace filter keys on this position axis, excluding them from both
        // bake sets (the lightmap and SH base/delta).
        dyn_light.is_dynamic = true;

        let with_dynamic = {
            let lights = std::slice::from_ref(&dyn_light);
            let static_lights = StaticBakedLights::from_lights(lights);
            let animated_lights = AnimatedBakedLights::from_lights(lights);
            let inputs = ShBakeCtx {
                bvh: &bvh,
                primitives: &prims,
                geometry: &geo,
                tree: &tree,
                exterior_leaves: &exterior,
                static_lights: &static_lights,
                animated_lights: &animated_lights,
                total_light_count: 0,
            };
            bake_sh_volume(&inputs, &ShConfig { probe_spacing: 1.0 })
        };

        assert_eq!(with_dynamic.probes.len(), baseline.probes.len());
        assert!(with_dynamic.animation_descriptors.is_empty());
        for (a, b) in with_dynamic.probes.iter().zip(baseline.probes.iter()) {
            assert_eq!(a.validity, b.validity);
        }
        assert_eq!(
            with_dynamic.atlas_texels, baseline.atlas_texels,
            "dynamic-light bake diverged from baseline atlas",
        );
    }

    /// Indirect-not-starved contract (sdf-per-light-shadows Task 3): an
    /// `sdf`-typed light's *bounce* MUST still bake into SH. The namespace
    /// filter keys on the position axis, so shadow type never drops a light from
    /// indirect — tagging a light `sdf` moves only its direct term to runtime.
    /// A bake with an sdf light therefore diverges from the empty baseline.
    #[test]
    fn sdf_typed_light_remains_in_sh_indirect_bake() {
        let geo = floor_and_walls_geometry();
        let (bvh, prims, _) = build_bvh(&geo).unwrap();
        let tree = tree_all_empty();
        let exterior: HashSet<usize> = HashSet::new();

        let baseline = {
            let lights: &[MapLight] = &[];
            let static_lights = StaticBakedLights::from_lights(lights);
            let animated_lights = AnimatedBakedLights::from_lights(lights);
            let inputs = ShBakeCtx {
                bvh: &bvh,
                primitives: &prims,
                geometry: &geo,
                tree: &tree,
                exterior_leaves: &exterior,
                static_lights: &static_lights,
                animated_lights: &animated_lights,
                total_light_count: 0,
            };
            bake_sh_volume(&inputs, &ShConfig { probe_spacing: 1.0 })
        };

        let sdf_light = MapLight {
            origin: DVec3::new(0.3, 1.0, 0.3),
            light_type: LightType::Point,
            intensity: 5.0,
            color: [1.0, 1.0, 1.0],
            falloff_model: FalloffModel::Linear,
            falloff_range: 20.0,
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
            // Tagged sdf: its DIRECT term goes runtime, but its bounce must
            // still reach SH — the namespace keeps it (position axis).
            shadow_type: crate::map_data::ShadowType::Sdf,
        };

        let with_sdf = {
            let lights = std::slice::from_ref(&sdf_light);
            let static_lights = StaticBakedLights::from_lights(lights);
            assert_eq!(
                static_lights.len(),
                1,
                "sdf light must remain in StaticBakedLights so SH sees its bounce",
            );
            let animated_lights = AnimatedBakedLights::from_lights(lights);
            let inputs = ShBakeCtx {
                bvh: &bvh,
                primitives: &prims,
                geometry: &geo,
                tree: &tree,
                exterior_leaves: &exterior,
                static_lights: &static_lights,
                animated_lights: &animated_lights,
                total_light_count: 1,
            };
            bake_sh_volume(&inputs, &ShConfig { probe_spacing: 1.0 })
        };

        assert_eq!(with_sdf.probes.len(), baseline.probes.len());
        let diverged = with_sdf.atlas_texels != baseline.atlas_texels;
        assert!(
            diverged,
            "an sdf-typed light's bounce must bake into the octahedral atlas (indirect not starved)",
        );
    }

    #[test]
    fn sphere_directions_cover_sphere_integral() {
        let dirs = sphere_directions(RAYS_PER_PROBE, SAMPLING_LATTICE_OFFSET);
        let weight = (4.0 * std::f32::consts::PI) / RAYS_PER_PROBE as f32;
        let integral: f32 = dirs.iter().map(|_| 1.0 * weight).sum();
        let expected = 4.0 * std::f32::consts::PI;
        assert!(
            (integral - expected).abs() < 1.0e-3,
            "integral of 1 over sphere: expected {expected}, got {integral}"
        );
    }

    #[test]
    fn build_bvh_traversal_interop() {
        let geo = one_triangle_geometry([[-1.0, 0.0, -1.0], [2.0, 0.0, -1.0], [0.0, 0.0, 2.0]]);
        let (bvh, _prims, _) = build_bvh(&geo).unwrap();
        let prims = collect_primitives(&geo);
        let ray = bvh::ray::Ray::new(
            nalgebra::Point3::new(0.0, 5.0, 0.0),
            nalgebra::Vector3::new(0.0, -1.0, 0.0),
        );
        let hits = bvh.traverse(&ray, &prims);
        assert!(!hits.is_empty(), "ray should hit the triangle face AABB");
    }
}
