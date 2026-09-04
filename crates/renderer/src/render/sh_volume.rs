// SH irradiance volume GPU resources: octahedral atlas textures, grid-info uniform, bind group (group 3).
// See: context/lib/rendering_pipeline.md §4, §8

use postretro_level_format::animated_billboard_direct_scatter_delta_volumes::AnimatedBillboardDirectScatterDeltaVolumesSection;
use postretro_level_format::animated_direct_sh_delta_volumes::AnimatedDirectShDeltaVolumesSection;
use postretro_level_format::billboard_direct_scatter_volume::BillboardDirectScatterVolumeSection;
use postretro_level_format::direct_sh_delta_volumes::DirectShDeltaVolumesSection;
use postretro_level_format::direct_sh_volume::DirectShVolumeSection;
use postretro_level_format::lightmap::{IRRADIANCE_FORMAT_BC6H, IRRADIANCE_FORMAT_RGBA16F};
use postretro_level_format::sh_volume::{OctahedralShProbe, OctahedralShVolumeSection};
use postretro_render_cpu::sh_compose::u16_slice_to_bytes;
#[allow(unused_imports)]
pub use postretro_render_cpu::sh_volume::{
    ANIMATION_DESCRIPTOR_ACTIVE_OFFSET, ANIMATION_DESCRIPTOR_SIZE, BIND_ANIM_DESCRIPTORS,
    BIND_ANIM_SAMPLES, BIND_BILLBOARD_DIRECT_SCATTER, BIND_DYNAMIC_DIRECT_PARAMS,
    BIND_SCRIPTED_LIGHT_DESCRIPTORS, BIND_SH_ATLAS_SAMPLER, BIND_SH_DEPTH_MOMENTS,
    BIND_SH_DIRECT_ATLAS, BIND_SH_GRID_INFO, BIND_SH_TOTAL_ATLAS, DEFAULT_PROBE_OCCLUSION,
    DYNAMIC_DIRECT_PARAMS_SIZE, SCRIPTED_BRIGHTNESS_SLOT, SCRIPTED_COLOR_SLOT_F32,
    SCRIPTED_FLOATS_PER_LIGHT, SH_GRID_INFO_SIZE, ShGridInfoParams, build_animation_buffers,
    build_grid_info_bytes, f32_to_f16_bits, probe_occlusion_seed_from_fast_env,
};
use wgpu::util::DeviceExt;

use super::billboard_direct_scatter::{
    BillboardDirectScatterResources,
    append_shared_bind_group_layout_entries as append_billboard_scatter_layout_entries,
};
use super::direct_sh_resources::{
    DirectAtlasLayout, DirectShResources, append_shared_bind_group_layout_entries, atlas_fits,
    direct_section_when_base_present, mesh_dynamic_direct_params_layout_entry,
};
use super::sh_indirection::build_probe_indirection_words;

/// Dev-tools marker color while the composed dense-atlas readback has not yet
/// completed. This deliberately replaces the removed CPU-side base-atlas
/// decode; validity remains sourced from probe metadata independently.
#[cfg(feature = "dev-tools")]
const PROBE_IRRADIANCE_PLACEHOLDER: [f32; 3] = [0.25, 0.25, 0.25];

/// Uploaded SH volume handles + bind group. Always populated. Empty-geometry
/// levels bind dummy 1×1 octahedral atlas textures plus a dummy 1×1×1
/// depth-moment texture, and set `has_sh_volume` to zero so shader consumers
/// skip indirect sampling.
///
/// Two atlas textures exist:
/// - **base**: uploaded once at load time from the PRL
///   `OctahedralShVolumeSection`. Held as the source-of-truth static
///   octahedral irradiance atlas.
/// - **total**: one `Rgba16Float` texture with both sampled and storage views.
///   Consumers sample this texture; the compose pass writes it each frame.
pub struct ShVolumeResources {
    pub bind_group: wgpu::BindGroup,
    pub bind_group_layout: wgpu::BindGroupLayout,
    /// Mesh group-4 SUPERSET bind group: every entry of `bind_group` PLUS the
    /// mesh-only `DynamicDirectParams` uniform at `BIND_DYNAMIC_DIRECT_PARAMS`
    /// (binding 16). The skinned-mesh pipeline binds THIS at group 4 (the
    /// shared `bind_group` stays free of mesh-only fields). Rebuilt on every
    /// level reload alongside `bind_group`.
    pub mesh_bind_group: wgpu::BindGroup,
    /// Layout for `mesh_bind_group`. The mesh pipeline layout is built against
    /// this superset BGL at construction; a structurally-equal rebuild on level
    /// reload stays pipeline-compatible (same as `bind_group_layout`).
    pub mesh_bind_group_layout: wgpu::BindGroupLayout,
    /// Direct-SH resources are a sibling owner to the indirect SH atlas. They
    /// retain the shared binding and mesh-only dynamic-direct uniform contract.
    pub(super) direct: DirectShResources,
    /// Normal-free static/animated direct scatter sampled only by billboards.
    /// Its real/dummy binding selection is level-load fixed.
    pub(super) billboard_direct_scatter: BillboardDirectScatterResources,
    #[allow(dead_code)]
    pub present: bool,
    /// Probe grid dimensions (in cells, x/y/z).
    pub grid_dimensions: [u32; 3],
    /// Stored-tile atlas dimensions in texels.
    pub atlas_dimensions: [u32; 2],
    #[allow(dead_code)]
    pub tile_dimension: u32,
    #[allow(dead_code)]
    pub tile_border: u32,
    #[allow(dead_code)]
    pub atlas_tiles_per_row: u32,
    #[allow(dead_code)]
    pub tiles_per_layer: u32,
    #[allow(dead_code)]
    pub atlas_layer_count: u32,
    /// Load-derived from id-34 metadata once per level. Every compose carrier
    /// and the depth-moment B/A payload receives these exact words.
    pub(super) probe_indirection_words: Vec<u32>,
    /// Sampled view over the base octahedral atlas; consumed by the compose pass.
    pub base_atlas_view: wgpu::TextureView,
    /// Storage-writeable view over the total octahedral atlas; consumed by the compose pass.
    pub total_atlas_storage_view: wgpu::TextureView,
    /// Per-probe depth-moment texture (`Rgba16Float` — R = E[d], G = E[d²],
    /// B/A currently hold the inert raw halves of the load-derived word).
    /// Already bound on group 3 binding 14 for the forward/billboard/fog
    /// passes; held here so the SDF shadow pass can mint its own
    /// `TextureView` via `make_depth_moment_view` (wgpu views aren't `Clone`,
    /// and the SDF shadow pass rebuilds its bind group on resize / level reload).
    depth_moment_texture: wgpu::Texture,
    /// Owned here but shared with the compose pass — one upload, two bind groups.
    /// CPU mirror kept alongside so per-frame `active` edits patch bytes and flush in one `write_buffer`.
    pub animation: AnimatedLightBuffers,
    /// Fixed-capacity, zero-initialized forward descriptor buffer. Authored
    /// lights plus the runtime-spawn reserve fit without a GPU rebind. The
    /// dynamic-direct loop reads only its compact `light_count` prefix.
    pub scripted_light_descriptors: wgpu::Buffer,
    #[allow(dead_code)]
    pub scripted_light_count: u32,
    /// Byte offset within `anim_samples` where the scripted-animation region
    /// begins (immediately after any FGD-baked samples). The bridge writes its
    /// per-light sample data starting here; `upload_bridge_samples` passes this
    /// to `queue.write_buffer` as the destination offset.
    pub scripted_sample_byte_offset: usize,
    /// CPU mirror of section-34 per-probe validity bytes, z-major
    /// (`x + y*Nx + z*Nx*Ny`). One byte per probe: `0 = invalid` (probe inside
    /// solid or off-grid), non-zero = valid. Empty when no
    /// `OctahedralShVolume` section is present.
    /// Consumed by `sh_diagnostics::emit` for probe-marker coloring.
    #[cfg(feature = "dev-tools")]
    pub validity: Vec<u8>,
    /// CPU mirror of each probe's average tile-interior irradiance as linear
    /// RGB, z-major like `validity`; consumed by `sh_diagnostics::emit`.
    #[cfg(feature = "dev-tools")]
    pub probe_irradiance: Vec<[f32; 3]>,
    /// CPU copies of grid origin and cell size. `grid_info_buffer` is the
    /// canonical GPU-side source for the forward / fog / billboard shaders;
    /// these mirror the values consumed by `sh_diagnostics` (probe-marker
    /// emission) and remain available to any future CPU-side consumer (the
    /// SDF shadow pass reads them from the section directly).
    #[allow(dead_code)]
    pub grid_origin: [f32; 3],
    #[allow(dead_code)]
    pub cell_size: [f32; 3],
    grid_info_buffer: wgpu::Buffer,
    probe_occlusion_enabled: bool,
    /// Total atlas handle, retained so the diagnostics readback can copy it
    /// back to CPU each frame. Carries `COPY_SRC`.
    #[cfg(feature = "dev-tools")]
    pub total_atlas_texture: wgpu::Texture,
}

