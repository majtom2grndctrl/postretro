// SH irradiance volume baker.
//
// Places L2 probes on a regular grid over the level AABB, traces stratified
// spherical samples through the Milestone 4 BVH, projects the incoming
// radiance into SH coefficients, and flags probes inside solid geometry as
// invalid. Animated lights bake into separate monochrome per-light layers.
//
// See: context/plans/in-progress/lighting-foundation/2-sh-baker.md

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
use crate::map_data::{FalloffModel, LightAnimation, LightType, MapLight};
use crate::partition::{BspTree, find_leaf_for_point};

/// Default grid cell size in meters. Overridden by `--probe-spacing`.
pub const DEFAULT_PROBE_SPACING: f32 = 1.0;

/// Rays fired per valid probe. Fixed for determinism; a future revision may
/// expose a CLI flag if bake-time budget becomes the bottleneck.
const RAYS_PER_PROBE: u32 = 256;

/// Constant Lambertian albedo used to approximate a single bounce at every
/// ray hit. Postretro's aesthetic is not photorealistic GI — a uniform grey
/// albedo is enough to carry directional indirect color and intensity. A
/// future revision may sample per-face albedo once a material system exists.
const BOUNCE_ALBEDO: f32 = 0.5;

/// Sky / miss color. Rays that miss all geometry contribute this constant
/// ambient.
const SKY_COLOR: [f32; 3] = [0.0, 0.0, 0.0];

/// Tiny offset applied along the ray direction to avoid self-intersections at
/// the probe origin and at shadow-ray hit points.
const RAY_EPSILON: f32 = 1.0e-3;

/// Fixed rotation offset applied to the Fibonacci-lattice sample directions.
/// The sampler is deterministic — there is no RNG — so two bakes of identical
/// input produce byte-identical probe coefficients. The constant just rotates
/// the lattice off the `(0, 0, 1)` axis so axis-aligned light directions
/// don't land on a degenerate sample.
const SAMPLING_LATTICE_OFFSET: u64 = 0x5048_4542_414b_4552; // "PHBAKER"

/// Inputs the baker pulls together from the rest of the compile stages.
pub struct BakeInputs<'a> {
    pub bvh: &'a Bvh<f32, 3>,
    pub primitives: &'a [BvhPrimitive],
    pub geometry: &'a GeometryResult,
    pub tree: &'a BspTree,
    pub lights: &'a [MapLight],
}

/// Bake an SH irradiance volume over the level AABB at the requested spacing.
///
/// Returns an empty section (`grid_dimensions == [0,0,0]`) if the input
/// geometry is empty — this keeps the degradation path the spec mandates
/// identical to the "no SH section" case at runtime.
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
            per_light_sh: Vec::new(),
        };
    }

    // World AABB over the extracted geometry. Engine space, meters.
    let (world_min, world_max) = world_aabb(inputs);
    let dims = grid_dimensions(world_min, world_max, probe_spacing_meters);
    let total = dims[0] as usize * dims[1] as usize * dims[2] as usize;

    // Decompose lights into static (folded into the base coefficients) and
    // animated (one per-light mono layer each).
    let (static_lights, animated_lights): (Vec<&MapLight>, Vec<&MapLight>) =
        inputs.lights.iter().partition(|l| l.animation.is_none());

    // Build probe list and flag validity against the BSP tree.
    let probe_positions: Vec<DVec3> = (0..total)
        .map(|i| probe_position(i, dims, world_min, probe_spacing_meters))
        .collect();

    let validity: Vec<u8> = probe_positions
        .iter()
        .map(|&p| if probe_is_valid(inputs.tree, p) { 1 } else { 0 })
        .collect();

    // Static-light base coefficients, parallelized per probe.
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

    // One monochrome layer per animated light.
    let mut animation_descriptors = Vec::with_capacity(animated_lights.len());
    let mut per_light_sh = Vec::with_capacity(animated_lights.len());

    for light in &animated_lights {
        animation_descriptors.push(animation_descriptor_for(light));

        let layer: Vec<f32> = (0..total)
            .into_par_iter()
            .flat_map_iter(|i| {
                let bake = if validity[i] == 0 {
                    [0f32; 9]
                } else {
                    let pos = vec3_from(probe_positions[i]);
                    bake_probe_mono(inputs, pos, light)
                };
                bake.into_iter()
            })
            .collect();
        per_light_sh.push(layer);
    }

    ShVolumeSection {
        grid_origin: [world_min.x as f32, world_min.y as f32, world_min.z as f32],
        cell_size: [probe_spacing_meters; 3],
        grid_dimensions: dims,
        probe_stride: PROBE_STRIDE,
        probes: base_probes,
        animation_descriptors,
        per_light_sh,
    }
}

