// Normal-free direct scatter baker for billboard shading.
// See: context/lib/build_pipeline.md §PRL section IDs

use bvh::ray::Ray;
use glam::Vec3;
use nalgebra::{Point3, Vector3};
use postretro_level_format::animated_billboard_direct_scatter_delta_volumes::{
    AnimatedBillboardDirectScatterDeltaVolumesSection,
    BILLBOARD_DIRECT_SCATTER_DELTA_RGBA_F16_COUNT,
    BILLBOARD_DIRECT_SCATTER_PROBES_PER_AFFINITY_ENTRY,
};
use postretro_level_format::animated_direct_sh_delta_volumes::AnimatedDirectShDeltaVolumesSection;
use postretro_level_format::billboard_direct_scatter_volume::{
    BILLBOARD_DIRECT_SCATTER_VALIDITY_ONE_F16, BillboardDirectScatterVolumeSection,
};
use postretro_level_format::lightmap::f32_to_f16_bits;
use rayon::prelude::*;

use crate::bake_control::BakeControl;
use crate::cache::{CacheKey, StageCache};
use crate::direct_sh_bake::{
    DirectBakeInputs, ReachIndex, build_reach_index, static_direct_lights,
};
use crate::light_namespaces::AnimatedBakedLights;
use crate::lightmap_bake::{DEFAULT_AREA_SAMPLE_COUNT, soft_visibility};
use crate::map_data::MapLight;
use crate::portals::Portal;
use crate::sh_bake::{
    ProbeGridLayout, RaytracingCtx, ShBakeCtx, ShConfig, incident_radiance_at_point,
    probe_grid_layout, static_light_refs, vec3_from,
};
use crate::sh_group::geometry_content_hash;

/// Cache stage for dense, normal-free static billboard scatter.
pub const BILLBOARD_DIRECT_SCATTER_STAGE_ID: &str = "billboard_direct_scatter";
/// Bump only when the static scatter calculation or its cached payload changes.
pub const BILLBOARD_DIRECT_SCATTER_STAGE_VERSION: u32 = 1;

/// Cache stage for dense, normal-free animated billboard scatter deltas.
pub const ANIMATED_BILLBOARD_DIRECT_SCATTER_STAGE_ID: &str = "animated_billboard_direct_scatter";
/// Bump only when the animated scatter calculation or its cached payload changes.
pub const ANIMATED_BILLBOARD_DIRECT_SCATTER_STAGE_VERSION: u32 = 1;

const RAY_EPSILON: f32 = 1.0e-3;
// Matches `sh_bake`'s direct-light seed with its ray axis fixed at zero, so a
// soft penumbra samples the same emitter lattice in normal-free scatter and
// direct-SH transport.
const SCATTER_SEED_OFFSET: u64 = 0x5048_4542_414B_4552; // "PHBAKER"

/// Shared geometry, light namespaces, and portal graph for both scatter bakes.
pub struct BillboardDirectScatterBakeInputs<'a, 'b> {
    pub sh_ctx: &'a ShBakeCtx<'b>,
    pub portals: &'a [Portal],
    pub animated_lights: &'a AnimatedBakedLights<'b>,
}