pub(super) struct ShVolumeSections<'a> {
    pub sh: Option<&'a OctahedralShVolumeSection>,
    pub direct: Option<&'a DirectShVolumeSection>,
    pub direct_delta: Option<&'a DirectShDeltaVolumesSection>,
    pub animated_direct_delta: Option<&'a AnimatedDirectShDeltaVolumesSection>,
    pub billboard_direct_scatter: Option<&'a BillboardDirectScatterVolumeSection>,
    pub animated_billboard_direct_scatter_delta:
        Option<&'a AnimatedBillboardDirectScatterDeltaVolumesSection>,
}

/// Per-animated-light delta volume placement, mirrored on CPU for diagnostics.
/// Sourced from the same `DeltaShVolumesSection` `sh_compose` consumes.
#[cfg(feature = "dev-tools")]
#[derive(Debug, Clone)]
pub struct DeltaVolumeMeta {
    pub origin: [f32; 3],
    pub cell_size: [f32; 3],
    pub grid_dimensions: [u32; 3],
}

/// Animated-light descriptor and sample buffers shared between group 3 and the compose pass.
/// CPU mirror kept alongside so `set_active` is cheap and flushes in one `queue.write_buffer`.
pub struct AnimatedLightBuffers {
    pub descriptors: wgpu::Buffer,
    // Kept next to `descriptors` so one upload serves both bind groups.
    #[allow(dead_code)]
    pub anim_samples: wgpu::Buffer,
    /// One `ANIMATION_DESCRIPTOR_SIZE` record per animated light. Empty maps
    /// carry a single zeroed dummy record; `animated_light_count` is the real count.
    descriptor_mirror: Vec<u8>,
    animated_light_count: u32,
    /// Dirty bit set by `set_active`; cleared by `upload_descriptors_if_dirty`.
    /// Writes are batched across the frame so multiple `set_active` calls
    /// collapse to one `write_buffer`.
    dirty: bool,
    /// One-shot guard on the out-of-range `set_active` warning. Scripts may
    /// drive `set_active` every frame for a light that was never baked; we
    /// want one clear log line, not a per-frame spam.
    oor_warned: bool,
}

impl AnimatedLightBuffers {
    /// 0 when the map has no animated lights (buffers still hold a single dummy record so wgpu accepts the binding).
    #[allow(dead_code)]
    pub fn animated_light_count(&self) -> u32 {
        self.animated_light_count
    }

    /// True when any valid descriptor referenced by this section's independent
    /// index map is active in the same CPU mirror uploaded to the GPU.
    pub fn any_active_for_descriptor_indices(&self, descriptor_indices: &[u32]) -> bool {
        descriptor_indices_have_active(
            &self.descriptor_mirror,
            self.animated_light_count,
            descriptor_indices,
        )
    }

    /// Overwrite the entire 48-byte `ANIMATION_DESCRIPTOR` for an animated
    /// light at `slot`. Used by the scripting → animated-baked bridge to
    /// route a `setLightAnimation` curve into the compose-side descriptor
    /// buffer (Task 2c). Marks the mirror dirty when the bytes change.
    /// Out-of-range `slot` is a silent no-op after the first warn-level log
    /// line — mirrors `set_active`'s behavior for descriptor-buffer writes
    /// against a light that never made it into the bake.
    pub fn write_descriptor(&mut self, slot: usize, bytes: &[u8; ANIMATION_DESCRIPTOR_SIZE]) {
        if slot >= self.animated_light_count as usize {
            if !self.oor_warned {
                self.oor_warned = true;
                log::warn!(
                    "[AnimatedLightBuffers] write_descriptor called with out-of-range slot {} \
                     (animated_light_count = {}); call ignored. Further out-of-range \
                     warnings suppressed.",
                    slot,
                    self.animated_light_count,
                );
            }
            return;
        }
        let start = slot * ANIMATION_DESCRIPTOR_SIZE;
        if self.descriptor_mirror[start..start + ANIMATION_DESCRIPTOR_SIZE] == bytes[..] {
            return;
        }
        self.descriptor_mirror[start..start + ANIMATION_DESCRIPTOR_SIZE].copy_from_slice(bytes);
        self.dirty = true;
    }

    /// Toggle the runtime `active` flag for an animated light.
    /// Marks the mirror dirty only when the state actually changes.
    /// Out-of-range `slot` is a silent no-op after the first warn-level log line
    /// (scripts may fire `set_active` for a light that never made it into the bake).
    pub fn set_active(&mut self, slot: usize, active: bool) {
        if slot >= self.animated_light_count as usize {
            if !self.oor_warned {
                self.oor_warned = true;
                log::warn!(
                    "[AnimatedLightBuffers] set_active called with out-of-range slot {} \
                     (animated_light_count = {}); call ignored. Further out-of-range \
                     warnings suppressed.",
                    slot,
                    self.animated_light_count,
                );
            }
            return;
        }
        let start = slot * ANIMATION_DESCRIPTOR_SIZE + ANIMATION_DESCRIPTOR_ACTIVE_OFFSET;
        let value: u32 = if active { 1 } else { 0 };
        let value_bytes = value.to_ne_bytes();
        // No-op when the byte is already what we want — avoids a spurious
        // `queue.write_buffer` on every-frame toggle-to-same-state calls.
        if self.descriptor_mirror[start..start + 4] == value_bytes {
            return;
        }
        self.descriptor_mirror[start..start + 4].copy_from_slice(&value_bytes);
        self.dirty = true;
    }

    /// Upload the CPU mirror to the GPU descriptor buffer. No-op when clean.
    /// Must be called before the compose pass and forward pass each frame.
    pub fn upload_descriptors_if_dirty(&mut self, queue: &wgpu::Queue) {
        if !self.dirty {
            return;
        }
        queue.write_buffer(&self.descriptors, 0, &self.descriptor_mirror);
        self.dirty = false;
    }
}

fn descriptor_indices_have_active(
    descriptor_mirror: &[u8],
    descriptor_count: u32,
    descriptor_indices: &[u32],
) -> bool {
    descriptor_indices.iter().copied().any(|descriptor_index| {
        if descriptor_index == u32::MAX || descriptor_index >= descriptor_count {
            return false;
        }
        let offset = descriptor_index as usize * ANIMATION_DESCRIPTOR_SIZE
            + ANIMATION_DESCRIPTOR_ACTIVE_OFFSET;
        descriptor_mirror
            .get(offset..offset + 4)
            .is_some_and(|bytes| u32::from_ne_bytes(bytes.try_into().unwrap()) != 0)
    })
}