/// Log bake statistics in the same shape as other compile stages.
pub fn log_stats(section: &ShVolumeSection) {
    let dims = section.grid_dimensions;
    let total = section.total_probes();
    let valid = section.probes.iter().filter(|p| p.validity == 1).count();
    log::info!(
        "ShVolume: grid {}x{}x{} = {total} probes ({valid} valid), \
         cell {}m, {} animated layer(s)",
        dims[0],
        dims[1],
        dims[2],
        section.cell_size[0],
        section.per_light_sh.len(),
    );
}

// ---------------------------------------------------------------------------
// Grid layout

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

/// z-major then y, then x — matches the format crate's probe iteration order.
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

fn probe_is_valid(tree: &BspTree, pos: DVec3) -> bool {
    if tree.leaves.is_empty() {
        return true;
    }
    let leaf = find_leaf_for_point(tree, pos);
    !tree.leaves[leaf].is_solid
}

fn vec3_from(v: DVec3) -> Vec3 {
    Vec3::new(v.x as f32, v.y as f32, v.z as f32)
}

// ---------------------------------------------------------------------------
// Ray generation

/// Deterministic, low-discrepancy unit-sphere directions.
///
/// The Fibonacci sphere produces an evenly-spaced direction set for any
/// sample count, with no RNG state — identical input always yields identical
/// rays. Combined with a fixed seed offset on the angle (a full-turn
/// golden-angle rotation per sample), two bakes of the same map are
/// byte-identical.
fn sphere_directions(count: u32, seed: u64) -> Vec<Vec3> {
    let mut out = Vec::with_capacity(count as usize);
    let phi = std::f32::consts::PI * (3.0 - (5.0_f32).sqrt()); // golden angle
    // Deterministic seed offset keeps the sequence stable across bakes while
    // allowing a different pattern if we ever want to rotate the probe sphere.
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

// ---------------------------------------------------------------------------
// Ray traversal

/// One triangle hit: position, surface normal, signed distance along the ray.
struct Hit {
    point: Vec3,
    normal: Vec3,
    distance: f32,
}

/// Closest-triangle hit along `ray`. `max_distance` clips the search. Uses
/// the Milestone 4 BVH primitive set plus the shared geometry index buffer.
fn closest_hit(
    inputs: &BakeInputs<'_>,
    ray_origin: Vec3,
    ray_dir: Vec3,
    max_distance: f32,
) -> Option<Hit> {
    let ray = Ray::new(
        Point3::new(ray_origin.x, ray_origin.y, ray_origin.z),
        Vector3::new(ray_dir.x, ray_dir.y, ray_dir.z),
    );
    let candidates = inputs.bvh.traverse(&ray, inputs.primitives);
    if candidates.is_empty() {
        return None;
    }

    let geom = &inputs.geometry.geometry;
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
        // Clip by caller's max_distance to support shadow-ray early-outs.
        if h.distance >= max_distance {
            return None;
        }
    }
    best
}