/// Bake static normal-free direct scatter. `None` means there is no
/// `static_light_map` source at all; callers must omit section 47 rather than
/// serializing an all-zero grid, which keeps legacy billboard lighting selected.
pub fn bake_billboard_direct_scatter_volume_cached_controlled(
    inputs: &BillboardDirectScatterBakeInputs<'_, '_>,
    config: &ShConfig,
    cache: Option<&StageCache>,
    control: &BakeControl,
) -> Option<BillboardDirectScatterVolumeSection> {
    let static_lights = static_light_refs(inputs.sh_ctx);
    let (direct_lights, global_indices) = static_direct_lights(&static_lights);
    if direct_lights.is_empty() {
        return None;
    }

    let layout = probe_grid_layout(inputs.sh_ctx, config);
    if layout.is_empty() {
        return None;
    }

    let direct_inputs = DirectBakeInputs {
        sh_ctx: inputs.sh_ctx,
        portals: inputs.portals,
    };
    let key = static_cache_key(
        inputs,
        &layout,
        &direct_lights,
        &global_indices,
        config.probe_spacing,
    );
    if let Some(cache) = cache {
        if let Some(bytes) = cache.get(&key) {
            match BillboardDirectScatterVolumeSection::from_bytes(&bytes) {
                Ok(section) => {
                    log::info!("[cache] billboard_direct_scatter hit");
                    control.publish_total(layout.total_probes());
                    control.governor().checkpoint();
                    control.advance(layout.total_probes());
                    return Some(section);
                }
                Err(error) => {
                    log::warn!(
                        "[cache] corrupt billboard_direct_scatter entry, re-baking: {error}"
                    );
                }
            }
        } else {
            log::info!("[cache] billboard_direct_scatter miss");
        }
    }

    let reach = build_reach_index(&direct_inputs, &direct_lights, config.probe_spacing);
    let section = bake_static_scatter(
        inputs,
        &layout,
        &direct_lights,
        &global_indices,
        &reach,
        control,
    );
    if let Some(cache) = cache {
        cache.put(&key, &section.to_bytes());
    }
    Some(section)
}

/// Bake unit-radiance animated scatter deltas. The supplied section-45 output
/// is the authority for descriptor mapping and CSR entry order; this function
/// copies that layout verbatim and only supplies the dense 4×4×4 RGB blocks.
pub fn bake_animated_billboard_direct_scatter_delta_volumes_cached_controlled(
    inputs: &BillboardDirectScatterBakeInputs<'_, '_>,
    config: &ShConfig,
    direct_deltas: &AnimatedDirectShDeltaVolumesSection,
    cache: Option<&StageCache>,
    control: &BakeControl,
) -> Option<AnimatedBillboardDirectScatterDeltaVolumesSection> {
    let layout = probe_grid_layout(inputs.sh_ctx, config);
    if layout.is_empty() {
        return None;
    }
    if direct_deltas.animation_descriptor_indices.len() != inputs.animated_lights.len() {
        log::warn!(
            "[Compiler] AnimatedBillboardDirectScatterDeltaVolumes skipped: section-45 descriptor count {} does not match {} animated baked lights",
            direct_deltas.animation_descriptor_indices.len(),
            inputs.animated_lights.len(),
        );
        return None;
    }
    if direct_deltas
        .affinity_lights
        .iter()
        .any(|&index| index as usize >= inputs.animated_lights.len())
    {
        log::warn!(
            "[Compiler] AnimatedBillboardDirectScatterDeltaVolumes skipped: section-45 CSR references an out-of-range animated baked light"
        );
        return None;
    }

    let key = animated_cache_key(inputs, &layout, config.probe_spacing, direct_deltas);
    if let Some(cache) = cache {
        if let Some(bytes) = cache.get(&key) {
            match AnimatedBillboardDirectScatterDeltaVolumesSection::from_bytes(&bytes) {
                Ok(section) if animated_layout_matches(direct_deltas, &section) => {
                    log::info!("[cache] animated_billboard_direct_scatter hit");
                    control.publish_total(direct_deltas.affinity_lights.len());
                    control.governor().checkpoint();
                    control.advance(direct_deltas.affinity_lights.len());
                    return Some(section);
                }
                Ok(_) => {
                    log::warn!(
                        "[cache] animated_billboard_direct_scatter layout mismatch, re-baking"
                    );
                }
                Err(error) => {
                    log::warn!(
                        "[cache] corrupt animated_billboard_direct_scatter entry, re-baking: {error}"
                    );
                }
            }
        } else {
            log::info!("[cache] animated_billboard_direct_scatter miss");
        }
    }

    let affinity_cell_count = direct_deltas.affinity_offsets.len().saturating_sub(1);
    let cells = csr_entry_cells(&direct_deltas.affinity_offsets);
    control.publish_total(direct_deltas.affinity_lights.len());
    let entries = inputs.animated_lights.entries();
    let delta_rgba: Vec<u16> = direct_deltas
        .affinity_lights
        .par_iter()
        .zip(cells.par_iter())
        .flat_map(|(&animated_index, &affinity_cell)| {
            let _permit = control.governor().enter();
            let entry = &entries[animated_index as usize];
            let block = bake_animated_scatter_block(
                inputs,
                &layout,
                entry.light,
                entry.source_index as u64,
                affinity_cell,
                direct_deltas.affinity_dims,
            );
            control.advance(1);
            block
        })
        .collect();
    let section = AnimatedBillboardDirectScatterDeltaVolumesSection {
        animation_descriptor_indices: direct_deltas.animation_descriptor_indices.clone(),
        affinity_cell_count: affinity_cell_count as u32,
        affinity_offsets: direct_deltas.affinity_offsets.clone(),
        affinity_lights: direct_deltas.affinity_lights.clone(),
        delta_rgba,
    };
    debug_assert!(animated_layout_matches(direct_deltas, &section));
    debug_assert_eq!(
        section.delta_rgba.len(),
        direct_deltas.affinity_lights.len()
            * BILLBOARD_DIRECT_SCATTER_PROBES_PER_AFFINITY_ENTRY
            * BILLBOARD_DIRECT_SCATTER_DELTA_RGBA_F16_COUNT,
    );
    if let Some(cache) = cache {
        cache.put(&key, &section.to_bytes());
    }
    Some(section)
}