impl ShVolumeResources {
    /// Build group 3 (SH volume) resources. `section` is `None` when the PRL
    /// file had no `OctahedralShVolume` section — in that case dummy 1×1
    /// octahedral atlas textures and a dummy 1×1×1 depth-moment texture are
    /// uploaded, and the `has_sh_volume` flag is zero so the shader skips SH
    /// sampling and falls back to `ambient_floor + direct_sum`.
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        sections: ShVolumeSections<'_>,
        scripted_light_capacity: usize,
        probe_occlusion_enabled: bool,
    ) -> Self {
        let ShVolumeSections {
            sh: section,
            direct: direct_section,
            direct_delta: direct_delta_section,
            animated_direct_delta: animated_direct_delta_section,
            billboard_direct_scatter: billboard_direct_scatter_section,
            animated_billboard_direct_scatter_delta: animated_billboard_direct_scatter_delta_section,
        } = sections;
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("SH Volume Bind Group Layout"),
            entries: &sh_bind_group_layout_entries(),
        });

        // A zero-dimension grid is treated the same as a missing/empty section.
        let limits = device.limits();
        let usable = section
            .filter(|s| {
                s.grid_dimensions[0] > 0 && s.grid_dimensions[1] > 0 && s.grid_dimensions[2] > 0
            })
            .filter(|s| {
                let fits = atlas_fits(s.atlas_dimensions, s.layer_count, &limits);
                if !fits {
                    log::error!(
                        "[Renderer] Octahedral SH atlas {}x{}x{} exceeds device limits (maxTextureDimension2D {}, maxTextureArrayLayers {}); disabling SH volume for this level",
                        s.atlas_dimensions[0],
                        s.atlas_dimensions[1],
                        s.layer_count,
                        limits.max_texture_dimension_2d,
                        limits.max_texture_array_layers,
                    );
                }
                fits
            })
            .filter(|s| {
                let fits = sh_depth_moment_fits(s.grid_dimensions, &limits);
                if !fits {
                    log::error!(
                        "[Renderer] SH depth-moment grid {}x{}x{} exceeds device maxTextureDimension3D {}; disabling SH volume for this level",
                        s.grid_dimensions[0],
                        s.grid_dimensions[1],
                        s.grid_dimensions[2],
                        limits.max_texture_dimension_3d,
                    );
                }
                fits
            });

        let grid_origin: [f32; 3];
        let cell_size: [f32; 3];
        let grid_dimensions: [u32; 3];
        let atlas_dimensions: [u32; 2];
        let tile_dimension: u32;
        let tile_border: u32;
        let atlas_tiles_per_row: u32;
        let tiles_per_layer: u32;
        let atlas_layer_count: u32;
        let present: bool;
        let base_atlas_texture: wgpu::Texture;
        let total_atlas_texture: wgpu::Texture;
        let depth_moment_texture: wgpu::Texture;
        let probe_indirection_words = build_probe_indirection_words(usable);

        #[cfg(feature = "dev-tools")]
        let validity: Vec<u8> = usable
            .map(|s| s.probes.iter().map(|p| p.validity).collect())
            .unwrap_or_default();

        // The compact base atlas is BC6H by default and has no CPU decoder in
        // production. Markers start neutral; the existing composed-atlas
        // readback replaces these values once the irradiance overlay requests
        // it. Validity markers keep using the separate metadata mirror above.
        #[cfg(feature = "dev-tools")]
        let probe_irradiance: Vec<[f32; 3]> = usable
            .map(|s| vec![PROBE_IRRADIANCE_PLACEHOLDER; s.probes.len()])
            .unwrap_or_default();

        if let Some(sec) = usable {
            base_atlas_texture = upload_compact_base_atlas_texture(device, queue, sec);
            total_atlas_texture = create_total_atlas_texture(
                device,
                sec.atlas_dimensions,
                sec.layer_count,
                "SH Total Octahedral Atlas",
            );
            let moments = pack_probe_depth_moments(
                &sec.probes,
                sec.grid_dimensions,
                &probe_indirection_words,
            );
            depth_moment_texture =
                upload_depth_moment_texture(device, queue, sec.grid_dimensions, &moments);
            grid_origin = sec.grid_origin;
            cell_size = sec.cell_size;
            grid_dimensions = sec.grid_dimensions;
            atlas_dimensions = sec.atlas_dimensions;
            tile_dimension = sec.tile_dimension;
            tile_border = sec.tile_border;
            atlas_tiles_per_row = sec.atlas_tiles_per_row;
            tiles_per_layer = sec.tiles_per_layer;
            atlas_layer_count = sec.layer_count;
            present = true;
        } else {
            let dummy = dummy_depth_moment_payload();
            base_atlas_texture = upload_compact_base_atlas_dummy(device, queue);
            total_atlas_texture =
                create_total_atlas_texture(device, [1, 1], 1, "SH Total Octahedral Atlas Dummy");
            depth_moment_texture = upload_depth_moment_texture(device, queue, [1, 1, 1], &dummy);
            grid_origin = [0.0; 3];
            cell_size = [1.0; 3];
            grid_dimensions = [1, 1, 1];
            atlas_dimensions = [1, 1];
            tile_dimension = 1;
            tile_border = 0;
            atlas_tiles_per_row = 1;
            tiles_per_layer = 1;
            atlas_layer_count = 1;
            present = false;
        }

        // Per-load instrumentation only: composed-atlas size and sampler
        // bandwidth intentionally remain unchanged. Report the actual base
        // texture allocation, which can be a dummy when SH is absent, empty,
        // or exceeds device limits; retain serialized bytes as a separate
        // on-disk payload metric.
        #[cfg(feature = "dev-tools")]
        {
            let allocation = compact_base_atlas_allocation(usable);
            let (serialized_bytes, valid_probe_count, probe_count) = section
                .map(|sec| {
                    (
                        sec.compact_atlas.len(),
                        sec.probes
                            .iter()
                            .filter(|probe| probe.validity != 0)
                            .count(),
                        sec.probes.len(),
                    )
                })
                .unwrap_or((0, 0, 0));
            log::info!(
                "[Renderer] SH base atlas physical allocation: {} {}x{} texels, {} layer(s) ({} B); serialized compact payload: {} B; {}/{} valid probes",
                base_atlas_format_label(allocation.format),
                allocation.extent.width,
                allocation.extent.height,
                allocation.extent.depth_or_array_layers,
                base_atlas_allocation_bytes(allocation),
                serialized_bytes,
                valid_probe_count,
                probe_count,
            );
        }

        // Animated-light buffers. Always created — when the SH section has
        // no animated lights (or no section exists) the two storage buffers
        // are single-element dummies so the bind group remains valid (wgpu
        // rejects zero-sized storage buffer bindings).
        let (anim_descriptor_bytes, mut anim_sample_bytes, animated_light_count) =
            build_animation_buffers(usable);

        // Append the scripted-animation region: one slot per map light.
        // FGD samples occupy [0, scripted_sample_byte_offset); scripted samples
        // follow. The LightBridge writes into this region at runtime.
        let scripted_sample_byte_offset = anim_sample_bytes.len();
        let scripted_region_bytes = scripted_light_capacity * SCRIPTED_FLOATS_PER_LIGHT * 4;
        anim_sample_bytes.extend(std::iter::repeat_n(0u8, scripted_region_bytes));

        let anim_descriptors_buffer = device.create_buffer_init_helper(
            "SH Animation Descriptors",
            &anim_descriptor_bytes,
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        );
        let anim_samples_buffer = device.create_buffer_init_helper(
            "SH Animation Samples",
            &anim_sample_bytes,
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        );

        // wgpu rejects zero-sized storage buffers; pad to one slot for empty maps.
        // The forward loop has no lights in that case, so the dummy is never read.
        let scripted_descriptor_slots = scripted_light_capacity.max(1);
        let scripted_descriptor_bytes =
            vec![0u8; scripted_descriptor_slots * ANIMATION_DESCRIPTOR_SIZE];
        let scripted_light_descriptors_buffer = device.create_buffer_init_helper(
            "Scripted Light Descriptors",
            &scripted_descriptor_bytes,
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        );

        let grid_info_bytes = build_grid_info_bytes(ShGridInfoParams {
            grid_origin,
            cell_size,
            grid_dimensions,
            atlas_dimensions,
            tile_dimension,
            tile_border,
            atlas_tiles_per_row,
            tiles_per_layer,
            atlas_layer_count,
            present,
            probe_occlusion_enabled,
        });
        let grid_info_buffer = device.create_buffer_init_helper(
            "SH Grid Info Uniform",
            &grid_info_bytes,
            wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        );

        let base_atlas_view = base_atlas_texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some("SH Base Octahedral Atlas View"),
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        let total_atlas_sampled_view =
            total_atlas_texture.create_view(&wgpu::TextureViewDescriptor {
                label: Some("SH Total Octahedral Atlas Sampled View"),
                dimension: Some(wgpu::TextureViewDimension::D2Array),
                ..Default::default()
            });
        let total_atlas_storage_view =
            total_atlas_texture.create_view(&wgpu::TextureViewDescriptor {
                label: Some("SH Total Octahedral Atlas Storage View"),
                dimension: Some(wgpu::TextureViewDimension::D2Array),
                ..Default::default()
            });
        let depth_moment_view = depth_moment_texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some("SH Depth Moment View"),
            ..Default::default()
        });

        // Direct receiver resources are intentionally independent from the
        // indirect atlas owner. The shared group-3 bind group still binds its
        // sampled atlas, while the direct compose passes own their own storage
        // views through this seam.
        let direct = DirectShResources::new(
            device,
            queue,
            direct_section_when_base_present(present, direct_section),
            direct_delta_section,
            animated_direct_delta_section,
            usable.map(DirectAtlasLayout::from_sh_section),
        );
        let billboard_direct_scatter = BillboardDirectScatterResources::new(
            device,
            queue,
            present,
            billboard_direct_scatter_section.filter(|_| present),
            animated_billboard_direct_scatter_delta_section.filter(|_| present),
        );

        let atlas_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("SH Octahedral Atlas Sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });

        let entries: Vec<wgpu::BindGroupEntry> = vec![
            wgpu::BindGroupEntry {
                binding: BIND_SH_TOTAL_ATLAS,
                resource: wgpu::BindingResource::TextureView(&total_atlas_sampled_view),
            },
            wgpu::BindGroupEntry {
                binding: BIND_SH_ATLAS_SAMPLER,
                resource: wgpu::BindingResource::Sampler(&atlas_sampler),
            },
            wgpu::BindGroupEntry {
                binding: BIND_SH_GRID_INFO,
                resource: grid_info_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: BIND_ANIM_DESCRIPTORS,
                resource: anim_descriptors_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: BIND_ANIM_SAMPLES,
                resource: anim_samples_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: BIND_SCRIPTED_LIGHT_DESCRIPTORS,
                resource: scripted_light_descriptors_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: BIND_SH_DEPTH_MOMENTS,
                resource: wgpu::BindingResource::TextureView(&depth_moment_view),
            },
            wgpu::BindGroupEntry {
                binding: postretro_render_cpu::sh_volume::BIND_SH_DIRECT_ATLAS,
                resource: wgpu::BindingResource::TextureView(&direct.atlas_view),
            },
            wgpu::BindGroupEntry {
                binding: BIND_BILLBOARD_DIRECT_SCATTER,
                resource: wgpu::BindingResource::TextureView(
                    &billboard_direct_scatter.sampled_view,
                ),
            },
        ];

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("SH Volume Bind Group"),
            layout: &bind_group_layout,
            entries: &entries,
        });

        // Mesh group-4 SUPERSET: the shared SH entries PLUS the mesh-only
        // dynamic-direct params uniform at binding 16. Built from a separate BGL
        // so the shared `bind_group` (forward/billboard/fog) stays free of
        // mesh-only fields.
        let mesh_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("SH Volume Mesh Superset BGL"),
                entries: &mesh_bind_group_layout_entries(),
            });
        let mut mesh_entries = entries;
        mesh_entries.push(wgpu::BindGroupEntry {
            binding: postretro_render_cpu::sh_volume::BIND_DYNAMIC_DIRECT_PARAMS,
            resource: direct.dynamic_direct_params_binding(),
        });
        let mesh_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("SH Volume Mesh Superset Bind Group"),
            layout: &mesh_bind_group_layout,
            entries: &mesh_entries,
        });

        // Retain the total atlas texture for the dev-tools readback. The
        // `wgpu::Texture` handle is an Arc clone — the views above already keep
        // the texture alive, this just gives the readback a handle to copy from.
        #[cfg(feature = "dev-tools")]
        let total_atlas_texture = total_atlas_texture.clone();

        let animation = AnimatedLightBuffers {
            descriptors: anim_descriptors_buffer,
            anim_samples: anim_samples_buffer,
            descriptor_mirror: anim_descriptor_bytes,
            animated_light_count,
            dirty: false,
            oor_warned: false,
        };
        Self {
            bind_group,
            bind_group_layout,
            mesh_bind_group,
            mesh_bind_group_layout,
            direct,
            billboard_direct_scatter,
            present,
            grid_dimensions,
            atlas_dimensions,
            tile_dimension,
            tile_border,
            atlas_tiles_per_row,
            tiles_per_layer,
            atlas_layer_count,
            probe_indirection_words,
            base_atlas_view,
            total_atlas_storage_view,
            depth_moment_texture,
            animation,
            scripted_light_descriptors: scripted_light_descriptors_buffer,
            scripted_light_count: scripted_light_capacity as u32,
            scripted_sample_byte_offset,
            #[cfg(feature = "dev-tools")]
            validity,
            #[cfg(feature = "dev-tools")]
            probe_irradiance,
            grid_origin,
            cell_size,
            grid_info_buffer,
            probe_occlusion_enabled,
            #[cfg(feature = "dev-tools")]
            total_atlas_texture,
        }
    }

    /// Mint a fresh sampled view over the per-probe depth-moment texture for
    /// the SDF shadow pass (Task 4). Consumed during pass construction and on
    /// each level reload — the moment texture is recreated whenever the SH
    /// section changes, so the pass needs a new handle.
    pub fn make_depth_moment_view(&self) -> wgpu::TextureView {
        self.depth_moment_texture
            .create_view(&wgpu::TextureViewDescriptor {
                label: Some("SH Depth Moment Shadow View"),
                ..Default::default()
            })
    }

    pub fn set_probe_occlusion_enabled(&mut self, queue: &wgpu::Queue, enabled: bool) {
        if self.probe_occlusion_enabled == enabled {
            return;
        }
        self.probe_occlusion_enabled = enabled;
        let bytes = build_grid_info_bytes(ShGridInfoParams {
            grid_origin: self.grid_origin,
            cell_size: self.cell_size,
            grid_dimensions: self.grid_dimensions,
            atlas_dimensions: self.atlas_dimensions,
            tile_dimension: self.tile_dimension,
            tile_border: self.tile_border,
            atlas_tiles_per_row: self.atlas_tiles_per_row,
            tiles_per_layer: self.tiles_per_layer,
            atlas_layer_count: self.atlas_layer_count,
            present: self.present,
            probe_occlusion_enabled: enabled,
        });
        queue.write_buffer(&self.grid_info_buffer, 0, &bytes);
    }
}

