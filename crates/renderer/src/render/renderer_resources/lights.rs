// Level light install: the filtered light records and their buffers, the
// forward lighting bind group, and the mesh and mover group-2 rebinds.
// See: context/lib/rendering_pipeline.md §4

use super::super::renderer_types::FullRenderer;
use super::*;
use crate::render::gpu_create::GpuCreate;

/// The level's filtered light sets, handed from the light upload to the shadow
/// cache reset and the lighting bind group.
pub(super) struct LevelLightSelection<'g> {
    pub(super) dynamic_influences: Vec<LightInfluence>,
    pub(super) animated_baked_descriptor_indices: &'g [u32],
    pub(super) animated_baked_candidates: Vec<AnimatedBakedShadowCandidate>,
    pub(super) shadow_candidate_lights: Vec<MapLight>,
    pub(super) shadow_candidate_influences: Vec<LightInfluence>,
    pub(super) shadow_candidate_source_indices: Vec<usize>,
    pub(super) shadow_candidate_selection_indices: Vec<Option<usize>>,
    pub(super) shadow_candidate_animated_baked_indices: Vec<Option<usize>>,
    pub(super) level_light_count: usize,
    pub(super) selected_static_count: usize,
    pub(super) dynamic_light_capacity: usize,
}

/// One level's direct-light records and their storage buffer.
pub(super) struct LevelLights {
    pub(super) lights_buffer: wgpu::Buffer,
    pub(super) light_count: u32,
    pub(super) animated_baked_light_count: usize,
    pub(super) total_light_count: u32,
    pub(super) dynamic_light_capacity: usize,
    pub(super) level_lights: Vec<MapLight>,
    pub(super) level_light_source_indices: Vec<usize>,
    pub(super) entity_shadow_lights: Vec<MapLight>,
    pub(super) entity_shadow_light_influences: Vec<LightInfluence>,
    pub(super) entity_shadow_light_source_indices: Vec<usize>,
    pub(super) entity_shadow_spec_light_indices: Vec<u32>,
}

/// One level's shadow candidates, influence buffer, and the forward and
/// mirror lighting groups over its light buffers.
pub(super) struct LevelLighting {
    pub(super) shadow_candidate_lights: Vec<MapLight>,
    pub(super) shadow_candidate_source_indices: Vec<usize>,
    pub(super) shadow_candidate_influences: Vec<LightInfluence>,
    pub(super) shadow_candidate_selection_indices: Vec<Option<usize>>,
    pub(super) shadow_candidate_animated_baked_indices: Vec<Option<usize>>,
    pub(super) influence_buffer: wgpu::Buffer,
    pub(super) level_light_influences: Vec<LightInfluence>,
    pub(super) lighting_bind_group: wgpu::BindGroup,
    pub(super) mirror_lighting: renderer_mirror_lights::MirrorLighting,
}

/// The lighting bind group's spec-light and chunk-grid buffers, which the SDF
/// shadow rebind binds again later in the install.
pub(super) struct LevelLightBuffers {
    pub(super) spec_lights: wgpu::Buffer,
    pub(super) chunk_grid_info: wgpu::Buffer,
    pub(super) chunk_grid_offsets: wgpu::Buffer,
    pub(super) chunk_grid_indices: wgpu::Buffer,
}