fn bake_static_scatter(
    inputs: &BillboardDirectScatterBakeInputs<'_, '_>,
    layout: &ProbeGridLayout,
    direct_lights: &[&MapLight],
    global_indices: &[u64],
    reach: &ReachIndex,
    control: &BakeControl,
) -> BillboardDirectScatterVolumeSection {
    let dims = layout.dims;
    let nx = dims[0] as usize;
    let ny = dims[1] as usize;
    control.publish_total(layout.total_probes());
    let scatter_rgba: Vec<u16> = (0..layout.total_probes())
        .into_par_iter()
        .flat_map(|probe_index| {
            let _permit = control.governor().enter();
            let rgba = static_probe_scatter(
                inputs,
                layout,
                direct_lights,
                global_indices,
                reach,
                probe_index,
                nx,
                ny,
            );
            control.advance(1);
            rgba
        })
        .collect();
    BillboardDirectScatterVolumeSection {
        grid_origin: [
            layout.world_min.x as f32,
            layout.world_min.y as f32,
            layout.world_min.z as f32,
        ],
        cell_size: layout.cell_size,
        grid_dimensions: dims,
        scatter_rgba,
    }
}

#[allow(clippy::too_many_arguments)]
fn static_probe_scatter(
    inputs: &BillboardDirectScatterBakeInputs<'_, '_>,
    layout: &ProbeGridLayout,
    direct_lights: &[&MapLight],
    global_indices: &[u64],
    reach: &ReachIndex,
    probe_index: usize,
    nx: usize,
    ny: usize,
) -> [u16; 4] {
    if layout.validity[probe_index] == 0 {
        return [0; 4];
    }
    let (px, py, pz) = probe_coordinates(probe_index, nx, ny);
    let reaching = reach
        .cell_lights
        .get(reach.cell_for_probe(px, py, pz))
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let lights: Vec<&MapLight> = reaching
        .iter()
        .map(|&index| direct_lights[index as usize])
        .collect();
    let seeds: Vec<u64> = reaching
        .iter()
        .map(|&index| global_indices[index as usize])
        .collect();
    let ray_ctx = RaytracingCtx {
        bvh: inputs.sh_ctx.bvh,
        primitives: inputs.sh_ctx.primitives,
        geometry: inputs.sh_ctx.geometry,
    };
    encode_static_scatter(
        vec3_from(layout.probe_positions[probe_index]),
        &lights,
        &seeds,
        probe_index as u64,
        |from, to| segment_clear(&ray_ctx, from, to),
    )
}