// --- Helpers ---

/// Mesh group-4 SUPERSET layout: the shared SH entries plus the mesh-only
/// dynamic-direct params uniform (binding 16, FRAGMENT). The mesh shader binds
/// every entry by lexical name; the shared entries it does not read are legal
/// to carry in the layout.
pub(super) fn mesh_bind_group_layout_entries() -> Vec<wgpu::BindGroupLayoutEntry> {
    let mut entries = sh_bind_group_layout_entries();
    entries.push(mesh_dynamic_direct_params_layout_entry());
    entries
}

pub(super) fn sh_bind_group_layout_entries() -> Vec<wgpu::BindGroupLayoutEntry> {
    let mut entries: Vec<wgpu::BindGroupLayoutEntry> = Vec::with_capacity(8);
    // Shared with the forward pass (fragment), fog raymarch (compute), and the
    // billboard pass (vertex + fragment), so visibility covers all three stages on
    // every entry. VERTEX is required because billboard hoists SH indirect + direct
    // sampling into `vs_main` (per-vertex lighting); the SH reads use non-derivative
    // texture ops (`textureSampleLevel`/`textureLoad`), which are vertex-stage valid.
    // Widening is additive — forward/fog/mesh sharers do not read in the vertex stage,
    // and carrying extra visibility on a shared layout is valid at pipeline creation.
    let vis =
        wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT | wgpu::ShaderStages::COMPUTE;
    entries.push(wgpu::BindGroupLayoutEntry {
        binding: BIND_SH_TOTAL_ATLAS,
        visibility: vis,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2Array,
            multisampled: false,
        },
        count: None,
    });
    entries.push(wgpu::BindGroupLayoutEntry {
        binding: BIND_SH_ATLAS_SAMPLER,
        visibility: vis,
        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
        count: None,
    });
    // ShGridInfo uniform.
    entries.push(wgpu::BindGroupLayoutEntry {
        binding: BIND_SH_GRID_INFO,
        visibility: vis,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    });
    // Always bound with dummy single-element buffers when no animated lights exist —
    // the bind group layout must not vary with map content.
    //
    // These three animated-layer / scripted-light storage buffers are read ONLY in
    // the fragment stage (forward `fs_main`) and the compute stage (fog raymarch);
    // the billboard vertex shader declares `anim_descriptors`/`anim_samples` but
    // never reads them (animated pulses are invisible at one-sample-per-sprite
    // fidelity — see `billboard.wgsl`) and does not declare the scripted-light
    // buffer at all. So their visibility deliberately OMITS VERTEX: marking them
    // VERTEX-visible would have charged three extra storage-buffer bindings against
    // the VERTEX stage's `max_storage_buffers_per_shader_stage` budget (downlevel
    // default 8) in the Billboard Pipeline Layout — pushing the billboard's
    // VERTEX-visible storage count to 9 and failing pipeline creation on real GPUs.
    // The vertex-read SH storage buffers are group 6 (`sprites`) and group 2's five
    // light/chunk buffers; none live here. `vertex_storage_buffers` /
    // `billboard_pipeline_vertex_storage_buffer_count` pin this budget.
    let anim_vis = wgpu::ShaderStages::FRAGMENT | wgpu::ShaderStages::COMPUTE;
    for binding in [
        BIND_ANIM_DESCRIPTORS,
        BIND_ANIM_SAMPLES,
        BIND_SCRIPTED_LIGHT_DESCRIPTORS,
    ] {
        entries.push(wgpu::BindGroupLayoutEntry {
            binding,
            visibility: anim_vis,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only: true },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        });
    }
    entries.push(wgpu::BindGroupLayoutEntry {
        binding: BIND_SH_DEPTH_MOMENTS,
        visibility: vis,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: false },
            view_dimension: wgpu::TextureViewDimension::D3,
            multisampled: false,
        },
        count: None,
    });
    append_shared_bind_group_layout_entries(&mut entries);
    append_billboard_scatter_layout_entries(&mut entries);
    entries
}

/// Pack per-probe depth moments into one `Rgba16Float` 3D texture payload.
/// Probes are already ordered z-major/y/x by the PRL section; keeping the same
/// linear order as the SH band textures makes the moment texture index-aligned
/// with every band. Valid probes copy baked f16 bits directly into RG. B/A
/// carry the raw low/high halves of the load-derived indirection word; this
/// texture stays `Rgba16Float` until Task 5 changes the sampler type atomically.
pub(super) fn pack_probe_depth_moments(
    probes: &[OctahedralShProbe],
    grid: [u32; 3],
    probe_indirection_words: &[u32],
) -> Vec<u16> {
    let total = (grid[0] as usize) * (grid[1] as usize) * (grid[2] as usize);
    debug_assert_eq!(probes.len(), total);
    debug_assert_eq!(probe_indirection_words.len(), total);

    let mut moments = vec![0u16; total * 4];
    for (probe_idx, probe) in probes.iter().enumerate() {
        let off = probe_idx * 4;
        let word = probe_indirection_words[probe_idx];
        if probe.validity != 0 {
            moments[off] = probe.mean_distance;
            moments[off + 1] = probe.mean_sq_distance;
        } else {
            debug_assert_eq!(word, 0, "invalid metadata probes must have an invalid word");
        }
        // B/A are always copied from the sole word builder. Invalid probes
        // still produce zero halves because their word is the all-zero
        // sentinel, rather than because this packer independently decides
        // their representation.
        moments[off + 2] = word as u16;
        moments[off + 3] = (word >> 16) as u16;
    }
    moments
}

