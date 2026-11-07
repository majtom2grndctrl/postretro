// Level lightmap install: the static pool plan, the animated atlas, the
// lightmap resources, and the SDF shadow rebind over the fresh SH depth.
// See: context/lib/rendering_pipeline.md §4

use super::super::install_pipelines::InstallPipelines;
use super::super::level_build::LevelBuildContext;
use super::lights::LevelLightBuffers;
use super::*;

/// The lightmap family's planned pool and built animated atlas, handed from
/// the plan to the lightmap install.
pub(super) struct BuiltLightmapFamily {
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
/// returns the BVH leaves the cull install takes. `prepared` is the CPU plan
/// a load's prep built on the pool; `None` (the main-thread oracle) plans
/// here, from the SH volume's installed animated count.
pub(super) fn plan_lightmap_family(
    device: &wgpu::Device,
    sh_volume_resources: &ShVolumeResources,
    pipelines: &InstallPipelines,
    geometry: &LevelGeometry<'_>,
    gpu_lighting_payloads: &postretro_level_loader::GpuLightingPayloads,
    prepared: Option<postretro_render_cpu::lightmap_family::LightmapFamilyPlan>,
) -> (
    BuiltLightmapFamily,
    Vec<postretro_render_data::geometry::BvhLeaf>,
) {
    let lightmap_bgl = crate::lighting::lightmap::bind_group_layout(device);
    let block_table_bgl = crate::lighting::lightmap::block_table_bind_group_layout(device);
    let animated_lm_debug = animated_lightmap::AnimatedLmDebugConfig::from_env();
    let bvh_leaves: Vec<postretro_render_data::geometry::BvhLeaf> = geometry.bvh.leaves.clone();
    let cpu_plan = prepared.unwrap_or_else(|| {
        postretro_render_cpu::lightmap_family::plan_lightmap_family(
            &lightmap_family_inputs(
                geometry,
                gpu_lighting_payloads,
                &bvh_leaves,
                sh_volume_resources.animation.animated_light_count(),
            ),
            &super::super::install_prep::device_limits(&device.limits()),
        )
    });
    let static_pool = cpu_plan.static_pool;
    let (animated_lightmap, animated_block_table) = build_animated_lightmap(
        device,
        &sh_volume_resources.animation,
        pipelines,
        cpu_plan.animated,
        animated_lm_debug,
    );

    (
        BuiltLightmapFamily {
            lightmap_bgl,
            block_table_bgl,
            static_pool,
            animated_lightmap,
            animated_block_table,
        },
        bvh_leaves,
    )
}

/// The lightmap family's CPU plan inputs from this install's geometry.
/// `animated_light_count` is the SH volume's animated descriptor count. Shared
/// by the oracle install and the app's load prep, so both plan from the same
/// inputs.
pub fn lightmap_family_inputs<'a>(
    geometry: &LevelGeometry<'a>,
    gpu_lighting_payloads: &'a postretro_level_loader::GpuLightingPayloads,
    bvh_leaves: &'a [postretro_render_data::geometry::BvhLeaf],
    animated_light_count: u32,
) -> postretro_render_cpu::lightmap_family::LightmapFamilyInputs<'a> {
    postretro_render_cpu::lightmap_family::LightmapFamilyInputs {
        lightmap: geometry.lightmap,
        shadowmask: geometry.shadowmask_atlas,
        streaming: geometry.lightmap_streaming.map(|streaming| {
            postretro_render_cpu::lightmap_family::LightmapStreamingInputs {
                content_tag: streaming.content_tag,
                pool_sizing: streaming.pool_sizing,
            }
        }),
        payload_blocks: &gpu_lighting_payloads.blocks,
        weight_maps: geometry.animated_light_weight_maps,
        animated_chunks: geometry.animated_light_chunks,
        bvh_leaves,
        animated_light_count,
    }
}

/// The animated atlas's GPU half and the forward block table that describes
/// it. A rejected plan, or a build that fails, logs once and binds the
/// fallback dummy with an empty table, so no vertex applies a compact offset
/// against it.
fn build_animated_lightmap(
    device: &wgpu::Device,
    animation: &sh_volume::AnimatedLightBuffers,
    pipelines: &InstallPipelines,
    plan: Result<postretro_render_cpu::animated_lightmap::plan::AnimatedLightmapPlan, String>,
    animated_lm_debug: animated_lightmap::AnimatedLmDebugConfig,
) -> (animated_lightmap::AnimatedLightmapResources, Vec<u8>) {
    let built = plan.and_then(|plan| {
        let block_table = plan.block_table_bytes();
        animated_lightmap::AnimatedLightmapResources::from_plan(
            device,
            plan,
            animation,
            Some(&pipelines.animated_lightmap),
            animated_lm_debug,
        )
        .map(|resources| (resources, block_table))
    });
    animated_lightmap::with_dummy_fallback(
        built,
        || {
            (
                animated_lightmap::AnimatedLightmapResources::dummy(
                    device,
                    animation,
                    animated_lm_debug,
                )
                .into_fallback(),
                postretro_render_cpu::animated_lightmap::plan::empty_animated_block_table(),
            )
        },
        "animated lightmap install",
    )
}

pub(super) fn install_lightmap(
    device: &wgpu::Device,
    writes: &dyn crate::render::queue_writes::QueueWrites,
    geometry: &LevelGeometry<'_>,
    gpu_lighting_payloads: postretro_level_loader::GpuLightingPayloads,
    plan: BuiltLightmapFamily,
    generation_floor: u64,
) -> LevelLightmapFamily {
    let BuiltLightmapFamily {
        lightmap_bgl,
        block_table_bgl,
        static_pool,
        animated_lightmap,
        animated_block_table,
    } = plan;
    let lightmap_resources = LightmapResources::new(
        device,
        writes,
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
    ctx: &LevelBuildContext,
    geometry: &LevelGeometry<'_>,
    light_buffers: &LevelLightBuffers,
    streaming_manifest: Option<&postretro_level_loader::ShStreamManifest>,
    sh_volume_resources: &ShVolumeResources,
    sh_streaming: Option<&sh_streaming::ShResidencyState>,
) -> sdf_shadow::SdfShadowLevelState {
    // SDF half-res shadow pass — rebind to the freshly-loaded SH
    // depth-moment texture + static-light buffers. The pass itself is always
    // allocated; the dispatch is gated on `sdf_atlas_resources.present`,
    // which `install_compose_family` sets.
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
    ctx.sdf_shadow.level_state(
        device,
        &ctx.scene.depth_view,
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
