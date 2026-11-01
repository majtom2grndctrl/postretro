// Level SH install: the SH volume, streamed-SH residency init, the SDF atlas
// and the compose family, with the one allocation ledger they share.
// See: context/lib/rendering_pipeline.md §4, §12

use super::super::install_pipelines::InstallPipelines;
use super::*;

/// One level's SH volume, streamed residency state, and the allocation
/// ledger the compose family continues.
pub(super) struct LevelShVolume {
    pub(super) sh_volume_resources: ShVolumeResources,
    pub(super) sh_streaming: Option<sh_streaming::ShResidencyState>,
    pub(super) ledger: sh_residency::ShAllocationLedger,
}

/// One level's SDF atlas and compose family, with the finished SH report.
pub(super) struct LevelComposeFamily {
    pub(super) sdf_atlas_resources: SdfAtlasResources,
    pub(super) lightmap_mode: postretro_level_loader::LightmapMode,
    pub(super) sh_compose: ShComposeResources,
    pub(super) direct_sh_compose: DirectShComposeResources,
    pub(super) billboard_direct_scatter_compose: BillboardDirectScatterComposeResources,
    pub(super) sh_residency_report: ShResidencyReport,
    #[cfg(feature = "dev-tools")]
    pub(super) sh_delta_volumes_meta: Vec<sh_volume::DeltaVolumeMeta>,
    #[cfg(feature = "dev-tools")]
    pub(super) sh_probe_readback: sh_diagnostics::ShProbeReadback,
}

#[allow(clippy::too_many_arguments)]
pub(super) fn install_sh_volume(
    device: &wgpu::Device,
    writes: &dyn crate::render::queue_writes::QueueWrites,
    probe_occlusion_enabled: bool,
    pipelines: &InstallPipelines,
    geometry: &LevelGeometry<'_>,
    streaming_manifest: Option<&postretro_level_loader::ShStreamManifest>,
    level_light_count: usize,
    animated_baked_descriptor_indices: &[u32],
    promoted_static_weight_buffer: &wgpu::Buffer,
    sh_plan: Option<postretro_render_cpu::sh_residency::ShResidencyPlan>,
) -> Result<LevelShVolume, sh_streaming::ShResidencyDrainError> {
    // --- SH volume and streamed-SH residency state ---
    // This ledger is intentionally scoped to one level install. Every SH
    // constructor records the exact descriptor it sends to wgpu, and the
    // completed plain-Rust report stays with the renderer for capture.
    let mut sh_allocation_ledger = sh_residency::ShAllocationLedger::new();
    let mut sh_volume_resources = ShVolumeResources::new(
        device,
        writes,
        ShVolumeSections {
            sh: geometry.sh_volume,
            stream_base_present: matches!(
                &geometry.sh_storage,
                LevelGeometryShStorage::Streaming(_)
            ),
            stream_animation_descriptors: match &geometry.sh_storage {
                LevelGeometryShStorage::Legacy => None,
                LevelGeometryShStorage::Streaming(manifest) => {
                    Some(manifest.base().animation_descriptors.as_slice())
                }
            },
            indirect_delta_present: geometry.delta_sh_volumes.is_some(),
            direct: geometry.direct_sh_volume,
            direct_delta: geometry.direct_sh_delta_volumes,
            animated_direct_delta: geometry.animated_direct_sh_delta_volumes,
            billboard_direct_scatter: geometry.billboard_direct_scatter_volume,
            animated_billboard_direct_scatter_delta: geometry
                .animated_billboard_direct_scatter_delta_volumes,
        },
        scripted_light_capacity(
            geometry.lights.len(),
            level_light_count,
            animated_baked_descriptor_indices,
        ),
        probe_occlusion_enabled,
        &mut sh_allocation_ledger,
    );

    // The plan already ran every level-data check; what can still fail
    // here is the first pool generation against the device and the live
    // buffer sizes. Either way the error fails the load.
    let sh_streaming = match (streaming_manifest, sh_plan) {
        (Some(manifest), Some(plan)) => {
            debug_assert_eq!(
                plan.content_tag,
                manifest.content_tag(),
                "the residency plan was built from a different stream manifest"
            );
            let mut state = sh_streaming::ShResidencyState::from_plan(
                plan,
                manifest.base(),
                manifest.sources(),
            );
            state.initialize_gpu(
                device,
                writes,
                manifest,
                probe_occlusion_enabled,
                &mut sh_volume_resources,
                &pipelines.sh_streaming,
                promoted_static_weight_buffer,
            )?;
            Some(state)
        }
        (None, None) => None,
        (Some(_), None) | (None, Some(_)) => {
            return Err(sh_streaming::ShResidencyDrainError::InvalidBatch(
                "a streamed level installs with exactly its residency plan".to_owned(),
            ));
        }
    };

    Ok(LevelShVolume {
        sh_volume_resources,
        sh_streaming,
        ledger: sh_allocation_ledger,
    })
}