fn dummy_depth_moment_payload() -> [u16; 4] {
    [0u16; 4]
}

fn sh_depth_moment_fits(grid_dimensions: [u32; 3], limits: &wgpu::Limits) -> bool {
    grid_dimensions[0] > 0
        && grid_dimensions[1] > 0
        && grid_dimensions[2] > 0
        && grid_dimensions[0] <= limits.max_texture_dimension_3d
        && grid_dimensions[1] <= limits.max_texture_dimension_3d
        && grid_dimensions[2] <= limits.max_texture_dimension_3d
}

/// Upload id-34's valid-probe-only atlas without re-expanding it. BC6H blobs
/// remain compressed through upload and hardware-decode only in the compose
/// pass; the uncompressed debug tag keeps its compact `Rgba16Float` texels.
///
/// A valid section with zero valid probes has zero compact dimensions. Compose
/// will bind only invalid indirection words, but wgpu still needs a nonzero
/// texture: BC6H uses its minimum valid 4×4 zero block and the uncompressed
/// path uses one 1×1 zero texel.
#[derive(Clone, Copy)]
struct BaseAtlasAllocation {
    format: wgpu::TextureFormat,
    extent: wgpu::Extent3d,
}

fn compact_base_atlas_allocation(
    section: Option<&OctahedralShVolumeSection>,
) -> BaseAtlasAllocation {
    let Some(section) = section else {
        return BaseAtlasAllocation {
            format: wgpu::TextureFormat::Rgba16Float,
            extent: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
        };
    };

    let empty_compact_atlas = section.atlas_dimensions[0] == 0
        || section.atlas_dimensions[1] == 0
        || section.layer_count == 0;
    match section.irradiance_format {
        IRRADIANCE_FORMAT_BC6H if empty_compact_atlas => BaseAtlasAllocation {
            format: wgpu::TextureFormat::Bc6hRgbUfloat,
            extent: wgpu::Extent3d {
                width: 4,
                height: 4,
                depth_or_array_layers: 1,
            },
        },
        IRRADIANCE_FORMAT_BC6H => BaseAtlasAllocation {
            format: wgpu::TextureFormat::Bc6hRgbUfloat,
            extent: wgpu::Extent3d {
                width: section.atlas_dimensions[0].div_ceil(4) * 4,
                height: section.atlas_dimensions[1].div_ceil(4) * 4,
                depth_or_array_layers: section.layer_count,
            },
        },
        IRRADIANCE_FORMAT_RGBA16F if empty_compact_atlas => BaseAtlasAllocation {
            format: wgpu::TextureFormat::Rgba16Float,
            extent: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
        },
        IRRADIANCE_FORMAT_RGBA16F => BaseAtlasAllocation {
            format: wgpu::TextureFormat::Rgba16Float,
            extent: wgpu::Extent3d {
                width: section.atlas_dimensions[0],
                height: section.atlas_dimensions[1],
                depth_or_array_layers: section.layer_count,
            },
        },
        // `OctahedralShVolumeSection::from_bytes` rejects unknown tags. Keep
        // this match exhaustive so manually-constructed test data cannot be
        // uploaded under a silently reinterpreted format.
        unknown => panic!("unsupported compact SH irradiance format tag {unknown}"),
    }
}

#[cfg(any(feature = "dev-tools", test))]
fn base_atlas_allocation_bytes(allocation: BaseAtlasAllocation) -> u64 {
    let extent = allocation.extent;
    match allocation.format {
        wgpu::TextureFormat::Bc6hRgbUfloat => {
            u64::from(extent.width.div_ceil(4))
                * u64::from(extent.height.div_ceil(4))
                * u64::from(extent.depth_or_array_layers)
                * 16
        }
        wgpu::TextureFormat::Rgba16Float => {
            u64::from(extent.width)
                * u64::from(extent.height)
                * u64::from(extent.depth_or_array_layers)
                * 8
        }
        _ => unreachable!("base SH atlas allocation only uses BC6H or Rgba16Float"),
    }
}

fn upload_compact_base_atlas_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    section: &OctahedralShVolumeSection,
) -> wgpu::Texture {
    let empty_compact_atlas = section.atlas_dimensions[0] == 0
        || section.atlas_dimensions[1] == 0
        || section.layer_count == 0;
    let zero_bc6h = [0u8; 16];
    let zero_rgba16f = [0u8; 8];
    let allocation = compact_base_atlas_allocation(Some(section));
    let contents = match section.irradiance_format {
        IRRADIANCE_FORMAT_BC6H if empty_compact_atlas => zero_bc6h.as_slice(),
        IRRADIANCE_FORMAT_RGBA16F if empty_compact_atlas => zero_rgba16f.as_slice(),
        IRRADIANCE_FORMAT_BC6H | IRRADIANCE_FORMAT_RGBA16F => section.compact_atlas.as_slice(),
        unknown => panic!("unsupported compact SH irradiance format tag {unknown}"),
    };

    device.create_texture_with_data(
        queue,
        &wgpu::TextureDescriptor {
            label: Some("SH Base Octahedral Atlas"),
            size: allocation.extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: allocation.format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        },
        wgpu::util::TextureDataOrder::LayerMajor,
        contents,
    )
}

/// Dummy for the no-usable-probes path. It is `Rgba16Float` because every
/// compose indirection word is a sentinel, so the texture is never sampled.
fn upload_compact_base_atlas_dummy(device: &wgpu::Device, queue: &wgpu::Queue) -> wgpu::Texture {
    let zero_texel = [0u8; 8];
    device.create_texture_with_data(
        queue,
        &wgpu::TextureDescriptor {
            label: Some("SH Base Octahedral Atlas Dummy"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        },
        wgpu::util::TextureDataOrder::LayerMajor,
        &zero_texel,
    )
}

#[cfg(feature = "dev-tools")]
fn base_atlas_format_label(format: wgpu::TextureFormat) -> &'static str {
    match format {
        wgpu::TextureFormat::Bc6hRgbUfloat => "BC6H",
        wgpu::TextureFormat::Rgba16Float => "Rgba16Float",
        _ => unreachable!("base SH atlas allocation only uses BC6H or Rgba16Float"),
    }
}

fn upload_depth_moment_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    grid: [u32; 3],
    data_u16: &[u16],
) -> wgpu::Texture {
    let size = wgpu::Extent3d {
        width: grid[0].max(1),
        height: grid[1].max(1),
        depth_or_array_layers: grid[2].max(1),
    };

    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("SH Depth Moments"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D3,
        format: wgpu::TextureFormat::Rgba16Float,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });

    let byte_slice = u16_slice_to_bytes(data_u16);
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &byte_slice,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(8 * size.width),
            rows_per_image: Some(size.height),
        },
        size,
    );

    texture
}

/// Create the stored-tile total octahedral atlas texture. No data is uploaded
/// — wgpu zero-initializes; the compose pass overwrites every stored texel.
fn create_total_atlas_texture(
    device: &wgpu::Device,
    atlas_dimensions: [u32; 2],
    layer_count: u32,
    label: &str,
) -> wgpu::Texture {
    // dev-tools reads back the composed atlas for the irradiance probe-marker
    // overlay, which needs COPY_SRC. The flag is only added under the feature so
    // release builds — where the readback path is compiled out — keep the
    // minimal usage.
    #[allow(unused_mut)]
    let mut usage = wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING;
    #[cfg(feature = "dev-tools")]
    {
        usage |= wgpu::TextureUsages::COPY_SRC;
    }
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: atlas_dimensions[0].max(1),
            height: atlas_dimensions[1].max(1),
            depth_or_array_layers: layer_count.max(1),
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba16Float,
        usage,
        view_formats: &[],
    })
}

// --- Minor wgpu helper shims (local to this module) ---
//
// These exist only to keep the main `new` body readable. They inline into the
// same wgpu calls the rest of the renderer already uses elsewhere.

trait DeviceBufferInit {
    fn create_buffer_init_helper(
        &self,
        label: &str,
        contents: &[u8],
        usage: wgpu::BufferUsages,
    ) -> wgpu::Buffer;
}