/// Double-sided Möller-Trumbore intersection. Returns `(t, geometric_normal)`.
/// The normal is flipped to face the incoming ray so indirect illumination
/// does not vanish at back-facing walls.
fn ray_triangle_hit(origin: Vec3, dir: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Option<(f32, Vec3)> {
    let edge1 = b - a;
    let edge2 = c - a;
    let h = dir.cross(edge2);
    let det = edge1.dot(h);
    // Two-sided test — treat very small determinants as parallel.
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

/// True if the straight path from `from` to `to` is unoccluded. Used by
/// shadow rays from hit points toward each map light.
fn segment_clear(inputs: &BakeInputs<'_>, from: Vec3, to: Vec3) -> bool {
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
    let candidates = inputs.bvh.traverse(&ray, inputs.primitives);
    let max_distance = length - RAY_EPSILON;
    let geom = &inputs.geometry.geometry;
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

// ---------------------------------------------------------------------------
// Light evaluation

/// Lambert contribution from one light at a surface point. Does not include
/// visibility — shadow testing is done by the caller so the same path can be
/// reused by both static and animated light bakes.
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
            // The map light `cone_direction` is the aim vector — the direction
            // photons travel. The vector pointing from the surface toward the
            // light is the negation.
            let l = (-dir).normalize_or_zero();
            let n_dot_l = surface_normal.dot(l).max(0.0);
            if n_dot_l <= 0.0 {
                return Vec3::ZERO;
            }
            Vec3::from(light.color) * (light.intensity * n_dot_l)
        }
    }
}

/// Light-source position for shadow testing. For directional lights there is
/// no true position — the caller must march a long distance along the aim
/// vector.
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
            // 10 km along the aim vector is beyond any indoor map AABB.
            surface_point + to_light * 10_000.0
        }
    }
}

/// Distance attenuation for Point and Spot lights.
///
/// Must match `falloff` in `postretro/src/shaders/forward.wgsl` exactly —
/// the runtime direct path and the bake-time indirect projection share one
/// authored intensity, so any divergence here produces "ghost glow" or
/// missing bounce light at the light's edge of influence.
///
/// - Linear: `1 - d/range`, clamped to `[0, 1]`.
/// - InverseDistance: `1/d` with a hard zero past `range`. No upper clamp —
///   close-range values may exceed 1.0, exactly as the shader allows.
/// - InverseSquared: `1/d²` with a hard zero past `range`. Same no-upper-clamp
///   convention as InverseDistance.
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

/// Spot cone attenuation matching the shader's `cone_attenuation` in
/// `forward.wgsl` — Hermite cubic smoothstep between `cos_outer` and
/// `cos_inner`. The shader uses WGSL's built-in `smoothstep`, which is
/// `t² × (3 - 2t)` over the unit interval; we reproduce that here so direct
/// and indirect agree along the cone fringe.
fn spot_cone_attenuation(light: &MapLight, light_to_surface: Vec3) -> f32 {
    let dir = Vec3::from(light.cone_direction.unwrap_or([0.0, -1.0, 0.0])).normalize_or_zero();
    let inner = light.cone_angle_inner.unwrap_or(0.0);
    let outer = light.cone_angle_outer.unwrap_or(inner + 0.01);
    let cos_outer = outer.cos();
    let cos_inner = inner.cos();
    let cos_theta = dir.dot(light_to_surface.normalize_or_zero());
    smoothstep(cos_outer, cos_inner, cos_theta)
}

/// Hermite cubic smoothstep matching WGSL's `smoothstep(edge0, edge1, x)`.
/// Returns 0 below `edge0`, 1 above `edge1`, and `t² × (3 - 2t)` between.
fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0).max(1.0e-4)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

// ---------------------------------------------------------------------------
// SH L2 projection

/// Evaluate the first nine SH L2 basis functions at a unit-length direction.
/// Standard Ramamoorthi-Hanrahan convention (Condon-Shortley phase omitted —
/// we use the real-valued y basis).
fn sh_basis_l2(dir: Vec3) -> [f32; 9] {
    let x = dir.x;
    let y = dir.y;
    let z = dir.z;
    let mut b = [0f32; 9];
    // l = 0
    b[0] = 0.282_094_8; // 0.5 * sqrt(1/pi)
    // l = 1
    b[1] = -0.488_602_5 * y;
    b[2] = 0.488_602_5 * z;
    b[3] = -0.488_602_5 * x;
    // l = 2
    b[4] = 1.092_548_4 * x * y;
    b[5] = -1.092_548_4 * y * z;
    b[6] = 0.315_391_6 * (3.0 * z * z - 1.0);
    b[7] = -1.092_548_4 * x * z;
    b[8] = 0.546_274_2 * (x * x - y * y);
    b
}

