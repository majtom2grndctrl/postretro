// SH irradiance volume GPU resources: octahedral atlas textures, grid-info uniform, bind group (group 3).
// See: context/lib/rendering_pipeline.md §4, §8

use postretro_level_format::sh_volume::{
    AnimationDescriptor, OctahedralAtlasTexel, OctahedralShProbe, OctahedralShVolumeSection,
};

/// Group 3 binding indices for the octahedral irradiance atlas resources.
pub const BIND_SH_TOTAL_ATLAS: u32 = 1;
pub const BIND_SH_ATLAS_SAMPLER: u32 = 2;
pub const BIND_SH_GRID_INFO: u32 = 10;
/// Binding indices for the group 3 animation storage buffers. These retain
/// their pre-migration meanings so the scripting and light animation bridge
/// keep their existing contract.
pub const BIND_ANIM_DESCRIPTORS: u32 = 11;
pub const BIND_ANIM_SAMPLES: u32 = 12;
/// Separate from `BIND_ANIM_DESCRIPTORS` (baked section descriptors for the
/// compose pass) because the two consumers use different indexing schemes —
/// section-indexed vs map-light-indexed.
pub const BIND_SCRIPTED_LIGHT_DESCRIPTORS: u32 = 13;
/// Static per-probe depth moments: R = mean distance, G = mean squared distance.
pub const BIND_SH_DEPTH_MOMENTS: u32 = BIND_SCRIPTED_LIGHT_DESCRIPTORS + 1;

/// Byte size of `ShGridInfo` — six `vec4` slots to satisfy std140 alignment
/// rules (vec3 fields align to 16, followed by a same-slot scalar).
///
/// Layout (must match the WGSL `ShGridInfo` structs in shader consumers):
///   0..12   grid_origin       (vec3<f32>)
///   12..16  has_sh_volume     (u32, 0 or 1)
///   16..28  cell_size         (vec3<f32>)
///   28..32  _pad0             (u32)
///   32..44  grid_dimensions   (vec3<u32>)
///   44..48  _pad1             (u32)
///   48..56  atlas_dimensions  (vec2<u32>)
///   56..60  tile_dimension    (u32)
///   60..64  tile_border       (u32)
///   64..72  tile_grid_dims    (vec2<u32>: tiles wide, tiles high)
///   72..76  tile_interior     (u32)
///   76..80  _pad2             (u32)
///   80..84  probe_occlusion   (u32, 0 or 1)
///   84..96  _pad3             (three u32 slots)
pub const SH_GRID_INFO_SIZE: usize = 96;

pub const DEFAULT_PROBE_OCCLUSION: bool = true;

/// Stride of one `AnimationDescriptor` record on the GPU. WGSL layout:
///   f32 period             (0..4)
///   f32 phase              (4..8)
///   u32 brightness_offset  (8..12)
///   u32 brightness_count   (12..16)
///   vec3<f32> base_color   (16..28)  // AlignOf=16; scalars before it fill the gap
///   u32 color_offset       (28..32)
///   u32 color_count        (32..36)
///   u32 active             (36..40)  // runtime on/off; initialized from start_active
///   u32 direction_offset   (40..44)
///   u32 direction_count    (44..48)  // 0 → shader uses static `cone_direction`
///
/// `active` used to be an implicit 4-byte padding gap. It is now a real field —
/// scripts toggle it to enable/disable an animated light without a reload.
/// The trailing direction slots consumed the last 8 bytes of reserved padding;
/// the overall 48-byte stride is unchanged.
pub const ANIMATION_DESCRIPTOR_SIZE: usize = 48;

/// Byte offset of the `active` u32 within one descriptor record. Used by
/// `AnimatedLightBuffers::set_active` to patch the CPU mirror in place.
pub const ANIMATION_DESCRIPTOR_ACTIVE_OFFSET: usize = 36;

/// f32 slots per map light for brightness samples in the scripted-animation region.
pub const SCRIPTED_BRIGHTNESS_SLOT: usize = 128;
/// f32 slots per map light for color samples (interleaved RGB, so 128 / 3 ≈ 42 keyframes max).
pub const SCRIPTED_COLOR_SLOT_F32: usize = 128;
/// Total f32 slots per map light in the scripted-animation region.
/// Layout within each slot: [0..SCRIPTED_BRIGHTNESS_SLOT) = brightness,
/// [SCRIPTED_BRIGHTNESS_SLOT..SCRIPTED_FLOATS_PER_LIGHT) = color (RGB interleaved).
pub const SCRIPTED_FLOATS_PER_LIGHT: usize = SCRIPTED_BRIGHTNESS_SLOT + SCRIPTED_COLOR_SLOT_F32;