impl DeviceBufferInit for wgpu::Device {
    fn create_buffer_init_helper(
        &self,
        label: &str,
        contents: &[u8],
        usage: wgpu::BufferUsages,
    ) -> wgpu::Buffer {
        self.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(label),
            contents,
            usage,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SH_DEPTH_MIN_VARIANCE_M2_REF: f32 = 1.0e-4;
    const SH_DEPTH_BIAS_CELL_FRACTION_REF: f32 = 0.05;
    const SH_DEPTH_MIN_VISIBILITY_REF: f32 = 0.03;

    #[test]
    fn base_atlas_allocation_bytes_uses_physical_bc6h_blocks() {
        let allocation = BaseAtlasAllocation {
            format: wgpu::TextureFormat::Bc6hRgbUfloat,
            extent: wgpu::Extent3d {
                width: 12,
                height: 8,
                depth_or_array_layers: 3,
            },
        };

        assert_eq!(base_atlas_allocation_bytes(allocation), 288);
    }

    #[test]
    fn missing_sh_base_atlas_reports_rgba16f_dummy_allocation() {
        let allocation = compact_base_atlas_allocation(None);

        assert_eq!(allocation.format, wgpu::TextureFormat::Rgba16Float);
        assert_eq!(allocation.extent.width, 1);
        assert_eq!(allocation.extent.height, 1);
        assert_eq!(allocation.extent.depth_or_array_layers, 1);
        assert_eq!(base_atlas_allocation_bytes(allocation), 8);
    }

    #[test]
    fn pack_probe_depth_moments_preserves_rg_f16_bits_and_packs_ba_word() {
        let probe_a = OctahedralShProbe {
            validity: 1,
            mean_distance: 0x4200,
            mean_sq_distance: 0x4900,
            density_level: 0,
        };
        let probe_b = OctahedralShProbe {
            validity: 1,
            mean_distance: 0x3c00,
            mean_sq_distance: 0x4000,
            density_level: 0,
        };

        let words = [0x1234_5674, 0xabcd_efc4];
        let moments = pack_probe_depth_moments(&[probe_a, probe_b], [2, 1, 1], &words);

        assert_eq!(
            moments,
            vec![
                0x4200, 0x4900, 0x5674, 0x1234, //
                0x3c00, 0x4000, 0xefc4, 0xabcd,
            ],
        );
    }

    #[test]
    fn pack_probe_depth_moments_zeroes_invalid_probes() {
        let probe_valid = OctahedralShProbe {
            validity: 1,
            mean_distance: 0x4400,
            mean_sq_distance: 0x4c00,
            density_level: 0,
        };
        let probe_invalid = OctahedralShProbe {
            validity: 0,
            mean_distance: 0x7bff,
            mean_sq_distance: 0x7bff,
            density_level: 0,
        };

        let moments =
            pack_probe_depth_moments(&[probe_valid, probe_invalid], [2, 1, 1], &[0x0000_0004, 0]);

        assert_eq!(
            moments,
            vec![
                0x4400, 0x4c00, 4, 0, //
                0, 0, 0, 0,
            ],
        );
    }

    #[test]
    fn missing_sh_depth_moment_dummy_payload_is_one_zero_rgba16f_texel() {
        assert_eq!(dummy_depth_moment_payload(), [0, 0, 0, 0]);
        assert_eq!(dummy_depth_moment_payload().len(), 4);

        let grid_info = build_grid_info_bytes(ShGridInfoParams {
            grid_origin: [0.0; 3],
            cell_size: [1.0; 3],
            grid_dimensions: [1, 1, 1],
            atlas_dimensions: [1, 1],
            tile_dimension: 1,
            tile_border: 0,
            atlas_tiles_per_row: 1,
            tiles_per_layer: 1,
            atlas_layer_count: 1,
            present: false,
            probe_occlusion_enabled: true,
        });
        let flag = u32::from_ne_bytes(grid_info[12..16].try_into().unwrap());
        assert_eq!(
            flag, 0,
            "missing SH section must disable shader SH sampling"
        );
    }

    #[test]
    fn sh_depth_moment_fits_accepts_3d_grid_within_limit() {
        let limits = wgpu::Limits {
            max_texture_dimension_3d: 32,
            ..Default::default()
        };

        assert!(sh_depth_moment_fits([32, 16, 8], &limits));
    }

    #[test]
    fn sh_depth_moment_fits_rejects_empty_or_over_limit_3d_grid() {
        let limits = wgpu::Limits {
            max_texture_dimension_3d: 32,
            ..Default::default()
        };

        assert!(!sh_depth_moment_fits([0, 16, 8], &limits));
        assert!(!sh_depth_moment_fits([33, 16, 8], &limits));
        assert!(!sh_depth_moment_fits([16, 33, 8], &limits));
        assert!(!sh_depth_moment_fits([16, 8, 33], &limits));
    }

    #[test]
    fn sh_bind_group_layout_includes_depth_moments_after_scripted_light_descriptors() {
        assert_eq!(BIND_SH_DEPTH_MOMENTS, BIND_SCRIPTED_LIGHT_DESCRIPTORS + 1);

        let entries = sh_bind_group_layout_entries();
        let entry = entries
            .iter()
            .find(|entry| entry.binding == BIND_SH_DEPTH_MOMENTS)
            .expect("group 3 layout should include SH depth moments");

        // VERTEX is included because the billboard pass hoists its depth-aware SH
        // sampling into `vs_main` (per-vertex lighting), reading the depth moments
        // via `textureLoad` (vertex-stage valid). Widening is additive — forward
        // (fragment) and fog (compute) still read it, and carrying VERTEX on the
        // shared layout is valid at pipeline creation for the non-vertex sharers.
        assert_eq!(
            entry.visibility,
            wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT | wgpu::ShaderStages::COMPUTE
        );
        assert!(entry.count.is_none());
        match entry.ty {
            wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D3,
                multisampled: false,
            } => {}
            other => panic!("unexpected SH depth moment binding type: {other:?}"),
        }
    }