/// Filter the level's lights into the dynamic, selected-static and shadow
/// candidate sets, and upload the direct-light records.
pub(super) fn install_level_lights<'g>(
    device: &wgpu::Device,
    geometry: &'g LevelGeometry<'_>,
) -> (LevelLights, LevelLightSelection<'g>) {
    // --- Lights + lighting bind group ---
    let filtered_level_lights = filter_dynamic_lights(geometry.lights, geometry.light_influences);
    let level_lights = filtered_level_lights.lights;
    let dynamic_influences = filtered_level_lights.influences;
    let level_light_source_indices = filtered_level_lights.source_indices;
    let selected_static = filter_selected_static_entity_shadow_lights(
        geometry.lights,
        geometry.light_influences,
        geometry.entity_shadow_lights,
    );
    let (animated_baked_descriptor_indices, animated_baked_affinity_lights) =
        geometry.animated_baked_roster();
    let animated_baked_candidates = animated_baked_shadow_candidates_with_direct_delta(
        geometry.lights,
        geometry.light_influences,
        animated_baked_descriptor_indices,
        animated_baked_affinity_lights,
    );
    let filtered_shadow_candidates = filter_entity_shadow_candidates_with_selection(
        geometry.lights,
        geometry.light_influences,
        geometry.entity_shadow_lights,
        &animated_baked_candidates,
    );
    let shadow_candidate_lights = filtered_shadow_candidates.lights;
    let shadow_candidate_influences = filtered_shadow_candidates.influences;
    let shadow_candidate_source_indices = filtered_shadow_candidates.source_indices;
    let shadow_candidate_selection_indices = filtered_shadow_candidates.selection_indices;
    let shadow_candidate_animated_baked_indices = filtered_shadow_candidates.animated_baked_indices;
    let light_count = level_lights.len() as u32;
    let animated_baked_light_count = animated_baked_descriptor_indices.len();
    let level_light_count = level_lights.len();
    let selected_static_count = selected_static.lights.len();
    let dynamic_light_capacity = level_light_count + RUNTIME_DYNAMIC_LIGHT_RESERVE;

    let light_record_capacity =
        (dynamic_light_capacity + animated_baked_light_count + selected_static_count).max(1);
    let mut lights_data = Vec::with_capacity(light_record_capacity * GPU_LIGHT_SIZE);
    if !level_lights.is_empty() {
        lights_data.extend_from_slice(&pack_lights(&level_lights));
    }
    lights_data.resize(light_record_capacity * GPU_LIGHT_SIZE, 0);
    let lights_buffer = device.new_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("Direct Lights Storage Buffer"),
        contents: &lights_data,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
    });
    let lights = LevelLights {
        lights_buffer,
        light_count,
        animated_baked_light_count,
        total_light_count: light_count + animated_baked_light_count as u32,
        dynamic_light_capacity,
        level_lights,
        level_light_source_indices,
        entity_shadow_lights: selected_static.lights,
        entity_shadow_light_influences: selected_static.influences,
        entity_shadow_light_source_indices: selected_static.source_indices,
        entity_shadow_spec_light_indices: shadowmask::build_selection_spec_light_indices(
            geometry.lights,
            geometry.entity_shadow_lights,
        ),
    };

    let selection = LevelLightSelection {
        dynamic_influences,
        animated_baked_descriptor_indices,
        animated_baked_candidates,
        shadow_candidate_lights,
        shadow_candidate_influences,
        shadow_candidate_source_indices,
        shadow_candidate_selection_indices,
        shadow_candidate_animated_baked_indices,
        level_light_count,
        selected_static_count,
        dynamic_light_capacity,
    };
    (lights, selection)
}

/// Store the shadow candidates, upload the influence, spec-light and chunk-grid
/// buffers, and rebuild the forward and mirror lighting groups over them.
pub(super) fn install_lighting_bind_group(
    device: &wgpu::Device,
    lighting_bind_group_layout: &wgpu::BindGroupLayout,
    lights: &LevelLights,
    geometry: &LevelGeometry<'_>,
    selection: LevelLightSelection<'_>,
) -> (LevelLighting, LevelLightBuffers) {
    let LevelLightSelection {
        dynamic_influences,
        shadow_candidate_lights,
        shadow_candidate_influences,
        shadow_candidate_source_indices,
        shadow_candidate_selection_indices,
        shadow_candidate_animated_baked_indices,
        selected_static_count,
        dynamic_light_capacity,
        ..
    } = selection;
    let influence_record_capacity = shadowmask::influence_capacity_with_shadowmask_metadata(
        dynamic_light_capacity,
        lights.animated_baked_light_count,
        selected_static_count,
    );
    let mut influence_data = Vec::with_capacity(influence_record_capacity * 16);
    if !dynamic_influences.is_empty() {
        influence::pack_influence_into(&mut influence_data, &dynamic_influences);
    }
    influence_data.resize(influence_record_capacity * 16, 0);
    let influence_buffer = device.new_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("Light Influence Storage Buffer"),
        contents: &influence_data,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
    });

    let spec_lights_data = {
        let packed = pack_spec_lights(geometry.lights);
        if packed.is_empty() {
            vec![0u8; SPEC_LIGHT_SIZE]
        } else {
            packed
        }
    };
    let spec_lights_buffer = device.new_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("Spec-Only Lights Storage Buffer"),
        contents: &spec_lights_data,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
    });

    let chunk_grid = match geometry.chunk_light_list {
        Some(sec) => ChunkGrid::from_section(sec),
        None => ChunkGrid::fallback(),
    };
    let chunk_grid_info_buffer = device.new_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("Chunk Grid Info Uniform"),
        contents: &chunk_grid.grid_info,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
    });
    let chunk_grid_offsets_buffer = device.new_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("Chunk Grid Offset Table"),
        contents: &chunk_grid.offset_table,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
    });
    let chunk_grid_indices_buffer = device.new_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("Chunk Grid Index List"),
        contents: &chunk_grid.index_list,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
    });

    let lighting_bind_group = device.new_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("Lighting Bind Group"),
        layout: lighting_bind_group_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: lights.lights_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: influence_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: spec_lights_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: chunk_grid_info_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: chunk_grid_offsets_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 5,
                resource: chunk_grid_indices_buffer.as_entire_binding(),
            },
        ],
    });
    // The mirror's copy of this level's light records, and its group over
    // the level's influence, spec-light and chunk-grid buffers.
    let mirror_lighting = renderer_mirror_lights::MirrorLighting::new(
        device,
        lighting_bind_group_layout,
        &lights.lights_buffer,
        &renderer_mirror_lights::SharedLightingBuffers {
            influence: &influence_buffer,
            spec_lights: &spec_lights_buffer,
            chunk_grid_info: &chunk_grid_info_buffer,
            chunk_grid_offsets: &chunk_grid_offsets_buffer,
            chunk_grid_indices: &chunk_grid_indices_buffer,
        },
    );

    (
        LevelLighting {
            shadow_candidate_lights,
            shadow_candidate_source_indices,
            shadow_candidate_influences,
            shadow_candidate_selection_indices,
            shadow_candidate_animated_baked_indices,
            influence_buffer,
            level_light_influences: dynamic_influences,
            lighting_bind_group,
            mirror_lighting,
        },
        LevelLightBuffers {
            spec_lights: spec_lights_buffer,
            chunk_grid_info: chunk_grid_info_buffer,
            chunk_grid_offsets: chunk_grid_offsets_buffer,
            chunk_grid_indices: chunk_grid_indices_buffer,
        },
    )
}