#[allow(clippy::too_many_arguments)]
pub(super) fn install_compose_family(
    device: &wgpu::Device,
    writes: &dyn crate::render::queue_writes::QueueWrites,
    pipelines: &InstallPipelines,
    geometry: &LevelGeometry<'_>,
    sh_volume_resources: &ShVolumeResources,
    sh_streaming: Option<&sh_streaming::ShResidencyState>,
    promoted_static_weight_buffer: &wgpu::Buffer,
    mut sh_allocation_ledger: sh_residency::ShAllocationLedger,
) -> LevelComposeFamily {
    let sdf_atlas_resources = SdfAtlasResources::new(device, writes, geometry.sdf_atlas);
    let compose_sh_volume = geometry.sh_volume.filter(|_| sh_volume_resources.present);
    let compose_delta_sh_volumes = geometry
        .delta_sh_volumes
        .filter(|_| sh_volume_resources.present);
    let sh_compose = ShComposeResources::new(
        device,
        sh_volume_resources,
        compose_sh_volume,
        compose_delta_sh_volumes,
        geometry.sh_volume.is_some(),
        geometry.delta_sh_volumes.is_some(),
        &pipelines.sh_compose,
        &mut sh_allocation_ledger,
    );
    let direct_sh_compose = DirectShComposeResources::new(
        device,
        &sh_volume_resources.direct,
        &sh_volume_resources.animation,
        &sh_volume_resources.probe_indirection_words,
        geometry.direct_sh_delta_volumes,
        geometry.animated_direct_sh_delta_volumes,
        promoted_static_weight_buffer,
        &pipelines.direct_sh_compose,
        geometry.sh_volume.is_some(),
        &mut sh_allocation_ledger,
    );
    let billboard_direct_scatter_compose = BillboardDirectScatterComposeResources::new(
        device,
        &sh_volume_resources.billboard_direct_scatter,
        &sh_volume_resources.animation,
        geometry.animated_billboard_direct_scatter_delta_volumes,
        &pipelines.billboard_direct_scatter_compose,
        geometry
            .billboard_direct_scatter_volume
            .map_or(sh_volume_resources.grid_dimensions, |section| {
                section.grid_dimensions
            }),
        &mut sh_allocation_ledger,
    );
    if let Some(streaming) = sh_streaming {
        sh_allocation_ledger.record_streaming_summary(streaming.streaming_allocation_summary());
    }
    let sh_residency_report = sh_allocation_ledger.finish();
    LevelComposeFamily {
        sdf_atlas_resources,
        lightmap_mode: geometry.lightmap_mode,
        sh_compose,
        direct_sh_compose,
        billboard_direct_scatter_compose,
        sh_residency_report,
        #[cfg(feature = "dev-tools")]
        sh_delta_volumes_meta: collect_delta_volume_meta(geometry.delta_sh_volumes),
        // Atlas shape (hence readback buffer size) changes per level.
        #[cfg(feature = "dev-tools")]
        sh_probe_readback: sh_diagnostics::ShProbeReadback::new(
            device,
            sh_volume_resources.grid_dimensions,
            sh_volume_resources.atlas_dimensions,
            sh_volume_resources.tile_dimension,
            sh_volume_resources.tile_border,
            sh_volume_resources.atlas_tiles_per_row,
            sh_volume_resources.tiles_per_layer,
            sh_volume_resources.atlas_layer_count,
            &sh_volume_resources.probe_indirection_words,
        ),
    }
}