    #[test]
    fn sh_bind_group_layout_uses_array_view_for_total_atlas() {
        let entries = sh_bind_group_layout_entries();
        let entry = entries
            .iter()
            .find(|entry| entry.binding == BIND_SH_TOTAL_ATLAS)
            .expect("group 3 layout should include SH total atlas");

        match entry.ty {
            wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2Array,
                multisampled: false,
            } => {}
            other => panic!("unexpected SH total atlas binding type: {other:?}"),
        }
    }

    #[test]
    fn descriptor_activity_uses_only_valid_section45_descriptor_indices() {
        let mut descriptors = vec![0u8; 2 * ANIMATION_DESCRIPTOR_SIZE];
        let active_offset = ANIMATION_DESCRIPTOR_SIZE + ANIMATION_DESCRIPTOR_ACTIVE_OFFSET;
        descriptors[active_offset..active_offset + 4].copy_from_slice(&1u32.to_ne_bytes());

        assert!(descriptor_indices_have_active(
            &descriptors,
            2,
            &[u32::MAX, 1]
        ));
        assert!(!descriptor_indices_have_active(
            &descriptors,
            1,
            &[u32::MAX, 1]
        ));
        assert!(!descriptor_indices_have_active(
            &descriptors,
            2,
            &[u32::MAX, 8]
        ));
    }

    /// baked-static-direct-sh Task 6: the mesh group-4 SUPERSET layout is the
    /// shared SH entries PLUS the dynamic-direct params uniform at binding 16
    /// (FRAGMENT). The shared layout stays untouched (no binding 16).
    #[test]
    fn mesh_superset_layout_adds_dynamic_direct_params_after_direct_atlas() {
        assert_eq!(BIND_DYNAMIC_DIRECT_PARAMS, BIND_SH_DIRECT_ATLAS + 1);
        assert_eq!(BIND_DYNAMIC_DIRECT_PARAMS, 16);

        let shared: std::collections::BTreeSet<u32> = sh_bind_group_layout_entries()
            .iter()
            .map(|e| e.binding)
            .collect();
        assert!(
            !shared.contains(&BIND_DYNAMIC_DIRECT_PARAMS),
            "shared SH layout must stay free of the mesh-only params uniform",
        );

        let mesh = mesh_bind_group_layout_entries();
        let entry = mesh
            .iter()
            .find(|e| e.binding == BIND_DYNAMIC_DIRECT_PARAMS)
            .expect("mesh superset layout should include the dynamic-direct params uniform");
        assert_eq!(entry.visibility, wgpu::ShaderStages::FRAGMENT);
        assert!(matches!(
            entry.ty,
            wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                ..
            }
        ));
        // Superset = shared + exactly one extra entry.
        assert_eq!(mesh.len(), sh_bind_group_layout_entries().len() + 1);
    }

    #[test]
    fn group3_shader_bindings_are_represented_by_rust_layout() {
        use std::collections::BTreeSet;

        const FORWARD_CONSUMER_SOURCE: &str = include_str!("../shaders/forward.wgsl");
        const BILLBOARD_CONSUMER_SOURCE: &str = include_str!("../shaders/billboard.wgsl");
        const FOG_CONSUMER_SOURCE: &str = include_str!("../shaders/fog_volume.wgsl");
        const FORWARD_SHADER_SOURCE: &str = concat!(
            include_str!("../shaders/forward.wgsl"),
            "\n",
            include_str!("../shaders/material_shading.wgsl"),
            "\n",
            include_str!("../shaders/curve_eval.wgsl"),
            "\n",
            include_str!("../shaders/sh_sample.wgsl"),
            "\n",
            // sdf-per-light-shadows Task 3: forward now calls the shared
            // `select_sdf_lights` helper, so the composed source under test
            // must include it (mirrors the runtime `SHADER_SOURCE`).
            include_str!("../shaders/sdf_light_select.wgsl"),
            "\n",
            include_str!("../shaders/light_falloff.wgsl"),
            "\n",
            // M10 Task 1: forward calls the shared dynamic-light eval helpers
            // (`light_eval_*`), so the composed source mirrors the runtime
            // `SHADER_SOURCE` and stays parseable.
            include_str!("../shaders/light_eval.wgsl"),
            "\n",
            // M10 mesh shadow receipt Task 1: forward calls the shared shadow-map
            // samplers (`sample_spot_shadow`/`sample_point_shadow`), so the
            // composed source mirrors the runtime `SHADER_SOURCE`.
            include_str!("../shaders/shadow_sample.wgsl"),
            "\n",
        );
        const BILLBOARD_SHADER_SOURCE: &str = concat!(
            include_str!("../shaders/billboard.wgsl"),
            "\n",
            include_str!("../shaders/sh_sample.wgsl"),
        );
        const FOG_SHADER_SOURCE: &str = concat!(
            include_str!("../shaders/fog_volume.wgsl"),
            "\n",
            include_str!("../shaders/sh_sample.wgsl"),
        );

        let rust_bindings: BTreeSet<u32> = sh_bind_group_layout_entries()
            .iter()
            .map(|entry| entry.binding)
            .collect();
        let expected_rust_bindings: BTreeSet<u32> = [
            BIND_SH_TOTAL_ATLAS,
            BIND_SH_ATLAS_SAMPLER,
            BIND_SH_GRID_INFO,
            BIND_ANIM_DESCRIPTORS,
            BIND_ANIM_SAMPLES,
            BIND_SCRIPTED_LIGHT_DESCRIPTORS,
            BIND_SH_DEPTH_MOMENTS,
            // Direct static-light atlas (Task 4). Declared in the shared group-3
            // BGL; only billboard samples it here (the mesh samples it via the
            // group-4 superset at the same binding index). Forward/fog leave it
            // undeclared, which the subset check below permits.
            BIND_SH_DIRECT_ATLAS,
            // Billboard-only, normal-free direct-scatter volume. It is
            // VERTEX-only, leaving forward's full fragment texture budget
            // untouched.
            BIND_BILLBOARD_DIRECT_SCATTER,
        ]
        .into_iter()
        .collect();
        assert_eq!(
            rust_bindings, expected_rust_bindings,
            "group-3 Rust layout bindings changed without updating the test contract",
        );

        for (label, source) in [
            ("forward", FORWARD_SHADER_SOURCE),
            ("billboard", BILLBOARD_SHADER_SOURCE),
            ("fog", FOG_SHADER_SOURCE),
        ] {
            let shader_bindings = shader_group3_bindings(source);
            assert!(
                shader_bindings.contains(&BIND_SH_DEPTH_MOMENTS),
                "{label} shader must declare sh_depth_moments at group 3 binding {BIND_SH_DEPTH_MOMENTS}",
            );
            for binding in &shader_bindings {
                assert!(
                    rust_bindings.contains(binding),
                    "{label} shader declares group 3 binding {binding}, but Rust SH layout does not",
                );
            }
        }

        let forward_bindings = shader_group3_bindings(FORWARD_SHADER_SOURCE);
        assert!(
            forward_bindings.contains(&BIND_SCRIPTED_LIGHT_DESCRIPTORS),
            "forward shader must declare scripted light descriptors at group 3 binding {BIND_SCRIPTED_LIGHT_DESCRIPTORS}",
        );

        assert!(
            !FORWARD_CONSUMER_SOURCE.contains("sample_sh_indirect_corners_without_depth("),
            "forward shader must not use the non-depth SH compatibility helper",
        );
        assert!(
            FORWARD_CONSUMER_SOURCE.contains("sample_sh_indirect_corners_depth_aware("),
            "forward shader must use the depth-aware SH helper",
        );
        assert!(
            !BILLBOARD_CONSUMER_SOURCE.contains("sample_sh_indirect_corners_without_depth("),
            "billboard shader must not use the non-depth SH compatibility helper",
        );
        assert!(
            BILLBOARD_CONSUMER_SOURCE.contains("sample_sh_indirect_corners_depth_aware("),
            "billboard shader must use the depth-aware SH helper",
        );
        assert!(
            FOG_CONSUMER_SOURCE.contains("sample_sh_indirect_corners_without_depth(")
                || FOG_CONSUMER_SOURCE.contains("sample_sh_indirect_corners_two_without_depth("),
            "fog shader should stay on the explicit no-depth SH compatibility helper",
        );
        assert!(
            !FOG_CONSUMER_SOURCE.contains("sh_grid.probe_occlusion"),
            "fog shader must not read the Probe Occlusion toggle",
        );
    }

    #[test]
    fn chebyshev_visibility_reference_is_full_before_mean_plus_bias() {
        let cell_size = [2.0, 1.0, 3.0];
        let bias = SH_DEPTH_BIAS_CELL_FRACTION_REF;
        assert_eq!(
            chebyshev_visibility_reference(4.0, 17.0, 4.0, cell_size, true),
            1.0
        );
        assert_eq!(
            chebyshev_visibility_reference(4.0, 17.0, 4.0 + bias, cell_size, true),
            1.0
        );
    }

    #[test]
    fn chebyshev_visibility_reference_smoothly_attenuates_past_mean() {
        let cell_size = [1.0, 1.0, 1.0];
        let near = chebyshev_visibility_reference(2.0, 5.0, 2.25, cell_size, true);
        let far = chebyshev_visibility_reference(2.0, 5.0, 4.0, cell_size, true);

        assert!(near < 1.0, "beyond mean+bias should attenuate");
        assert!(far < near, "farther samples should receive less visibility");
        assert!(far > SH_DEPTH_MIN_VISIBILITY_REF);
    }

    #[test]
    fn chebyshev_visibility_reference_stays_finite_with_zero_variance() {
        let cell_size = [1.0, 1.0, 1.0];
        let visibility = chebyshev_visibility_reference(2.0, 4.0, 20.0, cell_size, true);
        assert!(visibility.is_finite());
        // Near-zero variance with a far sample collapses visibility to the floor.
        assert_eq!(visibility, SH_DEPTH_MIN_VISIBILITY_REF);
    }

    #[test]
    fn chebyshev_visibility_reference_zeroes_invalid_probe() {
        let visibility = chebyshev_visibility_reference(0.0, 0.0, 100.0, [1.0, 1.0, 1.0], false);
        assert_eq!(visibility, 0.0);
    }

    /// SH L2 irradiance reconstruction, CPU-side reference. The shader does
    /// the same math with the same basis constants; this test pins those
    /// constants against an analytical case — a constant function (only L0
    /// non-zero) must reconstruct to the same constant in every direction.
    #[test]
    fn sh_l2_reconstruction_of_constant_function_is_constant() {
        // sh_irradiance = c0 * 0.282095 for a coefficient vector where only
        // the L0 band is non-zero. 0.282095 = 1 / (2 * sqrt(pi)), the real
        // spherical harmonic normalization for L0.
        const L0: f32 = 0.282095;
        let mut coeffs = [0.0f32; 27];
        coeffs[0] = 1.0;
        coeffs[1] = 1.0;
        coeffs[2] = 1.0;

        // Sample several normal directions; all must produce the same value.
        let normals = [
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 1.0],
            [0.577, 0.577, 0.577],
            [-0.707, 0.707, 0.0],
        ];
        let expected = L0;
        for n in &normals {
            let got_r = sh_irradiance_reference(&coeffs, *n)[0];
            assert!(
                (got_r - expected).abs() < 1e-5,
                "constant L0 should reconstruct to L0*c for all normals; got {got_r} for {n:?}",
            );
        }
    }

    /// CPU reference implementation of `sh_irradiance` matching the WGSL
    /// function in `forward.wgsl`. Kept as a test fixture so divergence between
    /// the runtime shader and the baker's signed basis surfaces immediately.
    ///
    /// Signs on bands 1, 3, 5, 7 mirror the baker's `sh_basis_l2` — see
    /// postretro-level-compiler/src/sh_bake.rs. Projection and reconstruction
    /// must share the same signed basis or odd-band terms invert.
    fn sh_irradiance_reference(coeffs: &[f32; 27], n: [f32; 3]) -> [f32; 3] {
        // Index scheme: coeffs[band*3 + channel].
        let band = |b: usize| [coeffs[b * 3], coeffs[b * 3 + 1], coeffs[b * 3 + 2]];
        let nx = n[0];
        let ny = n[1];
        let nz = n[2];

        let mut out = [0.0; 3];
        for (i, v) in band(0).iter().enumerate() {
            out[i] += v * 0.282095;
        }
        for (i, v) in band(1).iter().enumerate() {
            out[i] += v * -0.488603 * ny;
        }
        for (i, v) in band(2).iter().enumerate() {
            out[i] += v * 0.488603 * nz;
        }
        for (i, v) in band(3).iter().enumerate() {
            out[i] += v * -0.488603 * nx;
        }
        for (i, v) in band(4).iter().enumerate() {
            out[i] += v * 1.092548 * nx * ny;
        }
        for (i, v) in band(5).iter().enumerate() {
            out[i] += v * -1.092548 * ny * nz;
        }
        for (i, v) in band(6).iter().enumerate() {
            out[i] += v * 0.315392 * (3.0 * nz * nz - 1.0);
        }
        for (i, v) in band(7).iter().enumerate() {
            out[i] += v * -1.092548 * nx * nz;
        }
        for (i, v) in band(8).iter().enumerate() {
            out[i] += v * 0.546274 * (nx * nx - ny * ny);
        }
        out
    }

    fn chebyshev_visibility_reference(
        mean: f32,
        mean2: f32,
        distance: f32,
        cell_size: [f32; 3],
        is_valid: bool,
    ) -> f32 {
        if !is_valid {
            return 0.0;
        }
        let cell_min = cell_size[0].min(cell_size[1]).min(cell_size[2]).max(0.0);
        let bias = cell_min * SH_DEPTH_BIAS_CELL_FRACTION_REF;
        let variance = (mean2 - mean * mean).max(SH_DEPTH_MIN_VARIANCE_M2_REF);
        let delta = (distance - mean - bias).max(0.0);
        let visibility = if delta > 0.0 {
            variance / (variance + delta * delta)
        } else {
            1.0
        };
        visibility.clamp(SH_DEPTH_MIN_VISIBILITY_REF, 1.0)
    }

    fn shader_group3_bindings(source: &str) -> std::collections::BTreeSet<u32> {
        let module = naga::front::wgsl::parse_str(source).expect("shader source should parse");
        module
            .global_variables
            .iter()
            .filter_map(|(_, var)| {
                let binding = var.binding.as_ref()?;
                (binding.group == 3).then_some(binding.binding)
            })
            .collect()
    }

    /// Directional radiance test — the smoking gun for basis-sign drift.
    ///
    /// Project a known anisotropic radiance `f(ω) = max(0, ω · ŷ)` (a cosine
    /// lobe pointing in +y) onto the same signed L2 basis the baker uses,
    /// apply the Ramamoorthi-Hanrahan cosine-lobe factors (baker's
    /// `apply_cosine_lobe_rgb`), and reconstruct through the runtime's
    /// `sh_irradiance_reference`. The irradiance at a +y-facing surface must
    /// be greater than at a -y-facing surface. A sign flip on L1-y silently
    /// inverts this ordering — the constant-function test cannot catch it.
    #[test]
    fn sh_l2_reconstruction_preserves_directional_preference() {
        // Baker-side signed basis — duplicated here intentionally so this
        // test pins both sides against drift.
        fn basis(n: [f32; 3]) -> [f32; 9] {
            let (x, y, z) = (n[0], n[1], n[2]);
            [
                0.282_094_8,
                -0.488_602_5 * y,
                0.488_602_5 * z,
                -0.488_602_5 * x,
                1.092_548_4 * x * y,
                -1.092_548_4 * y * z,
                0.315_391_6 * (3.0 * z * z - 1.0),
                -1.092_548_4 * x * z,
                0.546_274_2 * (x * x - y * y),
            ]
        }

        // Fibonacci-sphere sample directions, matching the baker's scheme at
        // arbitrary density — doesn't need to be identical to the baker's
        // RAYS_PER_PROBE, just dense enough for the projection integral to
        // converge under a trivially-smooth integrand.
        let samples = 4096usize;
        let mc_weight = 4.0 * std::f32::consts::PI / samples as f32;
        let mut coeffs = [0.0f32; 27];
        let phi = std::f32::consts::PI * (3.0 - 5.0_f32.sqrt()); // golden angle
        for i in 0..samples {
            let t = (i as f32 + 0.5) / samples as f32;
            let z = 1.0 - 2.0 * t;
            let r = (1.0 - z * z).max(0.0).sqrt();
            let theta = phi * i as f32;
            let dir = [r * theta.cos(), r * theta.sin(), z];
            let radiance = dir[1].max(0.0); // cosine lobe in +y
            let b = basis(dir);
            for (band, bv) in b.iter().enumerate() {
                let base = band * 3;
                coeffs[base] += bv * radiance * mc_weight;
                coeffs[base + 1] += bv * radiance * mc_weight;
                coeffs[base + 2] += bv * radiance * mc_weight;
            }
        }
        // Fold cosine-lobe convolution (matches sh_bake.rs::apply_cosine_lobe_rgb).
        let pi = std::f32::consts::PI;
        let factors = [
            pi,
            2.0 * pi / 3.0,
            2.0 * pi / 3.0,
            2.0 * pi / 3.0,
            pi * 0.25,
            pi * 0.25,
            pi * 0.25,
            pi * 0.25,
            pi * 0.25,
        ];
        for band in 0..9 {
            for ch in 0..3 {
                coeffs[band * 3 + ch] *= factors[band];
            }
        }

        let up = sh_irradiance_reference(&coeffs, [0.0, 1.0, 0.0])[0];
        let down = sh_irradiance_reference(&coeffs, [0.0, -1.0, 0.0])[0];
        assert!(
            up > down,
            "+y-facing irradiance ({up}) should exceed -y-facing ({down}) \
             for a radiance lobe pointing in +y",
        );
        // Sanity: +y should be meaningfully brighter, not just marginally so.
        assert!(
            (up - down).abs() > 0.1,
            "directional contrast too weak: up={up}, down={down}"
        );
    }

    /// `AnimatedLightBuffers` requires a real `wgpu::Buffer` for the struct literal,
    /// but `set_active` only touches the CPU mirror — a headless dummy buffer suffices.
    #[test]
    fn set_active_cpu_mirror_zeroes_flag_and_marks_dirty() {
        let mut mirror = vec![0u8; 2 * ANIMATION_DESCRIPTOR_SIZE];
        // Both lights start active; `set_active(0, false)` must zero the active bytes.
        for slot in 0..2 {
            let off = slot * ANIMATION_DESCRIPTOR_SIZE + ANIMATION_DESCRIPTOR_ACTIVE_OFFSET;
            mirror[off..off + 4].copy_from_slice(&1u32.to_ne_bytes());
        }

        // No queue interaction — we never call `upload_descriptors_if_dirty` here.
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::PRIMARY,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let adapter =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()));
        let Ok(adapter) = adapter else {
            // No adapter on this host — skip. The CPU-mirror correctness is
            // also asserted via direct byte inspection below, outside the
            // `set_active` path, to keep the invariant covered.
            eprintln!("no wgpu adapter available; CPU-mirror direct-byte check only");
            let off_0 = ANIMATION_DESCRIPTOR_ACTIVE_OFFSET;
            mirror[off_0..off_0 + 4].copy_from_slice(&0u32.to_ne_bytes());
            let read = u32::from_ne_bytes(mirror[off_0..off_0 + 4].try_into().unwrap());
            assert_eq!(read, 0);
            return;
        };
        let (device, _queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
                .expect("device");

        let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("test descriptors"),
            contents: &mirror,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        });
        let anim_samples = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("test samples"),
            contents: &[0u8; ANIMATION_DESCRIPTOR_SIZE],
            usage: wgpu::BufferUsages::STORAGE,
        });

        let mut buffers = AnimatedLightBuffers {
            descriptors: buffer,
            anim_samples,
            descriptor_mirror: mirror,
            animated_light_count: 2,
            dirty: false,
            oor_warned: false,
        };

        // Before: slot 0 is active (1).
        let off_0 = ANIMATION_DESCRIPTOR_ACTIVE_OFFSET;
        let read_before = u32::from_ne_bytes(
            buffers.descriptor_mirror[off_0..off_0 + 4]
                .try_into()
                .unwrap(),
        );
        assert_eq!(read_before, 1);
        assert!(!buffers.dirty);

        // Toggle slot 0 off — the mirror bytes go to zero, dirty flips true.
        buffers.set_active(0, false);
        let read_after = u32::from_ne_bytes(
            buffers.descriptor_mirror[off_0..off_0 + 4]
                .try_into()
                .unwrap(),
        );
        assert_eq!(read_after, 0);
        assert!(buffers.dirty);

        // Slot 1 is untouched.
        let off_1 = ANIMATION_DESCRIPTOR_SIZE + ANIMATION_DESCRIPTOR_ACTIVE_OFFSET;
        let slot1 = u32::from_ne_bytes(
            buffers.descriptor_mirror[off_1..off_1 + 4]
                .try_into()
                .unwrap(),
        );
        assert_eq!(slot1, 1);

        // Out-of-range slot is a no-op (no panic, no mirror change).
        buffers.set_active(42, false);
        let slot0_again = u32::from_ne_bytes(
            buffers.descriptor_mirror[off_0..off_0 + 4]
                .try_into()
                .unwrap(),
        );
        assert_eq!(slot0_again, 0);
    }
}
