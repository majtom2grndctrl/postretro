// Level lightmap install: the static pool plan, the animated atlas, the
// lightmap resources, and the SDF shadow rebind over the fresh SH depth.
// See: context/lib/rendering_pipeline.md §4

use super::super::install_pipelines::InstallPipelines;
use super::super::renderer_types::FullRenderer;
use super::lights::LevelLightBuffers;
use super::*;

/// The lightmap family's planned pool and built animated atlas, handed from
/// the plan to the lightmap install.
pub(super) struct LightmapFamilyPlan {
    lightmap_bgl: wgpu::BindGroupLayout,
    block_table_bgl: wgpu::BindGroupLayout,
    static_pool: crate::lighting::lightmap::StaticPool,
    animated_lightmap: animated_lightmap::AnimatedLightmapResources,
    animated_block_table: Vec<u8>,
}

/// One level's lightmap pool, animated atlas and their meter.
pub(super) struct LevelLightmapFamily {
    pub(super) lightmap_resources: LightmapResources,
    pub(super) shadowmask_present: bool,
    pub(super) lightmap_residency_report: LightmapResidencyReport,
    pub(super) animated_lightmap: animated_lightmap::AnimatedLightmapResources,
}

/// Plan the static pool and build the animated atlas that keys on it. Also
/// returns the BVH leaves the cull install takes.
pub(super) fn plan_lightmap_family(
    device: &wgpu::Device,
    sh_volume_resources: &ShVolumeResources,
    pipelines: &InstallPipelines,
    geometry: &LevelGeometry<'_>,
    gpu_lighting_payloads: &postretro_level_loader::GpuLightingPayloads,
) -> (
    LightmapFamilyPlan,
    Vec<postretro_render_data::geometry::BvhLeaf>,
) {
    let lightmap_bgl = crate::lighting::lightmap::bind_group_layout(device);
    let block_table_bgl = crate::lighting::lightmap::block_table_bind_group_layout(device);
    let animated_lm_debug = animated_lightmap::AnimatedLmDebugConfig::from_env();
    let bvh_leaves: Vec<postretro_render_data::geometry::BvhLeaf> = geometry.bvh.leaves.clone();
    // One plan decides the static pool for both consumers: the animated
    // atlas keys its blocks on the cell blocks the pool installs, and a
    // placeholder or rejected pool leaves it no block frame.
    let static_pool = match geometry.lightmap_streaming {
        Some(streaming) => crate::lighting::lightmap::plan_streaming_pool(
            geometry.lightmap,
            geometry.shadowmask_atlas,
            streaming.content_tag,
            streaming.pool_cap_layers,
            device.limits().max_texture_dimension_2d,
            device.limits().max_texture_array_layers,
        ),
        None => crate::lighting::lightmap::plan_static_pool(
            geometry.lightmap,
            geometry.shadowmask_atlas,
            &gpu_lighting_payloads.blocks,
            device.limits().max_texture_dimension_2d,
            device.limits().max_texture_array_layers,
        ),
    };

    let animated_lightmap = animated_lightmap::with_dummy_fallback(
        animated_lightmap::AnimatedLightmapResources::new(
            device,
            geometry.animated_light_weight_maps,
            geometry.animated_light_chunks,
            &bvh_leaves,
            &sh_volume_resources.animation,
            &pipelines.animated_lightmap,
            static_pool.static_block_extents(),
            animated_lm_debug,
        ),
        || {
            animated_lightmap::AnimatedLightmapResources::dummy(
                device,
                &sh_volume_resources.animation,
                animated_lm_debug,
            )
            .into_fallback()
        },
        "animated lightmap install",
    );
    // Built from the installed resource state: a dummy atlas gets an
    // empty table, so no vertex applies a compact offset against it.
    let animated_block_table = crate::lighting::lightmap::animated_block_table_bytes(
        animated_lightmap::installed_block_table_section(
            animated_lightmap.is_active(),
            geometry.animated_light_weight_maps,
        ),
    );

    (
        LightmapFamilyPlan {
            lightmap_bgl,
            block_table_bgl,
            static_pool,
            animated_lightmap,
            animated_block_table,
        },
        bvh_leaves,
    )
}

pub(super) fn install_lightmap(
    device: &wgpu::Device,
    queue: &UploadQueue,
    geometry: &LevelGeometry<'_>,
    gpu_lighting_payloads: postretro_level_loader::GpuLightingPayloads,
    plan: LightmapFamilyPlan,
    generation_floor: u64,
) -> LevelLightmapFamily {
    let LightmapFamilyPlan {
        lightmap_bgl,
        block_table_bgl,
        static_pool,
        animated_lightmap,
        animated_block_table,
    } = plan;
    let lightmap_resources = LightmapResources::new(
        device,
        queue.raw(),
        geometry.lightmap,
        geometry.shadowmask_atlas,
        &static_pool,
        gpu_lighting_payloads,
        &lightmap_bgl,
        &block_table_bgl,
        &animated_lightmap.forward_view,
        &animated_lightmap.direction_forward_view,
        &animated_block_table,
        // A reload's first drain must not accept a previous level's
        // generation: the set reserves from the renderer-wide floor.
        generation_floor,
    );
    let lightmap_residency_report = LightmapResidencyReport::new(
        lightmap_resources.residency.clone(),
        animated_lightmap.residency_rows(),
    );
    log::info!("{}", lightmap_residency_report.log_line());
    LevelLightmapFamily {
        shadowmask_present: lightmap_resources.shadowmask_present,
        lightmap_resources,
        lightmap_residency_report,
        animated_lightmap,
    }
}

pub(super) fn rebind_sdf_shadow(
    device: &wgpu::Device,
    full: &FullRenderer,
    geometry: &LevelGeometry<'_>,
    light_buffers: &LevelLightBuffers,
    streaming_manifest: Option<&postretro_level_loader::ShStreamManifest>,
    sh_volume_resources: &ShVolumeResources,
    sh_streaming: Option<&sh_streaming::ShResidencyState>,
) -> sdf_shadow::SdfShadowLevelState {
    // SDF half-res shadow pass — rebind to the freshly-loaded SH
    // depth-moment texture + static-light buffers. The pass itself is always
    // allocated; the dispatch is gated on `sdf_atlas_resources.present`,
    // which `install_level_geometry` may have just flipped.
    let sdf_shadow_sh_grid = streaming_manifest
        .filter(|_| sh_streaming.is_some())
        .map(|manifest| {
            let base = manifest.base();
            build_sdf_shadow_sh_grid_from_metadata(
                base.grid_origin,
                base.cell_size,
                base.grid_dimensions,
            )
        })
        .unwrap_or_else(|| {
            build_sdf_shadow_sh_grid(geometry.sh_volume, sh_volume_resources.present)
        });
    let sh_depth_moment_view = sh_streaming
        .and_then(sh_streaming::ShResidencyState::depth_moment_view)
        .unwrap_or_else(|| sh_volume_resources.make_depth_moment_view());
    full.sdf_shadow_pass.level_state(
        device,
        &full.depth_view,
        sh_depth_moment_view,
        sdf_shadow::SdfShadowLightBuffers {
            spec_lights: &light_buffers.spec_lights,
            chunk_grid_info: &light_buffers.chunk_grid_info,
            chunk_offsets: &light_buffers.chunk_grid_offsets,
            chunk_indices: &light_buffers.chunk_grid_indices,
        },
        sdf_shadow_sh_grid,
    )
}