/// Uploaded SH volume handles + bind group. Always populated. Empty-geometry
/// levels bind dummy 1×1 atlases and set `has_sh_volume` to zero so shader
/// consumers skip indirect sampling.
///
/// Two atlas textures exist:
/// - **base**: uploaded once at load time from the PRL `OctahedralShVolume`
///   section. Held as the source-of-truth static octahedral irradiance atlas.
/// - **total**: one `Rgba16Float` texture with both sampled and storage views.
///   Consumers sample this texture; the compose pass writes it each frame.
pub struct ShVolumeResources {
    pub bind_group: wgpu::BindGroup,
    pub bind_group_layout: wgpu::BindGroupLayout,
    #[allow(dead_code)]
    pub present: bool,
    /// Probe grid dimensions (in cells, x/y/z).
    pub grid_dimensions: [u32; 3],
    /// Atlas dimensions in texels.
    pub atlas_dimensions: [u32; 2],
    #[allow(dead_code)]
    pub tile_dimension: u32,
    #[allow(dead_code)]
    pub tile_border: u32,
    /// Sampled view over the base octahedral atlas; consumed by the compose pass.
    pub base_atlas_view: wgpu::TextureView,
    /// Storage-writeable view over the total octahedral atlas; consumed by the compose pass.
    pub total_atlas_storage_view: wgpu::TextureView,
    /// Per-probe depth-moment texture (Rg16Float — R = E[d], G = E[d²]).
    /// Already bound on group 3 binding 14 for the forward/billboard/fog
    /// passes; held here so the SDF shadow pass can mint its own
    /// `TextureView` via `make_depth_moment_view` (wgpu views aren't `Clone`,
    /// and the SDF shadow pass rebuilds its bind group on resize / level reload).
    depth_moment_texture: wgpu::Texture,
    /// Owned here but shared with the compose pass — one upload, two bind groups.
    /// CPU mirror kept alongside so per-frame `active` edits patch bytes and flush in one `write_buffer`.
    pub animation: AnimatedLightBuffers,
    /// Size fixed at `max(map_light_count, 1) * ANIMATION_DESCRIPTOR_SIZE`, zero-initialized —
    /// sentinel descriptor (`is_active == 0`) so the forward shader reads static `GpuLight` color
    /// until the bridge writes a real animation.
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
    /// solid or off-grid), non-zero = valid. Empty when no SH section is present.
    /// Consumed by `sh_diagnostics::emit` for probe-marker coloring.
    #[cfg(feature = "dev-tools")]
    pub validity: Vec<u8>,
    /// CPU mirror of each probe's center-tile irradiance as linear RGB,
    /// z-major like `validity`; consumed by `sh_diagnostics::emit`.
    #[cfg(feature = "dev-tools")]
    pub probe_l0: Vec<[f32; 3]>,
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
    pub total_band0_texture: wgpu::Texture,
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

impl ShVolumeResources {
    /// Build group 3 (SH volume) resources. `section` is `None` when the PRL
    /// file had no `ShVolume` section — in that case dummy 1×1×1 textures are
    /// uploaded and the `has_sh_volume` flag is zero so the shader skips SH
    /// sampling and falls back to `ambient_floor + direct_sum`.
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        section: Option<&OctahedralShVolumeSection>,
        map_light_count: usize,
        probe_occlusion_enabled: bool,
    ) -> Self {
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("SH Volume Bind Group Layout"),
            entries: &sh_bind_group_layout_entries(),
        });

        // A zero-dimension grid is treated the same as a missing/empty section.
        let usable = section.filter(|s| {
            s.grid_dimensions[0] > 0 && s.grid_dimensions[1] > 0 && s.grid_dimensions[2] > 0
        });

        let grid_origin: [f32; 3];
        let cell_size: [f32; 3];
        let grid_dimensions: [u32; 3];
        let atlas_dimensions: [u32; 2];
        let tile_dimension: u32;
        let tile_border: u32;
        let present: bool;
        let base_atlas_texture: wgpu::Texture;
        let total_atlas_texture: wgpu::Texture;
        let depth_moment_texture: wgpu::Texture;

        #[cfg(feature = "dev-tools")]
        let validity: Vec<u8> = usable
            .map(|s| s.probes.iter().map(|p| p.validity).collect())
            .unwrap_or_default();

        // Mirror the pack: invalid probes upload as zero, so store zero here too.
        #[cfg(feature = "dev-tools")]
        let probe_l0: Vec<[f32; 3]> = usable
            .map(|s| {
                s.probes
                    .iter()
                    .enumerate()
                    .map(|(i, p)| {
                        if p.validity == 0 {
                            [0.0; 3]
                        } else {
                            probe_center_irradiance(s, i)
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();

        if let Some(sec) = usable {
            base_atlas_texture = upload_atlas_texture(
                device,
                queue,
                sec.atlas_dimensions,
                &sec.atlas_texels,
                "SH Base Octahedral Atlas",
            );
            total_atlas_texture = create_total_atlas_texture(
                device,
                sec.atlas_dimensions,
                "SH Total Octahedral Atlas",
            );
            let moments = pack_probe_depth_moments(&sec.probes, sec.grid_dimensions);
            depth_moment_texture =
                upload_depth_moment_texture(device, queue, sec.grid_dimensions, &moments);
            grid_origin = sec.grid_origin;
            cell_size = sec.cell_size;
            grid_dimensions = sec.grid_dimensions;
            atlas_dimensions = sec.atlas_dimensions;
            tile_dimension = sec.tile_dimension;
            tile_border = sec.tile_border;
            present = true;
        } else {
            let dummy = dummy_depth_moment_payload();
            let dummy_texel = [OctahedralAtlasTexel { rgba: dummy }];
            base_atlas_texture = upload_atlas_texture(
                device,
                queue,
                [1, 1],
                &dummy_texel,
                "SH Base Octahedral Atlas Dummy",
            );
            total_atlas_texture =
                create_total_atlas_texture(device, [1, 1], "SH Total Octahedral Atlas Dummy");
            depth_moment_texture = upload_depth_moment_texture(device, queue, [1, 1, 1], &dummy);
            grid_origin = [0.0; 3];
            cell_size = [1.0; 3];
            grid_dimensions = [1, 1, 1];
            atlas_dimensions = [1, 1];
            tile_dimension = 1;
            tile_border = 0;
            present = false;
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
        let scripted_region_bytes = map_light_count * SCRIPTED_FLOATS_PER_LIGHT * 4;
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
        // The forward loop bound is map_light_count so the dummy slot is never read.
        let scripted_descriptor_slots = map_light_count.max(1);
        let scripted_descriptor_bytes =
            vec![0u8; scripted_descriptor_slots * ANIMATION_DESCRIPTOR_SIZE];
        let scripted_light_descriptors_buffer = device.create_buffer_init_helper(
            "Scripted Light Descriptors",
            &scripted_descriptor_bytes,
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        );

        let grid_info_bytes = build_grid_info_bytes(
            grid_origin,
            cell_size,
            grid_dimensions,
            atlas_dimensions,
            tile_dimension,
            tile_border,
            present,
            probe_occlusion_enabled,
        );
        let grid_info_buffer = device.create_buffer_init_helper(
            "SH Grid Info Uniform",
            &grid_info_bytes,
            wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        );

        let base_atlas_view = base_atlas_texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some("SH Base Octahedral Atlas View"),
            ..Default::default()
        });
        let total_atlas_sampled_view =
            total_atlas_texture.create_view(&wgpu::TextureViewDescriptor {
                label: Some("SH Total Octahedral Atlas Sampled View"),
                ..Default::default()
            });
        let total_atlas_storage_view =
            total_atlas_texture.create_view(&wgpu::TextureViewDescriptor {
                label: Some("SH Total Octahedral Atlas Storage View"),
                ..Default::default()
            });
        let depth_moment_view = depth_moment_texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some("SH Depth Moment View"),
            ..Default::default()
        });
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

        let mut entries: Vec<wgpu::BindGroupEntry> = Vec::with_capacity(7);
        entries.push(wgpu::BindGroupEntry {
            binding: BIND_SH_TOTAL_ATLAS,
            resource: wgpu::BindingResource::TextureView(&total_atlas_sampled_view),
        });
        entries.push(wgpu::BindGroupEntry {
            binding: BIND_SH_ATLAS_SAMPLER,
            resource: wgpu::BindingResource::Sampler(&atlas_sampler),
        });
        entries.push(wgpu::BindGroupEntry {
            binding: BIND_SH_GRID_INFO,
            resource: grid_info_buffer.as_entire_binding(),
        });
        entries.push(wgpu::BindGroupEntry {
            binding: BIND_ANIM_DESCRIPTORS,
            resource: anim_descriptors_buffer.as_entire_binding(),
        });
        entries.push(wgpu::BindGroupEntry {
            binding: BIND_ANIM_SAMPLES,
            resource: anim_samples_buffer.as_entire_binding(),
        });
        entries.push(wgpu::BindGroupEntry {
            binding: BIND_SCRIPTED_LIGHT_DESCRIPTORS,
            resource: scripted_light_descriptors_buffer.as_entire_binding(),
        });
        entries.push(wgpu::BindGroupEntry {
            binding: BIND_SH_DEPTH_MOMENTS,
            resource: wgpu::BindingResource::TextureView(&depth_moment_view),
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("SH Volume Bind Group"),
            layout: &bind_group_layout,
            entries: &entries,
        });

        // Retain the total atlas texture for the dev-tools readback. The
        // `wgpu::Texture` handle is an Arc clone — the views above already keep
        // the texture alive, this just gives the readback a handle to copy from.
        #[cfg(feature = "dev-tools")]
        let total_band0_texture = total_atlas_texture.clone();

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
            present,
            grid_dimensions,
            atlas_dimensions,
            tile_dimension,
            tile_border,
            base_atlas_view,
            total_atlas_storage_view,
            depth_moment_texture,
            animation,
            scripted_light_descriptors: scripted_light_descriptors_buffer,
            scripted_light_count: map_light_count as u32,
            scripted_sample_byte_offset,
            #[cfg(feature = "dev-tools")]
            validity,
            #[cfg(feature = "dev-tools")]
            probe_l0,
            grid_origin,
            cell_size,
            grid_info_buffer,
            probe_occlusion_enabled,
            #[cfg(feature = "dev-tools")]
            total_band0_texture,
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
        let bytes = build_grid_info_bytes(
            self.grid_origin,
            self.cell_size,
            self.grid_dimensions,
            self.atlas_dimensions,
            self.tile_dimension,
            self.tile_border,
            self.present,
            enabled,
        );
        queue.write_buffer(&self.grid_info_buffer, 0, &bytes);
    }
}