/// Collect SH samples into a 9-band accumulator. `weight` scales each sample
/// by `4π / N` so the Monte Carlo estimator is unbiased for uniform sphere
/// sampling.
fn accumulate_sh(acc: &mut [f32; 9], dir: Vec3, value: f32, weight: f32) {
    let basis = sh_basis_l2(dir);
    for (a, b) in acc.iter_mut().zip(basis.iter()) {
        *a += *b * value * weight;
    }
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

// Cosine-lobe convolution coefficients (Ramamoorthi & Hanrahan 2001, eq. 11).
// Convert L2 radiance projection coefficients into L2 irradiance coefficients
// by multiplying each band by its zonal-harmonic cosine-lobe factor.
const COSINE_LOBE_L0: f32 = std::f32::consts::PI; // π
const COSINE_LOBE_L1: f32 = 2.0 * std::f32::consts::PI / 3.0; // 2π/3
const COSINE_LOBE_L2: f32 = std::f32::consts::PI * 0.25; // π/4

/// Fold the cosine-lobe convolution into the SH coefficients in-place. After
/// this step the coefficients reconstruct irradiance (not radiance) when
/// sampled in the shading-normal direction, so the runtime shader only
/// applies the SH basis — no per-fragment A_l multiply.
fn apply_cosine_lobe_rgb(acc: &mut [f32; 27]) {
    for band in 0..9 {
        let factor = cosine_lobe_factor(band);
        let base = band * 3;
        acc[base] *= factor;
        acc[base + 1] *= factor;
        acc[base + 2] *= factor;
    }
}

fn apply_cosine_lobe_mono(acc: &mut [f32; 9]) {
    for (band, v) in acc.iter_mut().enumerate() {
        *v *= cosine_lobe_factor(band);
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

// ---------------------------------------------------------------------------
// Probe bakes

/// Bake the static-light contribution at a probe and project into SH L2 RGB.
fn bake_probe_rgb(
    inputs: &BakeInputs<'_>,
    probe_pos: Vec3,
    static_lights: &[&MapLight],
) -> [f32; 27] {
    let directions = sphere_directions(RAYS_PER_PROBE, SAMPLING_LATTICE_OFFSET);
    let mc_weight = (4.0 * std::f32::consts::PI) / RAYS_PER_PROBE as f32;
    let mut acc = [0f32; 27];
    for dir in &directions {
        let radiance = sample_radiance_rgb(inputs, probe_pos, *dir, static_lights);
        accumulate_sh_rgb(&mut acc, *dir, radiance, mc_weight);
    }
    apply_cosine_lobe_rgb(&mut acc);
    acc
}

/// Bake a single animated light at unit intensity / white color and project
/// into monochrome SH L2. The runtime multiplies the 9-coeff layer by the
/// light's evaluated animation color and brightness per frame.
fn bake_probe_mono(inputs: &BakeInputs<'_>, probe_pos: Vec3, light: &MapLight) -> [f32; 9] {
    // Bake at unit intensity / white — the runtime applies base_color and
    // curves via the animation descriptor.
    let unit_light = unit_reference_light(light);
    let directions = sphere_directions(RAYS_PER_PROBE, SAMPLING_LATTICE_OFFSET);
    let mc_weight = (4.0 * std::f32::consts::PI) / RAYS_PER_PROBE as f32;
    let mut acc = [0f32; 9];
    for dir in &directions {
        let luminance = sample_radiance_mono(inputs, probe_pos, *dir, &unit_light);
        accumulate_sh(&mut acc, *dir, luminance, mc_weight);
    }
    apply_cosine_lobe_mono(&mut acc);
    acc
}

/// Trace a single ray from `origin` along `dir`, evaluate direct lighting at
/// the closest hit (or sky), and return the RGB radiance heading back toward
/// `origin`.
fn sample_radiance_rgb(
    inputs: &BakeInputs<'_>,
    origin: Vec3,
    dir: Vec3,
    lights: &[&MapLight],
) -> Vec3 {
    match closest_hit(inputs, origin + dir * RAY_EPSILON, dir, f32::INFINITY) {
        None => Vec3::from(SKY_COLOR),
        Some(hit) => {
            let mut radiance = Vec3::ZERO;
            for light in lights {
                if !shadow_visible(inputs, hit.point, hit.normal, light) {
                    continue;
                }
                radiance += light_contribution_lambert(light, hit.point, hit.normal);
            }
            radiance * BOUNCE_ALBEDO / std::f32::consts::PI
        }
    }
}

/// Same as `sample_radiance_rgb` but returns scalar luminance (average of
/// RGB) for a single light baked at unit intensity.
fn sample_radiance_mono(inputs: &BakeInputs<'_>, origin: Vec3, dir: Vec3, light: &MapLight) -> f32 {
    match closest_hit(inputs, origin + dir * RAY_EPSILON, dir, f32::INFINITY) {
        None => 0.0,
        Some(hit) => {
            if !shadow_visible(inputs, hit.point, hit.normal, light) {
                return 0.0;
            }
            let rgb = light_contribution_lambert(light, hit.point, hit.normal);
            let luminance = (rgb.x + rgb.y + rgb.z) / 3.0;
            luminance * BOUNCE_ALBEDO / std::f32::consts::PI
        }
    }
}

fn shadow_visible(
    inputs: &BakeInputs<'_>,
    surface_point: Vec3,
    surface_normal: Vec3,
    light: &MapLight,
) -> bool {
    if !light.cast_shadows {
        return true;
    }
    let shadow_origin = light_shadow_origin(light, surface_point);
    // Nudge the surface sample a tiny bit along the normal so the first
    // traversal step does not self-intersect the face that was just hit.
    let probe_end = surface_point + surface_normal * RAY_EPSILON;
    segment_clear(inputs, probe_end, shadow_origin)
}

/// Build an "identity-intensity" copy of a light for animated baking: unit
/// intensity, white base color, unit cone color, animation stripped.
fn unit_reference_light(original: &MapLight) -> MapLight {
    MapLight {
        color: [1.0, 1.0, 1.0],
        intensity: 1.0,
        animation: None,
        ..original.clone()
    }
}

// ---------------------------------------------------------------------------
// Animation descriptor

fn animation_descriptor_for(light: &MapLight) -> AnimationDescriptor {
    let anim: &LightAnimation = light
        .animation
        .as_ref()
        .expect("animation_descriptor_for called on a non-animated light");
    AnimationDescriptor {
        period: anim.period.max(1.0e-6),
        phase: anim.phase,
        base_color: light.color,
        brightness: anim.brightness.clone().unwrap_or_default(),
        color: anim.color.clone().unwrap_or_default(),
    }
}

// ---------------------------------------------------------------------------
// Tests

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bvh_build::{build_bvh, collect_primitives};
    use crate::geometry::FaceIndexRange;
    use crate::map_data::{FalloffModel, LightType};
    use crate::partition::{Aabb as CompilerAabb, BspLeaf, BspTree};
    use postretro_level_format::geometry::{FaceMeta, GeometrySection, Vertex};
    use postretro_level_format::texture_names::TextureNamesSection;

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
        }
    }

    // --- falloff: shader parity ---
    //
    // These pin the contract that `sh_bake::falloff` mirrors `falloff` in
    // `postretro/src/shaders/forward.wgsl`. A drift here produces "ghost
    // glow" — probes pick up indirect contribution from lights that the
    // direct pass has correctly culled — and that's exactly the bug the
    // first review caught after sub-plan 3 landed.

    #[test]
    fn linear_falloff_matches_shader_curve() {
        let light = point_light_with_falloff(FalloffModel::Linear, 10.0);
        // f(0) = 1, f(range) = 0, f(range/2) = 0.5
        assert!((falloff(&light, 0.0) - 1.0).abs() < 1e-6);
        assert!((falloff(&light, 5.0) - 0.5).abs() < 1e-6);
        assert!(falloff(&light, 10.0).abs() < 1e-6);
        // Past range stays clamped to zero.
        assert_eq!(falloff(&light, 15.0), 0.0);
    }

    #[test]
    fn inverse_distance_zeroes_past_range() {
        let light = point_light_with_falloff(FalloffModel::InverseDistance, 10.0);
        // Inside range: 1/d, no upper clamp.
        assert!((falloff(&light, 1.0) - 1.0).abs() < 1e-6);
        assert!((falloff(&light, 5.0) - 0.2).abs() < 1e-6);
        // Past range: hard zero — must match shader behavior.
        assert_eq!(falloff(&light, 10.001), 0.0);
        assert_eq!(falloff(&light, 50.0), 0.0);
    }

    #[test]
    fn inverse_squared_zeroes_past_range() {
        let light = point_light_with_falloff(FalloffModel::InverseSquared, 10.0);
        // Inside range: 1/d², close-range exceeds 1.0 deliberately.
        assert!((falloff(&light, 1.0) - 1.0).abs() < 1e-6);
        assert!((falloff(&light, 0.5) - 4.0).abs() < 1e-6);
        // Past range: hard zero.
        assert_eq!(falloff(&light, 10.001), 0.0);
        assert_eq!(falloff(&light, 100.0), 0.0);
    }

    #[test]
    fn smoothstep_matches_wgsl_definition() {
        // `t² × (3 - 2t)` over the unit interval, clamped at the edges.
        assert_eq!(smoothstep(0.0, 1.0, -0.5), 0.0);
        assert_eq!(smoothstep(0.0, 1.0, 1.5), 1.0);
        assert!((smoothstep(0.0, 1.0, 0.5) - 0.5).abs() < 1e-6); // exact midpoint
        // Symmetric around the midpoint.
        let lower = smoothstep(0.0, 1.0, 0.25);
        let upper = smoothstep(0.0, 1.0, 0.75);
        assert!((lower + upper - 1.0).abs() < 1e-6);
    }

    fn tri_vertex(pos: [f32; 3]) -> Vertex {
        Vertex::new(pos, [0.0, 0.0], [0.0, 1.0, 0.0], [1.0, 0.0, 0.0], true)
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
        // Single empty leaf tree: every point is classified as empty, so
        // validity flags depend only on the point-in-leaf walk which returns
        // leaf 0 for any input.
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
        let inputs = BakeInputs {
            bvh: &bvh,
            primitives: &primitives,
            geometry: &geo,
            tree: &tree,
            lights: &[],
        };
        let section = bake_sh_volume(&inputs, 1.0);
        assert_eq!(section.grid_dimensions, [0, 0, 0]);
        assert!(section.probes.is_empty());
    }

    #[test]
    fn grid_dimensions_cover_world_aabb() {
        // A triangle spanning 3 meters on x, 2 on z should need 4 x-cells,
        // 1 y-cell (the face is flat), and 3 z-cells at 1m spacing.
        let min = DVec3::new(0.0, 0.0, 0.0);
        let max = DVec3::new(3.0, 0.0, 2.0);
        let dims = grid_dimensions(min, max, 1.0);
        assert_eq!(dims, [4, 1, 3]);
    }

    #[test]
    fn probe_iteration_matches_z_major_convention() {
        let dims = [2, 3, 4];
        let total = 2 * 3 * 4;
        // The first probe is at grid origin; the last is at the far-corner.
        assert_eq!(probe_index_to_xyz(0, dims), (0, 0, 0));
        assert_eq!(probe_index_to_xyz(total - 1, dims), (1, 2, 3));
        // Stepping x advances first.
        assert_eq!(probe_index_to_xyz(1, dims), (1, 0, 0));
        // After the x row we advance y.
        assert_eq!(probe_index_to_xyz(2, dims), (0, 1, 0));
        // After the y×x plane we advance z.
        assert_eq!(probe_index_to_xyz(2 * 3, dims), (0, 0, 1));
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
        // Determinism: same input yields same output.
        let again = sphere_directions(32, 1);
        assert_eq!(dirs, again);
    }

    #[test]
    fn cosine_lobe_makes_constant_radiance_reconstruct_to_pi() {
        // Identity: for constant incident radiance L = 1, the diffuse
        // irradiance integral equals π. Project constant 1 onto signed L2
        // basis, apply cosine-lobe convolution, reconstruct with the same
        // signed basis — must land near π regardless of normal direction.
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
        // Band 0 is a constant over the sphere — every direction projects to
        // the same value.
        let b0 = sh_basis_l2(Vec3::X)[0];
        let b1 = sh_basis_l2(Vec3::Y)[0];
        let b2 = sh_basis_l2(Vec3::new(0.3, 0.4, 0.5).normalize())[0];
        assert!((b0 - b1).abs() < 1.0e-6);
        assert!((b0 - b2).abs() < 1.0e-6);
    }

    #[test]
    fn bake_is_deterministic() {
        // Two bakes of the same input must produce byte-identical sections.
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
        };
        let inputs = BakeInputs {
            bvh: &bvh,
            primitives: &prims,
            geometry: &geo,
            tree: &tree,
            lights: std::slice::from_ref(&light),
        };
        let a = bake_sh_volume(&inputs, 1.0);
        let b = bake_sh_volume(&inputs, 1.0);
        assert_eq!(a.to_bytes(), b.to_bytes());
    }

    #[test]
    fn animated_light_produces_dedicated_layer() {
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
            }),
            cast_shadows: true,
            bake_only: false,
        };
        let inputs = BakeInputs {
            bvh: &bvh,
            primitives: &prims,
            geometry: &geo,
            tree: &tree,
            lights: std::slice::from_ref(&animated),
        };
        let section = bake_sh_volume(&inputs, 1.0);
        assert_eq!(section.animation_descriptors.len(), 1);
        assert_eq!(section.per_light_sh.len(), 1);
        assert_eq!(section.animation_descriptors[0].period, 1.0);
        assert_eq!(
            section.animation_descriptors[0].base_color,
            [1.0, 0.5, 0.25]
        );
        assert_eq!(section.per_light_sh[0].len(), section.total_probes() * 9);
    }

    #[test]
    fn no_animated_lights_emits_static_only_layout() {
        let geo = one_triangle_geometry([[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]]);
        let (bvh, prims, _) = build_bvh(&geo).unwrap();
        let tree = tree_all_empty();
        let inputs = BakeInputs {
            bvh: &bvh,
            primitives: &prims,
            geometry: &geo,
            tree: &tree,
            lights: &[],
        };
        let section = bake_sh_volume(&inputs, 1.0);
        assert!(section.animation_descriptors.is_empty());
        assert!(section.per_light_sh.is_empty());
        // Round-trip the static-only layout just to be sure the header flag
        // is zero.
        let bytes = section.to_bytes();
        assert_eq!(&bytes[40..44], &0u32.to_le_bytes());
    }

    #[test]
    fn solid_probes_are_flagged_invalid() {
        // Build a BSP tree where every point is classified into a solid leaf.
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
        let inputs = BakeInputs {
            bvh: &bvh,
            primitives: &prims,
            geometry: &geo,
            tree: &tree,
            lights: &[],
        };
        let section = bake_sh_volume(&inputs, 1.0);
        for probe in &section.probes {
            assert_eq!(probe.validity, 0);
        }
    }

    #[test]
    fn sphere_directions_cover_sphere_integral() {
        // Sanity check: the integral of 1 over the sphere is 4π. The Monte
        // Carlo estimator at the sample count we use should converge on this
        // value for a constant integrand.
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
        // Double-check that the live Bvh returned by build_bvh is traversable
        // from our baker code, closing the loop on the "one BVH, two
        // consumers" pattern from Milestone 4.
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
