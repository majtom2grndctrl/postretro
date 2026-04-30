// SH irradiance volume baker — indirect-only L2 probe grid over the level AABB.
// See: context/plans/in-progress/lighting-foundation/2-sh-baker.md

use std::collections::HashSet;

use bvh::bvh::Bvh;
use bvh::ray::Ray;
use glam::{DVec3, Vec3};
use nalgebra::{Point3, Vector3};
use postretro_level_format::sh_volume::{
    AnimationDescriptor, PROBE_STRIDE, ShProbe, ShVolumeSection,
};
use rayon::prelude::*;

use crate::bvh_build::BvhPrimitive;
use crate::geometry::GeometryResult;
use crate::light_namespaces::{AnimatedBakedLights, StaticBakedLights};
use crate::map_data::{FalloffModel, LightAnimation, LightType, MapLight};
use crate::partition::{BspTree, find_leaf_for_point};

/// Default grid cell size in meters. Overridden by `--probe-spacing`.
pub const DEFAULT_PROBE_SPACING: f32 = 1.0;

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

pub struct BakeInputs<'a> {
    pub bvh: &'a Bvh<f32, 3>,
    pub primitives: &'a [BvhPrimitive],
    pub geometry: &'a GeometryResult,
    pub tree: &'a BspTree,
    /// Probes in these leaves are flagged invalid — out-of-map radiance pollutes
    /// trilinear interpolation at the playable edge.
    pub exterior_leaves: &'a HashSet<usize>,
    pub static_lights: &'a StaticBakedLights<'a>,
    pub animated_lights: &'a AnimatedBakedLights<'a>,
}

impl<'a> BakeInputs<'a> {
    fn ray_ctx(&self) -> RaytracingCtx<'a> {
        RaytracingCtx {
            bvh: self.bvh,
            primitives: self.primitives,
            geometry: self.geometry,
        }
    }
}

/// Returns an empty section (`grid_dimensions == [0,0,0]`) for empty geometry,
/// matching the "no SH section" degradation path at runtime.
pub fn bake_sh_volume(inputs: &BakeInputs<'_>, probe_spacing_meters: f32) -> ShVolumeSection {
    let geom = &inputs.geometry.geometry;
    if geom.vertices.is_empty() {
        return ShVolumeSection {
            grid_origin: [0.0, 0.0, 0.0],
            cell_size: [probe_spacing_meters; 3],
            grid_dimensions: [0, 0, 0],
            probe_stride: PROBE_STRIDE,
            probes: Vec::new(),
            animation_descriptors: Vec::new(),
        };
    }

    let (world_min, world_max) = world_aabb(inputs);
    let dims = grid_dimensions(world_min, world_max, probe_spacing_meters);
    let total = dims[0] as usize * dims[1] as usize * dims[2] as usize;

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

    let base_probes: Vec<ShProbe> = (0..total)
        .into_par_iter()
        .map(|i| {
            if validity[i] == 0 {
                return ShProbe::default();
            }
            let pos = vec3_from(probe_positions[i]);
            let coeffs = bake_probe_rgb(inputs, pos, &static_lights);
            ShProbe {
                sh_coefficients: coeffs,
                validity: 1,
            }
        })
        .collect();

    // Per-light monochrome SH layers removed; animated indirect is handled by the lightmap compose pass.
    let animation_descriptors: Vec<AnimationDescriptor> = animated_lights
        .iter()
        .map(|l| animation_descriptor_for(l))
        .collect();

    ShVolumeSection {
        grid_origin: [world_min.x as f32, world_min.y as f32, world_min.z as f32],
        cell_size: [probe_spacing_meters; 3],
        grid_dimensions: dims,
        probe_stride: PROBE_STRIDE,
        probes: base_probes,
        animation_descriptors,
    }
}

pub fn log_stats(section: &ShVolumeSection) {
    let dims = section.grid_dimensions;
    let total = section.total_probes();
    let valid = section.probes.iter().filter(|p| p.validity == 1).count();
    log::info!(
        "ShVolume: grid {}x{}x{} = {total} probes ({valid} valid), \
         cell {}m, {} animated light(s)",
        dims[0],
        dims[1],
        dims[2],
        section.cell_size[0],
        section.animation_descriptors.len(),
    );
}