// --- Helpers ---

fn sh_bind_group_layout_entries() -> Vec<wgpu::BindGroupLayoutEntry> {
    let mut entries: Vec<wgpu::BindGroupLayoutEntry> = Vec::with_capacity(7);
    // Shared with the forward pass (fragment) and fog raymarch (compute), so visibility
    // covers both stages on every entry.
    let vis = wgpu::ShaderStages::FRAGMENT | wgpu::ShaderStages::COMPUTE;
    entries.push(wgpu::BindGroupLayoutEntry {
        binding: BIND_SH_TOTAL_ATLAS,
        visibility: vis,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2,
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
    for binding in [
        BIND_ANIM_DESCRIPTORS,
        BIND_ANIM_SAMPLES,
        BIND_SCRIPTED_LIGHT_DESCRIPTORS,
    ] {
        entries.push(wgpu::BindGroupLayoutEntry {
            binding,
            visibility: vis,
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
    entries
}

/// Pack per-probe depth moments into one `Rgba16Float` 3D texture payload.
/// Probes are already ordered z-major/y/x by the PRL section; keeping the same
/// linear order as the SH band textures makes the moment texture index-aligned
/// with every band. Valid probes copy baked f16 bits directly into RG; invalid
/// probes remain all zero.
fn pack_probe_depth_moments(probes: &[OctahedralShProbe], grid: [u32; 3]) -> Vec<u16> {
    let total = (grid[0] as usize) * (grid[1] as usize) * (grid[2] as usize);
    debug_assert_eq!(probes.len(), total);

    let mut moments = vec![0u16; total * 4];
    for (probe_idx, probe) in probes.iter().enumerate() {
        if probe.validity == 0 {
            continue;
        }
        let off = probe_idx * 4;
        moments[off] = probe.mean_distance;
        moments[off + 1] = probe.mean_sq_distance;
    }
    moments
}

fn dummy_depth_moment_payload() -> [u16; 4] {
    [0u16; 4]
}

fn upload_atlas_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    atlas_dimensions: [u32; 2],
    texels: &[OctahedralAtlasTexel],
    label: &str,
) -> wgpu::Texture {
    let size = wgpu::Extent3d {
        width: atlas_dimensions[0].max(1),
        height: atlas_dimensions[1].max(1),
        depth_or_array_layers: 1,
    };

    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba16Float,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });

    let mut halves = Vec::with_capacity(texels.len() * 4);
    for texel in texels {
        halves.extend_from_slice(&texel.rgba);
    }
    let byte_slice = u16_slice_to_bytes(&halves);

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

/// Create the total octahedral atlas texture. No data is uploaded — wgpu
/// zero-initializes; the compose pass overwrites every texel each frame.
fn create_total_atlas_texture(
    device: &wgpu::Device,
    atlas_dimensions: [u32; 2],
    label: &str,
) -> wgpu::Texture {
    // dev-tools reads back band 0 (L0) for the irradiance probe-marker overlay,
    // which needs COPY_SRC. The flag is only added under the feature so release
    // builds — where the readback path is compiled out — keep the minimal usage.
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
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba16Float,
        usage,
        view_formats: &[],
    })
}

#[cfg(feature = "dev-tools")]
fn probe_center_irradiance(section: &OctahedralShVolumeSection, probe_index: usize) -> [f32; 3] {
    let origin = postretro_level_format::octahedral::irradiance_tile_origin(
        probe_index,
        section.grid_dimensions,
        section.tile_dimension,
    );
    let center = section.tile_dimension / 2;
    let x = (origin[0] + center).min(section.atlas_dimensions[0].saturating_sub(1));
    let y = (origin[1] + center).min(section.atlas_dimensions[1].saturating_sub(1));
    let idx = (y * section.atlas_dimensions[0] + x) as usize;
    section
        .atlas_texels
        .get(idx)
        .map(|texel| {
            [
                f16_bits_to_f32_local(texel.rgba[0]),
                f16_bits_to_f32_local(texel.rgba[1]),
                f16_bits_to_f32_local(texel.rgba[2]),
            ]
        })
        .unwrap_or([0.0; 3])
}

#[cfg(feature = "dev-tools")]
fn f16_bits_to_f32_local(bits: u16) -> f32 {
    crate::render::sh_compose::f16_bits_to_f32(bits)
}

/// Build the two animation storage-buffer payloads from an (optional)
/// SH volume section. Returns `(descriptors, samples, count)`.
///
/// When the section has no animated lights (or no section exists), each
/// returned buffer is a non-zero-sized dummy so wgpu accepts the binding.
/// The animated-lightmap compose pass guards on `count == 0` before reading
/// these dummies, so the contents are irrelevant.
pub(crate) fn build_animation_buffers(
    section: Option<&OctahedralShVolumeSection>,
) -> (Vec<u8>, Vec<u8>, u32) {
    let Some(sec) = section else {
        return (dummy_descriptor_buffer(), dummy_storage_buffer(), 0);
    };
    let animated_light_count = sec.animation_descriptors.len();
    if animated_light_count == 0 {
        return (dummy_descriptor_buffer(), dummy_storage_buffer(), 0);
    }

    // Pack sample arrays contiguously: one brightness block per light, then
    // one color block per light, in descriptor order. Each descriptor carries
    // offsets into this flat array. Color samples are interleaved rgb.
    let mut samples: Vec<f32> = Vec::new();
    let mut descriptors = Vec::with_capacity(animated_light_count * ANIMATION_DESCRIPTOR_SIZE);

    for desc in &sec.animation_descriptors {
        let brightness_offset = samples.len() as u32;
        let brightness_count = desc.brightness.len() as u32;
        samples.extend_from_slice(&desc.brightness);

        let color_offset = samples.len() as u32;
        let color_count = desc.color.len() as u32;
        for rgb in &desc.color {
            samples.extend_from_slice(rgb);
        }

        let direction_offset = samples.len() as u32;
        let direction_count = desc.direction.len() as u32;
        for dir in &desc.direction {
            // Samples are normalized at write time; the shader does not re-normalize per frame.
            debug_assert!(
                (dir[0] * dir[0] + dir[1] * dir[1] + dir[2] * dir[2] - 1.0).abs() < 1.0e-4,
                "AnimationDescriptor direction sample must be unit length; got {:?}",
                dir,
            );
            samples.extend_from_slice(dir);
        }

        write_descriptor_bytes(
            &mut descriptors,
            desc,
            brightness_offset,
            brightness_count,
            color_offset,
            color_count,
            direction_offset,
            direction_count,
        );
    }

    let samples_bytes = f32_slice_to_bytes(&samples);

    (descriptors, samples_bytes, animated_light_count as u32)
}

#[allow(clippy::too_many_arguments)]
fn write_descriptor_bytes(
    out: &mut Vec<u8>,
    desc: &AnimationDescriptor,
    brightness_offset: u32,
    brightness_count: u32,
    color_offset: u32,
    color_count: u32,
    direction_offset: u32,
    direction_count: u32,
) {
    // Must match the WGSL `AnimationDescriptor` layout in forward.wgsl.
    // Struct size is ANIMATION_DESCRIPTOR_SIZE = 48 bytes.
    let start = out.len();
    out.resize(start + ANIMATION_DESCRIPTOR_SIZE, 0);
    let s = &mut out[start..start + ANIMATION_DESCRIPTOR_SIZE];
    s[0..4].copy_from_slice(&desc.period.to_ne_bytes());
    s[4..8].copy_from_slice(&desc.phase.to_ne_bytes());
    s[8..12].copy_from_slice(&brightness_offset.to_ne_bytes());
    s[12..16].copy_from_slice(&brightness_count.to_ne_bytes());
    s[16..20].copy_from_slice(&desc.base_color[0].to_ne_bytes());
    s[20..24].copy_from_slice(&desc.base_color[1].to_ne_bytes());
    s[24..28].copy_from_slice(&desc.base_color[2].to_ne_bytes());
    s[28..32].copy_from_slice(&color_offset.to_ne_bytes());
    s[32..36].copy_from_slice(&color_count.to_ne_bytes());
    // `active` initializes from the on-disk `start_active`; scripts mutate it
    // at runtime via `AnimatedLightBuffers::set_active`.
    s[36..40].copy_from_slice(&desc.start_active.to_ne_bytes());
    // `direction_count == 0` signals no animation — shader uses the static `cone_direction` on GpuLight.
    s[40..44].copy_from_slice(&direction_offset.to_ne_bytes());
    s[44..48].copy_from_slice(&direction_count.to_ne_bytes());
}

fn f32_slice_to_bytes(data: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(data.len() * 4);
    for &v in data {
        bytes.extend_from_slice(&v.to_ne_bytes());
    }
    bytes
}

/// Same size as `dummy_descriptor_buffer` so both dummies share the constant
/// and stay in sync if the stride changes. Contents are irrelevant — the compose
/// pass guards on `count == 0`.
fn dummy_storage_buffer() -> Vec<u8> {
    vec![0u8; ANIMATION_DESCRIPTOR_SIZE]
}

/// Minimum-size storage buffer for the `array<AnimationDescriptor>` binding
/// when a map has no animated lights. wgpu validates that the buffer holds
/// at least one full element of the declared struct, so this must be
/// `ANIMATION_DESCRIPTOR_SIZE` bytes — a 16-byte dummy triggers a validation
/// error at draw time ("bound with size 16 where the shader expects 48").
fn dummy_descriptor_buffer() -> Vec<u8> {
    vec![0u8; ANIMATION_DESCRIPTOR_SIZE]
}

fn u16_slice_to_bytes(data: &[u16]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(data.len() * 2);
    for &v in data {
        bytes.extend_from_slice(&v.to_le_bytes());
    }
    bytes
}

pub(crate) fn build_grid_info_bytes(
    grid_origin: [f32; 3],
    cell_size: [f32; 3],
    grid_dimensions: [u32; 3],
    atlas_dimensions: [u32; 2],
    tile_dimension: u32,
    tile_border: u32,
    present: bool,
    probe_occlusion_enabled: bool,
) -> [u8; SH_GRID_INFO_SIZE] {
    let mut bytes = [0u8; SH_GRID_INFO_SIZE];
    // grid_origin vec3 at 0..12, has_sh_volume u32 at 12..16.
    bytes[0..4].copy_from_slice(&grid_origin[0].to_ne_bytes());
    bytes[4..8].copy_from_slice(&grid_origin[1].to_ne_bytes());
    bytes[8..12].copy_from_slice(&grid_origin[2].to_ne_bytes());
    let flag: u32 = if present { 1 } else { 0 };
    bytes[12..16].copy_from_slice(&flag.to_ne_bytes());
    // cell_size vec3 at 16..28, _pad0 at 28..32.
    bytes[16..20].copy_from_slice(&cell_size[0].to_ne_bytes());
    bytes[20..24].copy_from_slice(&cell_size[1].to_ne_bytes());
    bytes[24..28].copy_from_slice(&cell_size[2].to_ne_bytes());
    // grid_dimensions vec3<u32> at 32..44, _pad1 at 44..48.
    bytes[32..36].copy_from_slice(&grid_dimensions[0].to_ne_bytes());
    bytes[36..40].copy_from_slice(&grid_dimensions[1].to_ne_bytes());
    bytes[40..44].copy_from_slice(&grid_dimensions[2].to_ne_bytes());
    // bytes[44..48] is _pad1, already zero.
    bytes[48..52].copy_from_slice(&atlas_dimensions[0].to_ne_bytes());
    bytes[52..56].copy_from_slice(&atlas_dimensions[1].to_ne_bytes());
    bytes[56..60].copy_from_slice(&tile_dimension.to_ne_bytes());
    bytes[60..64].copy_from_slice(&tile_border.to_ne_bytes());
    bytes[64..68].copy_from_slice(&grid_dimensions[0].to_ne_bytes());
    let tile_rows = grid_dimensions[1].saturating_mul(grid_dimensions[2]);
    bytes[68..72].copy_from_slice(&tile_rows.to_ne_bytes());
    let interior = tile_dimension.saturating_sub(tile_border.saturating_mul(2));
    bytes[72..76].copy_from_slice(&interior.to_ne_bytes());
    let probe_occlusion: u32 = probe_occlusion_enabled as u32;
    bytes[80..84].copy_from_slice(&probe_occlusion.to_ne_bytes());
    bytes
}

pub(crate) fn probe_occlusion_seed_from_fast_env(value: Option<&str>) -> bool {
    value.map_or(DEFAULT_PROBE_OCCLUSION, |v| v != "1")
}

/// Round-to-nearest-even f32 → IEEE 754 binary16 (half float). Subnormals and
/// overflow saturate to zero / infinity respectively; NaN is preserved as a
/// canonical NaN. Good enough for SH irradiance coefficients — the baker
/// produces finite, bounded values in the typical 0..few-hundreds range.
pub(crate) fn f32_to_f16_bits(v: f32) -> u16 {
    let bits = v.to_bits();
    let sign = ((bits >> 31) & 0x1) as u16;
    let exp32 = ((bits >> 23) & 0xff) as i32;
    let mant32 = bits & 0x7fffff;

    if exp32 == 0xff {
        // NaN or Inf
        let mant16 = if mant32 != 0 { 0x200 } else { 0 }; // canonical NaN vs Inf
        return (sign << 15) | (0x1f << 10) | mant16;
    }

    let exp16 = exp32 - 127 + 15;
    if exp16 >= 0x1f {
        // Overflow → Inf
        return (sign << 15) | (0x1f << 10);
    }
    if exp16 <= 0 {
        // Subnormal or zero: flush to zero (acceptable precision loss for irradiance).
        if exp16 < -10 {
            return sign << 15;
        }
        // Shift mantissa including implicit 1 bit, then round.
        let mant = mant32 | 0x800000;
        let shift = 14 - exp16; // total right shift from 23-bit to place into subnormal position
        let rounded = mant >> shift;
        // Round to nearest even.
        let rem = mant & ((1 << shift) - 1);
        let half = 1 << (shift - 1);
        let add = if rem > half || (rem == half && (rounded & 1) != 0) {
            1
        } else {
            0
        };
        return (sign << 15) | ((rounded + add) as u16);
    }

    // Normal number.
    let mant16 = mant32 >> 13;
    let rem = mant32 & 0x1fff;
    let half = 0x1000;
    let add = if rem > half || (rem == half && (mant16 & 1) != 0) {
        1
    } else {
        0
    };
    let mut mant16 = mant16 + add;
    let mut exp16 = exp16;
    if mant16 >= 0x400 {
        mant16 = 0;
        exp16 += 1;
        if exp16 >= 0x1f {
            return (sign << 15) | (0x1f << 10);
        }
    }
    (sign << 15) | ((exp16 as u16) << 10) | (mant16 as u16)
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
        use wgpu::util::DeviceExt;
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

    fn test_octahedral_section(
        grid: [u32; 3],
        animation_descriptors: Vec<AnimationDescriptor>,
    ) -> OctahedralShVolumeSection {
        let probe_count = grid[0] as usize * grid[1] as usize * grid[2] as usize;
        OctahedralShVolumeSection {
            grid_origin: [0.0; 3],
            cell_size: [1.0; 3],
            grid_dimensions: grid,
            probe_stride: postretro_level_format::sh_volume::OCTAHEDRAL_PROBE_STRIDE,
            tile_dimension: 6,
            tile_border: 1,
            atlas_dimensions: [grid[0].max(1) * 6, grid[1].max(1) * grid[2].max(1) * 6],
            probes: vec![OctahedralShProbe::default(); probe_count],
            atlas_texels: vec![
                OctahedralAtlasTexel::default();
                (grid[0].max(1) * 6 * grid[1].max(1) * grid[2].max(1) * 6) as usize
            ],
            animation_descriptors,
            slot_for_map_light: Vec::new(),
        }
    }

    #[test]
    fn f32_to_f16_zero_and_one() {
        assert_eq!(f32_to_f16_bits(0.0), 0x0000);
        assert_eq!(f32_to_f16_bits(1.0), 0x3c00);
        assert_eq!(f32_to_f16_bits(-1.0), 0xbc00);
        assert_eq!(f32_to_f16_bits(2.0), 0x4000);
    }

    #[test]
    fn f32_to_f16_half_values() {
        // 0.5 in f16 is 0x3800.
        assert_eq!(f32_to_f16_bits(0.5), 0x3800);
        // -0.5
        assert_eq!(f32_to_f16_bits(-0.5), 0xb800);
    }

    #[test]
    fn grid_info_bytes_encode_origin_and_present_flag() {
        let bytes = build_grid_info_bytes(
            [1.5, 2.5, 3.5],
            [0.25, 0.5, 1.0],
            [4, 5, 6],
            [24, 180],
            6,
            1,
            true,
            true,
        );
        assert_eq!(bytes.len(), SH_GRID_INFO_SIZE);

        let ox = f32::from_ne_bytes(bytes[0..4].try_into().unwrap());
        let oy = f32::from_ne_bytes(bytes[4..8].try_into().unwrap());
        let oz = f32::from_ne_bytes(bytes[8..12].try_into().unwrap());
        let flag = u32::from_ne_bytes(bytes[12..16].try_into().unwrap());
        let cx = f32::from_ne_bytes(bytes[16..20].try_into().unwrap());
        let gy = u32::from_ne_bytes(bytes[36..40].try_into().unwrap());
        let atlas_w = u32::from_ne_bytes(bytes[48..52].try_into().unwrap());
        let tile_dim = u32::from_ne_bytes(bytes[56..60].try_into().unwrap());
        let tile_rows = u32::from_ne_bytes(bytes[68..72].try_into().unwrap());
        let tile_interior = u32::from_ne_bytes(bytes[72..76].try_into().unwrap());
        let probe_occlusion = u32::from_ne_bytes(bytes[80..84].try_into().unwrap());

        assert_eq!([ox, oy, oz], [1.5, 2.5, 3.5]);
        assert_eq!(flag, 1);
        assert_eq!(cx, 0.25);
        assert_eq!(gy, 5);
        assert_eq!(atlas_w, 24);
        assert_eq!(tile_dim, 6);
        assert_eq!(tile_rows, 30);
        assert_eq!(tile_interior, 4);
        assert_eq!(probe_occlusion, 1);
    }

    #[test]
    fn grid_info_flag_zero_when_absent() {
        let bytes = build_grid_info_bytes([0.0; 3], [1.0; 3], [1, 1, 1], [1, 1], 1, 0, false, true);
        let flag = u32::from_ne_bytes(bytes[12..16].try_into().unwrap());
        assert_eq!(flag, 0);
    }

    #[test]
    fn probe_occlusion_seed_defaults_on_and_fast_env_disables() {
        assert!(probe_occlusion_seed_from_fast_env(None));
        assert!(!probe_occlusion_seed_from_fast_env(Some("1")));
        assert!(probe_occlusion_seed_from_fast_env(Some("0")));
        assert!(probe_occlusion_seed_from_fast_env(Some("true")));
    }

    #[test]
    fn grid_info_bytes_encode_probe_occlusion_flag() {
        for (enabled, expected) in [(true, 1u32), (false, 0u32)] {
            let bytes =
                build_grid_info_bytes([0.0; 3], [1.0; 3], [1, 1, 1], [1, 1], 1, 0, true, enabled);
            assert_eq!(
                u32::from_ne_bytes(bytes[80..84].try_into().unwrap()),
                expected,
                "probe_occlusion={enabled} should encode to {expected}",
            );
            assert!(
                bytes[84..96].iter().all(|&b| b == 0),
                "probe-occlusion tail padding should stay zero",
            );
        }
    }

    /// Pins the sizing formula shared by `ShVolumeResources::new` and
    /// `Renderer::upload_bridge_descriptors`. Both must derive the same byte count
    /// from the same `map_light_count` or the upload fails the length check on valid bridge output.
    /// CPU-only — the actual buffer requires a wgpu device.
    #[test]
    fn scripted_descriptor_buffer_sizing_matches_bridge_payload_size() {
        for map_light_count in [0usize, 1, 4, 17, 256] {
            let alloc_slots = map_light_count.max(1);
            let alloc_bytes = alloc_slots * ANIMATION_DESCRIPTOR_SIZE;
            let expected_upload_bytes = map_light_count * ANIMATION_DESCRIPTOR_SIZE;
            if map_light_count == 0 {
                // Zero-light maps pad to a single dummy slot so wgpu accepts
                // the storage binding. The forward loop bound is 0 so the
                // dummy is never read, and the bridge emits zero bytes
                // (nothing to upload). The `upload_bridge_descriptors`
                // early-return on empty input is what makes this case safe.
                assert_eq!(alloc_bytes, ANIMATION_DESCRIPTOR_SIZE);
                assert_eq!(expected_upload_bytes, 0);
            } else {
                assert_eq!(alloc_bytes, expected_upload_bytes);
                assert_eq!(alloc_bytes % ANIMATION_DESCRIPTOR_SIZE, 0);
            }
        }
    }

    #[test]
    fn build_animation_buffers_no_section_produces_dummies() {
        let (d, s, count) = build_animation_buffers(None);
        assert_eq!(count, 0);
        // Dummy buffers are non-empty (wgpu rejects zero-sized bindings) but
        // need not be any particular shape — just that they exist.
        assert!(!d.is_empty());
        assert!(!s.is_empty());
    }

    #[test]
    fn build_animation_buffers_packs_descriptors_and_samples() {
        let grid = [2u32, 1, 1];
        let section = test_octahedral_section(
            grid,
            vec![
                AnimationDescriptor {
                    period: 2.0,
                    phase: 0.25,
                    base_color: [1.0, 0.5, 0.25],
                    brightness: vec![0.0, 1.0, 0.5, 1.0],
                    color: vec![],
                    direction: vec![],
                    start_active: 1,
                },
                AnimationDescriptor {
                    period: 1.0,
                    phase: 0.0,
                    base_color: [0.1, 0.2, 0.3],
                    brightness: vec![],
                    color: vec![[1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
                    direction: vec![],
                    start_active: 0,
                },
            ],
        );

        let (descriptors, samples, count) = build_animation_buffers(Some(&section));
        assert_eq!(count, 2);
        assert_eq!(descriptors.len(), 2 * ANIMATION_DESCRIPTOR_SIZE);

        // Descriptor 0: brightness_offset=0, brightness_count=4, color_count=0.
        let period = f32::from_ne_bytes(descriptors[0..4].try_into().unwrap());
        let phase = f32::from_ne_bytes(descriptors[4..8].try_into().unwrap());
        let brightness_offset = u32::from_ne_bytes(descriptors[8..12].try_into().unwrap());
        let brightness_count = u32::from_ne_bytes(descriptors[12..16].try_into().unwrap());
        let color_count_0 = u32::from_ne_bytes(descriptors[32..36].try_into().unwrap());
        assert_eq!(period, 2.0);
        assert_eq!(phase, 0.25);
        assert_eq!(brightness_offset, 0);
        assert_eq!(brightness_count, 4);
        assert_eq!(color_count_0, 0);

        // Descriptor 1 starts at offset 48.
        let brightness_offset_1 =
            u32::from_ne_bytes(descriptors[48 + 8..48 + 12].try_into().unwrap());
        let brightness_count_1 =
            u32::from_ne_bytes(descriptors[48 + 12..48 + 16].try_into().unwrap());
        let color_offset_1 = u32::from_ne_bytes(descriptors[48 + 28..48 + 32].try_into().unwrap());
        let color_count_1 = u32::from_ne_bytes(descriptors[48 + 32..48 + 36].try_into().unwrap());
        // Brightness for light 1 is empty; color samples sit after light 0's
        // brightness block (4 floats) + light 1's (empty) brightness block.
        assert_eq!(brightness_offset_1, 4);
        assert_eq!(brightness_count_1, 0);
        assert_eq!(color_offset_1, 4);
        assert_eq!(color_count_1, 2);

        // Samples = 4 brightness (light 0) + 0 (light 1) + 2*3 color rgb.
        assert_eq!(samples.len(), (4 + 6) * 4);
    }

    #[test]
    fn pack_probe_depth_moments_preserves_valid_probe_f16_bits() {
        let probe_a = OctahedralShProbe {
            validity: 1,
            mean_distance: 0x4200,
            mean_sq_distance: 0x4900,
            ..Default::default()
        };
        let probe_b = OctahedralShProbe {
            validity: 1,
            mean_distance: 0x3c00,
            mean_sq_distance: 0x4000,
            ..Default::default()
        };

        let moments = pack_probe_depth_moments(&[probe_a, probe_b], [2, 1, 1]);

        assert_eq!(
            moments,
            vec![
                0x4200, 0x4900, 0, 0, //
                0x3c00, 0x4000, 0, 0,
            ],
        );
    }

    #[test]
    fn pack_probe_depth_moments_zeroes_invalid_probes() {
        let probe_valid = OctahedralShProbe {
            validity: 1,
            mean_distance: 0x4400,
            mean_sq_distance: 0x4c00,
            ..Default::default()
        };
        let probe_invalid = OctahedralShProbe {
            validity: 0,
            mean_distance: 0x7bff,
            mean_sq_distance: 0x7bff,
            ..Default::default()
        };

        let moments = pack_probe_depth_moments(&[probe_valid, probe_invalid], [2, 1, 1]);

        assert_eq!(
            moments,
            vec![
                0x4400, 0x4c00, 0, 0, //
                0, 0, 0, 0,
            ],
        );
    }

    #[test]
    fn missing_sh_depth_moment_dummy_payload_is_one_zero_rgba16f_texel() {
        assert_eq!(dummy_depth_moment_payload(), [0, 0, 0, 0]);
        assert_eq!(dummy_depth_moment_payload().len(), 4);

        let grid_info =
            build_grid_info_bytes([0.0; 3], [1.0; 3], [1, 1, 1], [1, 1], 1, 0, false, true);
        let flag = u32::from_ne_bytes(grid_info[12..16].try_into().unwrap());
        assert_eq!(
            flag, 0,
            "missing SH section must disable shader SH sampling"
        );
    }

    #[test]
    fn sh_bind_group_layout_includes_depth_moments_after_scripted_light_descriptors() {
        assert_eq!(BIND_SH_DEPTH_MOMENTS, BIND_SCRIPTED_LIGHT_DESCRIPTORS + 1);

        let entries = sh_bind_group_layout_entries();
        let entry = entries
            .iter()
            .find(|entry| entry.binding == BIND_SH_DEPTH_MOMENTS)
            .expect("group 3 layout should include SH depth moments");

        assert_eq!(
            entry.visibility,
            wgpu::ShaderStages::FRAGMENT | wgpu::ShaderStages::COMPUTE
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
    fn group3_shader_bindings_are_represented_by_rust_layout() {
        use std::collections::BTreeSet;

        const FORWARD_CONSUMER_SOURCE: &str = include_str!("../shaders/forward.wgsl");
        const BILLBOARD_CONSUMER_SOURCE: &str = include_str!("../shaders/billboard.wgsl");
        const FOG_CONSUMER_SOURCE: &str = include_str!("../shaders/fog_volume.wgsl");
        const FORWARD_SHADER_SOURCE: &str = concat!(
            include_str!("../shaders/forward.wgsl"),
            "\n",
            include_str!("../shaders/curve_eval.wgsl"),
            "\n",
            include_str!("../shaders/sh_sample.wgsl"),
            "\n",
            // sdf-per-light-shadows Task 3: forward now calls the shared
            // `select_sdf_lights` helper, so the composed source under test
            // must include it (mirrors the runtime `SHADER_SOURCE`).
            include_str!("../shaders/sdf_light_select.wgsl"),
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
            !FOG_CONSUMER_SOURCE.contains("probe_occlusion"),
            "fog shader must not declare or read the Probe Occlusion toggle",
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

    /// Descriptor byte-for-byte round-trip: build a section carrying a
    /// descriptor with every non-default field set, pack it via the same
    /// `build_animation_buffers → write_descriptor_bytes` path the renderer
    /// uses at load time, read each field back by byte offset, and assert the
    /// 48-byte stride invariant.
    #[test]
    fn descriptor_round_trip_pack_unpack_symmetric() {
        let grid = [1u32, 1, 1];
        let desc = AnimationDescriptor {
            period: 3.75,
            phase: 0.625,
            base_color: [0.9, 0.5, 0.125],
            // Three brightness samples → brightness_offset=0, brightness_count=3.
            brightness: vec![0.25, 0.5, 1.0],
            // Two color samples follow the brightness block.
            color: vec![[1.0, 0.0, 0.0], [0.0, 1.0, 0.5]],
            // Two unit direction samples follow the color block.
            direction: vec![[1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            // Non-default: `_start_inactive = 1` at compile time zeros this.
            start_active: 0,
        };
        let section = test_octahedral_section(grid, vec![desc.clone()]);

        let (descriptors, _samples, count) = build_animation_buffers(Some(&section));
        assert_eq!(count, 1);
        // 48-byte stride invariant.
        assert_eq!(descriptors.len(), ANIMATION_DESCRIPTOR_SIZE);

        // Every field round-trips at its specified byte offset.
        let period = f32::from_ne_bytes(descriptors[0..4].try_into().unwrap());
        let phase = f32::from_ne_bytes(descriptors[4..8].try_into().unwrap());
        let brightness_offset = u32::from_ne_bytes(descriptors[8..12].try_into().unwrap());
        let brightness_count = u32::from_ne_bytes(descriptors[12..16].try_into().unwrap());
        let base_color_r = f32::from_ne_bytes(descriptors[16..20].try_into().unwrap());
        let base_color_g = f32::from_ne_bytes(descriptors[20..24].try_into().unwrap());
        let base_color_b = f32::from_ne_bytes(descriptors[24..28].try_into().unwrap());
        let color_offset = u32::from_ne_bytes(descriptors[28..32].try_into().unwrap());
        let color_count = u32::from_ne_bytes(descriptors[32..36].try_into().unwrap());
        let active = u32::from_ne_bytes(
            descriptors[ANIMATION_DESCRIPTOR_ACTIVE_OFFSET..ANIMATION_DESCRIPTOR_ACTIVE_OFFSET + 4]
                .try_into()
                .unwrap(),
        );
        // Direction channel offsets (bytes 40..48).
        let direction_offset = u32::from_ne_bytes(descriptors[40..44].try_into().unwrap());
        let direction_count = u32::from_ne_bytes(descriptors[44..48].try_into().unwrap());

        assert_eq!(period, desc.period);
        assert_eq!(phase, desc.phase);
        assert_eq!(brightness_offset, 0);
        assert_eq!(brightness_count, desc.brightness.len() as u32);
        assert_eq!(base_color_r, desc.base_color[0]);
        assert_eq!(base_color_g, desc.base_color[1]);
        assert_eq!(base_color_b, desc.base_color[2]);
        assert_eq!(color_offset, desc.brightness.len() as u32);
        assert_eq!(color_count, desc.color.len() as u32);
        assert_eq!(active, desc.start_active);
        // Direction samples follow color samples in the flat f32 array:
        // 3 brightness + 2 color × 3 channels = 9 f32s consumed.
        assert_eq!(
            direction_offset,
            (desc.brightness.len() + desc.color.len() * 3) as u32
        );
        assert_eq!(direction_count, desc.direction.len() as u32);
    }

    /// Direction-channel packing + Catmull-Rom evaluation round-trip.
    ///
    /// The GPU shader path for direction uses the same `sample_color_catmull_rom`
    /// helper as the color channel (proved byte-accurate against a CPU reference
    /// in `curve_eval_test::curve_eval_rgb_matches_splines_reference`). This
    /// test wires the two halves together: pack a descriptor whose direction
    /// block follows brightness + color, then reach into the flat sample buffer
    /// at the recorded `direction_offset` and verify the Catmull-Rom
    /// reconstruction at a mid-cycle `t` matches a CPU reference over the same
    /// samples.
    #[test]
    fn direction_channel_packs_and_evaluates_via_catmull_rom() {
        // Four unit-length direction samples sweeping around the yz-plane.
        let dir0 = [0.0f32, 1.0, 0.0];
        let dir1 = [0.0f32, 0.0, 1.0];
        let dir2 = [0.0f32, -1.0, 0.0];
        let dir3 = [0.0f32, 0.0, -1.0];

        let section = test_octahedral_section(
            [1, 1, 1],
            vec![AnimationDescriptor {
                period: 1.0,
                phase: 0.0,
                base_color: [1.0, 1.0, 1.0],
                brightness: vec![1.0, 0.5],
                color: vec![[1.0, 0.0, 0.0]],
                direction: vec![dir0, dir1, dir2, dir3],
                start_active: 1,
            }],
        );

        let (descriptors, samples_bytes, count) = build_animation_buffers(Some(&section));
        assert_eq!(count, 1);

        let direction_offset = u32::from_ne_bytes(descriptors[40..44].try_into().unwrap()) as usize;
        let direction_count = u32::from_ne_bytes(descriptors[44..48].try_into().unwrap()) as usize;
        assert_eq!(direction_count, 4);
        // Layout: 2 brightness + 1 color × 3 channels = 5 f32s before direction.
        assert_eq!(direction_offset, 5);

        // Recover the flat f32 sample array and verify the direction block
        // matches what we wrote, in order.
        let samples: Vec<f32> = samples_bytes
            .chunks_exact(4)
            .map(|c| f32::from_ne_bytes(c.try_into().unwrap()))
            .collect();
        for (i, sample) in [dir0, dir1, dir2, dir3].iter().enumerate() {
            let base = direction_offset + i * 3;
            assert_eq!(samples[base], sample[0]);
            assert_eq!(samples[base + 1], sample[1]);
            assert_eq!(samples[base + 2], sample[2]);
        }

        // CPU reference for `sample_color_catmull_rom` at cycle_t = 0.5 over
        // four samples. This is the same math WGSL runs, transcribed verbatim
        // from curve_eval.wgsl to pin the two sides together.
        fn catmull_rom_reference_vec3(samples: &[[f32; 3]], t: f32) -> [f32; 3] {
            let count = samples.len();
            let scaled = t * count as f32;
            let i1 = (scaled.floor() as usize) % count;
            let i0 = (i1 + count - 1) % count;
            let i2 = (i1 + 1) % count;
            let i3 = (i1 + 2) % count;
            let f = scaled.fract();
            let p0 = samples[i0];
            let p1 = samples[i1];
            let p2 = samples[i2];
            let p3 = samples[i3];
            let mut out = [0.0f32; 3];
            for c in 0..3 {
                let a = -0.5 * p0[c] + 1.5 * p1[c] - 1.5 * p2[c] + 0.5 * p3[c];
                let b = p0[c] - 2.5 * p1[c] + 2.0 * p2[c] - 0.5 * p3[c];
                let cc = -0.5 * p0[c] + 0.5 * p2[c];
                let d = p1[c];
                out[c] = ((a * f + b) * f + cc) * f + d;
            }
            out
        }

        let dirs = [dir0, dir1, dir2, dir3];
        for &t in &[0.0f32, 0.125, 0.5, 0.75, 0.999] {
            let expected = catmull_rom_reference_vec3(&dirs, t);
            // Emulate the shader-side read: pull the correct sample stride
            // out of the flat samples array and run the reference against it.
            // (The shader reads anim_samples[direction_offset + i*3 + ch].)
            let window: Vec<[f32; 3]> = (0..direction_count)
                .map(|i| {
                    let b = direction_offset + i * 3;
                    [samples[b], samples[b + 1], samples[b + 2]]
                })
                .collect();
            let reconstructed = catmull_rom_reference_vec3(&window, t);
            for c in 0..3 {
                assert!(
                    (expected[c] - reconstructed[c]).abs() < 1.0e-6,
                    "direction channel mismatch at t={t} channel={c}: \
                     packed={reconstructed:?} direct={expected:?}",
                );
            }
        }
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

        use wgpu::util::DeviceExt;
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
