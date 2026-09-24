//! Initial streamed-pool construction and adapter-limit preflight.
//!
//! Every wgpu allocation is preceded by the capacity checks in the shared
//! helpers. The resulting active pool owns dense textures, fixed indirection,
//! and independently growable compose carriers.

use super::*;

impl StreamingGpuPools {
    #[allow(clippy::too_many_arguments)]
    pub(in crate::render::sh_streaming) fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        base: &ShStreamBaseMetadata,
        sources: &ShStreamSourceMetadata,
        initial_floor: super::super::floor::InitialPoolFloor,
        probe_occlusion_enabled: bool,
        sh: &mut ShVolumeResources,
        uniform_bind_group_layout: &wgpu::BindGroupLayout,
        selection_weights: &wgpu::Buffer,
    ) -> Result<Self, ShResidencyDrainError> {
        let shape = AtlasShape::for_slots(initial_floor.dense_slots.max(1), &device.limits())?;
        preflight_initial_resource_limits(
            base,
            sources,
            shape,
            &initial_floor.sparse_capacities,
            &device.limits(),
        )?;
        Self::new_with_shape(
            device,
            queue,
            base,
            sources,
            shape,
            &initial_floor.sparse_capacities,
            probe_occlusion_enabled,
            sh,
            uniform_bind_group_layout,
            selection_weights,
            initial_floor.effective_floor_bytes,
            initial_floor.dense_group_minimum_bytes,
            initial_floor.sparse_group_minimum_bytes,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(in crate::render::sh_streaming::gpu) fn new_with_shape(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        base: &ShStreamBaseMetadata,
        sources: &ShStreamSourceMetadata,
        shape: AtlasShape,
        sparse_capacity_floors: &SparseCapacityFloors,
        probe_occlusion_enabled: bool,
        sh: &mut ShVolumeResources,
        uniform_bind_group_layout: &wgpu::BindGroupLayout,
        selection_weights: &wgpu::Buffer,
        effective_floor_bytes: u64,
        dense_group_minimum_bytes: u64,
        sparse_group_minimum_bytes: std::collections::BTreeMap<u32, u64>,
    ) -> Result<Self, ShResidencyDrainError> {
        let base_format = texture_format(base.irradiance_format)?;
        let extent = shape.extent();
        let base_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Streamed SH Base Atlas Pool"),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: base_format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let total_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Streamed SH Total Atlas Pool"),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::STORAGE_BINDING
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let base_view = array_view(&base_texture, "Streamed SH Base Atlas View");
        let total_storage_view = array_view(&total_texture, "Streamed SH Total Storage View");
        let total_sampled_view = array_view(&total_texture, "Streamed SH Total Sampled View");

        let (direct_base, direct_base_view, direct_format) = if let Some(direct) = &sources.direct {
            let format = texture_format(direct.irradiance_format)?;
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("Streamed Direct SH Base Atlas Pool"),
                size: extent,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::COPY_DST
                    | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            let view = array_view(&texture, "Streamed Direct SH Base Atlas View");
            sh.direct.enable_streamed_atlas(queue);
            (Some(texture), Some(view), Some(format))
        } else {
            (None, None, None)
        };
        let direct_compose_required =
            sources.direct_delta.is_some() || sources.animated_direct_delta.is_some();
        let (
            direct_intermediate,
            direct_intermediate_storage_view,
            direct_intermediate_sampled_view,
            direct_total,
            direct_total_storage_view,
            direct_total_sampled_view,
        ) = if direct_compose_required {
            if direct_base.is_none() {
                return Err(ShResidencyDrainError::GpuCapacity {
                    reason: "streamed direct compose has no id-35 base atlas",
                });
            }
            let total = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("Streamed Direct SH Total Atlas Pool"),
                size: extent,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba16Float,
                usage: wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::STORAGE_BINDING
                    | wgpu::TextureUsages::COPY_DST
                    | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            let total_storage = array_view(&total, "Streamed Direct SH Total Storage View");
            let total_sampled = array_view(&total, "Streamed Direct SH Total Sampled View");
            if sources.animated_direct_delta.is_some() {
                let intermediate = device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("Streamed Direct SH Intermediate Atlas Pool"),
                    size: extent,
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rgba16Float,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING
                        | wgpu::TextureUsages::STORAGE_BINDING
                        | wgpu::TextureUsages::COPY_DST
                        | wgpu::TextureUsages::COPY_SRC,
                    view_formats: &[],
                });
                let storage = array_view(
                    &intermediate,
                    "Streamed Direct SH Intermediate Storage View",
                );
                let sampled = array_view(
                    &intermediate,
                    "Streamed Direct SH Intermediate Sampled View",
                );
                (
                    Some(intermediate),
                    Some(storage),
                    Some(sampled),
                    Some(total),
                    Some(total_storage),
                    Some(total_sampled),
                )
            } else {
                (
                    None,
                    None,
                    None,
                    Some(total),
                    Some(total_storage),
                    Some(total_sampled),
                )
            }
        } else {
            (None, None, None, None, None, None)
        };

        let depth_extent = wgpu::Extent3d {
            width: base.grid_dimensions[0].max(1),
            height: base.grid_dimensions[1].max(1),
            depth_or_array_layers: base.grid_dimensions[2].max(1),
        };
        if depth_extent.width > device.limits().max_texture_dimension_3d
            || depth_extent.height > device.limits().max_texture_dimension_3d
            || depth_extent.depth_or_array_layers > device.limits().max_texture_dimension_3d
        {
            return Err(ShResidencyDrainError::GpuCapacity {
                reason: "streamed SH depth-moment grid exceeds adapter 3D texture limits",
            });
        }
        let depth_moments = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Streamed SH Depth Moments"),
            size: depth_extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D3,
            format: wgpu::TextureFormat::Rgba16Uint,
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let depth_view = depth_moments.create_view(&wgpu::TextureViewDescriptor {
            label: Some("Streamed SH Depth Moment View"),
            dimension: Some(wgpu::TextureViewDimension::D3),
            ..Default::default()
        });

        let indirection_len = u64::try_from(base.probes.len())
            .ok()
            .and_then(|count| count.checked_mul(u64::from(std::mem::size_of::<u32>() as u32)))
            .ok_or(ShResidencyDrainError::SlotOverflow)?
            .max(4);
        let compose_indirection = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Streamed SH Compose Indirection"),
            size: indirection_len,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_DST
                | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let sampled_indirection = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Streamed SH Sampled Indirection Mirror"),
            size: indirection_len,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_DST
                | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });

        let grid_bytes = build_grid_info_bytes(ShGridInfoParams {
            grid_origin: base.grid_origin,
            cell_size: base.cell_size,
            grid_dimensions: base.grid_dimensions,
            atlas_dimensions: [extent.width, extent.height],
            tile_dimension: base.tile_dimension,
            tile_border: base.tile_border,
            atlas_tiles_per_row: shape.tiles_per_row,
            physical_tile_stride: STREAMED_SH_PHYSICAL_TILE_STRIDE,
            tiles_per_layer: shape.tiles_per_layer,
            atlas_layer_count: shape.layers,
            present: true,
            probe_occlusion_enabled,
        });
        let grid_info = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Streamed SH Grid Info"),
            contents: &grid_bytes,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let indirect_compose = StreamingIndirectCompose::new(
            device,
            base,
            sources,
            shape,
            &base_view,
            &total_storage_view,
            &compose_indirection,
            sh,
            uniform_bind_group_layout,
            sparse_capacity_floors.get(&27).copied(),
        )?;
        let direct_compose = if direct_compose_required {
            Some(StreamingDirectCompose::new(
                device,
                base,
                sources,
                shape,
                StreamingDirectViews {
                    base: direct_base_view
                        .as_ref()
                        .ok_or(ShResidencyDrainError::GpuCapacity {
                            reason: "streamed direct compose lost its id-35 base view",
                        })?,
                    intermediate_storage: direct_intermediate_storage_view.as_ref(),
                    intermediate_sampled: direct_intermediate_sampled_view.as_ref(),
                    total_storage: direct_total_storage_view.as_ref().ok_or(
                        ShResidencyDrainError::GpuCapacity {
                            reason: "streamed direct compose lost its total storage view",
                        },
                    )?,
                    selection_weights,
                },
                &compose_indirection,
                sh,
                uniform_bind_group_layout,
                sparse_capacity_floors,
            )?)
        } else {
            None
        };
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Streamed SH Atlas Sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });
        let direct_view = direct_total_sampled_view
            .as_ref()
            .or(direct_base_view.as_ref())
            .unwrap_or(&sh.direct.atlas_view);
        let entries = vec![
            wgpu::BindGroupEntry {
                binding: BIND_SH_TOTAL_ATLAS,
                resource: wgpu::BindingResource::TextureView(&total_sampled_view),
            },
            wgpu::BindGroupEntry {
                binding: BIND_SH_ATLAS_SAMPLER,
                resource: wgpu::BindingResource::Sampler(&sampler),
            },
            wgpu::BindGroupEntry {
                binding: BIND_SH_GRID_INFO,
                resource: grid_info.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: BIND_ANIM_DESCRIPTORS,
                resource: sh.animation.descriptors.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: BIND_ANIM_SAMPLES,
                resource: sh.animation.anim_samples.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: BIND_SCRIPTED_LIGHT_DESCRIPTORS,
                resource: sh.scripted_light_descriptors.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: BIND_SH_DEPTH_MOMENTS,
                resource: wgpu::BindingResource::TextureView(&depth_view),
            },
            wgpu::BindGroupEntry {
                binding: BIND_SH_DIRECT_ATLAS,
                resource: wgpu::BindingResource::TextureView(direct_view),
            },
            wgpu::BindGroupEntry {
                binding: BIND_BILLBOARD_DIRECT_SCATTER,
                resource: wgpu::BindingResource::TextureView(
                    &sh.billboard_direct_scatter.sampled_view,
                ),
            },
        ];
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Streamed SH Volume Bind Group"),
            layout: &sh.bind_group_layout,
            entries: &entries,
        });
        let mut mesh_entries = entries;
        mesh_entries.push(wgpu::BindGroupEntry {
            binding: BIND_DYNAMIC_DIRECT_PARAMS,
            resource: sh.direct.dynamic_direct_params_binding(),
        });
        let mesh_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Streamed SH Mesh Bind Group"),
            layout: &sh.mesh_bind_group_layout,
            entries: &mesh_entries,
        });

        let active_capacity_bytes = texture_bytes(base_format, extent)
            .checked_add(texture_bytes(wgpu::TextureFormat::Rgba16Float, extent))
            .and_then(|bytes| {
                direct_format.map_or(Some(bytes), |format| {
                    bytes.checked_add(texture_bytes(format, extent))
                })
            })
            .and_then(|bytes| {
                direct_total.as_ref().map_or(Some(bytes), |_| {
                    bytes.checked_add(texture_bytes(wgpu::TextureFormat::Rgba16Float, extent))
                })
            })
            .and_then(|bytes| {
                direct_intermediate.as_ref().map_or(Some(bytes), |_| {
                    bytes.checked_add(texture_bytes(wgpu::TextureFormat::Rgba16Float, extent))
                })
            })
            .and_then(|bytes| bytes.checked_add(indirect_compose.active_pool_bytes()))
            .and_then(|bytes| {
                bytes.checked_add(
                    direct_compose
                        .as_ref()
                        .map_or(0, StreamingDirectCompose::active_capacity_bytes),
                )
            })
            .ok_or(ShResidencyDrainError::SlotOverflow)?;
        let fixed_metadata_bytes = indirection_len
            .checked_mul(2)
            .and_then(|bytes| bytes.checked_add(96))
            .and_then(|bytes| {
                bytes.checked_add(texture_bytes(
                    wgpu::TextureFormat::Rgba16Uint,
                    wgpu::Extent3d {
                        width: base.grid_dimensions[0].max(1),
                        height: base.grid_dimensions[1].max(1),
                        depth_or_array_layers: base.grid_dimensions[2].max(1),
                    },
                ))
            })
            .and_then(|bytes| bytes.checked_add(indirect_compose.fixed_metadata_bytes()))
            .and_then(|bytes| {
                bytes.checked_add(
                    direct_compose
                        .as_ref()
                        .map_or(0, StreamingDirectCompose::fixed_metadata_bytes),
                )
            })
            .ok_or(ShResidencyDrainError::SlotOverflow)?;
        // Animation/sample/scripted descriptor buffers are shared immutable
        // ShVolumeResources allocations. They participate in the initial
        // 256 MiB floor calculation, but are already represented by static
        // ledger rows and must not be charged again in the live streaming
        // summary.

        Ok(Self {
            shape,
            base_format,
            direct_format,
            base: base_texture,
            total: total_texture,
            base_view,
            total_storage_view,
            total_sampled_view,
            direct_base,
            direct_base_view,
            direct_intermediate,
            direct_intermediate_storage_view,
            direct_intermediate_sampled_view,
            direct_total,
            direct_total_storage_view,
            direct_total_sampled_view,
            depth_moments,
            compose_indirection,
            sampled_indirection,
            grid_info,
            indirect_compose,
            direct_compose,
            has_animated_direct_pass: sources.animated_direct_delta.is_some(),
            bind_group,
            mesh_bind_group,
            retiring_dense: None,
            retiring_indirect: None,
            retiring_direct_promotion: None,
            retiring_direct_animated: None,
            active_capacity_bytes,
            fixed_metadata_bytes,
            whole_resident_scatter_bytes: sh.billboard_direct_scatter.capacity_bytes(),
            effective_floor_bytes,
            dense_group_minimum_bytes,
            sparse_group_minimum_bytes,
            probe_occlusion_enabled,
            sparse_capacity_floors: sparse_capacity_floors.clone(),
            growth: PoolGrowthCounters::default(),
        })
    }
}