fn world_aabb(inputs: &BakeInputs<'_>) -> (DVec3, DVec3) {
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

/// Indirect-only SH L2 RGB. Shared with the per-light delta SH baker.
pub(crate) fn bake_probe_indirect_rgb(
    ctx: &RaytracingCtx<'_>,
    probe_pos: Vec3,
    lights: &[&MapLight],
) -> [f32; 27] {
    let directions = sphere_directions(RAYS_PER_PROBE, SAMPLING_LATTICE_OFFSET);
    let mc_weight = (4.0 * std::f32::consts::PI) / RAYS_PER_PROBE as f32;
    let mut acc = [0f32; 27];
    for dir in &directions {
        let radiance = sample_radiance_rgb(ctx, probe_pos, *dir, lights);
        accumulate_sh_rgb(&mut acc, *dir, radiance, mc_weight);
    }
    apply_cosine_lobe_rgb(&mut acc);
    acc
}

/// Direct SH L2 RGB for a single light at a probe — used by the delta SH baker.
/// Projects a delta-direction radiance pulse; cosine-lobe convolution at the end
/// matches the irradiance convention of `bake_probe_indirect_rgb`.
pub(crate) fn bake_probe_direct_rgb(
    ctx: &RaytracingCtx<'_>,
    probe_pos: Vec3,
    light: &MapLight,
) -> [f32; 27] {
    let mut acc = [0f32; 27];
    if !shadow_visible_at_point(ctx, probe_pos, light) {
        return acc;
    }
    let radiance = light_radiance_at_point(light, probe_pos);
    if radiance == Vec3::ZERO {
        return acc;
    }
    let to_light = light_direction_at_point(light, probe_pos);
    if to_light == Vec3::ZERO {
        return acc;
    }
    // Delta-direction emitter: weight = 1.0 (no sphere integration needed).
    accumulate_sh_rgb(&mut acc, to_light, radiance, 1.0);
    apply_cosine_lobe_rgb(&mut acc);
    acc
}

fn bake_probe_rgb(
    inputs: &BakeInputs<'_>,
    probe_pos: Vec3,
    static_lights: &[&MapLight],
) -> [f32; 27] {
    bake_probe_indirect_rgb(&inputs.ray_ctx(), probe_pos, static_lights)
}

/// Indirect (bounced) radiance along one ray — direct term is the lightmap's responsibility.
fn sample_radiance_rgb(
    ctx: &RaytracingCtx<'_>,
    origin: Vec3,
    dir: Vec3,
    lights: &[&MapLight],
) -> Vec3 {
    match closest_hit(ctx, origin + dir * RAY_EPSILON, dir, f32::INFINITY) {
        None => Vec3::from(SKY_COLOR),
        Some(hit) => {
            let mut radiance = Vec3::ZERO;
            for light in lights {
                if !shadow_visible(ctx, hit.point, hit.normal, light) {
                    continue;
                }
                radiance += light_contribution_lambert(light, hit.point, hit.normal);
            }
            radiance * BOUNCE_ALBEDO / std::f32::consts::PI
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

/// Like `shadow_visible` but for probe sample points — no host triangle to nudge off.
fn shadow_visible_at_point(ctx: &RaytracingCtx<'_>, point: Vec3, light: &MapLight) -> bool {
    if !light.cast_shadows {
        return true;
    }
    let shadow_origin = light_shadow_origin(light, point);
    segment_clear(ctx, point, shadow_origin)
}

fn light_direction_at_point(light: &MapLight, point: Vec3) -> Vec3 {
    match light.light_type {
        LightType::Point | LightType::Spot => {
            let to_light = Vec3::new(
                light.origin.x as f32 - point.x,
                light.origin.y as f32 - point.y,
                light.origin.z as f32 - point.z,
            );
            to_light.normalize_or_zero()
        }
        LightType::Directional => {
            let dir = Vec3::from(light.cone_direction.unwrap_or([0.0, -1.0, 0.0]));
            (-dir).normalize_or_zero()
        }
    }
}

/// Color × intensity × falloff (± cone) without N·L — that is reapplied per sample during projection.
fn light_radiance_at_point(light: &MapLight, point: Vec3) -> Vec3 {
    match light.light_type {
        LightType::Point => {
            let to_light = Vec3::new(
                light.origin.x as f32 - point.x,
                light.origin.y as f32 - point.y,
                light.origin.z as f32 - point.z,
            );
            let dist = to_light.length();
            if dist < 1.0e-4 {
                return Vec3::ZERO;
            }
            let attenuation = falloff(light, dist);
            Vec3::from(light.color) * (light.intensity * attenuation)
        }
        LightType::Spot => {
            let to_light = Vec3::new(
                light.origin.x as f32 - point.x,
                light.origin.y as f32 - point.y,
                light.origin.z as f32 - point.z,
            );
            let dist = to_light.length();
            if dist < 1.0e-4 {
                return Vec3::ZERO;
            }
            let l = to_light / dist;
            let attenuation = falloff(light, dist);
            let cone = spot_cone_attenuation(light, -l);
            Vec3::from(light.color) * (light.intensity * attenuation * cone)
        }
        LightType::Directional => Vec3::from(light.color) * light.intensity,
    }
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
        let inputs = BakeInputs {
            bvh: &bvh,
            primitives: &primitives,
            geometry: &geo,
            tree: &tree,
            exterior_leaves: &exterior,
            static_lights: &static_lights,
            animated_lights: &animated_lights,
        };
        let section = bake_sh_volume(&inputs, 1.0);
        assert_eq!(section.grid_dimensions, [0, 0, 0]);
        assert!(section.probes.is_empty());
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
            cone_angle_inner: None,
            cone_angle_outer: None,
            cone_direction: None,
            animation: None,
            cast_shadows: true,
            bake_only: false,
            is_dynamic: false,
            tags: vec![],
        };
        let exterior: HashSet<usize> = HashSet::new();
        let lights = std::slice::from_ref(&light);
        let static_lights = StaticBakedLights::from_lights(lights);
        let animated_lights = AnimatedBakedLights::from_lights(lights);
        let inputs = BakeInputs {
            bvh: &bvh,
            primitives: &prims,
            geometry: &geo,
            tree: &tree,
            exterior_leaves: &exterior,
            static_lights: &static_lights,
            animated_lights: &animated_lights,
        };
        let a = bake_sh_volume(&inputs, 1.0);
        let b = bake_sh_volume(&inputs, 1.0);
        assert_eq!(a.to_bytes(), b.to_bytes());
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
            tags: vec![],
        };
        let exterior: HashSet<usize> = HashSet::new();
        let lights = std::slice::from_ref(&animated);
        let static_lights = StaticBakedLights::from_lights(lights);
        let animated_lights = AnimatedBakedLights::from_lights(lights);
        let inputs = BakeInputs {
            bvh: &bvh,
            primitives: &prims,
            geometry: &geo,
            tree: &tree,
            exterior_leaves: &exterior,
            static_lights: &static_lights,
            animated_lights: &animated_lights,
        };
        let section = bake_sh_volume(&inputs, 1.0);
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
            tags: vec![],
        };
        let exterior: HashSet<usize> = HashSet::new();
        let lights = std::slice::from_ref(&light);
        let static_lights = StaticBakedLights::from_lights(lights);
        let animated_lights = AnimatedBakedLights::from_lights(lights);
        let inputs = BakeInputs {
            bvh: &bvh,
            primitives: &prims,
            geometry: &geo,
            tree: &tree,
            exterior_leaves: &exterior,
            static_lights: &static_lights,
            animated_lights: &animated_lights,
        };
        let section = bake_sh_volume(&inputs, 1.0);
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
        let inputs = BakeInputs {
            bvh: &bvh,
            primitives: &prims,
            geometry: &geo,
            tree: &tree,
            exterior_leaves: &exterior,
            static_lights: &static_lights,
            animated_lights: &animated_lights,
        };
        let section = bake_sh_volume(&inputs, 1.0);
        assert!(section.animation_descriptors.is_empty());
        // animated_light_count is the last u32 of the 48-byte header (bytes 44..48).
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
            }],
        };
        let geo = one_triangle_geometry([[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]]);
        let (bvh, prims, _) = build_bvh(&geo).unwrap();
        let exterior: HashSet<usize> = HashSet::new();
        let lights: &[MapLight] = &[];
        let static_lights = StaticBakedLights::from_lights(lights);
        let animated_lights = AnimatedBakedLights::from_lights(lights);
        let inputs = BakeInputs {
            bvh: &bvh,
            primitives: &prims,
            geometry: &geo,
            tree: &tree,
            exterior_leaves: &exterior,
            static_lights: &static_lights,
            animated_lights: &animated_lights,
        };
        let section = bake_sh_volume(&inputs, 1.0);
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
        };
        let interior_leaf = BspLeaf {
            face_indices: Vec::new(),
            bounds: CompilerAabb {
                min: DVec3::splat(-1.0),
                max: DVec3::splat(1.0),
            },
            is_solid: false,
        };
        let exterior_leaf = BspLeaf {
            face_indices: Vec::new(),
            bounds: CompilerAabb {
                min: DVec3::splat(-1.0),
                max: DVec3::splat(1.0),
            },
            is_solid: false,
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
        // Integration: verifies BakeInputs correctly wires exterior_leaves through the full bake.
        let geo = one_triangle_geometry([[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]]);
        let (bvh, prims, _) = build_bvh(&geo).unwrap();
        let tree = tree_all_empty();
        let mut exterior: HashSet<usize> = HashSet::new();
        exterior.insert(0);
        let lights: &[MapLight] = &[];
        let static_lights = StaticBakedLights::from_lights(lights);
        let animated_lights = AnimatedBakedLights::from_lights(lights);
        let inputs = BakeInputs {
            bvh: &bvh,
            primitives: &prims,
            geometry: &geo,
            tree: &tree,
            exterior_leaves: &exterior,
            static_lights: &static_lights,
            animated_lights: &animated_lights,
        };
        let section = bake_sh_volume(&inputs, 1.0);
        assert!(
            !section.probes.is_empty(),
            "expected at least one probe in the bake output",
        );
        for probe in &section.probes {
            assert_eq!(
                probe.validity, 0,
                "probes in an exterior-flagged leaf must be invalid after bake",
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
            let inputs = BakeInputs {
                bvh: &bvh,
                primitives: &prims,
                geometry: &geo,
                tree: &tree,
                exterior_leaves: &exterior,
                static_lights: &static_lights,
                animated_lights: &animated_lights,
            };
            bake_sh_volume(&inputs, 1.0)
        };

        let mut dyn_light = MapLight {
            origin: DVec3::new(0.3, 1.0, 0.3),
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
        };
        dyn_light.is_dynamic = true;

        let with_dynamic = {
            let lights = std::slice::from_ref(&dyn_light);
            let static_lights = StaticBakedLights::from_lights(lights);
            let animated_lights = AnimatedBakedLights::from_lights(lights);
            let inputs = BakeInputs {
                bvh: &bvh,
                primitives: &prims,
                geometry: &geo,
                tree: &tree,
                exterior_leaves: &exterior,
                static_lights: &static_lights,
                animated_lights: &animated_lights,
            };
            bake_sh_volume(&inputs, 1.0)
        };

        assert_eq!(with_dynamic.probes.len(), baseline.probes.len());
        assert!(with_dynamic.animation_descriptors.is_empty());
        for (a, b) in with_dynamic.probes.iter().zip(baseline.probes.iter()) {
            assert_eq!(a.validity, b.validity);
            for (ca, cb) in a.sh_coefficients.iter().zip(b.sh_coefficients.iter()) {
                assert!(
                    (ca - cb).abs() < 1.0e-5,
                    "dynamic-light bake diverged from baseline: {ca} vs {cb}",
                );
            }
        }
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