fn bake_animated_scatter_block(
    inputs: &BillboardDirectScatterBakeInputs<'_, '_>,
    layout: &ProbeGridLayout,
    light: &MapLight,
    source_index: u64,
    affinity_cell: u32,
    affinity_dims: [u32; 3],
) -> Vec<u16> {
    let cell_x = affinity_cell % affinity_dims[0];
    let cell_y = (affinity_cell / affinity_dims[0]) % affinity_dims[1];
    let cell_z = affinity_cell / (affinity_dims[0] * affinity_dims[1]);
    let nx = layout.dims[0] as usize;
    let ny = layout.dims[1] as usize;
    let ray_ctx = RaytracingCtx {
        bvh: inputs.sh_ctx.bvh,
        primitives: inputs.sh_ctx.primitives,
        geometry: inputs.sh_ctx.geometry,
    };
    let mut unit_light = light.clone();
    unit_light.intensity = 1.0;
    unit_light.color = [1.0; 3];

    let mut out = Vec::with_capacity(
        BILLBOARD_DIRECT_SCATTER_PROBES_PER_AFFINITY_ENTRY
            * BILLBOARD_DIRECT_SCATTER_DELTA_RGBA_F16_COUNT,
    );
    for local_z in 0..4 {
        for local_y in 0..4 {
            for local_x in 0..4 {
                let px = cell_x * 4 + local_x;
                let py = cell_y * 4 + local_y;
                let pz = cell_z * 4 + local_z;
                let rgba = if px < layout.dims[0] && py < layout.dims[1] && pz < layout.dims[2] {
                    let probe_index = pz as usize * nx * ny + py as usize * nx + px as usize;
                    if layout.validity[probe_index] == 0 {
                        [0; 4]
                    } else {
                        encode_animated_scatter(
                            vec3_from(layout.probe_positions[probe_index]),
                            &unit_light,
                            source_index,
                            probe_index as u64,
                            |from, to| segment_clear(&ray_ctx, from, to),
                        )
                    }
                } else {
                    [0; 4]
                };
                out.extend_from_slice(&rgba);
            }
        }
    }
    out
}

fn encode_static_scatter(
    probe: Vec3,
    lights: &[&MapLight],
    global_indices: &[u64],
    probe_index: u64,
    trace: impl Fn(Vec3, Vec3) -> bool,
) -> [u16; 4] {
    let radiance = visible_radiance_sum(probe, lights, global_indices, probe_index, trace);
    [
        f32_to_f16_bits(radiance.x),
        f32_to_f16_bits(radiance.y),
        f32_to_f16_bits(radiance.z),
        BILLBOARD_DIRECT_SCATTER_VALIDITY_ONE_F16,
    ]
}

fn encode_animated_scatter(
    probe: Vec3,
    light: &MapLight,
    source_index: u64,
    probe_index: u64,
    trace: impl Fn(Vec3, Vec3) -> bool,
) -> [u16; 4] {
    let radiance = visible_radiance_sum(probe, &[light], &[source_index], probe_index, trace);
    [
        f32_to_f16_bits(radiance.x),
        f32_to_f16_bits(radiance.y),
        f32_to_f16_bits(radiance.z),
        0,
    ]
}

/// Direct radiance without a surface cosine or SH projection. `light` remains
/// the only authority for point falloff, spotlight cones, and directional
/// intensity; `soft_visibility` remains the shadow authority.
fn visible_radiance_sum(
    probe: Vec3,
    lights: &[&MapLight],
    global_indices: &[u64],
    probe_index: u64,
    trace: impl Fn(Vec3, Vec3) -> bool,
) -> Vec3 {
    let mut sum = Vec3::ZERO;
    for (&light, &global_index) in lights.iter().zip(global_indices) {
        let Some((radiance, direction)) = incident_radiance_at_point(light, probe) else {
            continue;
        };
        let visibility = soft_visibility(
            probe,
            direction,
            light,
            scatter_visibility_seed(probe_index, global_index),
            DEFAULT_AREA_SAMPLE_COUNT,
            &trace,
        );
        sum += radiance * visibility;
    }
    sum
}

fn probe_coordinates(probe_index: usize, nx: usize, ny: usize) -> (u32, u32, u32) {
    let pz = (probe_index / (nx * ny)) as u32;
    let remainder = probe_index - pz as usize * nx * ny;
    let py = (remainder / nx) as u32;
    let px = (remainder - py as usize * nx) as u32;
    (px, py, pz)
}

