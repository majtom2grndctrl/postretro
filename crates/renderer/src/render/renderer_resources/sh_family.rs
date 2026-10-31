// Level SH install: the SH volume, streamed-SH residency init, the SDF atlas
// and the compose family, with the one allocation ledger they share.
// See: context/lib/rendering_pipeline.md §4, §12

use super::super::install_pipelines::InstallPipelines;
use super::super::renderer_types::FullRenderer;
use super::*;

#[allow(clippy::too_many_arguments)]
pub(super) fn install_sh_volume(
    device: &wgpu::Device,
    queue: &UploadQueue,
    full: &mut FullRenderer,
    pipelines: &InstallPipelines,
    geometry: &LevelGeometry<'_>,
    streaming_manifest: Option<&postretro_level_loader::ShStreamManifest>,
    level_light_count: usize,
    animated_baked_descriptor_indices: &[u32],
) -> sh_residency::ShAllocationLedger {
    // --- SH volume, sh_compose, lightmap, animated lightmap ---
    // This ledger is intentionally scoped to one level install. Every SH
    // constructor records the exact descriptor it sends to wgpu, and the
    // completed plain-Rust report stays with the renderer for capture.
    let mut sh_allocation_ledger = sh_residency::ShAllocationLedger::new();
    full.sh_volume_resources = ShVolumeResources::new(
        device,
        queue.raw(),
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
        full.probe_occlusion_enabled,
        &mut sh_allocation_ledger,
    );

    if let Some(manifest) = streaming_manifest {
        full.sh_streaming = Some(
            sh_streaming::ShResidencyState::from_manifest(manifest, geometry.cells)
                .and_then(|mut state| {
                    state.initialize_gpu(
                        device,
                        queue.raw(),
                        manifest,
                        full.probe_occlusion_enabled,
                        &mut full.sh_volume_resources,
                        &pipelines.sh_streaming,
                        &full.promoted_static_weight_buffer,
                    )?;
                    Ok(state)
                })
                // A streamed level deliberately omits the legacy bodies.
                // Continuing with dummy legacy bindings here would make a
                // valid map silently render unlit, which is substantially
                // worse than a named, fail-fast level-install error.
                .unwrap_or_else(|error| {
                    panic!("cannot install streamed SH level resources: {error}")
                }),
        );
    }

    sh_allocation_ledger
}

pub(super) fn install_compose_family(
    device: &wgpu::Device,
    queue: &UploadQueue,
    full: &mut FullRenderer,
    pipelines: &InstallPipelines,
    geometry: &LevelGeometry<'_>,
    mut sh_allocation_ledger: sh_residency::ShAllocationLedger,
) {
    full.sdf_atlas_resources = SdfAtlasResources::new(device, queue.raw(), geometry.sdf_atlas);
    full.lightmap_mode = geometry.lightmap_mode;
    let compose_sh_volume = geometry
        .sh_volume
        .filter(|_| full.sh_volume_resources.present);
    let compose_delta_sh_volumes = geometry
        .delta_sh_volumes
        .filter(|_| full.sh_volume_resources.present);
    full.sh_compose = ShComposeResources::new(
        device,
        &full.sh_volume_resources,
        compose_sh_volume,
        compose_delta_sh_volumes,
        geometry.sh_volume.is_some(),
        geometry.delta_sh_volumes.is_some(),
        &pipelines.sh_compose,
        &mut sh_allocation_ledger,
    );
    full.direct_sh_compose = DirectShComposeResources::new(
        device,
        &full.sh_volume_resources.direct,
        &full.sh_volume_resources.animation,
        &full.sh_volume_resources.probe_indirection_words,
        geometry.direct_sh_delta_volumes,
        geometry.animated_direct_sh_delta_volumes,
        &full.promoted_static_weight_buffer,
        &pipelines.direct_sh_compose,
        geometry.sh_volume.is_some(),
        &mut sh_allocation_ledger,
    );
    full.billboard_direct_scatter_compose = BillboardDirectScatterComposeResources::new(
        device,
        &full.sh_volume_resources.billboard_direct_scatter,
        &full.sh_volume_resources.animation,
        geometry.animated_billboard_direct_scatter_delta_volumes,
        &pipelines.billboard_direct_scatter_compose,
        geometry
            .billboard_direct_scatter_volume
            .map_or(full.sh_volume_resources.grid_dimensions, |section| {
                section.grid_dimensions
            }),
        &mut sh_allocation_ledger,
    );
    if let Some(streaming) = full.sh_streaming.as_ref() {
        sh_allocation_ledger.record_streaming_summary(streaming.streaming_allocation_summary());
    }
    let sh_residency_report = sh_allocation_ledger.finish();
    full.sh_residency_report = Some(sh_residency_report);
    #[cfg(feature = "dev-tools")]
    {
        full.sh_delta_volumes_meta = collect_delta_volume_meta(geometry.delta_sh_volumes);
        // Atlas shape (hence readback buffer size) changes per level — rebuild.
        full.sh_probe_readback = sh_diagnostics::ShProbeReadback::new(
            device,
            full.sh_volume_resources.grid_dimensions,
            full.sh_volume_resources.atlas_dimensions,
            full.sh_volume_resources.tile_dimension,
            full.sh_volume_resources.tile_border,
            full.sh_volume_resources.atlas_tiles_per_row,
            full.sh_volume_resources.tiles_per_layer,
            full.sh_volume_resources.atlas_layer_count,
            &full.sh_volume_resources.probe_indirection_words,
        );
    }
}