/// Build the mesh and mover group-2 light bind groups over one level's light,
/// influence and SH animation buffers and the renderer's current shadow pools.
pub(super) fn rebind_mesh_light_groups(
    device: &wgpu::Device,
    full: &FullRenderer,
    lights_buffer: &wgpu::Buffer,
    mirror_lights_buffer: &wgpu::Buffer,
    influence_buffer: &wgpu::Buffer,
    sh_volume_resources: &ShVolumeResources,
) -> (LevelLightGroups, LevelLightGroups) {
    // Rebuild the mesh group-2 dynamic-direct light bind group over the
    // just-reallocated runtime buffers — the `is_dynamic`-filtered
    // `lights_buffer` (b0), the fresh `influence_buffer` (b1), and the new
    // `sh_volume_resources` scripted-descriptor (b2) / anim-sample (b3)
    // buffers. The forward `lighting_bind_group` above is rebuilt for the same
    // reason; this mirrors it for the mesh pass so a level swap does not leave
    // the mesh group-2 bind group dangling at the prior level's buffers.
    // b5–b8 re-reference the current pool-owned shadow resources. Supplying
    // them here rebinds the mesh pass after reallocated b0–b4 buffers or any
    // level-boundary pool replacement. The cube view is `Some` iff
    // `cube_shadow_pool` is present (the `Some`-iff-layout invariant).
    let cube_sampling_view = full.cube_shadow_pool.as_ref().map(|p| &p.sampling_view);
    let promoted_spot_cache = full
        .promoted_depth_cache
        .as_ref()
        .map(PromotedDepthCache::spot_sampled_view)
        .unwrap_or(&full.spot_shadow_pool.array_view);
    let promoted_cube_cache = full
        .promoted_depth_cache
        .as_ref()
        .and_then(PromotedDepthCache::cube_sampled_view)
        .or(cube_sampling_view);
    let mesh = full.mesh_pass.light_bind_groups(
        device,
        lights_buffer,
        mirror_lights_buffer,
        influence_buffer,
        &sh_volume_resources.scripted_light_descriptors,
        &sh_volume_resources.animation.anim_samples,
        &full.spot_shadow_pool.array_view,
        &full.spot_shadow_pool.compare_sampler,
        &full.spot_shadow_pool.matrices_buffer,
        cube_sampling_view,
        promoted_spot_cache,
        promoted_cube_cache,
    );
    let kinematic = full.kinematic_brush.light_bind_groups(
        device,
        lights_buffer,
        mirror_lights_buffer,
        influence_buffer,
        &sh_volume_resources.scripted_light_descriptors,
        &sh_volume_resources.animation.anim_samples,
        &full.spot_shadow_pool.array_view,
        &full.spot_shadow_pool.compare_sampler,
        &full.spot_shadow_pool.matrices_buffer,
        cube_sampling_view,
        promoted_spot_cache,
        promoted_cube_cache,
    );
    (mesh, kinematic)
}