fn csr_entry_cells(offsets: &[u32]) -> Vec<u32> {
    let mut cells = Vec::with_capacity(offsets.last().copied().unwrap_or_default() as usize);
    for (cell, pair) in offsets.windows(2).enumerate() {
        cells.extend(std::iter::repeat_n(
            cell as u32,
            (pair[1] - pair[0]) as usize,
        ));
    }
    cells
}

fn animated_layout_matches(
    direct: &AnimatedDirectShDeltaVolumesSection,
    scatter: &AnimatedBillboardDirectScatterDeltaVolumesSection,
) -> bool {
    scatter.animation_descriptor_indices == direct.animation_descriptor_indices
        && scatter.affinity_cell_count as usize == direct.affinity_offsets.len().saturating_sub(1)
        && scatter.affinity_offsets == direct.affinity_offsets
        && scatter.affinity_lights == direct.affinity_lights
}

fn static_cache_key(
    inputs: &BillboardDirectScatterBakeInputs<'_, '_>,
    layout: &ProbeGridLayout,
    lights: &[&MapLight],
    global_indices: &[u64],
    probe_spacing: f32,
) -> CacheKey {
    let mut hasher = common_cache_hasher(inputs, layout, probe_spacing);
    hasher.update(&(lights.len() as u32).to_le_bytes());
    for (light, &global_index) in lights.iter().zip(global_indices) {
        hasher.update(&global_index.to_le_bytes());
        let encoded = postcard::to_allocvec(*light).expect("postcard serialize MapLight");
        hasher.update(&(encoded.len() as u32).to_le_bytes());
        hasher.update(&encoded);
    }
    CacheKey::new(
        BILLBOARD_DIRECT_SCATTER_STAGE_ID,
        BILLBOARD_DIRECT_SCATTER_STAGE_VERSION,
        hasher.finalize().as_bytes(),
    )
}

fn animated_cache_key(
    inputs: &BillboardDirectScatterBakeInputs<'_, '_>,
    layout: &ProbeGridLayout,
    probe_spacing: f32,
    direct_deltas: &AnimatedDirectShDeltaVolumesSection,
) -> CacheKey {
    let mut hasher = common_cache_hasher(inputs, layout, probe_spacing);
    hasher.update(&(inputs.animated_lights.len() as u32).to_le_bytes());
    for entry in inputs.animated_lights.entries() {
        hasher.update(&(entry.source_index as u64).to_le_bytes());
        let mut unit_light = entry.light.clone();
        unit_light.intensity = 1.0;
        unit_light.color = [1.0; 3];
        let encoded = postcard::to_allocvec(&unit_light).expect("postcard serialize MapLight");
        hasher.update(&(encoded.len() as u32).to_le_bytes());
        hasher.update(&encoded);
    }
    // Keep this cache tied to the final id-45 layout, but not its SH tile
    // payload: scatter reuses the descriptor/CSR contract, not SH coefficients.
    for descriptor in &direct_deltas.animation_descriptor_indices {
        hasher.update(&descriptor.to_le_bytes());
    }
    for offset in &direct_deltas.affinity_offsets {
        hasher.update(&offset.to_le_bytes());
    }
    for light in &direct_deltas.affinity_lights {
        hasher.update(&light.to_le_bytes());
    }
    CacheKey::new(
        ANIMATED_BILLBOARD_DIRECT_SCATTER_STAGE_ID,
        ANIMATED_BILLBOARD_DIRECT_SCATTER_STAGE_VERSION,
        hasher.finalize().as_bytes(),
    )
}

fn common_cache_hasher(
    inputs: &BillboardDirectScatterBakeInputs<'_, '_>,
    layout: &ProbeGridLayout,
    probe_spacing: f32,
) -> blake3::Hasher {
    let mut hasher = blake3::Hasher::new();
    hasher.update(&geometry_content_hash(inputs.sh_ctx.geometry));
    hasher.update(&probe_spacing.to_le_bytes());
    for value in [layout.world_min.x, layout.world_min.y, layout.world_min.z] {
        hasher.update(&value.to_le_bytes());
    }
    for value in &layout.cell_size {
        hasher.update(&value.to_le_bytes());
    }
    for value in &layout.dims {
        hasher.update(&value.to_le_bytes());
    }
    hasher.update(&layout.validity);
    hasher.update(&(inputs.portals.len() as u32).to_le_bytes());
    for portal in inputs.portals {
        hasher.update(&(portal.front_leaf as u64).to_le_bytes());
        hasher.update(&(portal.back_leaf as u64).to_le_bytes());
    }
    hasher
}

