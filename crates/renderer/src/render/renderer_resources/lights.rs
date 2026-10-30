// Level light install: the filtered light records and their buffers, the
// forward lighting bind group, and the mesh and mover group-2 rebinds.
// See: context/lib/rendering_pipeline.md §4

use super::super::renderer_types::FullRenderer;
use super::*;

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
    full: &mut FullRenderer,
    geometry: &'g LevelGeometry<'_>,
) -> LevelLightSelection<'g> {
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
    full.light_count = level_lights.len() as u32;
    full.animated_baked_light_count = animated_baked_descriptor_indices.len();
    full.total_light_count = full.light_count + full.animated_baked_light_count as u32;
    let level_light_count = level_lights.len();
    let selected_static_count = selected_static.lights.len();
    let dynamic_light_capacity = level_light_count + RUNTIME_DYNAMIC_LIGHT_RESERVE;
    full.dynamic_light_capacity = dynamic_light_capacity;

    let light_record_capacity =
        (dynamic_light_capacity + full.animated_baked_light_count + selected_static_count).max(1);
    let mut lights_data = Vec::with_capacity(light_record_capacity * GPU_LIGHT_SIZE);
    if !level_lights.is_empty() {
        lights_data.extend_from_slice(&pack_lights(&level_lights));
    }
    lights_data.resize(light_record_capacity * GPU_LIGHT_SIZE, 0);
    let lights_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("Direct Lights Storage Buffer"),
        contents: &lights_data,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
    });
    full.lights_buffer = lights_buffer;
    full.level_lights = level_lights;
    full.level_light_source_indices = level_light_source_indices;
    full.entity_shadow_lights = selected_static.lights;
    full.entity_shadow_light_influences = selected_static.influences;
    full.entity_shadow_light_source_indices = selected_static.source_indices;
    full.entity_shadow_spec_light_indices = shadowmask::build_selection_spec_light_indices(
        geometry.lights,
        geometry.entity_shadow_lights,
    );
    full.shadowmask_present = false;
    full.forward_shadowmask_metadata_scratch.clear();

    LevelLightSelection {
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
    }
}

/// Store the shadow candidates, upload the influence, spec-light and chunk-grid
/// buffers, and rebuild the forward and mirror lighting groups over them.
pub(super) fn install_lighting_bind_group(
    device: &wgpu::Device,
    full: &mut FullRenderer,
    geometry: &LevelGeometry<'_>,
    selection: LevelLightSelection<'_>,
) -> LevelLightBuffers {
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
    full.shadow_candidate_lights = shadow_candidate_lights;
    full.shadow_candidate_source_indices = shadow_candidate_source_indices;
    full.shadow_candidate_influences = shadow_candidate_influences;
    full.shadow_candidate_selection_indices = shadow_candidate_selection_indices;
    full.shadow_candidate_animated_baked_indices = shadow_candidate_animated_baked_indices;

    let influence_record_capacity = shadowmask::influence_capacity_with_shadowmask_metadata(
        dynamic_light_capacity,
        full.animated_baked_light_count,
        selected_static_count,
    );
    let mut influence_data = Vec::with_capacity(influence_record_capacity * 16);
    if !dynamic_influences.is_empty() {
        influence::pack_influence_into(&mut influence_data, &dynamic_influences);
    }
    influence_data.resize(influence_record_capacity * 16, 0);
    let influence_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("Light Influence Storage Buffer"),
        contents: &influence_data,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
    });
    full.influence_buffer = influence_buffer;
    full.level_light_influences = dynamic_influences;

    let spec_lights_data = {
        let packed = pack_spec_lights(geometry.lights);
        if packed.is_empty() {
            vec![0u8; SPEC_LIGHT_SIZE]
        } else {
            packed
        }
    };
    let spec_lights_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("Spec-Only Lights Storage Buffer"),
        contents: &spec_lights_data,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
    });

    let chunk_grid = match geometry.chunk_light_list {
        Some(sec) => ChunkGrid::from_section(sec),
        None => ChunkGrid::fallback(),
    };
    let chunk_grid_info_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("Chunk Grid Info Uniform"),
        contents: &chunk_grid.grid_info,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
    });
    let chunk_grid_offsets_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("Chunk Grid Offset Table"),
        contents: &chunk_grid.offset_table,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
    });
    let chunk_grid_indices_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("Chunk Grid Index List"),
        contents: &chunk_grid.index_list,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
    });

    full.lighting_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("Lighting Bind Group"),
        layout: &full.lighting_bind_group_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: full.lights_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: full.influence_buffer.as_entire_binding(),
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
    // The mirror's copy follows the reallocated light buffer, and its
    // group the reallocated influence, spec-light and chunk-grid buffers.
    full.mirror_lighting.rebuild(
        device,
        &full.lighting_bind_group_layout,
        &full.lights_buffer,
        &renderer_mirror_lights::SharedLightingBuffers {
            influence: &full.influence_buffer,
            spec_lights: &spec_lights_buffer,
            chunk_grid_info: &chunk_grid_info_buffer,
            chunk_grid_offsets: &chunk_grid_offsets_buffer,
            chunk_grid_indices: &chunk_grid_indices_buffer,
        },
    );

    LevelLightBuffers {
        spec_lights: spec_lights_buffer,
        chunk_grid_info: chunk_grid_info_buffer,
        chunk_grid_offsets: chunk_grid_offsets_buffer,
        chunk_grid_indices: chunk_grid_indices_buffer,
    }
}

pub(super) fn rebind_mesh_light_groups(device: &wgpu::Device, full: &mut FullRenderer) {
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
    full.mesh_pass.rebuild_light_bind_group(
        device,
        &full.lights_buffer,
        full.mirror_lighting.lights_buffer(),
        &full.influence_buffer,
        &full.sh_volume_resources.scripted_light_descriptors,
        &full.sh_volume_resources.animation.anim_samples,
        &full.spot_shadow_pool.array_view,
        &full.spot_shadow_pool.compare_sampler,
        &full.spot_shadow_pool.matrices_buffer,
        cube_sampling_view,
        promoted_spot_cache,
        promoted_cube_cache,
    );
    full.kinematic_brush.rebuild_light_bind_group(
        device,
        &full.lights_buffer,
        full.mirror_lighting.lights_buffer(),
        &full.influence_buffer,
        &full.sh_volume_resources.scripted_light_descriptors,
        &full.sh_volume_resources.animation.anim_samples,
        &full.spot_shadow_pool.array_view,
        &full.spot_shadow_pool.compare_sampler,
        &full.spot_shadow_pool.matrices_buffer,
        cube_sampling_view,
        promoted_spot_cache,
        promoted_cube_cache,
    );
}