fn scatter_visibility_seed(probe_index: u64, light_index: u64) -> u64 {
    let mut value = SCATTER_SEED_OFFSET
        ^ probe_index.wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ light_index.wrapping_mul(0x94D0_49BB_1331_11EB);
    value = (value ^ (value >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    value ^ (value >> 31)
}

fn segment_clear(ctx: &RaytracingCtx<'_>, from: Vec3, to: Vec3) -> bool {
    let delta = to - from;
    let length = delta.length();
    if length < RAY_EPSILON {
        return true;
    }
    let direction = delta / length;
    let origin = from + direction * RAY_EPSILON;
    let ray = Ray::new(
        Point3::new(origin.x, origin.y, origin.z),
        Vector3::new(direction.x, direction.y, direction.z),
    );
    let max_distance = length - RAY_EPSILON;
    let geometry = &ctx.geometry.geometry;
    for primitive in ctx.bvh.traverse_iterator(&ray, ctx.primitives) {
        let mut triangle = primitive.index_offset as usize;
        let end = triangle + primitive.index_count as usize;
        while triangle + 3 <= end {
            let a = Vec3::from(geometry.vertices[geometry.indices[triangle] as usize].position);
            let b = Vec3::from(geometry.vertices[geometry.indices[triangle + 1] as usize].position);
            let c = Vec3::from(geometry.vertices[geometry.indices[triangle + 2] as usize].position);
            triangle += 3;
            if let Some(distance) = ray_triangle_distance(origin, direction, a, b, c)
                && distance > 0.0
                && distance < max_distance
            {
                return false;
            }
        }
    }
    true
}

fn ray_triangle_distance(origin: Vec3, direction: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Option<f32> {
    let edge_one = b - a;
    let edge_two = c - a;
    let h = direction.cross(edge_two);
    let determinant = edge_one.dot(h);
    if determinant.abs() < 1.0e-8 {
        return None;
    }
    let inverse = 1.0 / determinant;
    let s = origin - a;
    let u = inverse * s.dot(h);
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = s.cross(edge_one);
    let v = inverse * direction.dot(q);
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let distance = inverse * edge_two.dot(q);
    (distance > 0.0).then_some(distance)
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use glam::DVec3;
    use postretro_level_format::geometry::{FaceMeta, GeometrySection, Vertex};
    use postretro_level_format::texture_names::TextureNamesSection;

    use super::*;
    use crate::bvh_build::build_bvh;
    use crate::geometry::{FaceIndexRange, GeometryResult};
    use crate::light_namespaces::StaticBakedLights;
    use crate::map_data::{FalloffModel, LightAnimation, LightType, ShadowType};
    use crate::partition::{Aabb as CompilerAabb, BspLeaf, BspTree};

    fn light(light_type: LightType) -> MapLight {
        MapLight {
            origin: DVec3::new(0.0, 0.0, 4.0),
            carrier: String::new(),
            light_type,
            intensity: 2.0,
            color: [0.5, 1.0, 0.25],
            falloff_model: FalloffModel::Linear,
            falloff_range: 10.0,
            light_size: 0.0,
            angular_diameter: 0.0,
            cone_angle_inner: None,
            cone_angle_outer: None,
            cone_direction: Some([0.0, 0.0, -1.0]),
            animation: None,
            bake_only: false,
            is_dynamic: false,
            casts_entity_shadows: false,
            is_animated: false,
            tags: Vec::new(),
            shadow_type: ShadowType::StaticLightMap,
        }
    }

    fn assert_vec3_close(actual: Vec3, expected: Vec3) {
        assert!(
            actual.abs_diff_eq(expected, 1.0e-6),
            "expected {expected:?}, got {actual:?}"
        );
    }

    fn cube_geometry() -> GeometryResult {
        let positions = [
            [-1.0, -1.0, -1.0],
            [1.0, -1.0, -1.0],
            [1.0, 1.0, -1.0],
            [-1.0, 1.0, -1.0],
            [-1.0, -1.0, 1.0],
            [1.0, -1.0, 1.0],
            [1.0, 1.0, 1.0],
            [-1.0, 1.0, 1.0],
        ];
        GeometryResult {
            geometry: GeometrySection {
                vertices: positions
                    .into_iter()
                    .map(|position| {
                        Vertex::new(
                            position,
                            [0.0, 0.0],
                            [0.0, 1.0, 0.0],
                            [1.0, 0.0, 0.0],
                            true,
                            [0.0, 0.0],
                            0,
                        )
                    })
                    .collect(),
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

    fn empty_tree() -> BspTree {
        BspTree {
            nodes: Vec::new(),
            leaves: vec![BspLeaf {
                face_indices: Vec::new(),
                bounds: CompilerAabb {
                    min: DVec3::splat(-100.0),
                    max: DVec3::splat(100.0),
                },
                is_solid: false,
                defining_planes: Vec::new(),
            }],
        }
    }

    #[test]
    fn visible_scatter_sums_normal_free_point_spot_and_directional_radiance() {
        let point = light(LightType::Point);
        let mut spot = light(LightType::Spot);
        spot.origin = DVec3::new(0.0, 0.0, 3.0);
        spot.cone_angle_inner = Some(0.2);
        spot.cone_angle_outer = Some(0.4);
        let mut directional = light(LightType::Directional);
        directional.origin = DVec3::ZERO;
        directional.cone_direction = Some([0.0, -1.0, 0.0]);

        let probe = Vec3::ZERO;
        let lights = [&point, &spot, &directional];
        let expected = lights.iter().fold(Vec3::ZERO, |sum, light| {
            sum + incident_radiance_at_point(light, probe)
                .expect("fixture must reach probe")
                .0
        });
        let actual = visible_radiance_sum(probe, &lights, &[0, 1, 2], 0, |_, _| true);

        assert_vec3_close(actual, expected);
    }

    #[test]
    fn visible_scatter_is_zero_for_an_occluded_light() {
        let point = light(LightType::Point);
        let actual = visible_radiance_sum(Vec3::ZERO, &[&point], &[0], 0, |_, _| false);

        assert_eq!(actual, Vec3::ZERO);
    }

    #[test]
    fn visible_scatter_bake_trace_blocks_an_occluded_light() {
        let geometry = cube_geometry();
        let (bvh, primitives, _) = build_bvh(&geometry).expect("fixture geometry must build a BVH");
        let trace_ctx = RaytracingCtx {
            bvh: &bvh,
            primitives: &primitives,
            geometry: &geometry,
        };
        let mut point = light(LightType::Point);
        point.origin = DVec3::new(0.5, -0.5, 0.0);
        let probe = Vec3::new(0.5, -0.5, -2.0);

        let actual = visible_radiance_sum(probe, &[&point], &[0], 0, |from, to| {
            segment_clear(&trace_ctx, from, to)
        });

        assert_eq!(actual, Vec3::ZERO);
    }

    #[test]
    fn visible_scatter_retains_falloff_and_spot_cone_rejection() {
        let mut out_of_range = light(LightType::Point);
        out_of_range.origin = DVec3::new(0.0, 0.0, 20.0);
        let mut outside_cone = light(LightType::Spot);
        outside_cone.origin = DVec3::new(0.0, 0.0, 3.0);
        outside_cone.cone_angle_inner = Some(0.1);
        outside_cone.cone_angle_outer = Some(0.2);
        outside_cone.cone_direction = Some([1.0, 0.0, 0.0]);

        let actual = visible_radiance_sum(
            Vec3::ZERO,
            &[&out_of_range, &outside_cone],
            &[0, 1],
            0,
            |_, _| true,
        );
        assert_eq!(actual, Vec3::ZERO);
    }

    #[test]
    fn animated_scatter_layout_copies_section_45_exactly() {
        let direct = AnimatedDirectShDeltaVolumesSection {
            affinity_factor: 4,
            affinity_dims: [2, 1, 1],
            tile_dimension: 6,
            tile_border: 1,
            animation_descriptor_indices: vec![0, u32::MAX],
            valid_probe_masks: vec![u64::MAX; 2],
            cell_levels: vec![0; 2],
            affinity_offsets: vec![0, 1, 2],
            affinity_lights: vec![0, 1],
            delta_subblocks: Vec::new(),
        };
        let scatter = AnimatedBillboardDirectScatterDeltaVolumesSection {
            animation_descriptor_indices: direct.animation_descriptor_indices.clone(),
            affinity_cell_count: 2,
            affinity_offsets: direct.affinity_offsets.clone(),
            affinity_lights: direct.affinity_lights.clone(),
            delta_rgba: vec![0; 2 * 64 * 4],
        };

        assert!(animated_layout_matches(&direct, &scatter));
        assert_eq!(scatter.delta_rgba.len(), 2 * 64 * 4);
        assert!(scatter.delta_rgba.chunks_exact(4).all(|rgba| rgba[3] == 0));
    }

    #[test]
    fn animated_scatter_bake_preserves_final_section_45_mapping_and_unit_scale() {
        let mut animated = light(LightType::Point);
        animated.animation = Some(LightAnimation {
            period: 1.0,
            phase: 0.0,
            brightness: Some(vec![0.0, 1.0]),
            color: Some(vec![[0.5, 1.0, 0.25]]),
            direction: None,
            start_active: true,
        });
        let lights = vec![animated];
        let geometry = cube_geometry();
        let (bvh, primitives, _) = build_bvh(&geometry).expect("fixture geometry must build a BVH");
        let tree = empty_tree();
        let exterior = HashSet::new();
        let static_lights = StaticBakedLights::from_lights(&lights);
        let animated_lights = AnimatedBakedLights::from_lights(&lights);
        let sh_ctx = ShBakeCtx {
            bvh: &bvh,
            primitives: &primitives,
            geometry: &geometry,
            tree: &tree,
            exterior_leaves: &exterior,
            static_lights: &static_lights,
            animated_lights: &animated_lights,
            total_light_count: lights.len(),
        };
        let inputs = BillboardDirectScatterBakeInputs {
            sh_ctx: &sh_ctx,
            portals: &[],
            animated_lights: &animated_lights,
        };
        let direct = AnimatedDirectShDeltaVolumesSection {
            affinity_factor: 4,
            affinity_dims: [1, 1, 1],
            tile_dimension: 6,
            tile_border: 1,
            animation_descriptor_indices: vec![0],
            valid_probe_masks: vec![u64::MAX],
            cell_levels: vec![0],
            affinity_offsets: vec![0, 1],
            affinity_lights: vec![0],
            delta_subblocks: Vec::new(),
        };

        let scatter = bake_animated_billboard_direct_scatter_delta_volumes_cached_controlled(
            &inputs,
            &ShConfig { probe_spacing: 1.0 },
            &direct,
            None,
            &BakeControl::unrestricted(),
        )
        .expect("a valid final section-45 mapping must bake dense scatter deltas");

        assert!(animated_layout_matches(&direct, &scatter));
        assert_eq!(scatter.delta_rgba.len(), 64 * 4);
        assert!(scatter.delta_rgba.chunks_exact(4).all(|rgba| rgba[3] == 0));
        assert!(
            scatter
                .delta_rgba
                .chunks_exact(4)
                .any(|rgba| rgba[..3].iter().any(|&channel| channel != 0)),
            "unit-scale animated scatter must retain visible transport"
        );
    }

    #[test]
    fn static_scatter_omission_is_source_based_not_zero_grid_based() {
        let mut sdf = light(LightType::Point);
        sdf.shadow_type = ShadowType::Sdf;
        let static_lights = vec![&sdf];
        let (direct, _) = static_direct_lights(&static_lights);
        assert!(direct.is_empty());
    }
}
